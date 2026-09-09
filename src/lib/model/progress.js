/**
 * Chapter and project rollups for the Home screen's two-level hierarchy
 * (Projects → Chapters, per the user's ruling - not a flat list). Both levels return the same shape of
 * ready-to-render numbers plus a status, so a project card and a chapter row
 * can share rendering logic.
 */

import { pageCounts } from './status.js'

/** @type {ReadonlyArray<'notStarted'|'inProgress'|'review'|'completed'>} */
const STATUSES = Object.freeze(['notStarted', 'inProgress', 'review', 'completed'])

/**
 * @typedef {Object} Progress
 * @property {string} id - chapterId or projectId
 * @property {number} totalPages
 * @property {number} pagesCleaned
 * @property {number} regionsNeedingReview
 * @property {'notStarted'|'inProgress'|'review'|'completed'} status
 * @property {string} statusKey - i18n key, e.g. 'progress.status.review'
 */

/**
 * How many regions in a chapter need review.
 *
 * **The chapter's review index first**. Home lists
 * every chapter of every project and a listing carries page *headers* - no
 * regions at all - so a count derived from `page.regions` reads zero for a
 * chapter full of flagged masks. The index is what a backend sends instead: one
 * short record per flagged region, chapter-wide, built one page at a time.
 *
 * The fallback is not dead code and is not a courtesy to the mock: a chapter
 * whose pages are in hand - the open one, mid-run, between event and refresh -
 * is still countable from its pages, and a fixture with no index is still a
 * chapter. It goes through `pageCounts`, so a page that is a *header* is
 * counted off its `reviewCount` rather than off the empty region list.
 *
 * @param {import('./types.js').Chapter & {review?: Array<Object>}} chapter
 * @returns {number}
 */
function reviewCount(chapter) {
  if (Array.isArray(chapter.review)) return chapter.review.length
  return chapter.pages.reduce((sum, page) => sum + pageCounts(page).review, 0)
}

/**
 * @param {import('./types.js').Chapter} chapter
 * @returns {Progress}
 */
export function chapterProgress(chapter) {
  const totalPages = chapter.pages.length
  const pagesCleaned = chapter.pages.filter((page) => page.status === 'cleaned').length
  const anyCleaning = chapter.pages.some((page) => page.status === 'cleaning')
  const regionsNeedingReview = reviewCount(chapter)

  let status
  if (anyCleaning) status = 'inProgress'
  else if (pagesCleaned === 0) status = 'notStarted'
  else if (regionsNeedingReview > 0) status = 'review'
  else if (pagesCleaned === totalPages) status = 'completed'
  else status = 'inProgress'

  return {
    id: chapter.id,
    totalPages,
    pagesCleaned,
    regionsNeedingReview,
    status,
    statusKey: `progress.status.${status}`,
  }
}

/**
 * Rolls up every chapter's progress into the project total. Chapters are
 * returned in the order given (callers pass them already ordered), so the
 * Home screen never has to re-sort.
 *
 * @param {import('./types.js').Project} project
 * @returns {Progress & { chapters: Progress[] }}
 */
export function projectProgress(project) {
  const chapters = project.chapters.map(chapterProgress)
  const totalPages = chapters.reduce((sum, c) => sum + c.totalPages, 0)
  const pagesCleaned = chapters.reduce((sum, c) => sum + c.pagesCleaned, 0)
  const regionsNeedingReview = chapters.reduce((sum, c) => sum + c.regionsNeedingReview, 0)
  const status = rollupStatus(chapters.map((c) => c.status))

  return {
    id: project.id,
    totalPages,
    pagesCleaned,
    regionsNeedingReview,
    status,
    statusKey: `progress.status.${status}`,
    chapters,
  }
}

/**
 * @param {Array<'notStarted'|'inProgress'|'review'|'completed'>} statuses
 * @returns {'notStarted'|'inProgress'|'review'|'completed'}
 */
function rollupStatus(statuses) {
  if (statuses.length === 0) return 'notStarted'
  if (statuses.includes('inProgress')) return 'inProgress'
  if (statuses.every((s) => s === 'notStarted')) return 'notStarted'
  if (statuses.includes('review')) return 'review'
  if (statuses.every((s) => s === 'completed')) return 'completed'
  return 'inProgress'
}

export { STATUSES }
