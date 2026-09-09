/**
 * `tile://` URLs - the one place the interface builds one.
 *
 * Pixels reach the browser over a custom protocol
 * rather than as base64 in an IPC payload, and the UI uses plain `<img>`. The
 * Rust half is `src-tauri/src/tile.rs`; this half decides only *what to ask
 * for*, and the two agree on one shape:
 *
 *     tile://localhost/<chapterId>/<pageIndex>/<variant>[/<tile>]?v=<version>
 *     http://tile.localhost/<chapterId>/<pageIndex>/<variant>[/<tile>]?v=<version>
 *
 * `variant` is `source` or `cleaned`.
 *
 * ## What the interface asks for is a proxy, never the page
 *
 * *The UI holds proxies, never the strip.* Previews cap the
 * **short** edge at 1024 - capping the long edge would draw an 800×20000 page
 * as a 40×1024 sliver - and tile the long axis, so a page reaches the webview
 * as a handful of independently fetched, decoded and evicted images rather than
 * as one 64 MB bitmap. [`proxyPlan`] is this side's copy of that arithmetic and
 * `src-tauri/src/tile.rs` serves against `cleaner_core::image::proxy`'s. The
 * fourth path segment is the tile index; a URL without one is the page at its
 * own resolution, which rule 7 reserves for the viewport past 100% zoom.
 *
 * Two copies of one rule is a drift risk taken deliberately, on the same terms
 * as `TILE_SCHEME` below: the alternative is putting the tile geometry on the
 * seam, which is a seam change. They are pinned to the same numbers by a test
 * on each side, but the risk a drift carries is not symmetric.
 *
 * A drift where this side's plan asks for **more** tiles than Rust's is loud:
 * the extra index is a `404` and a broken `<img>`, never a wrong picture. A
 * drift the other way is not. If this side's plan is *smaller* - this file's
 * `PROXY_TILE_LONG_EDGE` raised without the Rust one moving too, or a page
 * reaching [`proxyPlan`] with no declared dimensions - this side asks for a
 * subset of the tiles the real page has, and `PageArtwork.svelte` stretches
 * each one it does get over `extent%` of its own, too-small `proxyLong`. The
 * page renders. It renders wrong, and nothing here says so. The missing-
 * dimensions half of that is the one this file can close without a seam
 * change: [`proxyPlan`] refuses - answers no tiles - rather than guessing one
 * whole tile that does not match what Rust actually cut. No tiles is the same
 * stand-in path `PageArtwork` already takes when `tileUrl` returns `null`; one
 * guessed tile is a confident wrong picture.
 *
 * ## The origin comes from Tauri, not from a platform test here
 *
 * The origin differs by platform - `tile://localhost/` on macOS, iOS and
 * Linux, `http://tile.localhost/` on Windows and Android. Tauri already makes
 * exactly this choice, in `convertFileSrc`, from the operating system it was
 * compiled for and from whether the window was built with `use_https_scheme`.
 * Asking it is therefore better than sniffing a user agent: a user agent is a
 * guess, this is the answer, and it cannot drift from what the webview actually
 * registered. It also means a machine this code has never run on - Windows and
 * Linux are both unexecuted here - gets the right origin without
 * anybody having predicted it.
 *
 * `convertFileSrc('')` is used for the origin alone. It is not used to build
 * the whole URL, because it percent-encodes its argument as a **single** path
 * segment, and this path has three.
 *
 * ## `v` is a hash, never a timestamp
 *
 * A hash decides staleness, an mtime only explains it. The token
 * is folded from the page's own content hashes - the source `sha256` and, for
 * `cleaned`, the `mask_sha256` and revision of every mask on the page - so the
 * URL changes exactly when the bytes behind it change, and never otherwise. The
 * protocol handler can then answer `Cache-Control: immutable`, which is the
 * whole reason the token exists: without it the webview would either re-fetch
 * every page on every render or show a stale one after an edit.
 *
 * The fold is not a cryptographic hash and does not need to be. Its inputs
 * already are; its job is to make a short cache key out of them.
 */

/** The scheme, matching `tile::SCHEME` in `src-tauri/src/tile.rs`. */
export const TILE_SCHEME = 'tile'

/**
 * The short edge's ceiling for a preview. Matches
 * `cleaner_core::image::proxy::PROXY_SHORT_EDGE`.
 */
export const PROXY_SHORT_EDGE = 1024

/**
 * How much of the long axis one tile covers, in proxy pixels. Matches
 * `cleaner_core::image::proxy::PROXY_TILE_LONG_EDGE`.
 *
 * **Provisional.** What decides it is how many
 * bytes WebView2's UI thread can absorb in one response - it raises
 * `WebResourceRequested` on that thread and pauses page load while the handler
 * runs, at a maintainer-measured ~200 ms for 10 MB against ~5 ms on macOS
 * - and that measurement has never been taken on a Windows
 * machine here. That is what happens when a one-machine number is
 * written down as a general one, so this is not tuned; it is bounded, at the
 * next power of two above the short-edge cap. It is folded into the version
 * token below, so changing it cannot leave a differently-cut tile behind in an
 * immutable cache.
 */
export const PROXY_TILE_LONG_EDGE = 2048

/**
 * The two variants the protocol serves. `original` is `PageArtwork`'s word for
 * the same thing and is mapped at that one call site rather than accepted here,
 * so the URL vocabulary has one spelling.
 *
 * @typedef {'source'|'cleaned'} TileVariant
 */

/**
 * Where tiles come from on this platform, with a trailing slash, or `null`
 * outside a Tauri window.
 *
 * `null` is not a failure: the mock backend still ships as `setBackend`'s
 * fallback and has no pixels at all, so a component that gets `null` draws its
 * stand-in artwork. That is what keeps the interface and its 432 tests working
 * in a plain browser.
 *
 * @returns {string|null}
 */
export function tileOrigin() {
  const convert = /** @type {any} */ (globalThis).__TAURI_INTERNALS__?.convertFileSrc
  if (typeof convert !== 'function') return null
  const origin = convert('', TILE_SCHEME)
  return typeof origin === 'string' && origin.endsWith('/') ? origin : null
}

/**
 * @typedef {Object} ProxyTile
 * @property {number} index - the URL's fourth segment
 * @property {number} offset - where the tile starts along the long axis, in percent
 * @property {number} extent - how much of the long axis it covers, in percent
 */

/**
 * @typedef {Object} ProxyPlan
 * @property {number} width - the proxy's width in px
 * @property {number} height - the proxy's height in px
 * @property {boolean} vertical - whether the tiled long axis is `y`
 * @property {ProxyTile[]} tiles - in order, covering the page exactly
 */

/**
 * How one page is cut into preview tiles - the same arithmetic as
 * `cleaner_core::image::proxy::ProxyPlan::for_page`, in integers so the two can
 * agree exactly rather than nearly.
 *
 * A page that declares no dimensions gets **no tiles**, not one tile guessed
 * at 1×1. Answering one whole tile here would ask the protocol for a real
 * tile index - 0 - and stretch whatever it returns over 100% of the sheet,
 * which is a wrong picture rendered with confidence (see the module header).
 * Refusing instead means `tiles` comes back empty, `PageArtwork.svelte` never
 * emits an `<img>`, and the sheet falls through to the same stand-in path it
 * already uses when `tileUrl` has no protocol to call. That path is honest
 * about not having pixels; a guessed tile is not. Today `library.rs` always
 * supplies `source.w`/`source.h` and the seam types are non-optional, so this
 * branch does not fire - that makes the refusal cheap to keep, not safe to
 * drop, because the invariant lives in the manifest's types and not in
 * anything this file controls.
 *
 * @param {{width?: number, height?: number}|null|undefined} page
 * @returns {ProxyPlan}
 */
export function proxyPlan(page) {
  const pageWidth = dimension(page?.width)
  const pageHeight = dimension(page?.height)
  if (pageWidth === null || pageHeight === null) {
    return { width: 0, height: 0, vertical: false, tiles: [] }
  }
  const short = Math.min(pageWidth, pageHeight)
  const long = Math.max(pageWidth, pageHeight)

  // A cap, never an enlargement: a preview of a 400×600 page is 400×600.
  const proxyShort = Math.min(short, PROXY_SHORT_EDGE)
  const proxyLong =
    proxyShort === short ? long : Math.max(proxyShort, Math.round((long * proxyShort) / short))

  const vertical = pageHeight >= pageWidth
  const count = Math.max(1, Math.ceil(proxyLong / PROXY_TILE_LONG_EDGE))

  /** @type {ProxyTile[]} */
  const tiles = []
  for (let index = 0; index < count; index += 1) {
    const start = index * PROXY_TILE_LONG_EDGE
    const size = Math.min(PROXY_TILE_LONG_EDGE, proxyLong - start)
    tiles.push({ index, offset: (start / proxyLong) * 100, extent: (size / proxyLong) * 100 })
  }

  return {
    width: vertical ? proxyShort : proxyLong,
    height: vertical ? proxyLong : proxyShort,
    vertical,
    tiles,
  }
}

/**
 * The URL for one page's tile, or `null` when there is no protocol to serve it
 * or no page to name.
 *
 * `tile` is a proxy tile index - what every drawing component asks for. Leaving
 * it out asks for the page at its own resolution, which is reserved
 * for the viewport past 100% zoom.
 *
 * @param {import('./backend.js').ApiPage|null|undefined} page
 * @param {TileVariant} variant
 * @param {number} [tile]
 * @returns {string|null}
 */
export function tileUrl(page, variant, tile) {
  const origin = tileOrigin()
  if (!origin || !page || !page.chapterId || !Number.isInteger(page.index)) return null
  if (tile !== undefined && !(Number.isInteger(tile) && tile >= 0)) return null
  const chapter = encodeURIComponent(page.chapterId)
  const path = tile === undefined ? '' : `/${tile}`
  return `${origin}${chapter}/${page.index}/${variant}${path}?v=${pageVersion(page, variant)}`
}

/**
 * The cache-busting token for one page and variant.
 *
 * `source` depends on the source file alone. `cleaned` depends on the source
 * *and* on every mask composited over it, which is the page's patch set - so a
 * re-run, a deleted mask or a hand-drawn one all move the token, and nothing
 * else does. Masks are folded in id order rather than array order, because the
 * backend is free to return regions in a different order without the page
 * having changed.
 *
 * **The tile geometry is folded in too**, because it decides what a tile index
 * *means*. `Cache-Control: immutable` pins a response for a year; if
 * `PROXY_TILE_LONG_EDGE` ever moves, tile 3 becomes a different band of the
 * page under the same URL, and the webview would keep drawing the old one. One
 * provisional constant in the token is what makes changing it safe -
 * **but the token folds in this file's own copy of that constant, not
 * Rust's.** `cleaner_core::image::proxy` has the number it actually cuts
 * tiles with; this is a second, independent declaration of the same value
 * (see `PROXY_TILE_LONG_EDGE` above). Moving Rust's alone changes what tile 3
 * *is* without changing this token, so the immutable cache keeps serving the
 * old band under a URL that never invalidated - the exact failure this
 * paragraph exists to prevent, reopened from the one side the token cannot
 * see. What actually prevents it today is discipline - moving both constants
 * in the same change - not machinery; a shared constant across the language
 * boundary would make it machinery, and that is a seam-level design change,
 * not something to patch here.
 *
 * **Visibility is not folded in, and that is a real gap, not a design
 * choice.** Neither `Mask` (`src/lib/model/types.js`) nor `ApiMask` carries
 * whether a patch is shown, so there is nothing here to fold; this token is
 * built only from `id`, `sequence` and `mask_sha256`. `src-tauri/src/tile.rs`
 * does honour `record.visible` when it composites, so toggling a patch off
 * changes the bytes the protocol serves without changing the URL that names
 * them - and `Cache-Control: immutable` (above) then pins the stale tile.
 * Putting visibility on the seam is a seam change, not something to decide
 * here.
 *
 * @param {import('./backend.js').ApiPage} page
 * @param {TileVariant} variant
 * @returns {string}
 */
export function pageVersion(page, variant) {
  const parts = [`${PROXY_SHORT_EDGE}x${PROXY_TILE_LONG_EDGE}`, page.sourceSha ?? '']
  if (variant === 'cleaned') {
    const masks = (page.regions ?? [])
      .map((region) => region.mask)
      .filter((mask) => mask != null)
      .map(
        (mask) =>
          `${mask.id}:${mask.sequence}:${mask.provenance?.mask_sha256 ?? ''}:${mask.provenance?.created ?? ''}:${mask.provenance?.engine ?? ''}`,
      )
      .sort()
    parts.push(...masks)
  }
  return fold(parts.join('|'))
}

/**
 * FNV-1a, twice, at two offsets - sixteen hex characters.
 *
 * `Math.imul` rather than `BigInt` because this runs inside a `$derived` for
 * every page on screen, and a longstrip column has forty of them in two
 * variants each.
 *
 * @param {string} text
 * @returns {string}
 */
export function fold(text) {
  return word(text, 0x811c9dc5) + word(text, 0x9e3779b9)
}

/**
 * A dimension as a positive whole number of pixels, or `null` when the page
 * does not declare one - the refusal `proxyPlan` acts on above.
 *
 * @param {unknown} value
 * @returns {number|null}
 */
function dimension(value) {
  const number = Math.round(Number(value))
  return Number.isFinite(number) && number > 0 ? number : null
}

/**
 * @param {string} text
 * @param {number} offset
 * @returns {string}
 */
function word(text, offset) {
  let hash = offset
  for (let i = 0; i < text.length; i += 1) {
    hash ^= text.charCodeAt(i)
    hash = Math.imul(hash, 0x01000193)
  }
  return (hash >>> 0).toString(16).padStart(8, '0')
}
