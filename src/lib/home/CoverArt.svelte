<script>
  import { tileUrl } from '../api/tile.js'

  /**
   * A project's cover: its first page, as a real tile over `tile://`.
   *
   * Cropped rather than stretched - the card's box is 2:3 and a longstrip page
   * is nothing like it - and anchored to the top, because the top of page one
   * is the part of a chapter a reader recognises.
   *
   * **The first proxy tile, not the page.** For a card the two
   * are the same picture: `object-fit: cover` anchored to the top shows the top
   * of the page either way, and asking for tile 0 is the difference between
   * fetching the first 2048 proxy rows of a webtoon segment and fetching all
   * twenty thousand of them for a thumbnail 96px tall.
   *
   * The stand-in below is the second half of the same arrangement
   * `PageArtwork` describes: `tileUrl` answers `null` in a plain browser and in
   * every frontend test, where the mock backend has no pixels, so the card
   * falls back to the panel geometry of the page - real `panels` from the
   * adapter, not decoration invented here, so two mock projects still look
   * different. It goes when the mock does.
   *
   * @type {{
   *   page?: (import('../api/backend.js').ApiPage)|null,
   * }}
   */
  let { page = null } = $props()

  const src = $derived(tileUrl(page, 'cleaned', 0))
  const panels = $derived(page?.panels ?? [])
  const seed = $derived(page?.layout ?? 0)
</script>

<span class="art" aria-hidden="true">
  {#if src}
    <img class="cover" {src} alt="" decoding="async" draggable="false" />
  {:else}
    {#each panels as panel, i (i)}
      <span
        class="panel"
        class:alt={(seed + i) % 2 === 1}
        style:left="{panel.x}%"
        style:top="{panel.y}%"
        style:width="{panel.w}%"
        style:height="{panel.h}%"
      ></span>
    {/each}
  {/if}
</span>

<style>
  .art { position: absolute; inset: 0; display: block }
  .panel { position: absolute; display: block; background: var(--art2) }
  .panel.alt { background: var(--art) }

  .cover {
    display: block;
    width: 100%;
    height: 100%;
    object-fit: cover;
    object-position: top;
  }
</style>
