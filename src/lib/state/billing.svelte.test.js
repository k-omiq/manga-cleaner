import { afterEach, expect, it, vi } from 'vitest'
import { billing, billingCycle, refreshBilling } from './billing.svelte.js'

afterEach(() => refreshBilling(null, {}))

it('keeps a late response from replacing the newly selected profile', async () => {
  let release
  const old = new Promise((resolve) => { release = resolve })
  const backend = { getCloudBilling: vi.fn().mockReturnValueOnce(old).mockResolvedValueOnce({
    workspace: 'new-studio', cycle: billingCycle(), metered_cost: 9, billed_cost: 0,
  }) }
  const first = refreshBilling('old', backend)
  expect(await refreshBilling('new', backend)).toBe(true)
  release({ workspace: 'old-studio', cycle: billingCycle(), metered_cost: 50, billed_cost: 0 })
  expect(await first).toBe(false)
  expect(billing.profileId).toBe('new')
  expect(billing.data.workspace).toBe('new-studio')
  expect(billing.data.metered_cost).toBe(9)
})

it('reports provider increases since connection without counting prior monthly spend', async () => {
  const backend = { getCloudBilling: vi.fn().mockResolvedValueOnce({
    workspace: 'delta-studio', cycle: billingCycle(), metered_cost: 15, billed_cost: 0,
  }).mockResolvedValueOnce({
    workspace: 'delta-studio', cycle: billingCycle(), metered_cost: 15.75, billed_cost: 0,
  }) }
  await refreshBilling('delta', backend)
  expect(billing.data.sessionChange).toBe(0)
  await refreshBilling('delta', backend)
  expect(billing.data.sessionChange).toBe(.75)
})
