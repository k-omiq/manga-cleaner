/**
 * The Masks panel's data shaping: the fill-mode
 * vocabulary, list ordering, the expanded provenance facts, and the two action
 * sets a row can offer - one for an actual mask, one for an unmasked "needs
 * review" entry (declined or gate-skipped, which by definition have no mask to
 * re-run).
 */

/** The fill modes, in the order the `cycleFill` action steps through them. */
export const FILL_MODES = Object.freeze(['match-surround', 'reconstruct', 'solid'])

/**
 * @param {string} fillMode
 * @returns {string} the next fill mode in the cycle
 */
export function nextFillMode(fillMode) {
  return FILL_MODES[(FILL_MODES.indexOf(fillMode) + 1) % FILL_MODES.length]
}

/** The three fill modes, and the i18n key each one displays under. */
const FILL_MODE_KEYS = Object.freeze({
  'match-surround': 'masks.fillMode.matchSurround',
  reconstruct: 'masks.fillMode.reconstruct',
  solid: 'masks.fillMode.solid',
})

/**
 * The display name of a fill mode, as `ladder.js#rungLabel` is for a rung:
 * the vocabulary belongs to the model, so the panel and the mock engine name
 * a fill mode the same way.
 *
 * @param {string} fillMode
 * @returns {string} i18n key
 */
export function fillModeLabel(fillMode) {
  return FILL_MODE_KEYS[fillMode] ?? FILL_MODE_KEYS['match-surround']
}

/**
 * Newest first by revision - the panel never re-sorts what this returns.
 *
 * @param {import('./types.js').Mask[]} masks
 * @returns {import('./types.js').Mask[]}
 */
export function orderMasks(masks) {
  return [...masks].sort((a, b) => b.sequence - a.sequence)
}

/**
 * @typedef {Object} ProvenanceFact
 * @property {string} key - i18n label key
 * @property {unknown} value - raw value; formatting/translation is the caller's job
 */

/**
 * The subset of provenance shown on an expanded mask row - engine, model
 * version, fill mode, elapsed time, and for cloud regions the cost and
 * request id. Not the full `Provenance` record: hashes
 * and execution provider are reproducibility data, not review copy.
 *
 * A mask the cloud rendered names Cloud as its engine, as its row does
 * (`maskEngine`), and the model its own record names. The version beside it
 * is not that model's: the native side keeps the one the patch it replaced
 * carried.
 *
 * @param {import('./types.js').Mask} mask
 * @returns {ProvenanceFact[]}
 */
export function provenanceFacts(mask) {
  const facts = [
    { key: 'masks.provenance.engine', value: maskEngine(mask) },
    { key: 'masks.provenance.modelVersion', value: mask.provenance.cloud?.model || mask.provenance.engine_version },
    { key: 'masks.provenance.fillMode', value: mask.fillMode },
    { key: 'masks.provenance.elapsed', value: mask.elapsedMs },
  ]
  if (mask.provenance.cloud) {
    facts.push({ key: 'masks.provenance.cloudCost', value: mask.provenance.cloud.cost ?? null })
    if (mask.provenance.cloud.request_id) {
      facts.push({ key: 'masks.provenance.cloudRequestId', value: mask.provenance.cloud.request_id })
    }
  }
  return facts
}

/**
 * @typedef {Object} MaskAction
 * @property {string} id
 * @property {string} labelKey
 */

/**
 * The local engines a row may be switched to, weakest first.
 *
 * Local only, because this list is also where Auto clean's two engine rows and
 * the AI mask brush get their rungs (`editor/tools.js`), and an automatic run
 * never reaches the cloud. The cloud is offered beside these, by the row and
 * the region menu themselves, when a cloud endpoint is ready: see `rowEngines`
 * and `CLOUD_ENGINE`. Choosing it asks for consent before anything is sent.
 *
 * `flux` is rung 3a and is **local**. What keeps it out of an automatic run is
 * that it costs ten to sixty seconds and several gigabytes a region, which is
 * a spend a person has to choose per region. It is in this list and is
 * offered only where the sidecar is actually installed; see `rowEngines`.
 *
 * @type {ReadonlyArray<string>}
 */
export const ROW_ENGINES = Object.freeze(['fill', 'denoise', 'lama', 'flux'])

/**
 * The engine a row or the region menu offers for rendering on the user's own
 * cloud GPU. Not a rung: it runs whatever model the endpoint serves, and only
 * after one consent per request (`editor/cloudflow.svelte.js`).
 */
export const CLOUD_ENGINE = 'cloud'

/**
 * The engines this machine may actually be asked for.
 *
 * Two reasons a rung is not offered, and they have the same shape. Rung 3a
 * ships with nothing and is installed by hand; rung 2 has weights that are
 * downloaded after install and are absent on a machine nobody has pressed
 * Download on. One rule covers both: a user is told why rather than shown a
 * control that fails, and an entry that cannot run is **left out** rather
 * than drawn disabled - for rung 3a because there is nothing to tell, and
 * for a missing weight because the remedy is one press in Settings › Models
 * and the tool window is not where that sentence belongs.
 *
 * The cloud follows the same rule. It comes last, after the strongest local
 * rung, and only when `options.cloud` is true: the permission is on and the
 * default endpoint has an access token (`state/cloud.svelte.js#cloudUsable`,
 * the same verdict as `api/backend.js#isCloudExecutionReady`). Otherwise it
 * is left out, not greyed.
 *
 * `available` is `state/capabilities.svelte.js`'s `engines` map, derived from
 * the catalogue's own `requiredBy` rather than from a list written twice. A
 * rung the map says nothing about is offered - the map is a record of what is
 * *missing*, and a new rung nobody has taught it about must not vanish from
 * every picker. With **no map at all**, which is a caller that has not asked
 * the machine yet, the answer is `UNANSWERED`: every rung that ships with the
 * application, and not rung 3a, which never does.
 *
 * @param {Record<string, boolean>} [available] - which rungs have what they need
 * @param {{cloud?: boolean}} [options] - `cloud`: whether a cloud endpoint is ready
 * @returns {ReadonlyArray<string>}
 */
export function rowEngines(available, options = {}) {
  const known = available ?? UNANSWERED
  const local = ROW_ENGINES.filter((rung) => (known[rung] ?? true) !== false)
  return options.cloud === true ? [...local, CLOUD_ENGINE] : local
}

/**
 * What is offered before anything has asked the machine.
 *
 * Rung 3a alone is withheld, and the asymmetry is the honest one: the sidecar
 * is a separate program that is absent on nearly every machine, so offering it
 * on a guess would be wrong nearly every time. The weights are the other way
 * round - the ordinary machine has them, and blanking three rungs for the
 * moment between mount and the first answer would flicker on every launch.
 */
const UNANSWERED = Object.freeze({ flux: false })

/**
 * Whether a mask was rendered in the cloud: a legacy `cloud` engine, or a
 * patch whose provenance carries a cloud record (the native side records a
 * cloud render as FLUX, run on the endpoint, with that record beside it).
 *
 * @param {import('./types.js').Mask|null} mask
 * @returns {boolean}
 */
export function isCloudMask(mask) {
  return Boolean(mask?.provenance) && (mask.provenance.engine === CLOUD_ENGINE || Boolean(mask.provenance.cloud))
}

/**
 * The engine a mask ran on, as a picker names it: `cloud` for a cloud render,
 * whichever model the endpoint ran, and the rung otherwise.
 *
 * @param {import('./types.js').Mask|null} mask
 * @returns {string|null}
 */
export function maskEngine(mask) {
  if (!mask?.provenance) return null
  return isCloudMask(mask) ? CLOUD_ENGINE : mask.provenance.engine
}

/**
 * Whether a mask may be re-run or swapped from a Layers row or the region
 * menu. Every mask with a provenance record may.
 *
 * A cloud mask included. Re-running one is another request to the user's
 * cloud GPU, and it is asked for like every other: Try again on a cloud mask
 * goes back to the cloud, and the consent dialog comes first
 * (`editor/maskactions.svelte.js#rerunMask`). With the cloud off or not set
 * up, that request is refused with a notice saying why, and nothing is sent;
 * the picker still offers the local engines, which re-clean it here.
 *
 * @param {import('./types.js').Mask|null} mask
 * @returns {boolean}
 */
export function reRunnable(mask) {
  return Boolean(mask?.provenance)
}

/**
 * The plain-language name a row's engine picker shows for a rung.
 *
 * Separate from `ladder.js#rungLabel`, which is the engine's *own* name  - 
 * "manga-LaMa" - and belongs on the provenance record, where naming
 * the actual model is the point. A picker is read by someone deciding what to
 * try next, and "manga-LaMa" tells them nothing about that.
 *
 * @param {string} rung
 * @returns {string} i18n key
 */
export function engineChoiceLabel(rung) {
  return `masks.engineChoice.${rung}`
}

/**
 * @typedef {Object} RegionMenuItem
 * @property {string} id - `retry`, `delete`, or `engine:<rung>`
 * @property {string} labelKey
 * @property {string} [icon]
 * @property {boolean} [selected] - present only on the engine choices
 *
 * @typedef {Object} RegionMenuSection
 * @property {string} id
 * @property {string|null} labelKey - the section's heading, where it has one
 * @property {RegionMenuItem[]} items
 */

/**
 * What the region context menu offers for one region - the canvas's menu and
 * the Layers row's menu are the same three things, so they are derived once
 * here rather than assembled twice.
 *
 * The three, in order: **Try again**, **Clean with** (the engines, with the
 * one in use checked), **Delete**.
 *
 * Nothing is offered as a disabled entry. A region with no mask has never
 * been cleaned, so *again* is meaningless and it gets Delete alone; a greyed
 * row saying so would be an explanation nobody asked for on a menu the
 * pointer opened by accident as often as on purpose.
 *
 * **Clean with** lists the local rungs this machine can run and, when
 * `options.cloud` is true, Cloud after them (`rowEngines`). The engine in use
 * leads the list when it is not otherwise offered, so a cloud mask still
 * reads as Cloud with the cloud off, and a mask whose rung has lost its
 * weights still names that rung. Cloud, and Try again on a cloud mask, both
 * ask for consent before anything is sent.
 *
 * Delete is on every menu: for a region with no mask it is the region itself
 * that goes - which is how a warning the user has read and decided about
 * leaves the list.
 *
 * @param {import('./types.js').Region} region
 * @param {{engines?: Record<string, boolean>, cloud?: boolean}} [options] -
 *   `capabilities.engines` (absent offers every shipped rung) and whether a
 *   cloud endpoint is ready
 * @returns {RegionMenuSection[]}
 */
export function regionMenuSections(region, options = {}) {
  const mask = region?.mask ?? null
  /** @type {RegionMenuSection[]} */
  const sections = []

  if (reRunnable(mask)) {
    sections.push({
      id: 'rerun',
      labelKey: null,
      items: [{ id: 'retry', labelKey: 'masks.action.retry', icon: 'refresh' }],
    })
    // The engine the mask actually used leads the list even when the picker
    // does not offer it, exactly as the row's `<select>` does it: a menu
    // showing nothing checked would be a menu claiming the region has no engine.
    const current = /** @type {string} */ (maskEngine(mask))
    const offered = rowEngines(options.engines, { cloud: options.cloud })
    const rungs = offered.includes(current) ? offered : [current, ...offered]
    sections.push({
      id: 'engine',
      labelKey: 'masks.action.engine',
      items: rungs.map((rung) => ({
        id: `engine:${rung}`,
        labelKey: engineChoiceLabel(rung),
        selected: rung === current,
      })),
    })
  }

  sections.push({
    id: 'remove',
    labelKey: null,
    items: [
      {
        id: 'delete',
        labelKey: mask ? 'masks.action.delete' : 'masks.action.deleteRegion',
        icon: 'trash',
      },
    ],
  })
  return sections
}

/**
 * The action set for an unmasked "needs review" entry - a declined or
 * gate-skipped region. Both kinds get `showOnPage` (declined regions carry
 * a canvas marker and are otherwise unfindable);
 * gate-skips additionally get `cleanAnyway`.
 *
 * @param {import('./types.js').Region} region
 * @returns {MaskAction[]}
 */
export function reviewEntryActions(region) {
  const actions = [{ id: 'showOnPage', labelKey: 'masks.action.showOnPage' }]
  if (region.outcome === 'gate-skipped') {
    actions.push({ id: 'cleanAnyway', labelKey: 'masks.action.cleanAnyway' })
  }
  return actions
}
