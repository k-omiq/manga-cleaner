import { getBackend } from '../api/backend.js'

export const billing = $state({ data: null, failed: false, loading: false, profileId: null })
const baselines = new Map()
let sequence = 0
export function billingCycle(now = new Date()) {
  return `${now.getUTCFullYear()}-${String(now.getUTCMonth() + 1).padStart(2, '0')}`
}
/** Billing changes are workspace-wide and delayed by the provider. */
export async function refreshBilling(profileId, backend = getBackend()) {
  const mine = ++sequence
  if (billing.profileId !== profileId) billing.data = null
  billing.profileId = profileId
  if (!profileId) { billing.data = null; billing.failed = false; billing.loading = false; return false }
  billing.loading = true
  try {
    const data = await backend.getCloudBilling({ profileId, cycle: billingCycle() })
    if (mine !== sequence) return false
    if (!data || !Number.isFinite(data.metered_cost) || data.metered_cost < 0 || typeof data.workspace !== 'string') throw new Error('Invalid billing summary')
    const key = `${data.workspace}:${data.cycle}`
    const baseline = baselines.get(key) ?? { cost: data.metered_cost, since: Date.now() }
    baselines.set(key, baseline)
    billing.data = { ...data, since: baseline.since, sessionChange: Math.max(0, data.metered_cost - baseline.cost) }
    billing.failed = false
    return true
  } catch { if (mine === sequence) { billing.failed = true; billing.data = null }; return false }
  finally { if (mine === sequence) billing.loading = false }
}
