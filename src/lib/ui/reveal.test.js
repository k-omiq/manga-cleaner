import { describe, expect, it } from 'vitest'
import { revealDelta } from './reveal.js'

/**
 * The arithmetic behind `reveal` - `nearest`, and nothing more than
 * `nearest`. The DOM half (which ancestor is picked, and that no other box
 * moves) is `reveal.dom.test.js`.
 */
describe('revealDelta', () => {
  const view = { top: 100, bottom: 300 }

  it('leaves a row that is already in view alone', () => {
    expect(revealDelta({ top: 120, bottom: 160 }, view)).toBe(0)
  })

  it('leaves a row flush with either edge alone', () => {
    expect(revealDelta({ top: 100, bottom: 140 }, view)).toBe(0)
    expect(revealDelta({ top: 260, bottom: 300 }, view)).toBe(0)
  })

  it('moves up by the gap when the row is above the view', () => {
    expect(revealDelta({ top: 60, bottom: 100 }, view)).toBe(-40)
  })

  it('moves down by the gap when the row is below the view', () => {
    expect(revealDelta({ top: 320, bottom: 360 }, view)).toBe(60)
  })

  it('moves by the overhang only, not to an edge', () => {
    // Half of the row hangs below: the row's bottom reaches the view's, and
    // no further - that is what makes repeated reveals stable.
    expect(revealDelta({ top: 280, bottom: 320 }, view)).toBe(20)
  })

  it('aligns the top of a row taller than the view', () => {
    expect(revealDelta({ top: 150, bottom: 500 }, view)).toBe(50)
    expect(revealDelta({ top: 50, bottom: 400 }, view)).toBe(-50)
  })
})
