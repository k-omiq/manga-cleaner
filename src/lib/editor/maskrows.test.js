import { describe, expect, it } from 'vitest'
import { flaggedCount, maskRow, maskRows } from './maskrows.js'
import { reviewList } from '../model/review.js'

function mask(overrides = {}) {
  return {
    id: `${overrides.regionId ?? 'r1'}-m1`,
    regionId: 'r1',
    sequence: 1,
    fillMode: 'match-surround',
    elapsedMs: 40,
    fittingReconstructed: false,
    cloudOutcome: null,
    provenance: {
      engine: 'fill',
      engine_version: 'planar-1.4',
      model_sha256: null,
      execution_provider: 'cpu',
      params_snapshot: {},
      mask_sha256: 'a',
      source_sha256: 'b',
      cloud: null,
      created: '2026-01-01T00:00:00.000Z',
    },
    ...overrides,
  }
}

function region(overrides = {}) {
  const id = overrides.id ?? 'r1'
  return {
    id,
    pageId: 'p1',
    bbox: { x: 0, y: 0, w: 1, h: 1 },
    source: 'auto',
    outcome: 'cleaned',
    gateSkipCause: null,
    declineReason: null,
    unusuallyLarge: false,
    detected: true,
    mask: mask({ id: `${id}-m1`, regionId: id }),
    ...overrides,
  }
}

describe('maskRows - unfiltered', () => {
  it('lists masked regions newest first and includes unmasked ones', () => {
    const rows = maskRows([
      region({ id: 'a', mask: mask({ id: 'a-m1', regionId: 'a', sequence: 1 }) }),
      region({ id: 'b', mask: null, outcome: 'gate-skipped', gateSkipCause: 'low-confidence' }),
      region({ id: 'c', mask: mask({ id: 'c-m1', regionId: 'c', sequence: 9 }) }),
    ])
    expect(rows.map((row) => row.id)).toEqual(['c', 'a', 'b'])
    expect(rows[2].status).toBe('review')
    expect(rows[2].glyph).toBe('△')
    expect(rows[2].titleKey).toBe('masks.title.gateSkipped')
    expect(rows[2].sub).toEqual([{ key: 'review.reason.gateSkippedLowConfidence' }])
    expect(rows[2].deletable).toBe(true)
  })

  it('names a row by the engine that produced it', () => {
    const [row] = maskRows([
      region({ mask: mask({ provenance: { ...mask().provenance, engine: 'denoise' } }) }),
    ])
    expect(row.titleKey).toBe('ladder.rung.denoise')
    expect(row.status).toBe('applied')
    expect(row.glyph).toBe('▪')
    expect(row.deletable).toBe(true)
  })

  it('puts fill mode, cloud cost and hand-drawn on the sub-line', () => {
    const cloud = mask({
      fillMode: 'reconstruct',
      provenance: {
        ...mask().provenance,
        engine: 'cloud',
        cloud: { provider: 'x', model: 'y', request_id: 'req-1', tier: 'a', cost: 0.014 },
      },
    })
    const [row] = maskRows([region({ source: 'hand', mask: cloud })])
    expect(row.sub).toEqual([
      { key: 'masks.fillMode.reconstruct' },
      { key: 'masks.value.cloudCost', params: { cost: 0.014 } },
      { key: 'masks.origin.hand' },
    ])
  })

  it('offers a mask no expandable actions - its controls are on the row itself', () => {
    const [row] = maskRows([region()])
    expect(row.actions).toEqual([])
    expect(row.deletable).toBe(true)
    expect(row.reRunnable).toBe(true)
    expect(row.engine).toBe('fill')
  })

  it('withholds Try again and the engine picker from a cloud mask', () => {
    const cloud = mask({
      provenance: {
        ...mask().provenance,
        engine: 'cloud',
        cloud: { provider: 'x', model: 'y', request_id: 'r', tier: 'a', cost: 0.01 },
      },
    })
    const [row] = maskRows([region({ mask: cloud })])
    expect(row.engine).toBe('cloud')
    // Re-running one is another billable request, and a list row cannot ask
    // for the confirmation that would make spending legitimate.
    expect(row.reRunnable).toBe(false)
    expect(row.deletable).toBe(true)
  })
})

describe('maskRows - the review filter', () => {
  const flagged = [
    region({ id: 'a' }),
    region({ id: 'b', unusuallyLarge: true }),
    region({ id: 'c', mask: null, outcome: 'declined', declineReason: 'decline.reason.qualityMetric' }),
    region({ id: 'd', mask: null, outcome: 'gate-skipped', gateSkipCause: 'outside-bubble' }),
  ]

  it('lists exactly the review set, in the review set’s order', () => {
    const rows = maskRows(flagged, { filtered: true })
    expect(rows.map((row) => row.id)).toEqual(reviewList(flagged).map((entry) => entry.id))
    expect(rows.map((row) => row.id)).toEqual(['b', 'c', 'd'])
  })

  it('is a contiguous, order-preserving slice of the chapter set the pill steps through', () => {
    const otherPage = [region({ id: 'z', pageId: 'p2', unusuallyLarge: true })]
    const chapter = reviewList([...flagged, ...otherPage]).map((entry) => entry.id)
    const scoped = maskRows(flagged, { filtered: true }).map((row) => row.id)
    const at = chapter.indexOf(scoped[0])
    expect(chapter.slice(at, at + scoped.length)).toEqual(scoped)
  })

  it('swaps the sub-line for the reason it was flagged', () => {
    const rows = maskRows(flagged, { filtered: true })
    expect(rows[0].sub).toEqual([{ key: 'review.reason.unusuallyLarge' }])
    expect(rows.map((row) => row.reasonKey)).toEqual([
      'review.reason.unusuallyLarge',
      'review.reason.declined',
      'review.reason.gateSkippedOutsideBubble',
    ])
  })

  it('gives a gate-skip Clean anyway and a declined region only Show on page', () => {
    const rows = maskRows(flagged, { filtered: true })
    expect(rows[1].actions.map((action) => action.id)).toEqual(['showOnPage'])
    expect(rows[2].actions.map((action) => action.id)).toEqual(['showOnPage', 'cleanAnyway'])
  })

  it('distinguishes declined from every other flagged region by glyph and word', () => {
    const rows = maskRows(flagged, { filtered: true })
    expect(rows.map((row) => row.glyph)).toEqual(['△', '!', '△'])
    expect(rows[1].statusKey).toBe('masks.status.declined')
    expect(rows[0].statusKey).toBe('masks.status.needsReview')
  })
})

describe('maskRow facts', () => {
  it('formats provenance for display and records where the mask came from', () => {
    const row = maskRow(region({ source: 'hand' }), false)
    expect(row.facts).toEqual([
      { key: 'masks.provenance.engine', valueKey: 'ladder.rung.fill' },
      { key: 'masks.provenance.modelVersion', value: 'planar-1.4' },
      { key: 'masks.provenance.fillMode', valueKey: 'masks.fillMode.matchSurround' },
      { key: 'masks.provenance.elapsed', valueKey: 'masks.value.elapsed', params: { ms: 40 } },
      { key: 'masks.provenance.origin', valueKey: 'masks.origin.hand' },
    ])
  })

  it('adds cost and request id for a cloud region, and the reason when flagged', () => {
    const cloudMask = mask({
      cloudOutcome: { accepted: true, rejectionCause: null },
      provenance: {
        ...mask().provenance,
        engine: 'cloud',
        cloud: { provider: 'p', model: 'm', request_id: 'req-77', tier: 't', cost: 0.02 },
      },
    })
    const row = maskRow(region({ mask: cloudMask }), false)
    const keys = row.facts.map((fact) => fact.key)
    expect(keys).toContain('masks.provenance.cloudCost')
    expect(keys).toContain('masks.provenance.cloudRequestId')
    expect(row.facts.at(-1)).toEqual({
      key: 'masks.provenance.flagged',
      valueKey: 'review.reason.cloudAccepted',
    })
    expect(row.facts.find((fact) => fact.key === 'masks.provenance.cloudRequestId').value).toBe(
      'req-77',
    )
  })

  it('says what a region with no mask has had done to it', () => {
    const row = maskRow(
      region({ mask: null, outcome: 'gate-skipped', gateSkipCause: 'low-confidence', detected: false }),
      true,
    )
    expect(row.titleKey).toBe('masks.title.gateSkipped')
    // Deletable like every other row: a warning with no way off the list is a
    // list that only grows. What goes is the region, not a mask it never had.
    expect(row.deletable).toBe(true)
    expect(row.engine).toBe(null)
    expect(row.reRunnable).toBe(false)
    expect(row.facts).toEqual([
      { key: 'masks.provenance.applied', valueKey: 'masks.value.nothingApplied' },
      { key: 'masks.provenance.detected', valueKey: 'masks.value.detectedNo' },
      {
        key: 'masks.provenance.flagged',
        valueKey: 'review.reason.gateSkippedLowConfidence',
      },
    ])
  })

  it('calls a region with no mask and nothing wrong with it unexamined, not applied', () => {
    const row = maskRow(region({ mask: null, detected: false }), false)
    expect(row.status).toBe('unexamined')
    expect(row.statusKey).toBe('masks.status.unexamined')
    expect(row.glyph).toBe('○')
    expect(row.titleKey).toBe('masks.title.noMask')
  })

  it('keeps applied for a region that actually has a mask', () => {
    expect(maskRow(region(), false).status).toBe('applied')
  })
})

describe('flaggedCount', () => {
  it('counts the regions the filter would show', () => {
    const regions = [region(), region({ id: 'b', unusuallyLarge: true })]
    expect(flaggedCount(regions)).toBe(1)
    expect(flaggedCount(regions)).toBe(maskRows(regions, { filtered: true }).length)
  })
})
