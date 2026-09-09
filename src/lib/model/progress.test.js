import { describe, expect, it } from 'vitest'
import { chapterProgress, projectProgress } from './progress.js'

function page(id, status, regions = []) {
  return { id, chapterId: 'c1', index: 0, status, skipReason: null, regions }
}

function declinedRegion(id) {
  return {
    id,
    pageId: 'p1',
    bbox: { x: 0, y: 0, w: 1, h: 1 },
    source: 'auto',
    outcome: 'declined',
    gateSkipCause: null,
    declineReason: 'quality',
    unusuallyLarge: false,
    mask: null,
  }
}

function chapter(id, pages, order = 0) {
  return { id, projectId: 'pr1', name: `Chapter ${id}`, order, pages }
}

describe('chapterProgress', () => {
  it('Not started - no pages cleaned', () => {
    const progress = chapterProgress(chapter('c1', [page('p1', 'unclean'), page('p2', 'unclean')]))
    expect(progress.status).toBe('notStarted')
    expect(progress.pagesCleaned).toBe(0)
  })

  it('In progress - a page is cleaning', () => {
    const progress = chapterProgress(chapter('c1', [page('p1', 'cleaned'), page('p2', 'cleaning')]))
    expect(progress.status).toBe('inProgress')
  })

  it('Review - cleaned pages leave regions needing review', () => {
    const progress = chapterProgress(
      chapter('c1', [page('p1', 'cleaned', [declinedRegion('r1')]), page('p2', 'unclean')]),
    )
    expect(progress.status).toBe('review')
    expect(progress.regionsNeedingReview).toBe(1)
  })

  it('Completed - every page cleaned, nothing outstanding', () => {
    const progress = chapterProgress(chapter('c1', [page('p1', 'cleaned'), page('p2', 'cleaned')]))
    expect(progress.status).toBe('completed')
    expect(progress.pagesCleaned).toBe(2)
    expect(progress.totalPages).toBe(2)
  })
})

describe('projectProgress', () => {
  it('sums totals across chapters and rolls up their statuses', () => {
    const done = chapter('c1', [page('p1', 'cleaned'), page('p2', 'cleaned')], 0)
    const untouched = chapter('c2', [page('p3', 'unclean')], 1)
    const project = {
      id: 'pr1', name: 'Proj', mode: 'single', readingDirection: 'rtl',
      created: 't', appVersion: '0', chapters: [done, untouched],
    }
    const progress = projectProgress(project)
    expect(progress.totalPages).toBe(3)
    expect(progress.pagesCleaned).toBe(2)
    // one chapter completed, one not started -> mixed, so the project overall is in progress
    expect(progress.status).toBe('inProgress')
    expect(progress.chapters.map((c) => c.id)).toEqual(['c1', 'c2'])
  })

  it('is completed only when every chapter is completed', () => {
    const c1 = chapter('c1', [page('p1', 'cleaned')], 0)
    const c2 = chapter('c2', [page('p2', 'cleaned')], 1)
    const project = {
      id: 'pr1', name: 'Proj', mode: 'single', readingDirection: 'rtl',
      created: 't', appVersion: '0', chapters: [c1, c2],
    }
    expect(projectProgress(project).status).toBe('completed')
  })

  /*
   * A listing carries page headers, so a chapter Home is drawing
   * has no regions to count. The review index is what it counts instead, and a
   * count derived from `page.regions` would read zero for a chapter full of
   * flagged masks.
   */
  it('counts review from the chapter index when the pages are headers', () => {
    const header = { id: 'p1', chapterId: 'c1', index: 0, status: 'cleaned', regions: [] }
    const c = {
      id: 'c1', name: 'Ch', number: 1, order: 0, pages: [header],
      review: [
        { id: 'r1', pageId: 'p1', pageIndex: 0, reasonKey: 'review.reason.declined' },
        { id: 'r2', pageId: 'p1', pageIndex: 0, reasonKey: 'review.reason.unusuallyLarge' },
      ],
    }
    const progress = chapterProgress(c)
    expect(progress.regionsNeedingReview).toBe(2)
    expect(progress.status).toBe('review')
  })

  it('falls back to the regions when a chapter carries no index', () => {
    const c = chapter('c1', [page('p1', 'cleaned', [{ id: 'r1', outcome: 'declined' }])], 0)
    expect(chapterProgress(c).regionsNeedingReview).toBe(1)
  })
})
