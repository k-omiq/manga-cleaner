/**
 * What a first launch has to offer, decided from the catalogue alone.
 *
 * The weights and the ONNX Runtime are downloaded after install
 * and until this existed the only route to them
 * was a dialog the user had to find: a fresh machine opened an editor whose
 * Auto clean button was disabled with a sentence naming Settings, which is
 * correct and is not the same as being asked. This module is
 * the question that offer is built on - *what is missing, what is ticked, and
 * how many bytes that is* - and it is pure so the answer can be pinned without
 * mounting a 460px dialog over a mock backend.
 *
 * **The groups come from the catalogue's own `requiredBy`, not from a list
 * written here.** `state/capabilities.svelte.js` already gates the editor's
 * engine pickers that way, and a second table naming the four Auto clean
 * artefacts would be a second thing to drift from `src-tauri/src/weights.rs`.
 * The only judgements this file makes are which *features* are the required
 * ones and which optional engine is ticked by default - both of which are
 * product decisions and neither of which the catalogue can answer.
 */

/** The id the ONNX Runtime's own download reports under (`src-tauri/src/weights.rs`). */
export const RUNTIME_ID = 'runtime'

/**
 * The runtime has no `models.kind.*` name of its own - it is an archive rather
 * than a weight - so it borrows the one Settings gives it. One name for one
 * thing in both places.
 */
export const RUNTIME_LABEL_KEY = 'settings.models.runtime.label'

/** The feature whose every artefact is required: nothing cleans without it. */
const REQUIRED_FEATURE = 'autoClean'

/**
 * The engines that are a choice, in the order they are offered, and whether
 * each starts ticked.
 *
 * LaMa is ticked because it is the rung the ladder falls to for ordinary text
 * over tone and the one a first run will actually use.
 *
 * **It is the only one now.** MI-GAN sat beside it, unticked, as 28 MB of a
 * faster and rougher engine somebody might want later; the engine was removed
 * outright and this row went with it. The list is still a list rather than a
 * constant, because the shape of this question - which engines are a *choice*
 * at install time, and which of them start ticked - is unchanged by there
 * being one answer to it today.
 */
const OPTIONAL_FEATURES = Object.freeze([
  ['lama', true],
])

/**
 * @typedef {Object} PlanRow
 * @property {string} id - a catalogue row's id, or `runtime`
 * @property {string} labelKey - the i18n key that names it
 * @property {number} bytes - what its download costs; 0 when the view cannot say
 * @property {boolean} installed
 * @property {boolean} [ticked] - optional rows only: whether it starts selected
 */

/**
 * @typedef {Object} FirstLaunchPlan
 * @property {boolean} offer - whether anything in the required group is missing
 * @property {PlanRow[]} required - the Auto clean set and the runtime, runtime last
 * @property {PlanRow[]} optional - the redraw engines that are not here yet
 * @property {number} requiredBytes - the sum of the required rows still to fetch
 * @property {boolean} runtimeUnavailable - this platform publishes no runtime, so the weights alone will not run
 */

/**
 * Turn a `ModelsView` into the offer.
 *
 * An unavailable runtime - an Intel Mac, where nothing is published - is
 * left out of the group entirely rather than listed as a row with no download
 * behind it: the offer is a button, and a row it can never satisfy would make
 * the button a lie. **The weights are still offered**, and `runtimeUnavailable`
 * is how the dialog says why they are not enough on their own: they are
 * exactly what the offline install needs beside a hand-placed
 * library, they are fetchable on any machine, and a runtime that arrives later
 * finds them here. Offering nothing at all would leave such a user with no
 * button to press *and* nothing to read.
 *
 * @param {import('../api/backend.js').ModelsView|null|undefined} view
 * @returns {FirstLaunchPlan}
 */
export function firstLaunchPlan(view) {
  const models = Array.isArray(view?.models) ? view.models : []
  const runtime = view?.runtime

  /** @type {PlanRow[]} */
  const required = models
    .filter((row) => row?.requiredBy?.includes(REQUIRED_FEATURE))
    .map((row) => rowOf(row.id, row.kindKey, row.bytes, row.installed))

  // Last in the list because it is the one row that is not a weight, and a
  // reader scanning names wants the four that are together.
  if (runtime && runtime.available !== false) {
    required.push(rowOf(RUNTIME_ID, RUNTIME_LABEL_KEY, runtime.bytes, runtime.installed))
  }

  /** @type {PlanRow[]} */
  const optional = []
  for (const [feature, ticked] of OPTIONAL_FEATURES) {
    for (const row of models) {
      if (!row?.requiredBy?.includes(feature) || row.installed === true) continue
      optional.push({ ...rowOf(row.id, row.kindKey, row.bytes, false), ticked })
    }
  }

  return {
    offer: required.some((row) => !row.installed),
    required,
    optional,
    requiredBytes: sumOf(required.filter((row) => !row.installed)),
    runtimeUnavailable: runtime ? runtime.available === false : false,
  }
}

/**
 * The ids a freshly opened dialog has selected: everything missing from the
 * required group, plus the optional rows that start ticked.
 *
 * @param {FirstLaunchPlan} plan
 * @returns {Record<string, boolean>}
 */
export function initialSelection(plan) {
  /** @type {Record<string, boolean>} */
  const selection = {}
  for (const row of plan.required) if (!row.installed) selection[row.id] = true
  for (const row of plan.optional) selection[row.id] = row.ticked === true
  return selection
}

/**
 * What the primary button's press will cost, in bytes.
 *
 * Counts a row once and only while it is both selected and missing, so
 * unticking the redraw engine takes its 207 MB off the label and an artefact
 * that arrived while the dialog was open stops being charged for.
 *
 * @param {FirstLaunchPlan} plan
 * @param {Record<string, boolean>} selection
 * @returns {number}
 */
export function plannedBytes(plan, selection) {
  return sumOf(rowsOf(plan).filter((row) => !row.installed && selection?.[row.id] === true))
}

/**
 * The order the press downloads in: **the runtime first**, then the required
 * weights, then the optional ones.
 *
 * The runtime leads because it is the one artefact everything else needs to be
 * *used*: a machine that takes four weights and then loses its connection can
 * run nothing at all, where a machine with a runtime and one weight can at
 * least load what it has. Within each group the catalogue's own order is kept,
 * which is the order the rows were drawn in.
 *
 * @param {FirstLaunchPlan} plan
 * @param {Record<string, boolean>} selection
 * @returns {string[]}
 */
export function downloadQueue(plan, selection) {
  const wanted = rowsOf(plan).filter((row) => !row.installed && selection?.[row.id] === true)
  return [
    ...wanted.filter((row) => row.id === RUNTIME_ID),
    ...wanted.filter((row) => row.id !== RUNTIME_ID),
  ].map((row) => row.id)
}

/**
 * One row's name, for a sentence that has to say which artefact it is about.
 *
 * @param {FirstLaunchPlan} plan
 * @param {string} id
 * @returns {string|null} the i18n key, or null for an id this plan never drew
 */
export function labelKeyFor(plan, id) {
  return rowsOf(plan).find((row) => row.id === id)?.labelKey ?? null
}

/** Both groups as one list, in the order they are drawn. @param {FirstLaunchPlan} plan */
function rowsOf(plan) {
  return [...(plan?.required ?? []), ...(plan?.optional ?? [])]
}

/**
 * @param {string} id
 * @param {unknown} labelKey
 * @param {unknown} bytes
 * @param {unknown} installed
 * @returns {PlanRow}
 */
function rowOf(id, labelKey, bytes, installed) {
  return {
    id,
    labelKey: typeof labelKey === 'string' ? labelKey : '',
    // A view that cannot say how large something is - the runtime's `bytes` is
    // nullable - contributes nothing to the total rather than an invented
    // figure the button would then promise.
    bytes: typeof bytes === 'number' && Number.isFinite(bytes) && bytes > 0 ? bytes : 0,
    installed: installed === true,
  }
}

/** @param {PlanRow[]} rows */
function sumOf(rows) {
  return rows.reduce((total, row) => total + row.bytes, 0)
}
