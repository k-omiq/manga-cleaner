import { describe, expect, it } from 'vitest'
import { pageMark } from './status.js'

/** @returns {import('./types.js').Page} */
function page(overrides = {}) {
  return { id: 'p1', chapterId: 'c1', index: 0, status: 'unclean', skipReason: null, regions: [], ...overrides }
}

/** @returns {import('./types.js').Region} */
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

describe('pageMark', () => {
  it('· not yet cleaned', () => {
    expect(pageMark(page({ status: 'unclean' }))).toEqual({
      glyph: '·', tone: 'muted', count: 0, titleKey: 'pages.status.unclean',
    })
  })

  it('● cleaning now', () => {
    expect(pageMark(page({ status: 'cleaning' }))).toEqual({
      glyph: '●', tone: 'active', count: 0, titleKey: 'pages.status.cleaning',
    })
  })

  it('✓ cleaned, nothing to review', () => {
    expect(pageMark(page({ status: 'cleaned', regions: [] }))).toEqual({
      glyph: '✓', tone: 'normal', count: 0, titleKey: 'pages.status.cleaned',
    })
  })

  it('✓ n cleaned, n regions need review', () => {
    const regions = [declinedRegion('a'), declinedRegion('b')]
    expect(pageMark(page({ status: 'cleaned', regions }))).toEqual({
      glyph: '✓', tone: 'warn', count: 2, titleKey: 'pages.status.cleanedNeedsReview',
    })
  })

  it('! skipped', () => {
    expect(pageMark(page({ status: 'skipped', skipReason: 'corrupt header' }))).toEqual({
      glyph: '!', tone: 'warn', count: 0, titleKey: 'pages.status.skipped',
    })
  })
})
