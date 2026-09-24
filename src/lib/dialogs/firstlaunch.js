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
 * mounting the setup dialog over a mock backend. The same goes for what the
 * setup's download step draws from it: the groups, their states and the
 * bytes a run has moved.
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
 * The setup's steps, in order, one decision each.
 *
 * There is no Hugging Face token step: every repository a default download
 * reads from is public (`src-tauri/src/weights.rs`), so the token only ever
 * helps with rate limits, and Settings > Models is where it is kept.
 */
export const FIRST_LAUNCH_STEPS = Object.freeze([
  'welcome',
  'models',
  'defaults',
  'cloud',
  'behavior',
  'done',
])

/**
 * The model Settings falls back to when the sidecar lists models and none has
 * been chosen. Settings > Models makes the same choice with the same id.
 */
export const DEFAULT_FLUX_MODEL = 'flux2-klein-4b'

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
 * @property {PlanRow[]} language - optional Japanese reader files
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

  const language = models
    .filter((row) => ['ocrEncoder', 'ocrDecoder', 'ocrVocab'].includes(row?.id) && row.installed !== true)
    .map((row) => ({ ...rowOf(row.id, row.kindKey, row.bytes, false), ticked: false }))

  return {
    offer: required.some((row) => !row.installed),
    required,
    optional,
    language,
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
  for (const row of plan.language ?? []) selection[row.id] = false
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

/**
 * @typedef {Object} PlanGroup
 * @property {'required'|'redraw'|'japanese'} id
 * @property {PlanRow[]} rows
 * @property {boolean} optional - whether a checkbox answers for it
 */

/**
 * The download step's lines: the required set as one, then each optional
 * engine as its own choice.
 *
 * Grouped because the step is a decision rather than an inventory. The
 * required files are one answer - there is no cleaning without all of them -
 * and the Japanese reader is three files and one tick. Settings > Models keeps
 * the one-row-per-file list for anyone who wants it. An empty group is left
 * out, so a replay on a machine that has everything draws the required line
 * alone.
 *
 * @param {FirstLaunchPlan|null|undefined} plan
 * @returns {PlanGroup[]}
 */
export function planGroups(plan) {
  /** @type {PlanGroup[]} */
  const groups = []
  if (plan?.required?.length) groups.push({ id: 'required', rows: plan.required, optional: false })
  if (plan?.optional?.length) groups.push({ id: 'redraw', rows: plan.optional, optional: true })
  if (plan?.language?.length) groups.push({ id: 'japanese', rows: plan.language, optional: true })
  return groups
}

/**
 * What is left to fetch of one group, in bytes: rows that were missing when
 * the plan was drawn and have not arrived since.
 *
 * @param {PlanGroup} group
 * @param {Record<string, boolean>} finished
 * @returns {number}
 */
export function groupBytes(group, finished) {
  return sumOf(group.rows.filter((row) => !row.installed && finished?.[row.id] !== true))
}

/**
 * @typedef {Object} RunState
 * @property {Record<string, boolean>} selection
 * @property {Record<string, boolean>} finished
 * @property {{id: string}|null} failure
 * @property {string|null} current
 * @property {boolean} running
 * @property {boolean} paused
 */

/**
 * One group's word for the right-hand column, or null while the size is the
 * thing to show.
 *
 * Ordered by what the reader needs first: a group that is complete says so
 * whatever else happened, then the failure they have to act on, then the
 * transfer in flight, then the rows still queued behind it. A group nobody
 * selected is not part of the run and keeps its size.
 *
 * @param {PlanGroup} group
 * @param {RunState} run
 * @returns {'installed'|'failed'|'downloading'|'waiting'|'paused'|null}
 */
export function groupState(group, run) {
  const here = (/** @type {PlanRow} */ row) => row.installed || run.finished?.[row.id] === true
  if (group.rows.every(here)) return 'installed'
  const ids = group.rows.map((row) => row.id)
  if (run.failure && ids.includes(run.failure.id)) return 'failed'
  if (run.running && run.current && ids.includes(run.current)) return 'downloading'
  const queued = group.rows.some((row) => !here(row) && run.selection?.[row.id] === true)
  if (!queued) return null
  if (run.running) return 'waiting'
  if (run.paused) return 'paused'
  return null
}

/**
 * How far a run has got, in bytes, for the one bar the step draws.
 *
 * The total is the whole selection rather than what is left of it, so the bar
 * does not jump backwards when a pause and a resume shorten the queue. Done is
 * every selected row that arrived plus what the transfer in flight has
 * reported, capped at the row's own size because a runtime package reports
 * its archive and the catalogue its contents.
 *
 * @param {FirstLaunchPlan|null|undefined} plan
 * @param {Record<string, boolean>} selection
 * @param {Record<string, boolean>} finished
 * @param {Record<string, {downloaded: number, total: number|null}>} progress
 * @returns {{done: number, total: number}}
 */
export function runProgress(plan, selection, finished, progress) {
  const rows = rowsOf(plan).filter((row) => !row.installed && selection?.[row.id] === true)
  let done = 0
  for (const row of rows) {
    if (finished?.[row.id] === true) {
      done += row.bytes
      continue
    }
    const moved = progress?.[row.id]?.downloaded
    if (typeof moved === 'number' && Number.isFinite(moved) && moved > 0) done += Math.min(moved, row.bytes)
  }
  return { done, total: sumOf(rows) }
}

/**
 * Whether the runtime can be asked which processors it offers.
 *
 * Asking maps the runtime's library into this process, and Windows will not
 * replace a file that is mapped - which is exactly what the runtime's own
 * download has to do. So the question waits until the runtime is here and is
 * not the transfer in flight. A view with no runtime row at all is an adapter
 * older than the row, and is answered the way `capabilities` answers it.
 *
 * @param {FirstLaunchPlan|null|undefined} plan
 * @param {Record<string, boolean>} finished
 * @param {string|null} current
 * @returns {boolean}
 */
export function runtimeReady(plan, finished, current) {
  if (!plan || plan.runtimeUnavailable) return false
  if (current === RUNTIME_ID) return false
  const runtime = plan.required.find((row) => row.id === RUNTIME_ID)
  if (!runtime) return true
  return runtime.installed || finished?.[RUNTIME_ID] === true
}

/**
 * The sentence a refused download has, when the backend has one.
 *
 * The runtime's refusals arrive as `key name=value ...` - `notice.runtime.noSpace
 * needed=253000000 free=1200000` - which is the grammar Settings > Models
 * reads as well. Only `notice.runtime.*` is honoured, for the reason Settings
 * gives: a failure free to name any key would be the backend choosing what the
 * interface says. The caller still checks that the key exists.
 *
 * @param {unknown} message
 * @returns {{key: string, params: Record<string, number>}|null}
 */
export function runtimeNotice(message) {
  const text = typeof message === 'string' ? message.trim() : ''
  const [key, ...pairs] = text.split(/\s+/)
  if (!key || !/^notice\.runtime\.[a-zA-Z0-9]+$/.test(key)) return null
  /** @type {Record<string, number>} */
  const params = {}
  for (const pair of pairs) {
    const at = pair.indexOf('=')
    if (at <= 0) continue
    const value = Number(pair.slice(at + 1))
    if (Number.isFinite(value)) params[pair.slice(0, at)] = value
  }
  return { key, params }
}

/**
 * Which AI redraw model a first choice lands on: the recommended one when the
 * sidecar has it, else the first it lists, else none.
 *
 * @param {Array<{id: string}>|null|undefined} models
 * @returns {string|null}
 */
export function defaultFluxModel(models) {
  const list = Array.isArray(models) ? models : []
  if (list.some((model) => model?.id === DEFAULT_FLUX_MODEL)) return DEFAULT_FLUX_MODEL
  return typeof list[0]?.id === 'string' ? list[0].id : null
}

/** Both groups as one list, in the order they are drawn. @param {FirstLaunchPlan} plan */
function rowsOf(plan) {
  return [...(plan?.required ?? []), ...(plan?.optional ?? []), ...(plan?.language ?? [])]
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
