import { it, expect } from 'vitest'
import { monthStartMs, formatUsageCost } from './cloud-usage.js'
it('uses the current local calendar month, including rollover', () => {
  expect(monthStartMs(new Date(2026, 8, 26))).toBe(new Date(2026, 8, 1).getTime())
  expect(monthStartMs(new Date(2027, 0, 1))).toBe(new Date(2027, 0, 1).getTime())
})
it('distinguishes zero, partial prices and no reported prices', () => {
  expect(formatUsageCost({ attempts: 0, reportedUsd: 0, unpricedAttempts: 0 })).toBe('$0.00')
  expect(formatUsageCost({ attempts: 2, reportedUsd: 0, unpricedAttempts: 2 })).toBe('Not reported')
  expect(formatUsageCost({ attempts: 2, reportedUsd: 0.0123, unpricedAttempts: 1 })).toBe('$0.0123 + ?')
  expect(formatUsageCost({ attempts: 0, reportedUsd: 0, unpricedAttempts: 0 }, 'Not reported', true)).toBe('Not reported')
  expect(formatUsageCost({ attempts: 1, reportedUsd: 2, unpricedAttempts: 0 }, 'Not reported', true)).toBe('$2.00 + ?')
})
