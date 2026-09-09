/**
 * Page status marks. Five marks, five outcomes - this is
 * also the Pages list's progress indicator, so `pageMark` is called once per
 * row on every render.
 *
 * The glyph, the review count and its translated title are returned
 * separately rather than pre-composed into "✓ 2", so the component controls
 * layout and the i18n string controls word order (`t('pages.status
 * .cleanedNeedsReview', { count })`) instead of this module assuming
 * English order (constraints.md).
 */

import { needsReview } from './review.js'

/**
 * @typedef {Object} PageMark
 * @property {'·'|'●'|'✓'|'!'} glyph
 * @property {'muted'|'active'|'normal'|'warn'} tone
 * @property {number} count - regions needing review; nonzero only for the cleaned+review mark
 * @property {string} titleKey
 */

/**
 * @typedef {Object} PageCounts
 * @property {number} total - regions found on the page
 * @property {number} done - regions masked and flagged for nothing
 * @property {number} review - regions needing review
 */

/**
 * One page's three numbers - **from wherever they are honest**.
 *
 * A page inside the resident window is counted from the regions it is holding:
 * they are the live copy, and an edit changes them before any header can be
 * refreshed. A page outside it has no regions at all, and
 * counting `page.regions` there is what made the Pages list draw `0 / 0` under
 * a bare `·` for every page but the three in hand - so the header's own
 * `regionCount` / `doneCount` / `reviewCount` answer instead. The backend reads
 * them out of the manifest, which is why they are there the moment a project
 * reopens.
 *
 * `resident === false` is the only reading that means "not loaded": a fixture
 * or the mock carries whole pages and no flag at all, and those are counted
 * from their regions like the window's own.
 *
 * @param {import('./types.js').Page & {resident?: boolean, regionCount?: number, doneCount?: number, reviewCount?: number}} page
 * @returns {PageCounts}
 */
export function pageCounts(page) {
  if (page.resident === false) {
    return {
      total: page.regionCount ?? 0,
      done: page.doneCount ?? 0,
      review: page.reviewCount ?? 0,
    }
  }
  const regions = page.regions ?? []
  let done = 0
  let review = 0
  for (const region of regions) {
    if (needsReview(region)) review += 1
    else if (region.mask) done += 1
  }
  return { total: regions.length, done, review }
}

/**
 * Write a resident page's counts back onto its header, so the row and the
 * regions cannot disagree once the page is evicted or an edit has moved it.
 *
 * @param {import('./types.js').Page & {regionCount?: number, doneCount?: number, reviewCount?: number}} page
 * @returns {import('./types.js').Page}
 */
export function recountPage(page) {
  const counts = pageCounts(page)
  page.regionCount = counts.total
  page.doneCount = counts.done
  page.reviewCount = counts.review
  return page
}

/**
 * @param {import('./types.js').Page} page
 * @returns {PageMark}
 */
export function pageMark(page) {
  if (page.status === 'skipped') {
    return { glyph: '!', tone: 'warn', count: 0, titleKey: 'pages.status.skipped' }
  }
  if (page.status === 'cleaning') {
    return { glyph: '●', tone: 'active', count: 0, titleKey: 'pages.status.cleaning' }
  }
  if (page.status === 'unclean') {
    return { glyph: '·', tone: 'muted', count: 0, titleKey: 'pages.status.unclean' }
  }
  // Every page of the chapter carries this mark, and only three of them carry
  // their regions - so the count comes through `pageCounts`, not off the list.
  const count = pageCounts(page).review
  if (count > 0) {
    return { glyph: '✓', tone: 'warn', count, titleKey: 'pages.status.cleanedNeedsReview' }
  }
  return { glyph: '✓', tone: 'normal', count: 0, titleKey: 'pages.status.cleaned' }
}
