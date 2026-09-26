/**
 * The two pipelines a page goes through, the engines each one offers, and the
 * capability graph Settings and setup draw the models from.
 *
 * **Detection** finds the text to remove. **Cleaning** redraws what was under
 * it. Onboarding and Settings both draw from this one module, so the two can
 * never disagree about which choice needs which files.
 *
 * `files` are catalogue ids from `src-tauri/src/weights.rs`, written out
 * because an engine is a *combination* of files and the catalogue's
 * `requiredBy` names features, not combinations. An engine is
 * `ready` when the app can download and run it today; the rest are listed so
 * the choice reads as a roadmap, and are drawn disabled.
 *
 * **What a choice needs follows the workflow, not a universal list.** Legacy
 * script filtering needs its detector files, the script gate pair and, only
 * when the optional rescue is on and Japanese is cleaned, the three manga-ocr
 * files. The all-text policy never needs the gate or OCR. See `filesFor`.
 *
 * `rating` is two scores out of 5, higher is better on both:
 *  - `efficiency` - speed per page for the quality it gives
 *  - `light` - how little disk and memory it needs
 * These are provisional: derived from file sizes and the per-page timings in
 * `docs/findings.md`, not from a common benchmark. See "What has not been
 * measured" there. Every table that draws them says so beside the stars.
 */

/** The source languages the detection pipeline cleans. Latin text is left alone by design. */
export const LANGUAGES = Object.freeze([
  { id: 'ja', labelKey: 'pipelines.language.ja' },
  { id: 'zh', labelKey: 'pipelines.language.zh' },
  { id: 'ko', labelKey: 'pipelines.language.ko' },
])

/** The legacy script gate: the model and the labels it must match. One logical capability. */
export const SCRIPT_GATE_FILES = Object.freeze(['scriptGate', 'scriptGateLabels'])

/** The optional Japanese OCR rescue reader: encoder, decoder, vocabulary. One logical capability. */
export const OCR_FILES = Object.freeze(['ocrEncoder', 'ocrDecoder', 'ocrVocab'])

/** The two text policies, as `session.textPolicy` stores them. */
export const LEGACY_POLICY = 'legacy_gate'
export const ALL_TEXT_POLICY = 'all_text'

/**
 * @typedef {Object} Engine
 * @property {string} id
 * @property {string} name - a product name, not translated
 * @property {string} [noteKey] - one short line on what it is
 * @property {string[]} files - catalogue ids it needs
 * @property {string[]} [languages] - detection only: which languages it serves
 * @property {string} [sidecar] - cleaning only: the sidecar model directory it runs from
 * @property {string} [cloudModel] - cleaning only: offered through cloud setup, never downloaded here
 * @property {boolean} ready
 * @property {{efficiency: number, light: number}} rating
 */

/** @type {readonly Engine[]} */
export const DETECTORS = Object.freeze([
  {
    id: 'ctd-rtdetr',
    name: 'CTD + RT-DETR v2',
    noteKey: 'pipelines.detector.ctdRtdetr',
    // The detector pair only. The script gate belongs to the legacy policy
    // and OCR to its optional rescue, so neither rides along with a detector.
    files: ['textDetector', 'balloonDetector'],
    languages: ['ja', 'zh', 'ko'],
    ready: true,
    rating: { efficiency: 4, light: 4 },
  },
])

/**
 * Detector ids a stored session may still hold, and what each one meant.
 *
 * `ctd-rtdetr-ocr` was a per-language row that queued the 440 MB reader while
 * the run obeyed the separate `ocrRescue` switch. OCR rescue is one explicit
 * capability now, so the row reads back as the plain detector with the rescue
 * switched on: nobody who picked it loses what they asked for.
 */
export const RETIRED_DETECTORS = Object.freeze({
  'ctd-rtdetr-ocr': Object.freeze({ detector: 'ctd-rtdetr', ocrRescue: true, languages: Object.freeze(['ja']) }),
})

/**
 * Read a stored detector choice for a language, migrating a retired id.
 *
 * @param {string} language
 * @param {unknown} value - what the session stored
 * @returns {{detector: string|null|undefined, ocrRescue: boolean}} `undefined` when the value is not a choice this language can hold
 */
export function migrateDetectorChoice(language, value) {
  if (value === null) return { detector: null, ocrRescue: false }
  if (typeof value !== 'string') return { detector: undefined, ocrRescue: false }
  const retired = Object.hasOwn(RETIRED_DETECTORS, value) ? RETIRED_DETECTORS[value] : null
  if (retired) {
    return retired.languages.includes(language)
      ? { detector: retired.detector, ocrRescue: retired.ocrRescue }
      : { detector: undefined, ocrRescue: false }
  }
  return detectorsFor(language).some((engine) => engine.id === value)
    ? { detector: value, ocrRescue: false }
    : { detector: undefined, ocrRescue: false }
}

/** The review preset corresponding to a selected detection combination. */
export function workflowForDetectorModels(value) {
  const selected = normalizeDetectorModels(value)
  const ctd = selected.includes('ctd')
  const rt = selected.includes('rtSmall') || selected.includes('rtFull')
  const sam = selected.includes('samTs')
  if (ctd && rt && sam) return 'ctd_text_shape'
  if (ctd && rt) return 'ctd_regions'
  if (ctd && sam) return 'ctd_mask'
  if (rt && sam) return 'text_shape'
  if (ctd) return 'ctd'
  if (rt) return 'regions'
  return 'mask'
}

/** Independent model analysis presets; a selected combination chooses the initial one. */
export const WORKFLOW_PRESETS = Object.freeze([
  { id: 'ctd', name: 'Comic Text Detector (CTD)', needs: ['ctd'], description: 'CTD text boxes' },
  { id: 'ctd_regions', name: 'CTD + RT-DETR v2', needs: ['ctd', 'rt'], description: 'CTD text with RT-DETR regions' },
  { id: 'ctd_mask', name: 'CTD + SAM-TS-L', needs: ['ctd', 'sam'], description: 'CTD text with SAM lettering pixels' },
  { id: 'ctd_text_shape', name: 'CTD + RT-DETR v2 + SAM-TS-L', needs: ['ctd', 'rt', 'sam'], description: 'All three detection models' },
  { id: 'regions', name: 'Regions only', needs: ['rt'], description: 'RT-DETR text and bubble boxes' },
  { id: 'mask', name: 'Mask only', needs: ['sam'], description: 'SAM-TS-L lettering pixels, without RT-DETR or OCR' },
  { id: 'text_shape', name: 'Text-shaped review', needs: ['rt', 'sam'], description: 'RT-DETR context with the unchanged SAM mask' },
])

export const OPTIONAL_SFX = Object.freeze({ id: 'coo-mtsv3', name: 'COO MTSv3 SFX assist', selectable: false })

/** @type {readonly Engine[]} */
export const CLEANERS = Object.freeze([
  { id: 'lama-manga', name: 'LaMa Manga', noteKey: 'pipelines.cleaner.lamaManga', files: ['inpainter'], ready: true, rating: { efficiency: 4, light: 4 } },
  { id: 'big-lama', name: 'Big LaMa', noteKey: 'pipelines.cleaner.bigLama', files: [], ready: false, rating: { efficiency: 3, light: 3 } },
  { id: 'flux2-klein-4b', name: 'FLUX.2 Klein 4B', noteKey: 'pipelines.cleaner.flux', files: [], sidecar: 'flux2-klein-4b', ready: false, rating: { efficiency: 3, light: 2 } },
  { id: 'flux2-klein-4b-cloud', name: '☁ FLUX.2 Klein 4B', noteKey: 'pipelines.cleaner.fluxCloud', files: [], cloudModel: 'flux2-klein-4b', ready: false, rating: { efficiency: 3, light: 2 } },
  { id: 'flux2-klein-9b', name: '☁ FLUX.2 Klein 9B', noteKey: 'pipelines.cleaner.fluxCloud', files: [], cloudModel: 'flux2-klein-9b', ready: false, rating: { efficiency: 2, light: 1 } },
  { id: 'flux1-schnell', name: 'FLUX.1 [schnell]', noteKey: 'pipelines.cleaner.flux', files: [], sidecar: 'flux1-schnell', ready: false, rating: { efficiency: 3, light: 1 } },
  { id: 'flux1-dev', name: 'FLUX.1 [dev]', noteKey: 'pipelines.cleaner.flux', files: [], sidecar: 'flux1-dev', ready: false, rating: { efficiency: 2, light: 1 } },
  { id: 'qwen-image-edit-2511', name: 'Qwen-Image-Edit-2511', noteKey: 'pipelines.cleaner.qwen', files: [], ready: false, rating: { efficiency: 2, light: 1 } },
])

/** The detector a language starts on. */
export const DEFAULT_DETECTOR = 'ctd-rtdetr'

/** Independent detection capabilities. RT-DETR has two mutually exclusive profiles. */
export const DETECTOR_MODEL_IDS = Object.freeze(['ctd', 'rtSmall', 'rtFull', 'samTs'])
export const DEFAULT_DETECTOR_MODELS = Object.freeze(['ctd', 'rtSmall'])

/** Validate a persisted or incoming combination without silently enabling models. */
export function normalizeDetectorModels(value) {
  if (!Array.isArray(value)) return [...DEFAULT_DETECTOR_MODELS]
  const selected = [...new Set(value)]
  if (!selected.length || selected.some((id) => !DETECTOR_MODEL_IDS.includes(id))) return [...DEFAULT_DETECTOR_MODELS]
  if (selected.includes('rtSmall') && selected.includes('rtFull')) return [...DEFAULT_DETECTOR_MODELS]
  return DETECTOR_MODEL_IDS.filter((id) => selected.includes(id))
}

/**
 * The ready detectors a language can use, in table order.
 * @param {string} language
 */
export function detectorsFor(language) {
  return DETECTORS.filter((engine) => engine.ready && engine.languages?.includes(language))
}

/**
 * Every detector that serves a language, ready or not, in table order.
 * @param {string} language
 */
export function allDetectorsFor(language) {
  return DETECTORS.filter((engine) => engine.languages?.includes(language))
}

/** @param {string} id */
export function detector(id) {
  return DETECTORS.find((engine) => engine.id === id) ?? null
}

/** @param {string} id */
export function cleaner(id) {
  return CLEANERS.find((engine) => engine.id === id) ?? null
}

/**
 * The detector a language resolves to, retired ids included, or null when the
 * language is skipped or holds nothing this table can run.
 *
 * @param {Record<string, string|null>|null|undefined} detection
 * @param {string} language
 */
function chosenDetector(detection, language) {
  const { detector: id } = migrateDetectorChoice(language, detection?.[language] ?? null)
  const engine = id ? detector(id) : null
  return engine?.ready && engine.languages?.includes(language) ? engine : null
}

/**
 * Whether a set of choices asks for the Japanese OCR rescue and can use it:
 * the switch is on (or a retired `ctd-rtdetr-ocr` row still says so), and
 * Japanese is cleaned. The native run applies the same rule
 * (`RunSelection::from_args`: `ocr_rescue &= ja`).
 *
 * @param {Record<string, string|null>|null|undefined} detection
 * @param {boolean|undefined} ocrRescue
 */
export function rescueRuns(detection, ocrRescue) {
  if (!chosenDetector(detection, 'ja')) return false
  return ocrRescue === true || migrateDetectorChoice('ja', detection?.ja ?? null).ocrRescue
}

/**
 * The logical models a workflow needs before it can run, in download order.
 *
 * - **Legacy script filtering** with at least one language cleaned: the text
 *   finder, the speech bubble finder, the script gate pair, and the OCR
 *   reader only when `rescueRuns`.
 * - **All text**: the speech bubble finder for the review's small whole-page
 *   profile and the SAM-TS-L graphs. Never the gate, never OCR. SAM is an
 *   import, so it is listed but never downloaded.
 *
 * @param {{textPolicy?: string, detection?: Record<string, string|null>|null, ocrRescue?: boolean}} [choices]
 * @returns {string[]} logical model ids (`MODELS`)
 */
export function workflowNeeds({ textPolicy = LEGACY_POLICY, detection = null, ocrRescue = false, detectorModels = null } = {}) {
  if (detectorModels) {
    const selected = normalizeDetectorModels(detectorModels)
    const cleaned = LANGUAGES.some((language) => chosenDetector(detection, language.id))
    if (!cleaned && textPolicy !== ALL_TEXT_POLICY) return []
    return [...selected, ...(textPolicy === LEGACY_POLICY ? ['scriptGate'] : []),
      ...(textPolicy === LEGACY_POLICY && rescueRuns(detection, ocrRescue) ? ['mangaOcr'] : [])]
  }
  if (textPolicy === ALL_TEXT_POLICY) return ['rtSmall', 'samTs']
  const cleaned = LANGUAGES.some((language) => chosenDetector(detection, language.id))
  if (!cleaned) return []
  return rescueRuns(detection, ocrRescue)
    ? ['ctd', 'rtSmall', 'scriptGate', 'mangaOcr']
    : ['ctd', 'rtSmall', 'scriptGate']
}

/**
 * Where the engine runtime leaves a workflow. Every native workflow loads its
 * models through ONNX Runtime, so a workflow with every file on disk still
 * cannot run while the runtime is missing: a readiness line that counted files
 * alone called it ready, and the run then refused to start.
 *
 * A workflow with no needs (legacy with every language skipped) runs nothing,
 * so the runtime does not decide anything for it.
 *
 * **Installed is not enough.** The catalogue's `installed` is a file found; a
 * native run also loads it and refuses to start when that fails - a CUDA build
 * on a machine without CUDA, a quarantined or damaged library. `load` is what
 * the `diagnostics` command said when it made that same load: `loaded`,
 * `failed`, `checking` while the question is out, `unchecked` when the
 * question itself was refused. Only `loaded` makes an installed runtime
 * `installed` here; anything else keeps the workflow from reading as ready.
 *
 * @param {{installed?: boolean, available?: boolean, downloading?: boolean}|null|undefined} runtime - the catalogue's runtime row
 * @param {readonly string[]} needs - what `workflowNeeds` answered
 * @param {'loaded'|'failed'|'checking'|'unchecked'} [load] - whether the installed runtime loads
 * @returns {'notNeeded'|'installed'|'unloadable'|'checking'|'unchecked'|'downloading'|'missing'|'unavailable'}
 */
export function runtimeState(runtime, needs, load) {
  if (needs.length === 0) return 'notNeeded'
  if (runtime?.installed === true) {
    if (load === 'loaded') return 'installed'
    if (load === 'failed') return 'unloadable'
    return load === 'unchecked' ? 'unchecked' : 'checking'
  }
  if (runtime?.downloading === true) return 'downloading'
  return runtime?.available === false ? 'unavailable' : 'missing'
}

/**
 * The catalogue ids a set of choices needs, deduplicated, in first-seen order.
 * Pure: the same answer for the same choices, whatever is installed.
 *
 * Downloads follow the selected workflow. Legacy filtering needs each cleaned
 * language's detector files, then the script gate pair, then the three OCR
 * files only when the rescue is on and Japanese is cleaned. All-text needs the
 * speech bubble finder and nothing from the gate or the reader. Imported
 * graphs (SAM-TS-L, full RT-DETR) are never downloads. Every ready cleaner
 * that is wanted adds its own files after that.
 *
 * @param {Record<string, string|null>|null|undefined} detection - language id → detector id, or null to skip the language
 * @param {Record<string, boolean>|null|undefined} cleaners - cleaner id → wanted
 * @param {{textPolicy?: string, ocrRescue?: boolean}} [options] - the policy (legacy by default) and the rescue switch
 * @returns {string[]}
 */
export function filesFor(detection, cleaners, { textPolicy = LEGACY_POLICY, ocrRescue = false, detectorModels = null } = {}) {
  const ids = new Set()
  if (detectorModels) {
    for (const id of workflowNeeds({ textPolicy, detection, ocrRescue, detectorModels })) {
      for (const file of model(id)?.files ?? []) ids.add(file)
    }
  } else if (textPolicy === ALL_TEXT_POLICY) {
    for (const file of model('rtSmall')?.files ?? []) ids.add(file)
  } else {
    for (const language of LANGUAGES) {
      for (const file of chosenDetector(detection, language.id)?.files ?? []) ids.add(file)
    }
    const needs = workflowNeeds({ textPolicy, detection, ocrRescue })
    if (needs.includes('scriptGate')) for (const file of SCRIPT_GATE_FILES) ids.add(file)
    if (needs.includes('mangaOcr')) for (const file of OCR_FILES) ids.add(file)
  }
  for (const engine of CLEANERS) {
    if (engine.ready && cleaners?.[engine.id]) for (const file of engine.files) ids.add(file)
  }
  return [...ids]
}

/* ------------------------------------------------------------------ */
/* The capability graph                                                */
/* ------------------------------------------------------------------ */

/**
 * What a removal can stop, by workflow. Named so a Delete can say which of
 * them it disables before it happens.
 *
 * - `legacy` - legacy automatic cleaning (CTD + RT-DETR + script gate)
 * - `ocrRescue` - the optional Japanese OCR rescue inside legacy filtering
 * - `reviewSmall` - the text-shaped review's small whole-page RT profile
 * - `reviewFull` - the text-shaped review's full two-tile RT profile
 * - `review` - the text-shaped review itself (its lettering mask)
 * - `lama` - LaMa redraw
 */
export const WORKFLOWS = Object.freeze(['legacy', 'ocrRescue', 'reviewSmall', 'reviewFull', 'review', 'lama'])

/**
 * @typedef {Object} LogicalModel
 * @property {string} id
 * @property {string} nameKey - what the row is called
 * @property {string|null} product - the product name, data rather than copy
 * @property {'download'|'import'|'excluded'} source - how it reaches the machine
 * @property {string[]} files - catalogue ids, for downloads; empty otherwise
 * @property {string|null} group - the native group id that installs and removes the files as one unit
 * @property {'samTs'|'fullRt'|null} importId - which import command owns it
 * @property {string[]} disables - the workflows its removal stops
 * @property {string} roleKey - one line on what it does and for whom
 * @property {string|null} removeKey - the confirmation a removal asks, naming what it disables
 */

/** @type {readonly LogicalModel[]} */
export const MODELS = Object.freeze([
  {
    id: 'ctd',
    nameKey: 'models.kind.textDetector',
    product: 'Comic Text Detector (CTD)',
    source: 'download',
    files: ['textDetector'],
    group: null,
    importId: null,
    disables: ['legacy'],
    roleKey: 'settings.detection.role.ctd',
    removeKey: 'settings.models.remove.ctd',
  },
  {
    id: 'rtSmall',
    nameKey: 'models.kind.balloonDetector',
    product: 'RT-DETR v2 small',
    source: 'download',
    files: ['balloonDetector'],
    group: null,
    importId: null,
    disables: ['legacy', 'reviewSmall'],
    roleKey: 'settings.detection.role.rtSmall',
    removeKey: 'settings.models.remove.rtSmall',
  },
  {
    id: 'rtFull',
    nameKey: 'settings.detection.model.rtFull',
    product: 'RT-DETR v2 full',
    source: 'download',
    files: ['fullRt'],
    group: null,
    importId: null,
    disables: ['reviewFull'],
    roleKey: 'settings.detection.role.rtFull',
    removeKey: 'settings.models.remove.rtFull',
  },
  {
    id: 'samTs',
    nameKey: 'settings.detection.model.samTs',
    product: 'SAM-TS-L',
    source: 'import',
    files: [],
    group: null,
    importId: 'samTs',
    disables: ['review'],
    roleKey: 'settings.detection.role.samTs',
    removeKey: 'settings.models.remove.samTs',
  },
  {
    id: 'coo',
    nameKey: 'settings.detection.model.coo',
    product: 'COO MTSv3',
    source: 'excluded',
    files: [],
    group: null,
    importId: null,
    disables: [],
    roleKey: 'settings.detection.role.coo',
    removeKey: null,
  },
  {
    id: 'scriptGate',
    nameKey: 'settings.models.groups.scriptGate',
    product: 'ogkalu Image Script Identification',
    source: 'download',
    files: [...SCRIPT_GATE_FILES],
    group: 'scriptGate',
    importId: null,
    disables: ['legacy'],
    roleKey: 'settings.detection.role.scriptGate',
    removeKey: 'settings.models.remove.scriptGate',
  },
  {
    id: 'mangaOcr',
    nameKey: 'settings.models.groups.mangaOcr',
    product: 'Manga OCR',
    source: 'download',
    files: [...OCR_FILES],
    group: 'mangaOcr',
    importId: null,
    disables: ['ocrRescue'],
    roleKey: 'settings.detection.role.mangaOcr',
    removeKey: 'settings.models.remove.mangaOcr',
  },
  {
    id: 'lama',
    nameKey: 'models.kind.inpainter',
    product: 'LaMa Manga',
    source: 'download',
    files: ['inpainter'],
    group: null,
    importId: null,
    disables: ['lama'],
    roleKey: 'settings.cleaning.role.lama',
    removeKey: 'settings.models.remove.lama',
  },
])

/**
 * The sections Settings draws, in order, each with the logical models it
 * holds. `pipeline` is the Settings tab the section lives on.
 */
export const CAPABILITIES = Object.freeze([
  { id: 'findRegions', pipeline: 'detection', headingKey: 'settings.detection.capability.findRegions', noteKey: 'settings.detection.capability.findRegionsNote', models: ['ctd', 'rtSmall', 'rtFull'] },
  { id: 'shapeMask', pipeline: 'detection', headingKey: 'settings.detection.capability.shapeMask', noteKey: 'settings.detection.capability.shapeMaskNote', models: ['samTs'] },
  { id: 'sfx', pipeline: 'detection', headingKey: 'settings.detection.capability.sfx', noteKey: 'settings.detection.capability.sfxNote', models: ['coo'] },
  { id: 'japanese', pipeline: 'detection', headingKey: 'settings.detection.capability.japanese', noteKey: 'settings.detection.capability.japaneseNote', models: ['scriptGate', 'mangaOcr'] },
  { id: 'rebuild', pipeline: 'cleaning', headingKey: 'settings.cleaning.capability.rebuild', noteKey: 'settings.cleaning.capability.rebuildNote', models: ['lama'] },
])

/** @param {string} id @returns {LogicalModel|null} */
export function model(id) {
  return MODELS.find((entry) => entry.id === id) ?? null
}

/**
 * The logical model a catalogue file belongs to, or null for a file no model
 * claims (a weight the backend added before this table knew it).
 *
 * @param {string} fileId
 * @returns {LogicalModel|null}
 */
export function modelOfFile(fileId) {
  return MODELS.find((entry) => entry.files.includes(fileId)) ?? null
}

/**
 * What removing a logical model, or any one of its files, stops. A group
 * member answers for its whole group: the native delete removes the group as
 * one unit. Another model's files are never part of the answer, so removing
 * Japanese filtering or OCR never names the shared RT-DETR weights.
 *
 * @param {string} id - a logical model id or a catalogue file id
 * @returns {{model: LogicalModel, files: string[], disables: string[]}|null}
 */
export function removalImpact(id) {
  const entry = model(id) ?? modelOfFile(id)
  if (!entry || entry.source === 'excluded') return null
  return { model: entry, files: [...entry.files], disables: [...entry.disables] }
}

/**
 * Whether the workflow the choices select uses a logical model, so a removal
 * can add that it stops the cleaning in force now.
 *
 * @param {string} id
 * @param {{textPolicy?: string, detection?: Record<string, string|null>|null, ocrRescue?: boolean}} [choices]
 */
export function usedNow(id, choices = {}) {
  return workflowNeeds(choices).includes(id)
}

/**
 * Bytes an engine still needs, given what is on disk.
 *
 * @param {Engine} engine
 * @param {Record<string, {bytes: number, installed: boolean}>} files - by catalogue id
 * @param {Record<string, boolean>} [finished] - ids that arrived since `files` was read
 */
export function engineBytes(engine, files, finished = {}) {
  return engine.files.reduce((sum, id) => {
    const file = files[id]
    return file && !file.installed && !finished[id] ? sum + file.bytes : sum
  }, 0)
}
