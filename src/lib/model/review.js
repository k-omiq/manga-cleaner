/**
 * "Needs review" - every cause, and the filtered,
 * steppable list the Masks panel's review filter and the bottom bar's
 * ⌃ ⌄ issue navigation are built on.
 *
 * Causes, checked in this order:
 *  1. fitting failed and a model reconstructed the area
 *  2. the region is unusually large for the page
 *  3. the app declined
 *  4. the script gate skipped it - low confidence, or text outside a bubble
 *  5. a cloud request was rejected, by one of five causes
 *  6. a cloud request was accepted - unconditionally
 *
 * A region can technically match more than one (e.g. declined *and*
 * unusually large); `reviewReason` reports the first match in the order
 * above, since that is also review priority - a fit failure is more
 * actionable than a size heuristic.
 */

/**
 * The gate's three held-back causes. Three, not two: the backend's
 * `Verdict::NotJapanese` is a *confident* refusal - the gate read the script and
 * it was Latin - and the two-value union this used to be folded it into
 * `low-confidence`, reporting the opposite of what happened.
 */
const GATE_SKIP_KEYS = {
  'low-confidence': 'review.reason.gateSkippedLowConfidence',
  'outside-bubble': 'review.reason.gateSkippedOutsideBubble',
  'not-japanese': 'review.reason.gateSkippedNotJapanese',
}

const CLOUD_REJECTION_KEYS = {
  'safety-filter': 'review.reason.cloudRejectedSafetyFilter',
  'transport-error': 'review.reason.cloudRejectedTransportError',
  'parameter-test': 'review.reason.cloudRejectedParameterTest',
  'residual-test': 'review.reason.cloudRejectedResidualTest',
  structural: 'review.reason.cloudRejectedStructural',
}

/**
 * @param {import('./types.js').Region} region
 * @returns {string|null} the i18n key for the review cause, or null if the region needs no review
 */
export function reviewReason(region) {
  const mask = region.mask
  if (mask?.fittingReconstructed) return 'review.reason.fittingReconstructed'
  if (region.unusuallyLarge) return 'review.reason.unusuallyLarge'
  if (region.outcome === 'declined') return 'review.reason.declined'
  if (region.outcome === 'gate-skipped') {
    // An unrecognised cause still flags the region. Returning null here would
    // drop it out of review entirely, which is the one outcome worse than
    // naming the cause imprecisely.
    return GATE_SKIP_KEYS[region.gateSkipCause] ?? 'review.reason.gateSkippedLowConfidence'
  }
  if (mask?.cloudOutcome) {
    if (mask.cloudOutcome.rejectionCause) {
      return CLOUD_REJECTION_KEYS[mask.cloudOutcome.rejectionCause] ?? null
    }
    if (mask.cloudOutcome.accepted) return 'review.reason.cloudAccepted'
  }
  return null
}

/**
 * @param {import('./types.js').Region} region
 * @returns {boolean}
 */
export function needsReview(region) {
  return reviewReason(region) !== null
}

/**
 * @typedef {Object} ReviewEntry
 * @property {string} id - the region's id; stable key for list rendering
 * @property {string} pageId
 * @property {string} reasonKey
 * @property {import('./types.js').Region} region
 */

/**
 * Builds the ordered, ready-to-render review list for a scope of regions.
 * The caller decides what the scope is - a page's regions, a whole
 * chapter's, or (longstrip) just the regions in the current viewport
 * - this only filters and shapes them.
 *
 * @param {import('./types.js').Region[]} scope
 * @returns {ReviewEntry[]}
 */
export function reviewList(scope) {
  const entries = []
  for (const region of scope) {
    const reasonKey = reviewReason(region)
    if (reasonKey) entries.push({ id: region.id, pageId: region.pageId, reasonKey, region })
  }
  return entries
}

/**
 * Steps to the next or previous issue in a review list, wrapping around at
 * either end. Used by the bottom bar's ⌃ ⌄ controls. Returns null for an
 * empty list; if `currentId` is not in the list, starts from the first
 * entry (stepping next) or the last (stepping prev).
 *
 * @param {ReviewEntry[]} list
 * @param {string|null} currentId
 * @param {'next'|'prev'} direction
 * @returns {ReviewEntry|null}
 */
export function stepIssue(list, currentId, direction) {
  if (list.length === 0) return null
  const index = list.findIndex((entry) => entry.id === currentId)
  if (index === -1) return direction === 'next' ? list[0] : list[list.length - 1]
  const step = direction === 'next' ? 1 : -1
  return list[(index + step + list.length) % list.length]
}
