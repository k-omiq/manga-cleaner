<script>
  import { tick, untrack } from 'svelte'
  import {
    editor,
    pages,
    currentPage,
    consumeStripScroll,
    reportFitScale,
    setStripPosition,
    setStripScope,
    setZoom,
    MIN_ZOOM,
    MAX_ZOOM,
  } from '../state/editor.svelte.js'
  import {
    anchoredScroll,
    fitScale,
    fitWidth,
    naturalWidth,
    pageRatio,
    sheetWidth,
    wheelZoom,
  } from './zoom.js'
  import {
    STRIP_GAP,
    centreIndex,
    scrollTopFor,
    stripMetrics,
    stripWindow,
    visibleIndices,
  } from './strip.js'
  import { Empty } from '../ui/index.js'
  import { t } from '../i18n/index.js'
  import PageSheet from './PageSheet.svelte'

  /**
   * The canvas: the page column inside the editor's viewport.
   * The viewport itself - the screen's only scroll container, its `--bg`
   * field and its `70px 104px 66px` - belongs to `EditorScreen.svelte` and
   * stays there. **A second scroll container here would break autosave's
   * restore**, so this component reaches the shell's scroller
   * (`stageEl.parentElement`, the idiom `PageList` already uses) rather than
   * growing one.
   *
   * What it owns is everything the chrome cannot know:
   *
   * - **Fit.** The sheet's width comes from the viewport's content box, not
   *   from a constant. The unit is the fraction of the page's own pixels the
   *   sheet is drawn at, so the bottom pill's readout is the truth; with `fit`
   *   on, the computed scale goes to `editor.fitScale` through
   *   `reportFitScale` and `displayZoom()` answers for both cases.
   * - **Pinch and modifier-scroll**, anchored under the pointer. A plain wheel
   *   is left alone and keeps scrolling.
   * - **The longstrip column**: a virtual window over the pages, the position
   *   readout following the viewport's centre, and the scroll request
   *   `goToPage` leaves for it.
   */

  /** @type {HTMLElement|undefined} */
  let stageEl = $state()

  /* The viewport's content box, for fit. */
  let contentWidth = $state(0)
  let contentHeight = $state(0)

  /* The column's position, for the strip. Measured against the scroller's
     padding box, so the viewport's own padding never enters the arithmetic
     twice. `columnTop` goes negative as the reader scrolls into the column. */
  let columnTop = $state(0)
  let viewportHeight = $state(0)

  const list = $derived(pages())
  const longstrip = $derived(editor.project?.mode === 'longstrip')
  // The single page's model. A longstrip column has no single model page: it
  // is measured page by page through `stripMetrics`, because a chapter's
  // segments are not uniform by construction.
  const model = $derived(longstrip ? null : currentPage())

  // What "fit" is fitting. For one page that is the page; for a column it is
  // the **widest** page in it, so that fitting the column fits every page in
  // it rather than only the first. `ratio` is the single page's, and enters
  // `fitWidth` only through its height term, which longstrip does not use.
  const natural = $derived(longstrip ? widest(list) : naturalWidth(model))
  const ratio = $derived(pageRatio(longstrip ? null : model))
  const fitSpec = $derived({ contentWidth, contentHeight, ratio, longstrip, natural })
  const fitZoom = $derived(fitScale(fitSpec))
  const zoom = $derived(editor.fit ? fitZoom : editor.zoom)

  // Fit draws from the *exact* fit width, not from `fitZoom`: the reported
  // scale is rounded to two places so the readout is stable, and at a natural
  // width of 1600 those two places are 16px of sheet - enough to overflow the
  // viewport a fit is supposed to fit inside. Expressed as a scale rather than
  // as a width, because every page in a column is drawn at the same *scale*
  // and at its own width.
  const scale = $derived(editor.fit ? fitWidth(fitSpec) / natural : editor.zoom)
  const width = $derived(sheetWidth({ natural, zoom: scale }))

  const metrics = $derived(
    stripMetrics({ pages: longstrip ? list : [], scale, gap: STRIP_GAP }),
  )

  const band = $derived(
    stripWindow({ metrics, columnTop, viewportHeight, include: editor.pageIndex }),
  )
  const visible = $derived(longstrip ? list.slice(band.start, band.end) : [])

  /**
   * The widest page in the column, at 1:1. Not `list[0]`: a chapter whose
   * first segment is narrower than a later one would fit the viewport to the
   * narrow one and push the wide one under the floating windows.
   *
   * @param {ReadonlyArray<{width?: number}>} pages
   * @returns {number}
   */
  function widest(pages) {
    let out = 0
    for (const page of pages) out = Math.max(out, naturalWidth(page))
    return out || naturalWidth(null)
  }

  /* ---------------------------------------------------------------- */
  /* Measurement                                                       */
  /* ---------------------------------------------------------------- */

  /** @returns {HTMLElement|null} the shell's scroller */
  function scroller() {
    return stageEl?.parentElement ?? null
  }

  /**
   * Where the column sits in the viewport right now. Read from the boxes
   * rather than from `scrollTop` so the viewport's padding never enters the
   * arithmetic; called on every scroll, and again immediately after the canvas
   * scrolls the column itself - a `scroll` event is dispatched during the
   * browser's rendering steps, which is one frame too late to stop the page
   * readout bouncing back to where the column used to be.
   */
  function measureColumn() {
    const box = scroller()
    if (!box || !stageEl) return
    columnTop = stageEl.getBoundingClientRect().top - box.getBoundingClientRect().top
    viewportHeight = box.clientHeight
  }

  /**
   * The viewport's content box - its padding taken off, because "fit" has to
   * fit what is actually free, and `70px 104px 66px` of it is not.
   */
  function measureViewport() {
    const box = scroller()
    if (!box) return
    const style = globalThis.getComputedStyle?.(box)
    const px = (/** @type {string|undefined} */ value) => Number.parseFloat(value ?? '') || 0
    contentWidth = box.clientWidth - px(style?.paddingLeft) - px(style?.paddingRight)
    contentHeight = box.clientHeight - px(style?.paddingTop) - px(style?.paddingBottom)
  }

  // Measured once outright and then on every resize. The first measurement is
  // not left to the observer: a `ResizeObserver` notifies during the browser's
  // rendering steps, which a background tab does not run, and a canvas that
  // draws at its minimum width until the tab is looked at is not acceptable.
  $effect(() => {
    const box = scroller()
    if (!box) return
    measureViewport()
    if (!globalThis.ResizeObserver) return
    const observer = new ResizeObserver(measureViewport)
    observer.observe(box)
    return () => observer.disconnect()
  })

  $effect(() => {
    reportFitScale(fitZoom)
  })

  // Where the column sits relative to the viewport, for the strip only. The
  // shell's own `onscroll` runs first (it is registered first) and records the
  // settle through `setScroll`; this only reads.
  $effect(() => {
    if (!longstrip) {
      setStripScope([])
      return
    }
    const box = scroller()
    if (!box) return
    measureColumn()
    box.addEventListener('scroll', measureColumn, { passive: true })
    const observer = globalThis.ResizeObserver ? new ResizeObserver(measureColumn) : null
    observer?.observe(box)
    return () => {
      box.removeEventListener('scroll', measureColumn)
      observer?.disconnect()
    }
  })

  // The strip reports what it has on screen and which page is at the centre.
  // Neither goes through `goToPage`, which resets scroll to {0,0} and would
  // fight the reader for the scrollbar.
  $effect(() => {
    if (!longstrip || list.length === 0) return
    const spec = { metrics, columnTop, viewportHeight }
    setStripScope(visibleIndices(spec))
    setStripPosition(centreIndex(spec))
  })

  // The other half of the handshake: the Pages list, the bottom pill and
  // `stepReview` all ask for a position through `goToPage`, which cannot know
  // where that position sits in the column and leaves a request instead.
  $effect(() => {
    const request = editor.stripScrollRequest
    if (!request || !longstrip) return
    untrack(() => {
      const box = scroller()
      if (!box) return
      consumeStripScroll()
      box.scrollTop = scrollTopFor({
        index: request.index,
        metrics,
        scrollTop: box.scrollTop,
        columnTop,
      })
      measureColumn()
    })
  })

  /* ---------------------------------------------------------------- */
  /* Zoom                                                              */
  /* ---------------------------------------------------------------- */

  /**
   * A `wheel` with `ctrlKey` is both a trackpad pinch and a Ctrl/⌘-scroll, and
   * both mean zoom. Anything else is a scroll and is left alone - which is why
   * `preventDefault` is on the zooming path only, and why the listener is
   * registered `{ passive: false }` (a passive listener may not prevent).
   *
   * @param {WheelEvent} event
   */
  async function onwheel(event) {
    if (!event.ctrlKey && !event.metaKey) return
    event.preventDefault()

    const box = scroller()
    if (!box || !stageEl) return

    const target = wheelZoom({ deltaY: event.deltaY, zoom, min: MIN_ZOOM, max: MAX_ZOOM })
    if (target === null) return

    const before = boxOf(stageEl)
    setZoom(target)
    // The scale change and the scroll correction have to land together, or the
    // page jumps and then slides. `tick()` resolves after the DOM update and
    // before the browser paints.
    await tick()
    if (!stageEl) return
    const next = anchoredScroll({
      pointerX: event.clientX,
      pointerY: event.clientY,
      before,
      after: boxOf(stageEl),
      scrollLeft: box.scrollLeft,
      scrollTop: box.scrollTop,
    })
    box.scrollLeft = next.left
    box.scrollTop = next.top
    measureColumn()
  }

  $effect(() => {
    const box = scroller()
    if (!box) return
    box.addEventListener('wheel', onwheel, { passive: false })
    return () => box.removeEventListener('wheel', onwheel)
  })

  /**
   * @param {HTMLElement} element
   * @returns {import('./zoom.js').Box}
   */
  function boxOf(element) {
    const rect = element.getBoundingClientRect()
    return { left: rect.left, top: rect.top, width: rect.width, height: rect.height }
  }
</script>

<div class="stage" bind:this={stageEl} style:--strip-gap="{STRIP_GAP}px">
  {#if editor.loading}
    <Empty title={t('editor.state.opening')} />
  {:else if list.length === 0}
    <Empty title={t('editor.state.noPages')} />
  {:else if longstrip}
    {#if band.padTop > 0}<div class="pad" style:height="{band.padTop}px"></div>{/if}
    {#each visible as page, offset (page.id)}
      {@const index = band.start + offset}
      <div class="slot">
        <PageSheet
          {page}
          width={metrics.widths[index]}
          strip
          current={index === editor.pageIndex}
          tabbable={index === editor.pageIndex}
        />
      </div>
    {/each}
    {#if band.padBottom > 0}<div class="pad" style:height="{band.padBottom}px"></div>{/if}
  {:else if model}
    <PageSheet page={model} {width} />
  {/if}
</div>

<style>
  .stage {
    display: flex;
    flex-direction: column;
    align-items: center;
    flex: none;
  }

  /* The column carries no CSS gap: each page owns the space under it, which is
     what makes the virtual window's spacers exact (see `strip.js`). */
  .slot {
    flex: none;
    margin-bottom: var(--strip-gap);
  }

  .pad { flex: none }
</style>
