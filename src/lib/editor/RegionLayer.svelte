<script>
  import { tick } from 'svelte'
  import {
    editor,
    hover,
    isHighlighted,
    readingDirection,
    select,
  } from '../state/editor.svelte.js'
  import { readingOrder, regionMarker } from './regions.js'
  import { menuPoint } from './gesture.js'
  import { applyActiveToolToRegion } from './toolapply.svelte.js'
  import { t } from '../i18n/index.js'
  import RegionMenu from './RegionMenu.svelte'

  /**
   * Everything the *application* draws on a page: the region outlines, the two
   * region markers, and the hit target for each region. Not the artwork -
   * `PageArtwork` is the image, and this layer sits over it.
   *
   * **A cleaned region is an outline, never a fill.** The overlay's job is to
   * say "a layer exists here"; a filled box says it by hiding the very artwork
   * the user is judging. So a region is drawn as a dashed, sky-blue outline
   * and nothing else - blue rather than ink because the artwork under it is
   * black ink, and a dark dash on a dark panel is one the user cannot find -
   * and it is **off by default**:
   *
   *   - pointer over a region, or keyboard focus on it - its outline appears.
   *     Hover shows, it does not select.
   *   - a click selects the region, and a selected region keeps its outline
   *     until something else is selected or `Esc` clears it.
   *   - the Layers panel selecting a row lights the same outline, through the
   *     shared highlight contract and not through any reference between them.
   *   - the mask overlay (`M`) shows **every** region's outline at once. It is
   *     off when a chapter opens: an editor that starts covered in boxes is one
   *     the user has to switch off before they can look at the page.
   *
   * **Every region is a real `<button>`**, so it has a role, an accessible
   * name and native activation. The prototype is pointer-only; that is not
   * shippable. The set carries a **roving
   * tabindex** the way `PageList` does - a flat list of dozens of buttons in
   * the tab order is worse than no keyboard route - and the arrows move
   * between regions in *reading order*, right-to-left by default, which is the
   * order the page is actually read in.
   *
   * Arrow keys `stopPropagation()`. The global shortcut layer binds ← and → in
   * editor scope to page the chapter, and it cannot tell an arrow meant for a
   * local widget from one meant for the chapter; `FloatingWindow` already
   * settles that the same way.
   *
   * **Nothing here is clipped by the wipe.** An outline is a statement about
   * the region, not about the cleaned pixels, so it holds wherever the wipe
   * stands. A declined region's marker is the strongest case of the same rule:
   * it is on the page whatever the overlay, the filter or the wipe says,
   * because it is the one outcome where nothing visibly happened.
   *
   * Selection and hover go through the contract in `state/editor.svelte.js`
   * and nowhere near the Layers panel: `hover()` on pointer and focus,
   * `select()` on activation, `isHighlighted()` to read.
   */

  /**
   * `tabbable` is false for every page of a longstrip column except the
   * current position: the pointer reaches all of them, but only one page at a
   * time may put a stop in the tab order.
   *
   * `interactive` is false while a drag tool has its drawing surface over the
   * sheet (`DrawLayer`). The layer yields the **pointer** only: the buttons
   * keep their tab stops, their names and their roving index, so every
   * keyboard route into a region survives, and the surface routes a tap back
   * to the region under it. Without this the buttons would swallow the
   * pointerdown that starts a stroke - and starting a stroke over an existing
   * region is exactly what erasing means.
   *
   * @type {{
   *   page: import('../api/backend.js').ApiPage,
   *   tabbable?: boolean,
   *   interactive?: boolean,
   * }}
   */
  let { page, tabbable = true, interactive = true } = $props()

  /** @type {HTMLElement|undefined} */
  let layerEl = $state()
  let focusIndex = $state(0)
  /**
   * The open context menu: where it is, and which region it is about.
   *
   * @type {{x: number, y: number, region: import('../api/backend.js').ApiRegion} | null}
   */
  let menu = $state(null)

  const marksVisible = $derived(editor.maskOverlay || editor.reviewFilter)
  const ordered = $derived(readingOrder(page.regions ?? [], readingDirection()))
  const markers = $derived(ordered.map((region) => regionMarker(region, { marksVisible })))

  // The tab stop follows the selection while the selection is on this page, so
  // tabbing to the canvas lands on the region being worked on rather than back
  // at the top of the page.
  //
  // `focusIndex` is clamped on the way out, not only on the way in: a region
  // set that shrinks under a stale index (a mask deleted, a re-run, a filter)
  // would otherwise match no marker at all, and a set where no marker carries
  // `tabindex="0"` is a set the Tab key cannot reach.
  const selectedIndex = $derived(markers.findIndex((marker) => marker.id === editor.selectionId))
  const rovingIndex = $derived(
    selectedIndex >= 0 ? selectedIndex : Math.min(focusIndex, markers.length - 1),
  )

  /**
   * Arrowing selects as it goes. Unlike the Pages list - where selection means
   * loading a page - selecting a region only highlights it, and the whole
   * point of moving between regions by keyboard is to inspect them.
   *
   * @param {number} index
   */
  async function focusRegion(index) {
    const count = markers.length
    if (count === 0) return
    const next = Math.min(count - 1, Math.max(0, index))
    focusIndex = next
    select(markers[next].id)
    await tick()
    const element = layerEl?.querySelector(`[data-region="${markers[next].id}"]`)
    if (element instanceof HTMLElement) {
      element.focus({ preventScroll: true })
      element.scrollIntoView({ block: 'nearest', inline: 'nearest' })
    }
  }

  /**
   * @param {KeyboardEvent} event
   * @param {number} index
   */
  function onkeydown(event, index) {
    if (event.metaKey || event.ctrlKey || event.altKey) return
    switch (event.key) {
      case 'ArrowDown':
      case 'ArrowRight':
        focusRegion(index + 1)
        break
      case 'ArrowUp':
      case 'ArrowLeft':
        focusRegion(index - 1)
        break
      case 'Home':
        focusRegion(0)
        break
      case 'End':
        focusRegion(markers.length - 1)
        break
      default:
        return
    }
    event.preventDefault()
    // The editor's global ← / → page the chapter. An arrow this layer has
    // already used must not also turn the page.
    event.stopPropagation()
  }

  /**
   * A click on a region selects it. When the armed tool is Content-aware fill
   * (a click-to-apply tool, not a drawing tool), it also applies the tool to
   * the region.
   *
   * @param {string} regionId
   */
  function onRegionClick(regionId) {
    select(regionId)
    if (editor.tool === 'contentAwareFill') {
      // Started and not awaited, and `void` says so on purpose: a click handler
      // has nothing to do with the answer, and the outline and the selection
      // are already correct whichever way the apply goes. What made this safe
      // is that `applyActiveToolToRegion` now reports its own failures and
      // settles rather than rejecting - an unawaited promise that can reject is
      // a failure with nowhere to land, which is exactly what this line was.
      void applyActiveToolToRegion(regionId)
    }
  }

  /**
   * The secondary press: **selects, and then offers what to do about it**.
   *
   * Selecting first is not decoration. The menu's entries are the Layers row's
   * entries, and a right-click that opened a menu for one region while the
   * canvas and the panel still lit another would be two answers to "which
   * region is this about". It never applies the armed tool - that is what the
   * left button is for, and a menu is a question, not a command.
   *
   * The same handler serves `Shift`+`F10` and the context-menu key, which the
   * browser reports as this event with no coordinates; `menuPoint` anchors
   * those to the region's own box.
   *
   * @param {MouseEvent} event
   * @param {string} regionId
   */
  function onRegionMenu(event, regionId) {
    const region = (page.regions ?? []).find((candidate) => candidate.id === regionId)
    if (!region) return
    event.preventDefault()
    select(regionId)
    const target = /** @type {HTMLElement} */ (event.currentTarget)
    menu = { ...menuPoint(event, target.getBoundingClientRect()), region }
  }
</script>

<div class="regions" class:inert={!interactive} bind:this={layerEl}>
  <!-- The page is not a click target; only its regions are. This is the empty
       space, and clearing the selection is all it does. It is out of the tab
       order on purpose: Escape already clears the selection from the keyboard
       (`cancelInteraction`), and a tab stop that means "nothing here" is
       noise.

       `aria-hidden` for the same reason, and it is safe *because* of the
       `tabindex="-1"`: nothing focusable is being hidden. Without it a screen
       reader meets a full-sheet button ahead of every region on the page - on
       every page of a longstrip - announcing an action the Escape key already
       performs. It keeps its name for the pointer's tooltip-free hit target
       and for anything reading the DOM. -->
  <button
    type="button"
    class="clear"
    tabindex="-1"
    aria-hidden="true"
    aria-label={t('canvas.action.clearSelection')}
    onclick={() => select(null)}
  ></button>

  {#each markers as marker, index (marker.id)}
    <button
      type="button"
      class="region {marker.status}"
      class:marked={!!marker.badge}
      class:lit={isHighlighted(marker.id)}
      class:outlined={editor.maskOverlay}
      data-region={marker.id}
      tabindex={tabbable && index === rovingIndex ? 0 : -1}
      aria-current={editor.selectionId === marker.id ? 'true' : undefined}
      aria-label={t(marker.nameKey, marker.nameParams)}
      title={t(marker.nameKey, marker.nameParams)}
      style:left="{marker.bbox.x}%"
      style:top="{marker.bbox.y}%"
      style:width="{marker.bbox.w}%"
      style:height="{marker.bbox.h}%"
      onkeydown={(event) => onkeydown(event, index)}
      onpointerenter={() => hover(marker.id)}
      onpointerleave={() => hover(null)}
      onfocus={() => hover(marker.id)}
      onblur={() => hover(null)}
      onclick={() => onRegionClick(marker.id)}
      oncontextmenu={(event) => onRegionMenu(event, marker.id)}
    >
      {#if marker.badge}
        <span class="badge" aria-hidden="true">{marker.badge}</span>
      {/if}
    </button>
  {/each}

  <RegionMenu at={menu} onclose={() => (menu = null)} />
</div>

<style>
  .regions {
    position: absolute;
    inset: 0;
  }

  /* The pointer only. `pointer-events` does not affect focus, so every region
     keeps its tab stop, its name and its activation. */
  .regions.inert { pointer-events: none }

  .clear {
    position: absolute;
    inset: 0;
    padding: 0;
    border: 0;
    background: transparent;
    cursor: default;
  }

  .region {
    position: absolute;
    padding: 0;
    border: 0;
    background: transparent;
    border-radius: 2px;
    /* The design file's armed-tool cursor. A click here applies the active
       tool to the region - the pointer should say that before the click, not
       after. A tool is always armed in this app (`editor.tool` is never null,
       unlike the prototype's rail), so the crosshair is unconditional. */
    cursor: crosshair;
    transition:
      outline-color var(--dur) var(--ease),
      box-shadow var(--dur) var(--ease);
    /* Declared unconditionally and transparent, so the rules below change only
       the colour: an outline that appears from nothing jumps the layout of the
       artwork's perceived edge, and one that fades in does not. */
    outline: 1.5px dashed transparent;
    outline-offset: 2px;
  }

  /* The layer indicator: a dashed mark, no fill, ever. On when the pointer or
     the keyboard is on the region, when it is selected, or when the mask
     overlay asks for all of them at once. In --page-mark rather than the
     page's ink: most manga is black, and a near-black dash over it is the one
     place the outline is needed and cannot be seen. */
  .region.outlined,
  .region.lit {
    outline-color: var(--page-mark);
  }

  /* Selected or hovered, among a page full of outlines: heavier, and haloed so
     the dashes survive the artwork underneath. The halo is dark now, not the
     white --page-halo: the blue carries itself over black ink, and it is pale
     panels and white gutters that need the rim. */
  .region.lit {
    outline-width: 2px;
    box-shadow: 0 0 0 1px var(--page-mark-halo);
  }

  /* Declined: always, whatever the overlay and the filter say. Ordered after
     the two rules above, which carry the same specificity, so the warn colour
     wins on a region that is both declined and lit. */
  .region.declined {
    outline-color: var(--page-warn);
  }

  /* Needs review: only when the overlay or the review filter put its badge on. */
  .region.review.marked {
    outline-style: dotted;
    outline-color: var(--page-line);
  }

  /* The sheet is --paper in both themes, so the global --accent focus ring
     would disappear on the page in dark. */
  .region:focus-visible {
    outline: 2px solid var(--page-mark);
    outline-offset: 2px;
  }

  .badge {
    position: absolute;
    top: -9px;
    right: -9px;
    display: flex;
    align-items: center;
    justify-content: center;
    width: 15px;
    height: 15px;
    border-radius: var(--r-xs);
    background: var(--paper);
    border: 1px solid var(--page-line);
    color: var(--page-ink);
    font-size: 10px;
    font-weight: 700;
    line-height: 1;
  }
  .region.declined .badge {
    border-color: var(--page-warn);
    color: var(--page-warn);
  }
</style>
