/**
 * The two pipelines a page goes through, and the engines each one offers.
 *
 * **Detection** finds the text to remove. **Cleaning** redraws what was under
 * it. Onboarding and Settings both draw from this one table, so the two can
 * never disagree about which engine needs which files.
 *
 * `files` are catalogue ids from `src-tauri/src/weights.rs`, written out
 * because an engine is a *combination* of files and the catalogue's
 * `requiredBy` names features, not combinations. An engine is
 * `ready` when the app can download and run it today; the rest are listed so
 * the choice reads as a roadmap, and are drawn disabled.
 *
 * `rating` is two scores out of 5, higher is better on both:
 *  - `efficiency` - speed per page for the quality it gives
 *  - `light` - how little disk and memory it needs
 * These are provisional: derived from file sizes and the per-page timings in
 * `docs/findings.md`, not from a common benchmark. See "What has not been
 * measured" there.
 */

/** The source languages the detection pipeline cleans. Latin text is left alone by design. */
export const LANGUAGES = Object.freeze([
  { id: 'ja', labelKey: 'pipelines.language.ja' },
  { id: 'zh', labelKey: 'pipelines.language.zh' },
  { id: 'ko', labelKey: 'pipelines.language.ko' },
])

/** What every current detector needs: text mask, balloons, and the script gate. */
const BASE_DETECTION = Object.freeze(['textDetector', 'balloonDetector', 'scriptGate', 'scriptGateLabels'])

/**
 * @typedef {Object} Engine
 * @property {string} id
 * @property {string} name - a product name, not translated
 * @property {string} [noteKey] - one short line on what it is
 * @property {string[]} files - catalogue ids it needs
 * @property {string[]} [languages] - detection only: which languages it serves
 * @property {string} [sidecar] - cleaning only: the sidecar model directory it runs from
 * @property {boolean} ready
 * @property {{efficiency: number, light: number}} rating
 */

/** @type {readonly Engine[]} */
export const DETECTORS = Object.freeze([
  {
    id: 'ctd-rtdetr',
    name: 'CTD + RT-DETR v2',
    noteKey: 'pipelines.detector.ctdRtdetr',
    files: [...BASE_DETECTION],
    languages: ['ja', 'zh', 'ko'],
    ready: true,
    rating: { efficiency: 4, light: 4 },
  },
  {
    id: 'ctd-rtdetr-ocr',
    name: 'CTD + RT-DETR v2 + manga-ocr',
    noteKey: 'pipelines.detector.ctdRtdetrOcr',
    files: [...BASE_DETECTION, 'ocrEncoder', 'ocrDecoder', 'ocrVocab'],
    languages: ['ja'],
    ready: true,
    rating: { efficiency: 3, light: 2 },
  },
  // The next pipeline (`Plan for models/AGENT_START_HERE.md`): RT-DETR v2 for
  // text and bubbles, COO MTSv3 for sound effects, SAM-TS for the removal mask.
  {
    id: 'rtdetr-coo-samts',
    name: 'RT-DETR v2 + COO + SAM-TS',
    noteKey: 'pipelines.detector.rtdetrCoo',
    files: [],
    languages: ['ja', 'zh', 'ko'],
    ready: false,
    rating: { efficiency: 4, light: 3 },
  },
])

/** @type {readonly Engine[]} */
export const CLEANERS = Object.freeze([
  { id: 'lama-manga', name: 'LaMa Manga', noteKey: 'pipelines.cleaner.lamaManga', files: ['inpainter'], ready: true, rating: { efficiency: 4, light: 4 } },
  { id: 'big-lama', name: 'Big LaMa', noteKey: 'pipelines.cleaner.bigLama', files: [], ready: false, rating: { efficiency: 3, light: 3 } },
  { id: 'flux2-klein-4b', name: 'FLUX.2 Klein 4B', noteKey: 'pipelines.cleaner.flux', files: [], sidecar: 'flux2-klein-4b', ready: false, rating: { efficiency: 3, light: 2 } },
  { id: 'flux2-klein-9b', name: 'FLUX.2 Klein 9B', noteKey: 'pipelines.cleaner.flux', files: [], sidecar: 'flux2-klein-9b', ready: false, rating: { efficiency: 2, light: 1 } },
  { id: 'flux1-schnell', name: 'FLUX.1 [schnell]', noteKey: 'pipelines.cleaner.flux', files: [], sidecar: 'flux1-schnell', ready: false, rating: { efficiency: 3, light: 1 } },
  { id: 'flux1-dev', name: 'FLUX.1 [dev]', noteKey: 'pipelines.cleaner.flux', files: [], sidecar: 'flux1-dev', ready: false, rating: { efficiency: 2, light: 1 } },
  { id: 'qwen-image-edit-2511', name: 'Qwen-Image-Edit-2511', noteKey: 'pipelines.cleaner.qwen', files: [], ready: false, rating: { efficiency: 2, light: 1 } },
])

/** The detector a language starts on. */
export const DEFAULT_DETECTOR = 'ctd-rtdetr'

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
 * The catalogue ids a set of choices needs, deduplicated, in first-seen order.
 *
 * @param {Record<string, string|null>} detection - language id → detector id, or null to skip the language
 * @param {Record<string, boolean>} cleaners - cleaner id → wanted
 * @returns {string[]}
 */
export function filesFor(detection, cleaners) {
  const ids = new Set()
  for (const language of LANGUAGES) {
    const engine = detector(detection?.[language.id] ?? '')
    if (engine?.ready && engine.languages?.includes(language.id)) for (const file of engine.files) ids.add(file)
  }
  for (const engine of CLEANERS) {
    if (engine.ready && cleaners?.[engine.id]) for (const file of engine.files) ids.add(file)
  }
  return [...ids]
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
