<script>
  import { proxyPlan, tileUrl } from '../api/tile.js'

  /**
   * ==========================================================================
   * THE PAGE. A tile over `tile://` where there is one, the mock's stand-in
   * artwork where there is not.
   * ==========================================================================
   *
   * It is the only module **in the editor** that reads `ApiPage.layout`,
   * `ApiPage.panels`, `ApiRegion.kind` and `ApiRegion.text` - the stand-in
   * image fields Task 3 flagged in its report as mock-canvas-only fields the
   * canvas task must isolate. `ApiRegion.kind` and `ApiRegion.text` are
   * read here and nowhere else in the app; **`ApiPage.panels` and
   * `ApiPage.layout` have a second reader**, `src/lib/home/CoverArt.svelte`
   * (fed by `home/ProjectCard.svelte`), which draws a project's cover from the
   * same panel geometry under the same exception. That is why the tile swap
   * was **two** sites and not one.
   *
   * It also spends the colour exception `constraints.md` grants, and it spends
   * it only on things that are *image* data: the paper of a speech bubble and
   * the ink of the text in it. Everything else on the sheet is a token.
   *
   * **The real page has arrived, and it is a stack of `<img>` over `tile://`.**
   * `tileUrl` (src/lib/api/tile.js) answers with a URL inside a Tauri window
   * and with `null` everywhere else - in a plain browser, and in every one of
   * the frontend's tests, where there is no protocol to serve pixels and the
   * mock backend is still the one answering. So both paths are here, and the
   * stand-in below is not dead code: it is what the mock renders, and the mock
   * still ships as `setBackend`'s fallback. It goes when the
   * mock does, not before.
   *
   * **A stack and not one image**: the UI holds
   * proxies, never the strip. A webtoon segment is 800×20000, which is one
   * `<img>` of about 64 MB decoded, held for as long as the page is mounted and
   * multiplied by every page the strip's virtual window keeps. So the page
   * arrives as `proxyPlan`'s tiles - the short edge capped at 1024, the long
   * axis cut - each absolutely positioned over its own share of the sheet and
   * `loading="lazy"`, so the browser fetches, decodes and evicts them by what
   * is actually on screen. The virtual window in `strip.js` does the same job
   * one level up, between pages; this is the same rule *inside* a page, which
   * is where a longstrip segment needs it.
   *
   * The tiles overlap by a pixel, except the last. Each `<img>` is stretched to
   * a box whose height is a percentage, and percentages land on fractional
   * device pixels: without the overlap a hairline of the sheet's `--paper`
   * shows through at every join. The tile below is painted after the tile above
   * and covers the extra row, so nothing is displaced - one row of a downscaled
   * image is stretched by well under a tenth of a percent.
   *
   * The swap stayed inside this component rather than moving to
   * `PageSheet.svelte` for the reason the original note gives from the other
   * side: the props are the page and the variant and nothing else, so this is
   * the only file that has to know which of the two it is drawing. It draws no
   * markers, no masks, no selection and no page tag - those are the app's, and
   * they live in `RegionLayer` and `PageSheet` - and it scales its type from
   * the sheet's own width (`cqw`) rather than taking the zoom as a prop, which
   * the `<img>` has no use for.
   *
   * `original` is this component's word and `source` is the protocol's; the
   * mapping happens once, below, so the URL has one spelling
   * (`src-tauri/src/tile.rs` fixes it).
   *
   * **The two variants are two images, not one image with toggles.**
   * `original` is the source page: every region's text is on it. `cleaned` is
   * what the pipeline produced: the text is gone from the regions that were
   * masked, and still there on every region that was not - declined,
   * gate-skipped, pending and undetected regions were never touched, so their
   * text survives into the cleaned page. That is how the real pair of tiles
   * differs - `tile.rs` composites the page's visible patches for `cleaned`
   * and nothing at all for `source` - and it is what makes the wipe a clip over
   * two layers rather than a per-region predicate.
   */

  /**
   * @type {{
   *   page: import('../api/backend.js').ApiPage,
   *   variant: 'original'|'cleaned',
   * }}
   */
  let { page, variant } = $props()

  const plan = $derived(proxyPlan(page))

  /**
   * One `<img>` per proxy tile, each carrying the CSS that puts it over its own
   * share of the sheet. Empty outside a Tauri window, which is what selects the
   * stand-in below.
   */
  const tiles = $derived(
    plan.tiles
      .map((tile, at) => {
        const overlap = at + 1 < plan.tiles.length ? ' + 1px' : ''
        const extent = `calc(${tile.extent}%${overlap})`
        return {
          index: tile.index,
          src: tileUrl(page, variant === 'original' ? 'source' : 'cleaned', tile.index),
          top: plan.vertical ? `${tile.offset}%` : '0',
          left: plan.vertical ? '0' : `${tile.offset}%`,
          width: plan.vertical ? '100%' : extent,
          height: plan.vertical ? extent : '100%',
        }
      })
      .filter((tile) => tile.src !== null),
  )

  const panels = $derived(page.panels ?? [])

  /** A region keeps its text unless something was actually applied to it. */
  const regions = $derived(
    (page.regions ?? []).map((region) => ({
      id: region.id,
      bbox: region.bbox,
      bubble: region.kind === 'bubble',
      sfx: region.kind === 'sfx',
      text: region.text ?? '',
      inked: variant === 'original' || !region.mask,
    })),
  )
</script>

<!-- `data-artwork` is a stable hook rather than a styling class: `PaintLayer`
     samples these very tiles for clone / heal's live preview and has to be able
     to find the `cleaned` stack inside this page's sheet without depending on
     Svelte's scoped class names. -->
<div class="artwork" data-artwork={variant === 'original' ? 'source' : 'cleaned'} aria-hidden="true">
  {#if tiles.length > 0}
    <!-- The page, in proxy tiles, decoded by the browser rather than by us.
         `alt` is empty and the container is already `aria-hidden`: the scan
         carries no information the interface can name, and every mark that does
         is a control in `RegionLayer`. -->
    {#each tiles as tile (tile.index)}
      <img
        class="scan"
        src={tile.src}
        alt=""
        decoding="async"
        loading="lazy"
        draggable="false"
        style:top={tile.top}
        style:left={tile.left}
        style:width={tile.width}
        style:height={tile.height}
      />
    {/each}
  {:else}
    {#each panels as panel, index (index)}
      <div
        class="panel"
        class:alt={((page.number ?? 1) + index) % 2 === 0}
        style:left="{panel.x}%"
        style:top="{panel.y}%"
        style:width="{panel.w}%"
        style:height="{panel.h}%"
      ></div>
    {/each}

    {#each regions as region (region.id)}
      <div
        class="bubble"
        class:round={region.bubble}
        style:left="{region.bbox.x}%"
        style:top="{region.bbox.y}%"
        style:width="{region.bbox.w}%"
        style:height="{region.bbox.h}%"
      >
        {#if region.inked}
          <!-- Japanese sample text is image data, not copy: it is what the scan
               says, so it does not go through t() (Task 3 documented this). -->
          <div class="text" class:sfx={region.sfx}>{region.text}</div>
        {/if}
      </div>
    {/each}
  {/if}
</div>

<style>
  .artwork {
    position: absolute;
    inset: 0;
    overflow: hidden;
  }

  /* The sheet already carries the page's own aspect ratio (`PageSheet` takes it
     from `page.width`/`page.height`), and each tile is given its own share of
     it inline, so a tile fills its box exactly and `object-fit` never has
     anything to decide. `draggable="false"` above keeps a drag over the page
     belonging to the editor's gestures rather than starting a native image
     drag. */
  .scan {
    position: absolute;
    display: block;
  }

  .panel {
    position: absolute;
    background: var(--art);
    border-radius: 1px;
  }
  .panel.alt { background: var(--art2) }

  /* Below here is the colour exception: a bubble's paper and its ink are the
     photograph, not the interface. Both are fixed in light and dark, because a
     scan does not change when the application's theme does. */
  .bubble {
    position: absolute;
    display: flex;
    align-items: center;
    justify-content: center;
    border-radius: 2px;
    background: rgba(255, 255, 255, .92);
  }
  .bubble.round {
    border-radius: 50%;
    background: #fff;
    border: 1px solid rgba(30, 28, 24, .45);
  }

  .text {
    writing-mode: vertical-rl;
    font-size: max(8px, 2.6cqw);
    line-height: 1.15;
    letter-spacing: .05em;
    font-weight: 500;
    color: #111;
    max-height: 88%;
    overflow: hidden;
  }
  .text.sfx { font-weight: 700 }
</style>
