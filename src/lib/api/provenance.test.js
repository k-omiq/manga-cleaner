import { describe, expect, it } from 'vitest'
import { capRung, routeRung } from './provenance.js'
import { cleanRegionAutomatically } from './tools.js'

describe('routeRung', () => {
  it('never routes to FLUX or cloud during automatic routing', () => {
    for (let hash = 0; hash < 100; hash += 1) {
      const rungWithFluxCeiling = routeRung(hash, 'flux')
      expect(['fill', 'denoise', 'lama']).toContain(rungWithFluxCeiling)
      expect(rungWithFluxCeiling).not.toBe('flux')
      expect(rungWithFluxCeiling).not.toBe('cloud')
    }
  })

  it('routes legacy cloud ceiling without degrading to fill', () => {
    // Bucket >= 29 (e.g. hash 29, 30, 31) routes to LaMa when ceiling is 'cloud'
    expect(routeRung(29, 'cloud')).toBe('lama')
    expect(routeRung(30, 'cloud')).toBe('lama')
    expect(routeRung(31, 'cloud')).toBe('lama')

    // Bucket >= 22 (e.g. hash 22) routes to denoise
    expect(routeRung(22, 'cloud')).toBe('denoise')

    // Bucket < 22 routes to fill
    expect(routeRung(10, 'cloud')).toBe('fill')
  })

  it('honors lower ceilings like fill and denoise', () => {
    expect(routeRung(31, 'fill')).toBe('fill')
    expect(routeRung(31, 'denoise')).toBe('denoise')
    expect(routeRung(22, 'fill')).toBe('fill')
  })
})

describe('capRung', () => {
  it('caps requested rung within ladder order', () => {
    expect(capRung('lama', 'fill')).toBe('fill')
    expect(capRung('fill', 'lama')).toBe('fill')
    expect(capRung('flux', 'lama')).toBe('lama')
    expect(capRung('denoise', 'flux')).toBe('denoise')
  })

  it('handles legacy cloud ceiling gracefully without crashing or defaulting to fill', () => {
    expect(capRung('lama', 'cloud')).toBe('lama')
    expect(capRung('denoise', 'cloud')).toBe('denoise')
    expect(capRung('cloud', 'lama')).toBe('lama')
    expect(capRung('cloud', 'cloud')).toBe('cloud')
  })
})

describe('cleanRegionAutomatically', () => {
  it('never produces FLUX masks in automatic cleaning even if requested', () => {
    const region = {
      id: 'r1',
      kind: 'outside',
      detected: true,
      source: 'auto',
      outcome: 'pending',
      gateSkipCause: null,
      declineReason: null,
      mask: null,
    }
    const page = { id: 'p1', status: 'unclean', colorMode: 'RGB8', regions: [region] }
    const ctx = {
      settings: { engineCeiling: 'flux' },
      nextSequence: () => 1,
      rng: {
        int: () => 10,
        sha256: () => 'abc',
      },
      created: () => '2026-01-01T00:00:00Z',
    }

    const mask = cleanRegionAutomatically(region, page, ctx, { engineCeiling: 'flux', outsideEngine: 'flux' })
    expect(mask.provenance.engine).toBe('lama')
    expect(mask.provenance.engine).not.toBe('flux')
  })

  it('caps automatic cleaning to LaMa when legacy cloud ceiling is passed', () => {
    const region = {
      id: 'r2',
      kind: 'outside',
      detected: true,
      source: 'auto',
      outcome: 'pending',
      gateSkipCause: null,
      declineReason: null,
      mask: null,
    }
    const page = { id: 'p1', status: 'unclean', colorMode: 'RGB8', regions: [region] }
    const ctx = {
      settings: { engineCeiling: 'cloud' },
      nextSequence: () => 1,
      rng: {
        int: () => 10,
        sha256: () => 'abc',
      },
      created: () => '2026-01-01T00:00:00Z',
    }

    const mask = cleanRegionAutomatically(region, page, ctx, { engineCeiling: 'cloud', outsideEngine: 'lama' })
    expect(mask.provenance.engine).toBe('lama')
    expect(mask.provenance.engine).not.toBe('cloud')
    expect(mask.provenance.engine).not.toBe('flux')
  })
})
