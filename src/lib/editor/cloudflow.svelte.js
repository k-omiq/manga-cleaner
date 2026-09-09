/**
 * The cloud gate - the carry-forward from Task 5, closed.
 *
 * `session.cloudAllowed` is persisted and, until this module, was read only by
 * the tool window, which disables the cloud rung. Nothing read it on the path
 * that actually sends. Two halves:
 *
 * 1. **When cloud is blocked, refuse in the interface.** Do not call the
 *    adapter and hope it says no. `notice.cloud.blocked` says explicitly that
 *    nothing was sent, which is the sentence the user needs; the adapter's own
 *    block stays in place as a second line of defence, not the first.
 * 2. **When cloud is allowed, ask.** `applyTool` answers `'needs-confirmation'`
 *    with a `confirmation.kind` - the transmission statement once per session,
 *    then the cost before the first spend
 *    unconditionally and before later spends only when *Confirm before
 *    spending* is set. The adapter owns *which* question is asked and
 *    when; this module owns raising it and carrying the answer back in.
 *
 * **Task 11 owns the two dialog bodies**, keyed by the `kind`s below. Until it
 * lands the shell's generic dialog renders them, which is enough to prove the
 * flow end to end. The contract is: a modal is pushed with an `onresolve`, and
 * the flow continues only on the confirming action's id.
 */

import { notify, pushModal } from '../state/app.svelte.js'
import { session } from '../state/session.svelte.js'
import { toolSpendsCloud } from './tools.js'

/**
 * The confirming action's id per confirmation kind, and the params it adds to
 * the next `applyTool` call. Anything else the dialog resolves with - another
 * action, Escape, a backdrop click, or the whole stack being dropped by a route
 * change - abandons the flow.
 */
const CONFIRMATIONS = /** @type {const} */ ({
  'cloud-transmission': {
    kind: 'cloudTransmission',
    titleKey: 'modal.title.cloudTransmission',
    confirmId: 'continue',
    confirmLabelKey: 'shell.action.continue',
    param: 'acknowledgeTransmission',
  },
  'cloud-cost': {
    kind: 'cloudCost',
    titleKey: 'modal.title.cloudCost',
    confirmId: 'confirm',
    confirmLabelKey: 'shell.action.confirmSpend',
    param: 'confirmSpend',
  },
})

/**
 * A run away with itself is worse than a refusal. Two confirmations is the
 * documented maximum; a third question means the adapter is asking for
 * something this module does not know how to answer, and the bound is on
 * questions **put to the user** - a dialog raised, answered and then thrown
 * away would be worse than one never raised.
 */
const MAX_CONFIRMATIONS = 2

/**
 * Would this call spend money, and is that switched off? Refuses in the
 * interface, before anything is sent.
 *
 * @param {string} tool
 * @param {Record<string, unknown>} params
 * @returns {boolean} true when the caller must not proceed
 */
export function cloudRefused(tool, params) {
  if (!toolSpendsCloud(tool, params)) return false
  if (session.cloudAllowed) return false
  notify({ key: 'notice.cloud.blocked', tone: 'warn' })
  return true
}

/**
 * Raise one confirmation dialog and wait for its answer.
 *
 * @param {{kind: string, regionId?: string, estimatedCost?: number}} confirmation
 * @returns {Promise<Record<string, boolean>|null>} the params to add, or null to abandon
 */
export function confirmCloud(confirmation) {
  const spec = CONFIRMATIONS[confirmation?.kind]
  if (!spec) return Promise.resolve(null)
  return new Promise((resolve) => {
    pushModal({
      kind: spec.kind,
      titleKey: spec.titleKey,
      // Blocking: money and a transmission of the user's scans are both
      // decisions, and a stray backdrop click is not one.
      blocking: true,
      props: {
        regionId: confirmation.regionId ?? null,
        estimatedCost: confirmation.estimatedCost ?? 0,
      },
      actions: [
        { id: 'cancel', labelKey: 'shell.action.cancel' },
        { id: spec.confirmId, labelKey: spec.confirmLabelKey, variant: 'primary' },
      ],
      onresolve: (result) => resolve(result === spec.confirmId ? { [spec.param]: true } : null),
    })
  })
}

/**
 * Call `applyTool`, answering whatever confirmations come back.
 *
 * The refusal is the caller's - it happens before this is reached - so by the
 * time a request gets here either it spends nothing or the user has allowed it
 * to.
 *
 * @param {(params: Record<string, unknown>) => Promise<import('../api/backend.js').ApplyResult>} call
 * @param {Record<string, unknown>} params
 * @returns {Promise<import('../api/backend.js').ApplyResult>} `{status: 'cancelled'}` when abandoned
 */
export async function applyWithConfirmations(call, params) {
  let next = params
  // `raised` counts the confirmations already put to the user, so the bound is
  // on questions asked rather than on calls made: the round trip after the last
  // allowed answer is still read - that is where the applied result arrives -
  // but a further question abandons the flow instead of being asked and then
  // thrown away.
  for (let raised = 0; raised <= MAX_CONFIRMATIONS; raised += 1) {
    const result = await call(next)
    if (result.status !== 'needs-confirmation') return result
    if (raised === MAX_CONFIRMATIONS) break
    const answer = await confirmCloud(result.confirmation ?? { kind: '' })
    if (!answer) return /** @type {any} */ ({ status: 'cancelled' })
    next = { ...next, ...answer }
  }
  return /** @type {any} */ ({ status: 'cancelled' })
}
