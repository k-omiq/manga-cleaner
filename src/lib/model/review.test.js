import { describe, expect, it } from 'vitest'
import { needsReview, reviewList, reviewReason, stepIssue } from './review.js'

/** @returns {import('./types.js').Region} */
function region(overrides = {}) {
  return {
    id: 'r1',
    pageId: 'p1',
    bbox: { x: 0, y: 0, w: 10, h: 10 },
    source: 'auto',
    outcome: 'cleaned',
    gateSkipCause: null,
    declineReason: null,
    unusuallyLarge: false,
    mask: null,
    ...overrides,
  }
}

/** @returns {import('./types.js').Mask} */
function mask(overrides = {}) {
  return {
    id: 'm1',
    regionId: 'r1',
    sequence: 1,
    fillMode: 'reconstruct',
    elapsedMs: 1200,
    fittingReconstructed: false,
    cloudOutcome: null,
    provenance: {
      engine: 'lama',
      engine_version: '1.0',
      model_sha256: null,
      execution_provider: 'cpu',
      params_snapshot: {},
      mask_sha256: 'x',
      source_sha256: 'y',
      cloud: null,
      created: '2026-08-11T00:00:00Z',
    },
    ...overrides,
  }
}

describe('needsReview / reviewReason', () => {
  it('a cleaned region with no flags needs no review', () => {
    const r = region({ mask: mask() })
    expect(needsReview(r)).toBe(false)
    expect(reviewReason(r)).toBeNull()
  })

  it('fitting failed and a model reconstructed the area', () => {
    const r = region({ mask: mask({ fittingReconstructed: true }) })
    expect(needsReview(r)).toBe(true)
    expect(reviewReason(r)).toBe('review.reason.fittingReconstructed')
  })

  it('the region is unusually large for the page', () => {
    const r = region({ mask: mask(), unusuallyLarge: true })
    expect(needsReview(r)).toBe(true)
    expect(reviewReason(r)).toBe('review.reason.unusuallyLarge')
  })

  it('the app declined', () => {
    const r = region({ outcome: 'declined', declineReason: 'quality', mask: null })
    expect(needsReview(r)).toBe(true)
    expect(reviewReason(r)).toBe('review.reason.declined')
  })

  it('the script gate skipped it on low confidence', () => {
    const r = region({ outcome: 'gate-skipped', gateSkipCause: 'low-confidence', mask: null })
    expect(needsReview(r)).toBe(true)
    expect(reviewReason(r)).toBe('review.reason.gateSkippedLowConfidence')
  })

  it('the script gate skipped text outside a bubble', () => {
    const r = region({ outcome: 'gate-skipped', gateSkipCause: 'outside-bubble', mask: null })
    expect(needsReview(r)).toBe(true)
    expect(reviewReason(r)).toBe('review.reason.gateSkippedOutsideBubble')
  })

  // The backend's third gate outcome. Confidently not CJK is not the same as
  // "not confident", and the two-value union this used to be reported it as
  // the latter.
  it('the script gate read the script and it was not Japanese', () => {
    const r = region({ outcome: 'gate-skipped', gateSkipCause: 'not-japanese', mask: null })
    expect(needsReview(r)).toBe(true)
    expect(reviewReason(r)).toBe('review.reason.gateSkippedNotJapanese')
  })

  it('an unrecognised gate cause still flags the region rather than hiding it', () => {
    const r = region({ outcome: 'gate-skipped', gateSkipCause: null, mask: null })
    expect(needsReview(r)).toBe(true)
  })

  it.each([
    ['safety-filter', 'review.reason.cloudRejectedSafetyFilter'],
    ['transport-error', 'review.reason.cloudRejectedTransportError'],
    ['parameter-test', 'review.reason.cloudRejectedParameterTest'],
    ['residual-test', 'review.reason.cloudRejectedResidualTest'],
    ['structural', 'review.reason.cloudRejectedStructural'],
  ])('a cloud request rejected by %s', (cause, expectedKey) => {
    const r = region({ mask: mask({ cloudOutcome: { accepted: false, rejectionCause: cause } }) })
    expect(needsReview(r)).toBe(true)
    expect(reviewReason(r)).toBe(expectedKey)
  })

  it('a cloud request accepted, unconditionally', () => {
    const r = region({ mask: mask({ cloudOutcome: { accepted: true, rejectionCause: null } }) })
    expect(needsReview(r)).toBe(true)
    expect(reviewReason(r)).toBe('review.reason.cloudAccepted')
  })
})

describe('reviewList', () => {
  it('keeps only regions needing review, in scope order', () => {
    const clean = region({ id: 'a', mask: mask() })
    const flagged = region({ id: 'b', mask: mask({ fittingReconstructed: true }) })
    const alsoFlagged = region({ id: 'c', outcome: 'declined', mask: null })
    expect(reviewList([clean, flagged, alsoFlagged]).map((e) => e.id)).toEqual(['b', 'c'])
  })
})

describe('stepIssue', () => {
  const list = reviewList([
    region({ id: 'a', outcome: 'declined' }),
    region({ id: 'b', outcome: 'declined' }),
    region({ id: 'c', outcome: 'declined' }),
  ])

  it('steps forward and wraps past the end', () => {
    expect(stepIssue(list, 'a', 'next').id).toBe('b')
    expect(stepIssue(list, 'c', 'next').id).toBe('a')
  })

  it('steps backward and wraps past the start', () => {
    expect(stepIssue(list, 'c', 'prev').id).toBe('b')
    expect(stepIssue(list, 'a', 'prev').id).toBe('c')
  })

  it('an empty list is a no-op', () => {
    expect(stepIssue([], 'anything', 'next')).toBeNull()
    expect(stepIssue([], null, 'prev')).toBeNull()
  })
})
