<script>
  import { editor } from '../state/editor.svelte.js'
  import { pageRatio } from './zoom.js'
  import { isDrawingTool } from './tools.js'
  import DrawLayer from './DrawLayer.svelte'
  import PageArtwork from './PageArtwork.svelte'
  import RegionLayer from './RegionLayer.svelte'

  /**
   * One page: the `--paper` sheet, its shadow, its aspect ratio, and the stack
   * of layers on it. The sheet is **the one thing in the centre that floats**;
   * the field around it carries no shadow at all: the image is the brightest
   * thing on screen, and in dark the field must not glow against the artwork.
   *
   * The stack, bottom to top:
   *
   *   PageArtwork variant="original"   the source page
   *   PageArtwork variant="cleaned"    clipped by the wipe
   *   RegionLayer                      region outlines, markers, hit targets
   *   the wipe divider
   *
   * **The wipe is a clip, not a predicate.** Two images, one of them cut off
   * at `editor.wipe`, is how the real thing will work once `PageArtwork` is an
   * `<img>` - and it is why a region is not asked whether it is left or right
   * of the wipe. The prototype's `(r.x + r.w/2) <= wipe` test is a prototype
   * shortcut. Holding or pinning the original (`editor.originalVisible`) clips
   * the cleaned layer away entirely and suppresses the divider: the source is
   * shown outright, not wiped to zero.
   */

  /**
   * @type {{
   *   page: import('../api/backend.js').ApiPage,
   *   width: number,
   *   current?: boolean,
   *   strip?: boolean,
   *   tabbable?: boolean,
   * }}
   */
  let { page, width, current = true, strip = false, tabbable = true } = $props()

  const ratio = $derived(pageRatio(page))
  const wipe = $derived(editor.wipe)
  const original = $derived(editor.originalVisible)

  // `inset(top right bottom left)`: the cleaned page occupies everything left
  // of the divider, so the right inset is whatever the wipe has not reached.
  const clip = $derived(original ? 'inset(0 100% 0 0)' : `inset(0 ${100 - wipe}% 0 0)`)
  const divider = $derived(!original && wipe < 100)

  // The four drag tools put a drawing surface over the sheet. It covers the
  // region buttons, so `RegionLayer` yields the *pointer* while it is up - a
  // drag has to be able to start over a region, and erasing means starting
  // over one deliberately. The buttons keep their tab stops either way, and
  // the surface routes a tap straight back to the region under it, so the
  // region-click seam survives underneath.
  const drawing = $derived(isDrawingTool(editor.tool))
</script>

<div
  class="sheet"
  class:marked={strip && current}
  style:width="{width}px"
  style:aspect-ratio="1 / {ratio}"
>
  <PageArtwork {page} variant="original" />
  <div class="cleaned" style:clip-path={clip}>
    <PageArtwork {page} variant="cleaned" />
  </div>

  <RegionLayer {page} {tabbable} interactive={!drawing} />

  {#if drawing}
    <DrawLayer {page} {tabbable} />
  {/if}

  {#if divider}
    <div class="divider" style:left="{wipe}%" aria-hidden="true"></div>
  {/if}
</div>

<style>
  .sheet {
    position: relative;
    flex: none;
    background: var(--paper);
    box-shadow: var(--page-shadow);
    /* Lets everything drawn on the page size itself from the page's own width
       (`cqw`) instead of taking the zoom as a prop - which is what keeps
       `PageArtwork` swappable for an <img>. */
    container-type: inline-size;
  }

  /* The strip's current position (design file: the canvas page's outline). */
  .sheet.marked {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }

  .cleaned {
    position: absolute;
    inset: 0;
  }

  .divider {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 1px;
    background: var(--page-divider);
    box-shadow: 0 0 0 1px var(--page-halo);
    pointer-events: none;
  }
</style>
