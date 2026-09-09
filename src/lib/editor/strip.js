/**
 * The longstrip column's arithmetic: how tall it is, which of its pages are
 * worth mounting, which ones are on screen, and which one the reader is
 * actually looking at.
 *
 * Pure - no Svelte, no DOM. Nothing in the fixtures is long enough to exercise
 * the virtualisation threshold in a browser, so the tests are the proof.
 *
 * **Every page is measured, none is assumed.** Revision 1 of this module took
 * one `unit` - page 0's height plus the gap - and multiplied it, the same trade
 * `pagerows.js#rowWindow` makes for the Pages list. That trade is sound there,
 * where a row is 25px of text by construction, and it was never sound here: it
 * was true of the mock's fixtures (800×4000 throughout) and of nothing else.
 * A webtoon chapter's segments are not uniform **by construction** - the strip
 * splits at content minima inside a ±500px tolerance band, so
 * consecutive segments differ by hundreds of pixels - and one unit standing for
 * all of them mis-sized the column *silently*: spacers, the position readout
 * and `goToPage` all drifting together, with a dev-mode `console.warn` as the
 * only sign.
 *
 * So the geometry is a table, not a multiplication. [`stripMetrics`] builds it
 * once per (page list, scale) and everything else reads it:
 *
 *     widths[i]    the sheet's drawn width, from page i's own width
 *     units[i]     page i's drawn height plus the gap under it
 *     offsets[i]   the top of page i, offsets[count] being the whole column
 *
 * **Geometry.** Every page occupies one `unit` - its own height plus the gap
 * below it - and the column carries no CSS `gap` at all: each slot owns its
 * bottom margin. That is what makes conservation exact and obvious,
 *
 *     padTop + sum(units[start..end]) + padBottom === total
 *
 * with no "…except the first and last gap" clause to get wrong. The one cost
 * is 12px of dead space under the final page, which the viewport's own 66px of
 * bottom padding swallows.
 *
 * That identity is true *by construction* here - `padTop` is `offsets[start]`
 * and `padBottom` is `total - offsets[end]` - so the test that asserts it
 * cannot fail. What it does not cover, and what would actually break the
 * column, is the other half of the model: that the rendered band really
 * occupies those pixels. That half lives in `CanvasStage.svelte`'s CSS -
 * `.slot { margin-bottom: var(--strip-gap) }` and **no `gap` on `.stage`** -
 * in `units` being derived from the same `STRIP_GAP` the slot is drawn with,
 * and in each `PageSheet` being handed `widths[i]` rather than a width of its
 * own. Add a CSS `gap`, or a border on a slot, and every assertion in
 * `strip.test.js` still passes while the column drifts a gap per page.
 *
 * **Position.** `columnTop` is the column's top edge measured from the top of
 * the viewport - negative once the reader has scrolled into it. Taking it from
 * the rendered box rather than from `scrollTop` keeps the viewport's padding
 * out of the arithmetic entirely.
 */

import { naturalWidth, pageRatio, sheetWidth } from './zoom.js'

/** The design file's column gap. */
export const STRIP_GAP = 12

/** Below this many pages the column renders whole. */
export const VIRTUAL_THRESHOLD = 12

/** Pages kept mounted above and below the visible band. */
export const OVERSCAN = 1

/**
 * One page's share of the column: its height plus the gap under it.
 *
 * @param {{pageHeight: number, gap?: number}} spec
 * @returns {number} px
 */
export function stripUnit(spec) {
  const height = Math.max(1, finite(spec.pageHeight, 1))
  return height + Math.max(0, finite(spec.gap, STRIP_GAP))
}

/**
 * @typedef {Object} StripMetrics
 * @property {number} count - pages in the column
 * @property {number[]} widths - each sheet's drawn width, px
 * @property {number[]} units - each page's drawn height plus the gap, px
 * @property {number[]} offsets - `count + 1` cumulative tops; the last is `total`
 * @property {number} total - the whole column's height, px
 */

/**
 * The column's geometry at one scale: what every page is drawn at, and where
 * every page starts.
 *
 * `scale` is the fraction of a page's own pixels the sheet is drawn at - the
 * same unit `zoom.js` uses, and the same number the bottom pill reports - so a
 * page's drawn width is its own width times the scale and never the column's.
 * Two pages of different widths therefore stay different widths, which is what
 * the gutter means on screen: a 700px page beside an 800px one is
 * narrower, not stretched.
 *
 * `sheetWidth` and `pageRatio` are imported rather than re-derived so that the
 * arithmetic here and the sheet `CanvasStage.svelte` actually draws come from
 * one expression. `widths[i]` is passed straight to `PageSheet`, so the two
 * cannot disagree about how big a page is.
 *
 * @param {{pages?: Array<{width?: number, height?: number}>, scale?: number, gap?: number}} spec
 * @returns {StripMetrics}
 */
export function stripMetrics(spec) {
  const gap = Math.max(0, finite(spec.gap, STRIP_GAP))
  const scale = finite(spec.scale, 1)
  const pages = Array.isArray(spec.pages) ? spec.pages : []

  /** @type {number[]} */ const widths = []
  /** @type {number[]} */ const units = []
  /** @type {number[]} */ const offsets = [0]

  for (const page of pages) {
    const width = sheetWidth({ natural: naturalWidth(page), zoom: scale })
    const unit = stripUnit({ pageHeight: width * pageRatio(page), gap })
    widths.push(width)
    units.push(unit)
    offsets.push(offsets[offsets.length - 1] + unit)
  }

  return { count: pages.length, widths, units, offsets, total: offsets[offsets.length - 1] }
}

/**
 * The page whose band contains `y`, clamped to the column. Binary search
 * rather than a division, because the offsets are no longer a multiple of
 * anything.
 *
 * @param {StripMetrics} metrics
 * @param {number} y
 * @returns {number}
 */
function indexAt(metrics, y) {
  if (metrics.count <= 0) return 0
  let low = 0
  let high = metrics.count - 1
  while (low < high) {
    const mid = (low + high + 1) >> 1
    if (metrics.offsets[mid] <= y) low = mid
    else high = mid - 1
  }
  return low
}

/**
 * @typedef {Object} StripWindow
 * @property {boolean} virtual - false when every page is mounted
 * @property {number} start - first mounted index
 * @property {number} end - one past the last mounted index
 * @property {number} padTop - px of spacer above
 * @property {number} padBottom - px of spacer below
 */

/**
 * Which pages to mount for a viewport `viewportHeight` px tall whose top edge
 * sits `-columnTop` px into the column `metrics` describes.
 *
 * `include` is the page that must stay mounted whatever the scroll position -
 * the one holding focus. Unmounting the focused element drops focus to the
 * document, and a strip that loses the keyboard the moment it scrolls is worse
 * than one that is not virtualised at all.
 *
 * @param {{
 *   metrics?: StripMetrics,
 *   columnTop?: number,
 *   viewportHeight?: number,
 *   include?: number,
 *   overscan?: number,
 *   threshold?: number,
 * }} spec
 * @returns {StripWindow}
 */
export function stripWindow(spec) {
  const metrics = spec.metrics ?? EMPTY
  const count = metrics.count
  const threshold = finite(spec.threshold, VIRTUAL_THRESHOLD)
  if (count <= threshold) return { virtual: false, start: 0, end: count, padTop: 0, padBottom: 0 }

  const overscan = Math.max(0, Math.trunc(finite(spec.overscan, OVERSCAN)))
  const top = clamp(-finite(spec.columnTop, 0), 0, metrics.total)
  const height = clamp(finite(spec.viewportHeight, 0), 0, metrics.total)

  let start = Math.max(0, indexAt(metrics, top) - overscan)
  let end = Math.min(count, indexAt(metrics, top + height) + 1 + overscan)

  const include = spec.include
  if (Number.isFinite(include) && include >= 0 && include < count) {
    start = Math.min(start, /** @type {number} */ (include))
    end = Math.max(end, /** @type {number} */ (include) + 1)
  }
  if (end <= start) end = Math.min(count, start + 1)

  return {
    virtual: true,
    start,
    end,
    padTop: metrics.offsets[start],
    padBottom: metrics.total - metrics.offsets[end],
  }
}

/**
 * The positions actually on screen, in strip order - what `scopePageIndices()`
 * answers for a longstrip chapter, and therefore what the Layers panel scopes
 * itself to.
 *
 * A page counts as on screen when any part of it overlaps the viewport, so a
 * viewport that stops exactly on the next page's first row does not include
 * it. The result is never empty for a chapter that has pages: a viewport
 * shorter than one page still has exactly one page in it.
 *
 * @param {{metrics?: StripMetrics, columnTop?: number, viewportHeight?: number}} spec
 * @returns {number[]}
 */
export function visibleIndices(spec) {
  const metrics = spec.metrics ?? EMPTY
  if (metrics.count === 0) return []
  const top = -finite(spec.columnTop, 0)
  const bottom = top + Math.max(0, finite(spec.viewportHeight, 0))

  const first = indexAt(metrics, top)
  let last = indexAt(metrics, bottom)
  if (last > first && metrics.offsets[last] >= bottom) last -= 1

  const indices = []
  for (let index = first; index <= last; index += 1) indices.push(index)
  return indices
}

/**
 * Which page occupies the centre of the viewport - the strip's answer to
 * "which page am I on", and what the bottom pill's readout follows.
 *
 * @param {{metrics?: StripMetrics, columnTop?: number, viewportHeight?: number}} spec
 * @returns {number} a page index, or 0 for an empty chapter
 */
export function centreIndex(spec) {
  const metrics = spec.metrics ?? EMPTY
  if (metrics.count === 0) return 0
  const centre = -finite(spec.columnTop, 0) + finite(spec.viewportHeight, 0) / 2
  return indexAt(metrics, centre)
}

/**
 * How far to scroll so that page `index` sits at the top of the viewport -
 * the "scroll the strip to this position" half of `goToPage`.
 *
 * `index === count` is a position too: the bottom of the column.
 *
 * @param {{index: number, metrics?: StripMetrics, scrollTop?: number, columnTop?: number}} spec
 * @returns {number} the scroller's new `scrollTop`
 */
export function scrollTopFor(spec) {
  const metrics = spec.metrics ?? EMPTY
  const index = clamp(Math.trunc(finite(spec.index, 0)), 0, metrics.count)
  // Where the column's top sits in the scroller's own coordinates: its current
  // offset from the viewport plus however far the scroller is already down.
  const columnOrigin = finite(spec.columnTop, 0) + finite(spec.scrollTop, 0)
  return Math.max(0, Math.round(columnOrigin + metrics.offsets[index]))
}

/** The column of a chapter with no pages. Shared, and never mutated. */
const EMPTY = /** @type {StripMetrics} */ (
  Object.freeze({ count: 0, widths: [], units: [], offsets: [0], total: 0 })
)

/**
 * @param {number} value
 * @param {number} min
 * @param {number} max
 * @returns {number}
 */
function clamp(value, min, max) {
  return Math.min(max, Math.max(min, value))
}

/**
 * @param {unknown} value
 * @param {number} fallback
 * @returns {number}
 */
function finite(value, fallback) {
  const number = Number(value)
  return Number.isFinite(number) ? number : fallback
}
