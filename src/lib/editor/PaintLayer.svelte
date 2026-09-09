<script>
  import { draft } from './draft.svelte.js'
  import { editor } from '../state/editor.svelte.js'
  import { pageVersion } from '../api/tile.js'
  import { cloneOffset } from './gesture.js'
  import { brushFromParams, nativePoints, planStroke } from './paintplan.js'
  import {
    MIN_PREVIEW_RADIUS,
    dabPath,
    presentStroke,
    renderCloneTiles,
    renderDabs,
  } from './paintrender.js'

  /**
   * ==========================================================================
   * THE LIVE PIXELS. What paint and clone / heal look like *while the pointer
   * is still down*, rather than after the round trip.
   * ==========================================================================
   *
   * Every other tool on this sheet previews as an outline, and that is honest
   * for them: a rectangle, a lasso, a mask brush's swept capsule all describe
   * a *region*, and the region is the whole of what the commit makes. Paint
   * and clone are the two tools where the outline is not the answer - the
   * answer is colour, at an opacity, with a soft rim, or the page read from
   * somewhere else. A dashed capsule cannot say what a 40% flow looks like
   * over this particular artwork, so until this the only way to find out was to
   * commit and wait.
   *
   * **The plan is the contract, not the pixels.** The dab
   * list is planned once by `paintplan.js`, a port of `paint/plan.rs`, in
   * **native page pixels** - the same list, from the same points, that the
   * commit stamps at full resolution. This canvas draws that list scaled down
   * to whatever the sheet currently occupies. Stabilisation, spacing and
   * pressure are therefore identical between what the user saw and what they
   * got; only the sampling grid differs, and it must, because a native-
   * resolution preview would mean native pixels in the webview, which the
   * rule exists to hold.
   *
   * ## Three canvases' worth of work on two canvases
   *
   *   1. an **offscreen stroke canvas**, stamped incrementally at per-dab
   *      `flow` alpha. Only dabs the previous frame did not draw are stamped,
   *      which the planner's prefix-stability makes safe.
   *   2. the **visible canvas**, cleared and re-presented from (1) every frame
   *      at `globalAlpha = opacity/100`.
   *
   * The split is the only way Canvas2D can express flow-building-towards-an-
   * opacity-ceiling: on one canvas a slow stroke composites over itself and
   * goes solid long before the ceiling. `paintrender.js` holds all of it, and
   * holds it as pure functions over a context, because this repo's vitest runs
   * in `node` and has no canvas to mount a component into.
   *
   * ## Clone / heal draws the page, not a colour
   *
   * The proxy is already in the document as a stack of `<img>` over `tile://`
   * (`PageArtwork.svelte`). So the preview clips to the union of the dab discs
   * and draws those same images, translated by the negative of the seam's
   * `cloneOffset`. `drawImage` needs no readback, so a tainted canvas is
   * irrelevant. Where there are no tiles - the mock backend, every test, a
   * plain browser - there is nothing to sample and the clone preview draws
   * nothing rather than something invented.
   *
   * ## Why the last frame outlives the gesture
   *
   * On commit the draft is dropped **synchronously**, and the real pixels
   * arrive only when the backend has run and the page's `tile://` version token
   * has moved. Clearing on `clearDraft` would put a hole between the two: the
   * stroke vanishes, then reappears. So a committed stroke is *held* - the last
   * frame stays up until the token changes or 1500 ms pass, whichever comes
   * first. An abandoned one (Escape, a pointer cancel) is not held at all,
   * because nothing is coming to replace it.
   */

  /**
   * @type {{
   *   page: import('../api/backend.js').ApiPage,
   *   committing?: boolean,
   * }}
   */
  let { page, committing = false } = $props()

  /** How long a held frame may outlive its gesture without the tiles moving. */
  const HOLD_MS = 1500

  /** @type {HTMLCanvasElement|undefined} */
  let canvasEl = $state()

  /* Everything below is deliberately *not* reactive: it is the render loop's
     own scratch, read and written inside a frame, and making it $state would
     schedule an effect for every dab. */
  /** @type {CanvasRenderingContext2D|null} */
  let ctx = null
  /** @type {HTMLCanvasElement|null} */
  let strokeCanvas = null
  /** @type {CanvasRenderingContext2D|null} */
  let strokeCtx = null
  /** How many of the planned dabs are already on the stroke canvas. */
  let drawn = 0
  /** @type {number} */
  let frame = 0
  /** @type {number} */
  let holdTimer = 0
  /** @type {Array<{image: HTMLImageElement, left: number, top: number, width: number, height: number}>} */
  let tiles = []
  /** @type {ResizeObserver|null} */
  let observer = null
  /** Whether a stroke was on screen on the previous effect run. */
  let wasDrawing = false

  /** The version token the held frame is waiting to see change. */
  let heldToken = $state(/** @type {string|null} */ (null))

  const active = $derived(draft.active?.pageId === page.id ? draft.active : null)

  /** Which of the two live-pixel tools this draft is, if either. */
  const kind = $derived.by(() => {
    if (!active || active.kind !== 'stroke') return null
    if (active.tool === 'cloneHeal') return 'clone'
    if (active.tool === 'brush') return 'paint'
    return null
  })

  /** The `cleaned` variant's cache token - the thing that moves when the real pixels land. */
  const version = $derived(pageVersion(page, 'cleaned'))

  /* ------------------------------------------------------------------ */
  /* The canvas                                                          */
  /* ------------------------------------------------------------------ */

  /**
   * Match the backing store to the CSS box times the device's pixel ratio, and
   * say whether that changed. A change invalidates every stamped dab, because
   * they were stamped at the old scale.
   *
   * @returns {boolean} whether the size moved
   */
  function resize() {
    if (!canvasEl) return false
    const ratio = globalThis.devicePixelRatio || 1
    const width = Math.max(1, Math.round(canvasEl.clientWidth * ratio))
    const height = Math.max(1, Math.round(canvasEl.clientHeight * ratio))
    if (canvasEl.width === width && canvasEl.height === height) return false
    canvasEl.width = width
    canvasEl.height = height
    if (strokeCanvas) {
      strokeCanvas.width = width
      strokeCanvas.height = height
    }
    drawn = 0
    return true
  }

  /** @returns {boolean} whether there is a context to draw into */
  function ready() {
    if (!canvasEl) return false
    if (!ctx) ctx = canvasEl.getContext('2d')
    if (!strokeCanvas) {
      strokeCanvas = document.createElement('canvas')
      strokeCanvas.width = canvasEl.width
      strokeCanvas.height = canvasEl.height
      strokeCtx = strokeCanvas.getContext('2d')
    }
    return !!ctx && !!strokeCtx
  }

  /** Drop everything on screen and forget the stroke that was on it. */
  function wipe() {
    drawn = 0
    tiles = []
    if (strokeCtx && strokeCanvas) strokeCtx.clearRect(0, 0, strokeCanvas.width, strokeCanvas.height)
    if (ctx && canvasEl) ctx.clearRect(0, 0, canvasEl.width, canvasEl.height)
  }

  /* ------------------------------------------------------------------ */
  /* The artwork, for clone / heal                                       */
  /* ------------------------------------------------------------------ */

  /**
   * The page's `cleaned` proxy tiles, as fractions of this canvas's box.
   *
   * Measured rather than parsed: `PageArtwork` positions each `<img>` in
   * percentages with a `calc(... + 1px)` overlap, and the canvas and the
   * artwork are both `inset: 0` in the same sheet, so two `getBoundingClientRect`
   * calls give the mapping exactly without this file knowing the tile plan.
   */
  function collectTiles() {
    if (!canvasEl) return []
    const sheet = canvasEl.closest('.sheet') ?? canvasEl.parentElement
    const base = canvasEl.getBoundingClientRect()
    if (!sheet || !(base.width > 0) || !(base.height > 0)) return []
    const found = sheet.querySelectorAll('[data-artwork="cleaned"] img')
    /** @type {Array<{image: HTMLImageElement, left: number, top: number, width: number, height: number}>} */
    const list = []
    for (const node of found) {
      const image = /** @type {HTMLImageElement} */ (node)
      if (!image.complete || !image.naturalWidth) continue
      const rect = image.getBoundingClientRect()
      list.push({
        image,
        left: (rect.left - base.left) / base.width,
        top: (rect.top - base.top) / base.height,
        width: rect.width / base.width,
        height: rect.height / base.height,
      })
    }
    return list
  }

  /* ------------------------------------------------------------------ */
  /* The frame                                                           */
  /* ------------------------------------------------------------------ */

  /**
   * Plan the stroke as it stands and stamp whatever the last frame missed.
   *
   * @param {'paint'|'clone'} mode
   */
  function paint(mode) {
    if (!active || !canvasEl || !ctx || !strokeCtx || !strokeCanvas) return
    const width = canvasEl.width
    const height = canvasEl.height
    const pageWidth = page.width ?? 1600
    const pageHeight = page.height ?? 2400
    const scale = width / pageWidth
    if (!(scale > 0)) return

    const params = editor.toolParams[active.tool] ?? {}
    const brush = brushFromParams(params)
    const dabs = planStroke(nativePoints(active.points, pageWidth, pageHeight), brush)
    if (dabs.length > drawn) {
      if (mode === 'paint') {
        drawn = renderDabs(strokeCtx, dabs, {
          scale,
          color: String(params.color ?? '#000000'),
          hardness: brush.hardness,
          from: drawn,
        })
      } else {
        drawn = stampClone(dabs, scale, width, height, brush)
      }
    }
    presentStroke(ctx, strokeCanvas, { width, height, opacity: brush.opacity })
  }

  /**
   * Clone / heal's stamp: the page, clipped to the discs this frame added.
   *
   * `copy` inside the clip rather than `source-over` is what keeps flow from
   * compounding where a slow stroke overlaps itself - every pixel of the swept
   * region ends up carrying the source once, at the flow's alpha, which is the
   * closest Canvas2D gets to the coverage buffer `cleaner-core` accumulates.
   *
   * @param {import('./paintplan.js').PlannedDab[]} dabs
   * @param {number} scale
   * @param {number} width
   * @param {number} height
   * @param {import('./paintplan.js').BrushSpec} brush
   * @returns {number} how many dabs are now accounted for
   */
  function stampClone(dabs, scale, width, height, brush) {
    if (!strokeCtx || !active) return dabs.length
    if (tiles.length === 0) tiles = collectTiles()
    if (tiles.length === 0) return dabs.length

    const start = active.points[0]
    const resolved = start
      ? cloneOffset({
          source: draft.cloneSource?.pageId === page.id ? draft.cloneSource : null,
          strokeStart: start,
          alignment: String(editor.toolParams.cloneHeal?.alignment ?? 'aligned'),
          offset: draft.cloneOffset,
        })
      : null
    if (!resolved) return dabs.length

    // `cloneOffset = source − strokeStart`, so the source pixel for a target
    // `t` is `t + offset` - and a canvas shows image pixel `T − translate`.
    const dx = -(resolved.offset.x / 100) * width
    const dy = -(resolved.offset.y / 100) * height

    const path = dabPath(Path2D, dabs, { scale, from: drawn, minRadius: MIN_PREVIEW_RADIUS })
    strokeCtx.save()
    strokeCtx.clip(path)
    strokeCtx.globalCompositeOperation = 'copy'
    strokeCtx.globalAlpha = Math.min(Math.max(brush.flow, 0), 100) / 100
    renderCloneTiles(strokeCtx, tiles, { width, height, dx, dy })
    strokeCtx.restore()
    return dabs.length
  }

  function tick() {
    frame = 0
    const mode = kind
    if (!mode) return
    if (!ready()) return
    if (resize()) {
      // A zoom mid-stroke invalidates every stamped dab; the stroke canvas is
      // already blank at its new size, so the next `paint` redraws all of them.
      if (strokeCtx && strokeCanvas) strokeCtx.clearRect(0, 0, strokeCanvas.width, strokeCanvas.height)
    }
    paint(mode)
    frame = requestAnimationFrame(tick)
  }

  function stopLoop() {
    if (frame) cancelAnimationFrame(frame)
    frame = 0
  }

  function releaseHold() {
    if (holdTimer) clearTimeout(holdTimer)
    holdTimer = 0
    heldToken = null
    wipe()
  }

  /* ------------------------------------------------------------------ */
  /* Lifecycle                                                           */
  /* ------------------------------------------------------------------ */

  $effect(() => {
    const drawing = !!kind
    if (drawing === wasDrawing) return
    wasDrawing = drawing

    if (drawing) {
      if (holdTimer) clearTimeout(holdTimer)
      holdTimer = 0
      heldToken = null
      if (ready()) {
        resize()
        wipe()
      }
      tiles = []
      stopLoop()
      frame = requestAnimationFrame(tick)
      return
    }

    stopLoop()
    if (!committing) {
      // Abandoned: nothing is coming to replace what is on screen.
      releaseHold()
      return
    }
    // Committed: hold the last frame until the page's own pixels move.
    heldToken = version
    holdTimer = setTimeout(releaseHold, HOLD_MS)
  })

  // The `tile://` token moved, so the real pixels are on the sheet now.
  $effect(() => {
    if (heldToken !== null && version !== heldToken) releaseHold()
  })

  $effect(() => {
    if (!canvasEl) return
    if (typeof ResizeObserver === 'function') {
      observer = new ResizeObserver(() => {
        if (kind) return // the loop resizes itself while a stroke is live
        if (ready()) resize()
      })
      observer.observe(canvasEl)
    }
    return () => {
      observer?.disconnect()
      observer = null
      stopLoop()
      if (holdTimer) clearTimeout(holdTimer)
      holdTimer = 0
    }
  })
</script>

<!-- Below the dashed cursor circle and above the artwork: the preview is the
     thing being aimed at, and the aiming ring has to stay legible over it. -->
<canvas class="paint" bind:this={canvasEl} aria-hidden="true"></canvas>

<style>
  .paint {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    pointer-events: none;
  }
</style>
