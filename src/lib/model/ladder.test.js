import { describe, expect, it } from 'vitest'
import { RUNGS, rungLabel, simpler, stronger } from './ladder.js'

describe('engine ladder clamping', () => {
  it('stronger stops at the top rung, cloud', () => {
    expect(RUNGS.at(-1)).toBe('cloud')
    expect(stronger('cloud')).toBe('cloud')
  })

  it('simpler stops at the bottom rung, fill', () => {
    expect(RUNGS[0]).toBe('fill')
    expect(simpler('fill')).toBe('fill')
  })

  it('steps exactly one rung at a time in between', () => {
    expect(stronger('fill')).toBe('denoise')
    expect(stronger('denoise')).toBe('lama')
    expect(stronger('lama')).toBe('cloud')
    expect(simpler('cloud')).toBe('lama')
    expect(simpler('lama')).toBe('denoise')
    expect(simpler('denoise')).toBe('fill')
  })
})

describe('rungLabel', () => {
  it('returns the i18n key for known rungs including flux, paint, and clone', () => {
    expect(rungLabel('fill')).toBe('ladder.rung.fill')
    expect(rungLabel('denoise')).toBe('ladder.rung.denoise')
    expect(rungLabel('lama')).toBe('ladder.rung.lama')
    expect(rungLabel('flux')).toBe('ladder.rung.flux')
    expect(rungLabel('cloud')).toBe('ladder.rung.cloud')
    expect(rungLabel('paint')).toBe('ladder.rung.paint')
    expect(rungLabel('clone')).toBe('ladder.rung.clone')
  })

  it('falls back to unknown for an unrecognised rung', () => {
    expect(rungLabel('unknown_engine')).toBe('ladder.rung.unknown')
  })
})
