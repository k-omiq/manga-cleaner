/**
 * The Layers panel's row shaping: what one region says collapsed, what it
 * shows expanded, and what it offers to do about itself. Pure, and every
 * decision it makes is deferred to `src/lib/model` - `review.js` for whether a
 * region needs review and why, `masks.js` for ordering, provenance and the
 * action sets, `ladder.js` for engine names. Nothing here re-derives any of
 * them; this module only turns them into rows.
 *
 * Text arrives as i18n keys, never as strings: a row descriptor is data, and
 * the component is the only thing that calls `t()`.
 */

import {
  fillModeLabel,
  orderMasks,
  provenanceFacts,
  reRunnable,
  reviewEntryActions,
} from '../model/masks.js'
import { needsReview, reviewList, reviewReason } from '../model/review.js'
import { rungLabel } from '../model/ladder.js'

/**
 * @typedef {Object} Fact
 * @property {string} key - i18n key for the label column
 * @property {string} [valueKey] - i18n key for the value, when the value is words
 * @property {Object} [params] - params for `valueKey`
 * @property {string} [value] - the value verbatim, when it is a proper noun (a version, a request id)
 */

/**
 * @typedef {Object} MaskRow
 * @property {string} id - the region's id
 * @property {string|null} maskId
 * @property {'applied'|'review'|'declined'|'unexamined'} status
 * @property {string} glyph - `▪` applied, `△` needs review, `!` declined, `○` not examined
 * @property {string} statusKey - the same status in words, for assistive tech
 * @property {string} titleKey - engine name, or what the row is when there is no mask
 * @property {Array<{key: string, params?: Object}>} sub - the sub-line, in parts
 * @property {string|null} reasonKey - why it needs review, if it does
 * @property {boolean} deletable - always; every row can be removed, warnings included
 * @property {string|null} engine - the rung that produced the mask, for the row's engine picker
 * @property {boolean} reRunnable - whether the row may offer Try again and an engine picker
 * @property {Fact[]} facts
 * @property {import('../model/masks.js').MaskAction[]} actions
 */

/**
 * Four states, not three. `unexamined` is the one a three-state classification
 * had to lie about: a region with no mask that nothing has yet declined, held
 * back or flagged has not been *examined*, and calling it `applied` made the
 * canvas announce "No mask - Applied" on every uncleaned page. Task 11 could
 * only reword `masks.status.applied` (this file was outside its blast radius);
 * the classification was the defect, so the fourth state is the fix and
 * `applied` goes back to naming what it is.
 */
const STATUS = Object.freeze({
  applied: { glyph: '▪', statusKey: 'masks.status.applied' },
  review: { glyph: '△', statusKey: 'masks.status.needsReview' },
  declined: { glyph: '!', statusKey: 'masks.status.declined' },
  unexamined: { glyph: '○', statusKey: 'masks.status.unexamined' },
})

/**
 * The rows the panel lists, in the order it lists them.
 *
 * Unfiltered: all regions in scope - masked regions newest first (`orderMasks`),
 * followed by maskless regions (gate warnings, declined, unexamined). Filtered:
 * exactly `reviewList` over the same regions, in *its* order, so the list is a
 * contiguous slice of the chapter review set the bottom-right pill's arrows
 * step through. Re-sorting the filtered list by revision, as the design
 * prototype does, would give the two surfaces two different orderings of one
 * set.
 *
 * @param {import('../api/backend.js').ApiRegion[]} regions - the panel's scope
 * @param {{ filtered?: boolean }} [options]
 * @returns {MaskRow[]}
 */
export function maskRows(regions, options = {}) {
  if (options.filtered) return reviewList(regions).map((entry) => maskRow(entry.region, true))

  const masked = regions.filter((region) => region.mask)
  const byMaskId = new Map(masked.map((region) => [region.mask.id, region]))
  const orderedMasked = orderMasks(masked.map((region) => region.mask)).map((mask) => byMaskId.get(mask.id))
  const unmasked = regions.filter((region) => !region.mask)
  return [...orderedMasked, ...unmasked].map((region) => maskRow(region, false))
}

/**
 * @param {import('../api/backend.js').ApiRegion} region
 * @param {boolean} filtered - whether the row is being shown by the review filter
 * @returns {MaskRow}
 */
export function maskRow(region, filtered) {
  const mask = region.mask ?? null
  const reasonKey = reviewReason(region)
  const status =
    region.outcome === 'declined'
      ? 'declined'
      : reasonKey
        ? 'review'
        : mask
          ? 'applied'
          : 'unexamined'

  return {
    id: region.id,
    maskId: mask ? mask.id : null,
    status,
    glyph: STATUS[status].glyph,
    statusKey: STATUS[status].statusKey,
    titleKey: titleKeyFor(region, mask),
    sub: (filtered || !mask) && reasonKey ? [{ key: reasonKey }] : subLine(region, mask),
    reasonKey,
    // Every row, warnings included. A warning with no way off the list is a
    // list that only grows: a region the user has looked at and decided to
    // leave alone has to be dismissable, and dismissing it is the same act as
    // deleting a mask - the region goes, and undo brings it back.
    deletable: true,
    engine: mask ? mask.provenance.engine : null,
    reRunnable: reRunnable(mask),
    facts: factsFor(region, mask, reasonKey),
    // Only the two an unmasked review entry offers. A mask's own four -
    // stronger, simpler, cycle fill, reopen - are gone: they were four buttons
    // asking the user to reason about an engine ladder, and the row now says
    // the same thing in one picker and one Try again.
    actions: mask ? [] : reviewEntryActions(region),
  }
}

/**
 * A mask is named by the engine that produced it; a region with no mask is
 * named by what happened to it instead, since "nothing" is exactly the case
 * the user cannot see on the page.
 *
 * @param {import('../api/backend.js').ApiRegion} region
 * @param {import('../model/types.js').Mask|null} mask
 * @returns {string}
 */
function titleKeyFor(region, mask) {
  if (mask) return rungLabel(mask.provenance.engine)
  if (region.outcome === 'declined') return 'masks.title.declined'
  if (region.outcome === 'gate-skipped') return 'masks.title.gateSkipped'
  return 'masks.title.noMask'
}

/**
 * Fill mode · cost · hand - the parts, not the sentence. A hand-drawn mask is
 * marked because that is the one thing about it a reader cannot infer;
 * everything else about it is identical to an automatic one.
 *
 * @param {import('../api/backend.js').ApiRegion} region
 * @param {import('../model/types.js').Mask|null} mask
 * @returns {Array<{key: string, params?: Object}>}
 */
function subLine(region, mask) {
  if (!mask) return [{ key: 'masks.sub.noMask' }]
  const parts = [{ key: fillModeLabel(mask.fillMode) }]
  if (mask.provenance.cloud) {
    parts.push({ key: 'masks.value.cloudCost', params: { cost: mask.provenance.cloud.cost } })
  }
  if (region.source === 'hand') parts.push({ key: 'masks.origin.hand' })
  return parts
}

/**
 * The expanded row. For a mask that is `provenanceFacts` - engine, model
 * version, fill mode, elapsed, and for a cloud region the cost and request id
 * - with its raw values turned into display keys, plus where the mask came
 * from. For a region with no mask it is what was applied (nothing) and whether
 * the detector ever saw it. Either way, a flagged region ends with why.
 *
 * @param {import('../api/backend.js').ApiRegion} region
 * @param {import('../model/types.js').Mask|null} mask
 * @param {string|null} reasonKey
 * @returns {Fact[]}
 */
function factsFor(region, mask, reasonKey) {
  const facts = mask
    ? provenanceFacts(mask).map(displayFact)
    : [
        { key: 'masks.provenance.applied', valueKey: 'masks.value.nothingApplied' },
        {
          key: 'masks.provenance.detected',
          valueKey: region.detected ? 'masks.value.detectedYes' : 'masks.value.detectedNo',
        },
      ]
  if (mask) {
    facts.push({
      key: 'masks.provenance.origin',
      valueKey: region.source === 'hand' ? 'masks.origin.hand' : 'masks.origin.auto',
    })
  }
  if (reasonKey) facts.push({ key: 'masks.provenance.flagged', valueKey: reasonKey })
  return facts
}

/**
 * `masks.js` returns raw values - a rung id, a fill mode, a millisecond count
 * - and leaves the formatting to whoever displays them. This is that.
 *
 * @param {import('../model/masks.js').ProvenanceFact} fact
 * @returns {Fact}
 */
function displayFact(fact) {
  switch (fact.key) {
    case 'masks.provenance.engine':
      return { key: fact.key, valueKey: rungLabel(String(fact.value)) }
    case 'masks.provenance.fillMode':
      return { key: fact.key, valueKey: fillModeLabel(String(fact.value)) }
    case 'masks.provenance.elapsed':
      return { key: fact.key, valueKey: 'masks.value.elapsed', params: { ms: fact.value } }
    case 'masks.provenance.cloudCost':
      return { key: fact.key, valueKey: 'masks.value.cloudCost', params: { cost: fact.value } }
    default:
      // A model version and a cloud request id are proper nouns; translating
      // them would be translating an identifier.
      return { key: fact.key, value: String(fact.value ?? '') }
  }
}

/**
 * What an action's tooltip says.
 *
 * One branch now that the four ladder actions are gone. It stays a function
 * rather than becoming a template literal at the call site because an action
 * whose hint wants a parameter is one edit away, and because the catalogue test
 * enumerates the keys through it.
 *
 * @param {string} actionId
 * @returns {{ key: string, params?: Object }}
 */
export function actionHint(actionId) {
  return { key: `masks.hint.${actionId}` }
}

/**
 * How many of a scope's regions need review. The filter button's count, and
 * the same predicate the filtered list itself uses.
 *
 * @param {import('../api/backend.js').ApiRegion[]} regions
 * @returns {number}
 */
export function flaggedCount(regions) {
  return regions.filter(needsReview).length
}
