/**
 * The mock engine - the only implementation of the backend interface today.
 *
 * It owns the fixture data and simulates the pipeline with timers. Every
 * method is async and progress arrives only on the event channel, because
 * that is all the real backend will be able to offer: a Tauri `invoke` cannot
 * return a streaming result, and no component may come to depend on timing
 * the real backend cannot provide.
 *
 * All latency is injected. `timing` overrides the base milliseconds and
 * `speed` divides them, so tests drive this with fake timers and a demo can
 * slow it down without touching the code.
 *
 * Nothing in this file imports from `svelte` or touches the DOM.
 */

import { createJournal, pushEntry, redoEntry, undoEntry, viewOf } from '../model/journal.js'
import { reviewReason } from '../model/review.js'
import { aboutInfo, buildChapter, buildFixtures, buildProject, defaultSettings } from './fixtures.js'
import { CLOUD_COST } from './provenance.js'
import { createRng } from './rng.js'
import { createRunner } from './runner.js'
import {
  applyCloudToRegion,
  applyToolToRegion,
  cleanRegionAnyway,
  cleanRegionAutomatically,
  isOutsideHeld,
  createHandRegion,
  deleteRegionMask,
  isQueueable,
  rerunRegionMask,
} from './tools.js'

/** Base milliseconds, before the speed factor. */
const DEFAULT_TIMING = Object.freeze({
  method: 140,
  openChapter: 220,
  export: 700,
  cloud: 900,
  region: 70,
  pageTail: 150,
  noticeStagger: 260,
})

const DEFAULT_TIMERS = Object.freeze({
  setTimeout: (fn, ms) => globalThis.setTimeout(fn, ms),
  clearTimeout: (id) => globalThis.clearTimeout(id),
  now: () => Date.now(),
})

/**
 * The four formats `exportChapter` can produce, and the extension each one
 * puts on a single file. Anything not here is refused by name rather than
 * substituted - `src-tauri/src/exporting.rs` is the side that decides it and
 * this is the same list, kept short so the two cannot drift far.
 */
const EXPORT_FORMATS = Object.freeze({
  PNG: 'png',
  TIFF: 'tiff',
  TIF: 'tiff',
  PSD: 'psd',
  CBZ: 'cbz',
})

/** @param {string} value */
function isAbsolutePath(value) {
  return /^(\/|[A-Za-z]:[\\/]|\\\\)/.test(String(value ?? ''))
}

/**
 * The strip is as wide as its widest
 * page and the rest are centred, so a narrower page contributes the columns
 * beside it - pixels no source file has. `ExportDialog.svelte` computes the
 * same figure to state it before the run; this is what a backend reports
 * having written.
 *
 * @param {Array<{width?: number, height?: number}>} pages
 */
function gutterPixels(pages) {
  const width = pages.reduce((widest, page) => Math.max(widest, page.width ?? 0), 0)
  return pages.reduce((sum, page) => sum + (width - (page.width ?? 0)) * (page.height ?? 0), 0)
}

/**
 * @typedef {Object} MockOptions
 * @property {number} [speed] - divides every delay; 2 is twice as fast
 * @property {Partial<typeof DEFAULT_TIMING>} [timing] - base milliseconds, before `speed`
 * @property {{ setTimeout: Function, clearTimeout: Function, now: () => number }} [timers] - injectable clock
 * @property {string} [seed] - seeds the session generator; equal seeds give equal sessions
 */

/**
 * @param {MockOptions} [options]
 * @returns {import('./backend.js').Backend}
 */
export function createMockBackend(options = {}) {
  const speed = options.speed ?? 1
  const timers = options.timers ?? DEFAULT_TIMERS
  const base = { ...DEFAULT_TIMING, ...(options.timing ?? {}) }
  const timing = Object.fromEntries(
    Object.entries(base).map(([key, ms]) => [key, Math.max(0, Math.round(ms / speed))]),
  )

  const fixtures = buildFixtures()
  const state = {
    projects: fixtures.projects,
    settings: defaultSettings(),
    session: { cloudAcknowledged: false, spendConfirmed: false },
    /**
     * Which catalogue rows this machine does *not* have. The redraw model
     * starts here on purpose - see `listModels` - so the engine gating is
     * visible in a browser rather than only inside a Tauri window with a
     * half-empty models directory. It used to be MI-GAN's row, which was the
     * cheapest one to be missing; that rung is gone and the inpainter is what
     * is left to withhold.
     *
     * @type {Set<string>}
     */
    // The redraw model, and all three parts of the rescue reader. See
    // `listModels` for why anything is missing at all; the reader is here
    // because *absent* is its real default - it is the one catalogue row
    // nothing requires, so a browser that showed it installed would never draw
    // an optional row with something to download.
    missingModels: new Set(['inpainter', 'ocrEncoder', 'ocrDecoder', 'ocrVocab']),
    /** What `verifyModel` has digested this session. @type {Map<string, boolean>} */
    verifiedModels: new Map(),
    /** Downloads in flight, by id, holding the timer that advances each one. @type {Map<string, any>} */
    downloads: new Map(),
    /**
     * What a stopped download left behind, by id: the bytes a `.part` file
     * would be holding on a real machine.
     *
     * The native side keeps the prefix so the next press can resume from it
     * and reports it as `partialBytes` so the row can offer to
     * throw it away. Here it is a number, which is enough to drive both
     * halves of the interface - the line under the row and the Discard press -
     * in a browser.
     *
     * @type {Map<string, number>}
     */
    partials: new Map(),
  }
  const sequence = { value: fixtures.sequence }
  const rng = createRng(options.seed ?? 'session')

  /** @type {Set<(event: Object) => void>} */
  const handlers = new Set()
  let noticeCounter = 0
  let projectCounter = 0
  let handCounter = 0

  const emit = (event) => {
    const frozen = structuredClone(event)
    for (const handler of handlers) handler(frozen)
  }

  /**
   * @param {string} key
   * @param {Object} [params]
   * @param {'info'|'warn'} [tone]
   */
  const notify = (key, params = {}, tone = 'info') => {
    noticeCounter += 1
    emit({ type: 'notice', id: `notice-${noticeCounter}`, key, params, tone })
  }

  const notifyAll = (notices) => {
    for (const notice of notices) notify(notice.key, notice.params, notice.tone)
  }

  const delay = (ms) => new Promise((resolve) => timers.setTimeout(resolve, ms))

  const toolContext = {
    rng,
    nextSequence: () => {
      sequence.value += 1
      return sequence.value
    },
    created: () => new Date(timers.now()).toISOString(),
    settings: state.settings,
  }

  /* ---------- lookups ---------- */

  const findProject = (projectId) => state.projects.find((p) => p.id === projectId) ?? null

  /** @returns {{ project: Object, chapter: Object }|null} */
  const findChapter = (chapterId) => {
    for (const project of state.projects) {
      const chapter = project.chapters.find((c) => c.id === chapterId)
      if (chapter) return { project, chapter }
    }
    return null
  }

  const findRegion = (regionId) => {
    for (const project of state.projects) {
      for (const chapter of project.chapters) {
        for (const page of chapter.pages) {
          const region = page.regions.find((r) => r.id === regionId)
          if (region) return { project, chapter, page, region }
        }
      }
    }
    return null
  }

  const findPage = (pageId) => {
    for (const project of state.projects) {
      for (const chapter of project.chapters) {
        const page = chapter.pages.find((p) => p.id === pageId)
        if (page) return page
      }
    }
    return null
  }

  const findMask = (maskId) => {
    const regionId = maskId.slice(0, maskId.lastIndexOf('-m'))
    const found = findRegion(regionId)
    return found && found.region.mask && found.region.mask.id === maskId ? found : null
  }

  /* ---------- the run scheduler ---------- */

  const runner = createRunner({
    emit,
    timers,
    timing: { region: timing.region, pageTail: timing.pageTail },
    cleanRegion: (region, page, ctx) => cleanRegionAutomatically(region, page, toolContext, ctx),
    onFinished: (summary) => {
      const found = findChapter(summary.chapterId)
      if (summary.reason === 'cancelled') {
        if (found) {
          found.project.interruptedJob = {
            chapterId: summary.chapterId,
            pageIndex: summary.nextPageIndex,
          }
        }
        notify('notice.run.cancelled', {}, 'warn')
        return
      }
      if (found) found.project.interruptedJob = null
      if (summary.regionsCleaned === 0) {
        notify('notice.chapter.emptyResult', { regions: 0, pages: summary.pagesCleaned }, 'warn')
      } else {
        notify('notice.run.finished', {
          pages: summary.pagesCleaned,
          regions: summary.regionsCleaned,
        })
      }
    },
  })

  // A page is runnable when something on it is queueable - and with the
  // outside-bubble opt-in on, a region held back as "text outside a speech
  // bubble" is queueable too (`isOutsideHeld`).
  const runnable = (page, outsideBubbles) =>
    page.status !== 'skipped' &&
    (page.status === 'unclean' ||
      page.regions.some(
        (region) => isQueueable(region) || (outsideBubbles === 'clean' && isOutsideHeld(region)),
      ))

  const queueFor = ({ scope, chapterId, pageIndex, outsideBubbles }) => {
    const found = findChapter(chapterId)
    if (!found) return []
    const chapters =
      scope === 'project' ? found.project.chapters : [found.chapter]
    const entries = []
    for (const chapter of chapters) {
      const pages =
        scope === 'page'
          ? chapter.pages.filter((page) => page.index === pageIndex)
          : chapter.pages
      for (const page of pages) {
        if (runnable(page, outsideBubbles)) {
          entries.push({ projectId: found.project.id, chapterId: chapter.id, page })
        }
      }
    }
    return entries
  }

  /**
   * What a real run holds in memory, as the loaded-models tab would see it.
   *
   * The sizes are this machine's actual weights and the measured
   * 510 MB for rung 2, so the tab's layout is exercised against the numbers it
   * will really be given rather than against round ones. The mock has no
   * sessions, so what stands in for "loaded" is "a run is in progress" - which
   * is when the real backend has these open too.
   */
  const MOCK_MODELS = Object.freeze([
    { id: 1, kindKey: 'models.kind.textDetector', bytes: 94_669_756, basis: 'weights', deviceKey: 'accel.coreml', gpu: true },
    { id: 2, kindKey: 'models.kind.balloonDetector', bytes: 11_120_765, basis: 'weights', deviceKey: 'accel.cpu', gpu: false },
    { id: 3, kindKey: 'models.kind.scriptGate', bytes: 3_722_314, basis: 'weights', deviceKey: 'accel.cpu', gpu: false },
    { id: 4, kindKey: 'models.kind.inpainter', bytes: 534_773_760, basis: 'measured', deviceKey: 'accel.webgpu', gpu: true },
  ])

  /** Ids the caller has asked back. Reset when a run starts, because a run reloads what it needs. */
  const unloadedModels = new Set()

  const startRun = ({
    scope,
    chapterId,
    pageIndex,
    engineCeiling,
    bubbleEngine,
    outsideEngine,
    outsideBubbles,
  }) => {
    if (runner.isRunning()) return { runId: runner.activeRunId(), pages: [], alreadyRunning: true }
    unloadedModels.clear()
    const queue = queueFor({ scope, chapterId, pageIndex, outsideBubbles })
    if (queue.length === 0) {
      notify('notice.run.nothingInScope', {}, 'warn')
      return { runId: null, pages: [] }
    }
    return runner.start(queue, {
      chapterId,
      engineCeiling: engineCeiling ?? state.settings.engineCeiling,
      bubbleEngine,
      outsideEngine,
      outsideBubbles,
    })
  }

  const snapshot = (value) => structuredClone(value)

  /**
   * A settings snapshot with the Hugging Face token taken out.
   *
   * The seam says the token never travels in this direction, and the mock
   * keeps it in the same object every other preference lives in - so the rule is enforced here
   * rather than left to fall out of where the value happens to be stored. The
   * native adapter strips it at the same boundary and for the same reason: on
   * a machine with no credential store, `settings.json` really is holding one.
   *
   * @param {Object} settings
   */
  const withoutToken = (settings) => {
    const { hfToken: _hfToken, ...rest } = settings
    return rest
  }

  /* ---------- residency ---------- */

  /*
   * The mock owns a fixture library and holds all of it, which a real backend
   * never does. What it must still do is *answer the way the real one answers*,
   * or the interface's paging is exercised by nothing: a chapter crosses the
   * seam as page **headers** plus a chapter-wide review index, and the regions
   * of a page arrive only when `loadPages` is asked for them.
   *
   * Without this the window in `src/lib/state/pagewindow.svelte.js` would be
   * dead code under test - every page would already be resident - and the first
   * time anyone ran the built application against the Tauri adapter, the Layers
   * panel would be empty for every page.
   */

  /**
   * The three numbers a header answers with: how many regions the page has,
   * how many are finished, how many need review. A real backend reads them out
   * of the manifest; the Pages list draws all three for every page of the
   * chapter, so a header without them is a row that says `0 / 0`.
   */
  const countsOf = (regions) => ({
    regionCount: regions.length,
    doneCount: regions.filter((region) => region.mask && !reviewReason(region)).length,
    reviewCount: regions.filter((region) => reviewReason(region) !== null).length,
  })

  /** One page, with its regions withheld. */
  const pageHeader = (page) => {
    const { regions, ...header } = page
    return { ...header, ...countsOf(regions), regions: [], resident: false }
  }

  /** One page, paged in. */
  const residentPage = (page) => ({
    ...page,
    ...countsOf(page.regions),
    resident: true,
  })

  /** The chapter's review set, in page order - `ReviewRef` on the seam. */
  const reviewIndexOf = (chapter) => {
    const refs = []
    for (const page of chapter.pages) {
      for (const region of page.regions) {
        const reasonKey = reviewReason(region)
        if (reasonKey) {
          refs.push({ id: region.id, pageId: page.id, pageIndex: page.index, reasonKey })
        }
      }
    }
    return refs
  }

  /** A chapter as it crosses the seam: headers, a review index, no regions. */
  const chapterHeaders = (chapter) => ({
    ...chapter,
    pages: chapter.pages.map(pageHeader),
    review: reviewIndexOf(chapter),
  })

  /** A project, with every chapter reduced the same way. */
  const projectHeaders = (project) => ({
    ...project,
    chapters: project.chapters.map(chapterHeaders),
  })

  /* ---------- the undo journal ---------- */

  /**
   * One journal per chapter. In a real backend this is
   * `<job>.mtclean.d/history.json`; here it is a Map, and the semantics are the
   * same three rules `src/lib/model/journal.js` states, because both sides call
   * the same functions.
   */
  /** @type {Map<string, import('../model/journal.js').Journal>} */
  const journals = new Map()
  const journalFor = (chapterId) => {
    let journal = journals.get(chapterId)
    if (!journal) {
      journal = createJournal()
      journals.set(chapterId, journal)
    }
    return journal
  }

  /* ---------- the model catalogue ---------- */

  /**
   * The five weights `src-tauri/src/weights.rs` pins, with the real sizes and
   * the real kind keys. Not the URLs or the digests: nothing here downloads
   * anything, and a second copy of a digest is a second thing to drift.
   */
  const MOCK_CATALOGUE = Object.freeze([
    { id: 'textDetector', fileName: 'comictextdetector.onnx', bytes: 94_669_756, kindKey: 'models.kind.textDetector', requiredBy: ['autoClean'] },
    { id: 'inpainter', fileName: 'lama-manga.onnx', bytes: 207_482_644, kindKey: 'models.kind.inpainter', requiredBy: ['lama'] },
    { id: 'scriptGate', fileName: 'image-script-identification-osd_lstm.onnx', bytes: 3_722_314, kindKey: 'models.kind.scriptGate', requiredBy: ['autoClean'] },
    { id: 'scriptGateLabels', fileName: 'image-script-identification-osd_labels.json', bytes: 1_163, kindKey: 'models.kind.scriptGateLabels', requiredBy: ['autoClean'] },
    { id: 'balloonDetector', fileName: 'comic-text-and-bubble-detector-detector-v4-s_int8.onnx', bytes: 11_120_765, kindKey: 'models.kind.balloonDetector', requiredBy: ['autoClean'] },
    // The gate's rescue reader. `requiredBy` is empty in the real table too,
    // and that is the row's whole character: it is downloadable and nothing
    // needs it, so it belongs in Settings and not in the first-launch offer.
    // Mirrored here so the Models section is exercised against a catalogue
    // that has such a row in it rather than against one where every row is a
    // precondition of something.
    { id: 'ocrEncoder', fileName: 'manga-ocr-encoder_model.onnx', bytes: 343_454_249, kindKey: 'models.kind.ocr', requiredBy: [] },
    { id: 'ocrDecoder', fileName: 'manga-ocr-decoder_model.onnx', bytes: 117_480_262, kindKey: 'models.kind.ocrDecoder', requiredBy: [] },
    { id: 'ocrVocab', fileName: 'manga-ocr-vocab.txt', bytes: 30_216, kindKey: 'models.kind.ocrVocab', requiredBy: [] },
  ])

  /**
   * The published size of the macOS arm64 archive, from
   * `crates/cleaner-core/src/runtime/package.rs`. The real number for the same
   * reason the catalogue's sizes are real: a second table that invented
   * plausible ones would be a table to disagree with the first.
   */
  const MOCK_RUNTIME_BYTES = 32_396_562

  /**
   * The chosen build, as the runtime row reports it. Whatever `runtimeFlavour`
   * says, the answer here is the one flavour this platform publishes - which is
   * exactly what `package::for_host_flavour` does with an id it does not
   * recognise.
   */
  function runtimeFlavour() {
    return { version: '1.28.0', flavour: 'stock', bytes: MOCK_RUNTIME_BYTES }
  }

  const MOCK_MODELS_DIR = '/Users/you/Library/Application Support/manga-cleaner/models'
  const MOCK_RUNTIME_DIR = '/Users/you/Library/Application Support/manga-cleaner/runtimes'
  const MOCK_RUNTIME_ID = 'runtime'

  // FIXTURE KNOB: `?firstLaunch=1` empties this machine, which is the only way
  // to see the first-launch offer in a browser - the fixture above is a
  // machine missing the redraw model alone. Applied here rather than
  // where `missingModels` is built so it can name the catalogue's own ids
  // instead of a second copy of them, and inert inside a Tauri window, which
  // answers from disk.
  if (new URLSearchParams(globalThis.location?.search ?? '').has('firstLaunch')) {
    state.missingModels = new Set([...MOCK_CATALOGUE.map((model) => model.id), MOCK_RUNTIME_ID])
  }

  /**
   * A download as a chain of timers, and the one place three of the seam's
   * seven model methods meet.
   *
   * Eight ticks whatever the size, because what is being imitated is the
   * *shape* of the report - a first event at zero, a run of partial ones, and
   * exactly one carrying `done` - rather than a transfer rate the mock has no
   * way to have. Cancelling ends it through the same `done` event a failure
   * would, so the interface has one path back to "not installed".
   *
   * The answer says **why** it did not start one:
   * `alreadyRunning` for an id in flight, `alreadyInstalled` for
   * a weight already on disk. An unknown id is neither and is still a rejection
   * - that is a caller's bug rather than a race between two windows.
   *
   * @param {string} id
   * @param {boolean} [installedIsFine] the runtime, which is re-downloaded to switch build
   * @returns {import('./backend.js').DownloadStart}
   */
  function startDownload(id, installedIsFine = false) {
    if (state.downloads.has(id)) return 'alreadyRunning'
    const known = id === MOCK_RUNTIME_ID || MOCK_CATALOGUE.some((model) => model.id === id)
    if (!known) throw new Error(`no such model: ${id}`)
    if (!installedIsFine && !state.missingModels.has(id)) return 'alreadyInstalled'
    const total = MOCK_CATALOGUE.find((model) => model.id === id)?.bytes ?? MOCK_RUNTIME_BYTES
    // A press resumes from whatever the last one left, which is the whole of
    // the `.part` contract: the first event carries the prefix
    // rather than zero, so a bar picking a download back up starts where it
    // stopped.
    let sent = state.partials.get(id) ?? 0
    const step = Math.max(1, Math.ceil(total / 8))
    const tick = () => {
      if (!state.downloads.has(id)) return
      sent = Math.min(total, sent + step)
      if (sent >= total) {
        state.downloads.delete(id)
        state.partials.delete(id)
        state.missingModels.delete(id)
        state.verifiedModels.set(id, true)
        emit({ type: 'model-progress', id, downloaded: total, total, done: true, error: null })
        return
      }
      // Written down on every tick rather than on the way out, because a
      // cancellation is a timer that simply stops: what survives it is what the
      // last tick had, exactly as a `.part` is however many bytes reached the
      // disk before the thread noticed the flag.
      state.partials.set(id, sent)
      emit({ type: 'model-progress', id, downloaded: sent, total, done: false, error: null })
      state.downloads.set(id, timers.setTimeout(tick, timing.method))
    }
    state.downloads.set(id, timers.setTimeout(tick, timing.method))
    emit({ type: 'model-progress', id, downloaded: sent, total, done: false, error: null })
    return 'started'
  }

  /* ---------- the adapter ---------- */

  return {
    subscribe(handler) {
      handlers.add(handler)
      return () => handlers.delete(handler)
    },

    async listProjects() {
      await delay(timing.method)
      // Headers only: Home draws page counts, status marks and a review total,
      // and every one of those is answered without a single region (§1b).
      return snapshot(state.projects.map(projectHeaders))
    },

    async createProject({ name, mode, sourcePath, readingDirection }) {
      await delay(timing.method)
      projectCounter += 1
      const project = buildProject(
        {
          id: `project-${projectCounter}`,
          name,
          mode,
          sourcePath: sourcePath ?? '~/scans/untitled',
          readingDirection,
        },
        sequence,
      )
      state.projects.unshift(project)
      notify('notice.project.created', { modeKey: `project.mode.${mode}` })
      return snapshot(project)
    },

    /**
     * The mock has no filesystem, so it cannot run the real backend's
     * inference - it cannot know whether a subfolder named after the chapter
     * exists. What it can mirror is the rule that inference now obeys:
     * **no chapter is ever given a folder
     * another chapter of the same project already reads.** Its default is the
     * subfolder the name implies, and a collision is refused with the same
     * notice the Tauri backend sends rather than quietly producing two chapters
     * over one folder.
     */
    async createChapter({ projectId, name, number: given, sourcePath }) {
      await delay(timing.method)
      const project = findProject(projectId)
      if (!project) return null
      const resolved = sourcePath || `${project.sourcePath}/${name}`
      const taken = project.chapters.find((c) => c.sourcePath === resolved)
      if (taken) {
        notify('notice.chapter.sourceTaken', { chapter: taken.name }, 'warn')
        return null
      }
      // The user's number when the dialog sent one; `max + 1` is only the
      // fallback for a caller that did not name one.
      const number =
        Number.isFinite(given) && Number(given) >= 1
          ? Math.trunc(Number(given))
          : Math.max(...project.chapters.map((c) => c.number), 0) + 1
      // The mock builds a chapter's id out of its number (`pagebuilder.js`
      // `makeChapter`), so a repeat here is not an ambiguous label - it is two
      // chapters, their pages and their regions sharing one set of ids. The
      // dialog already refuses a taken number; this is the seam saying so
      // rather than trusting it.
      if (project.chapters.some((c) => c.number === number)) {
        notify('notice.chapter.numberTaken', { chapter: number, project: project.name }, 'warn')
        return null
      }
      const chapter = buildChapter(project, { number, name, sourcePath: resolved }, sequence)
      project.chapters.unshift(chapter)
      project.chapters.forEach((c, order) => {
        c.order = order
      })
      notify('notice.chapter.added', { chapter: number, project: project.name })
      return snapshot(chapterHeaders(chapter))
    },

    async openChapter({ projectId, chapterId, convert }) {
      await delay(timing.openChapter)
      const found = findChapter(chapterId) ?? { project: findProject(projectId), chapter: null }
      if (!found.project || !found.chapter) return null
      const { project, chapter } = found
      const needsConversion = !!project.conversion && chapter.sourceFormat !== project.conversion.to

      if (needsConversion && convert) {
        for (const page of chapter.pages) {
          page.file = page.file.replace(/\.[^.]+$/, `.${project.conversion.to.toLowerCase()}`)
        }
        chapter.sourceFormat = project.conversion.to
        notify('notice.convert.finished', {
          count: chapter.pages.length,
          format: project.conversion.to,
        })
      }

      const reports = [...chapter.inputReports]
      const skipped = chapter.pages.filter((page) => page.status === 'skipped')
      if (skipped.length > 0) {
        reports.push({
          key: 'notice.input.fileSkipped',
          params: {
            count: skipped.length,
            file: skipped[0].file,
            reasonKey: skipped[0].skipReason,
          },
          tone: 'warn',
        })
      }
      if (chapter.noTextDetected) {
        reports.push({
          key: 'notice.chapter.emptyResult',
          params: { regions: 0, pages: chapter.pages.length },
          tone: 'warn',
        })
      }
      reports.forEach((report, i) => {
        timers.setTimeout(
          () => notify(report.key, report.params, report.tone),
          timing.noticeStagger * (i + 1),
        )
      })

      return {
        project: snapshot(projectHeaders(project)),
        chapter: snapshot(chapterHeaders(chapter)),
        pendingConversion:
          needsConversion && !convert
            ? { ...project.conversion, fileCount: chapter.pages.length }
            : null,
      }
    },

    /**
     * Bring a window of pages into residency.
     *
     * Whole pages rather than bare region lists, so the status a page's regions
     * imply travels with them: a page whose last mask was deleted while it sat
     * outside the window comes back with the status it now has, not the one its
     * header carried when it left.
     *
     * An index that is not there is skipped rather than refused - the window
     * slides past both ends of a chapter, and clamping it in two places is how
     * the two come to disagree.
     */
    async loadPages({ chapterId, indices }) {
      await delay(timing.method)
      const found = findChapter(chapterId)
      if (!found) return []
      const wanted = Array.isArray(indices) ? indices : []
      return snapshot(
        wanted
          .map((index) => found.chapter.pages.find((page) => page.index === index))
          .filter(Boolean)
          .map(residentPage),
      )
    },

    /* ---------- the undo journal ---------- */

    async historyLoad({ chapterId }) {
      await delay(timing.method)
      return snapshot(viewOf(journalFor(chapterId)))
    },

    async historyPush({ chapterId, entry }) {
      await delay(timing.method)
      const journal = journalFor(chapterId)
      pushEntry(journal, entry)
      return snapshot(viewOf(journal))
    },

    /**
     * One step. The cursor moves before anything is replayed, which is what
     * makes an interrupted undo resolve the same way twice: re-applying a delta
     * to a state already in it is a no-op by construction.
     */
    async historyMove({ chapterId, direction }) {
      await delay(timing.method)
      const journal = journalFor(chapterId)
      const entry = direction === 'redo' ? redoEntry(journal) : undoEntry(journal)
      return snapshot({ cursor: journal.cursor, entry: entry ?? null })
    },

    async renameProject({ projectId, name }) {
      await delay(timing.method)
      const project = findProject(projectId)
      if (!project) return null
      project.name = name
      notify('notice.project.renamed', { name })
      return snapshot(project)
    },

    async deleteProject({ projectId }) {
      await delay(timing.method)
      const index = state.projects.findIndex((p) => p.id === projectId)
      if (index === -1) return false
      const [removed] = state.projects.splice(index, 1)
      notify('notice.project.deleted', { name: removed.name }, 'warn')
      return true
    },

    /**
     * Delete a chapter. The mock holds no files, so `sourceFiles` cannot be
     * acted on - but the one refusal the seam promises is a rule about paths,
     * not about files, and the mock can and does keep it: a chapter reading the
     * project's own folder keeps that folder, and the notice says so.
     */
    async deleteChapter({ projectId, chapterId, sourceFiles }) {
      await delay(timing.method)
      const project = findProject(projectId)
      if (!project) return false
      const at = project.chapters.findIndex((c) => c.id === chapterId)
      if (at === -1) return false
      const [removed] = project.chapters.splice(at, 1)
      project.chapters.forEach((c, order) => {
        c.order = order
      })
      if (project.interruptedJob?.chapterId === chapterId) project.interruptedJob = null
      const key = !sourceFiles
        ? 'notice.chapter.deleted'
        : !removed.sourcePath
          ? 'notice.chapter.deletedSourceUnknown'
          : removed.sourcePath === project.sourcePath
            ? 'notice.chapter.deletedProjectFolderKept'
            : 'notice.chapter.deletedWithScans'
      notify(key, { chapter: removed.number }, 'warn')
      return true
    },

    async resumeJob({ projectId, chapterId }) {
      await delay(timing.method)
      const project = findProject(projectId)
      const job = project?.interruptedJob
      if (!job) return null
      const targetChapter = chapterId ?? job.chapterId
      const found = findChapter(targetChapter)
      if (!found) return null
      const run = startRun({ scope: 'chapter', chapterId: targetChapter })
      notify('notice.job.resumed', { page: job.pageIndex + 1 })
      return {
        project: snapshot(projectHeaders(project)),
        chapter: snapshot(chapterHeaders(found.chapter)),
        resumedFrom: job.pageIndex,
        runId: run.runId,
        pages: run.pages,
      }
    },

    async runClean({
      scope,
      chapterId,
      pageIndex,
      engineCeiling,
      bubbleEngine,
      outsideEngine,
      outsideBubbles,
    }) {
      await delay(timing.method)
      return startRun({
        scope,
        chapterId,
        pageIndex,
        engineCeiling,
        bubbleEngine,
        outsideEngine,
        outsideBubbles,
      })
    },

    async cancelRun({ runId } = {}) {
      await delay(timing.method)
      if (runId && runner.activeRunId() !== runId) return null
      return runner.cancel()
    },

    async applyTool({ tool, params = {}, chapterId, pageIndex, regionId }) {
      if (tool === 'autoClean') {
        await delay(timing.method)
        return { status: 'run-started', ...startRun({ scope: 'page', chapterId, pageIndex, ...params }) }
      }
      const found = findRegion(regionId)
      if (!found) return { status: 'not-found' }

      if (tool === 'contentAwareFill' && params.engine === 'cloud') {
        if (state.settings.cloudEngines !== 'allowed') {
          await delay(timing.method)
          notify('notice.cloud.blocked', {}, 'warn')
          return { status: 'blocked' }
        }
        if (params.acknowledgeTransmission) state.session.cloudAcknowledged = true
        if (!state.session.cloudAcknowledged) {
          return {
            status: 'needs-confirmation',
            confirmation: { kind: 'cloud-transmission', regionId, estimatedCost: CLOUD_COST },
          }
        }
        const mustConfirmSpend =
          !state.session.spendConfirmed || state.settings.confirmBeforeSpending
        if (mustConfirmSpend && !params.confirmSpend) {
          return {
            status: 'needs-confirmation',
            confirmation: { kind: 'cloud-cost', regionId, estimatedCost: CLOUD_COST },
          }
        }
        state.session.spendConfirmed = true
        const cloud = applyCloudToRegion(found.region, found.page, toolContext)
        // The round trip is its own delay: the mask's `elapsedMs` records the
        // real 3–15 s a cloud request costs, which nobody wants to sit through.
        await delay(timing.cloud)
        notifyAll(cloud.notices)
        return {
          status: 'applied',
          region: snapshot(found.region),
          mask: snapshot(cloud.mask),
          pageStatus: found.page.status,
        }
      }

      const applied = applyToolToRegion(found.region, found.page, toolContext, { tool, params })
      await delay(timing.method)
      notifyAll(applied.notices)
      return {
        status: 'applied',
        region: snapshot(found.region),
        mask: snapshot(applied.mask),
        pageStatus: found.page.status,
      }
    },

    /**
     * A region drawn by hand, where the detector found nothing.
     *
     * The additive half of the tool seam. `applyTool` needs a region id, and
     * Brush, Shapes and the AI mask brush exist precisely to make masks where
     * there is no region yet. The geometry arrives in the
     * region's own normalised coordinate space, so the canvas, the Layers panel
     * and the export all describe the same rectangle.
     *
     * Returns the page's status with the region for the same reason every other
     * region-level edit does: a hand mask on an unclean page makes it cleaned.
     */
    async createRegion({ chapterId, pageIndex, bbox, tool, params = {} }) {
      await delay(timing.method)
      const found = findChapter(chapterId)
      const page = found?.chapter.pages.find((candidate) => candidate.index === pageIndex)
      if (!page || !bbox) return null
      handCounter += 1
      const created = createHandRegion(page, toolContext, {
        id: `${page.id}-h${handCounter}`,
        bbox,
        tool,
        params,
      })
      notifyAll(created.notices)
      return { region: snapshot(created.region), pageStatus: page.status }
    },

    /**
     * Deleting a mask deletes the row: the original text comes back and the
     * region goes off the page. A region with no mask is one the Layers panel
     * lists as *unexamined* and the canvas draws a box for, so leaving one
     * behind answered the delete with an empty placeholder in the same place.
     * The native backend keeps the record on disk, invisible, so undo can put
     * it back - the mock's undo is the snapshot the caller kept, which is the
     * same round trip through `restoreRegion`.
     *
     * `region: null` is therefore the answer, and the page's status is the half
     * that carries information: a region-level edit can move the page it is on
     * - cleaning a gate-skipped region makes an unclean page a cleaned one, and
     * deleting the last mask on a page takes that back - and a caller that
     * replaced only the region would show a full track under a "not cleaned"
     * mark, or the reverse.
     */
    async deleteMask({ maskId }) {
      await delay(timing.method)
      const found = findMask(maskId)
      if (!found) return null
      notifyAll(deleteRegionMask(found.region).notices)
      const index = found.page.regions.findIndex((r) => r.id === found.region.id)
      if (index >= 0) found.page.regions.splice(index, 1)
      // Nothing on this page is cleaned any more, so it is not a cleaned page.
      if (found.page.status === 'cleaned' && found.page.regions.every((r) => !r.mask)) {
        found.page.status = 'unclean'
      }
      return { region: null, pageStatus: found.page.status }
    },

    /**
     * Puts a region back exactly as the caller last saw it. Undo needs a route
     * back through the seam, not only in the interface's own copy: after a
     * deleted mask is restored, `rerunMask` must be able to find it again, and
     * a re-run undone must leave the *previous* mask addressable. One method
     * covers delete, re-run and clean-anyway because all three are edits to one
     * region and all three are reversed by restoring it.
     *
     * *Exactly* as the caller last saw it: the stored region is replaced
     * wholesale rather than merged over. A merge restores only the keys the
     * snapshot happens to carry, so any key a forward edit added would survive
     * its own undo.
     *
     * `pageStatus` is the other half of the snapshot - see `deleteMask`. It is
     * optional, so a caller reversing an edit that cannot move the page need
     * not carry it.
     *
     * **A region that was not there is a state a snapshot can hold.** Undoing a
     * hand-drawn mask means removing the region the gesture created, and
     * redoing it means putting that exact region back - so `region: null`
     * removes, and a snapshot whose region is gone is re-inserted on the page
     * its `pageId` names. That keeps a creation's undo pair the same shape as
     * every other edit's: two snapshots, one call in each direction, no way for
     * redo to drift from undo. The method's signature is unchanged; only the
     * domain of `region` widens to include "nothing".
     *
     * Silent by design: the undo control is the feedback, and a notice per
     * undo would bury the ones that carry information.
     */
    async restoreRegion({ regionId, region, pageStatus }) {
      await delay(timing.method)
      const found = findRegion(regionId)

      if (!region) {
        if (!found) return null
        const index = found.page.regions.findIndex((r) => r.id === regionId)
        found.page.regions.splice(index, 1)
        if (pageStatus) found.page.status = pageStatus
        return null
      }

      const page = found?.page ?? findPage(region.pageId)
      if (!page) return null
      const index = page.regions.findIndex((r) => r.id === regionId)
      if (index >= 0) page.regions[index] = structuredClone(region)
      else page.regions.push(structuredClone(region))
      if (pageStatus) page.status = pageStatus
      return snapshot(page.regions.find((r) => r.id === regionId))
    },

    async rerunMask({ maskId, kind, engine }) {
      await delay(timing.method)
      const found = findMask(maskId)
      if (!found) return null
      const result = rerunRegionMask(found.region, toolContext, { kind, engine })
      notifyAll(result.notices)
      return {
        region: snapshot(found.region),
        mask: snapshot(result.mask),
        reopenTool: result.reopenTool,
        pageStatus: found.page.status,
      }
    },

    /**
     * Rung 3a is a native sidecar and the mock has no process to ask, so the
     * dev backend answers the honest thing about itself: not available, and
     * nothing to say about why. The picker's flux entry is therefore not shown
     * under `npm run dev`, which is the same answer a machine with nothing
     * installed gets.
     */
    async sidecarAvailable() {
      await delay(timing.method)
      return { available: false, reasonKey: null }
    },

    async listSidecarModels() {
      await delay(timing.method)
      return []
    },

    /**
     * The loaded-models tab's poll.
     *
     * **No `delay`.** Every other method here imitates a backend that has work
     * to do; this one imitates a walk over four rows under a mutex, and a
     * simulated 140 ms on a call the interface makes every couple of seconds
     * would be inventing a cost the real command does not have.
     */
    async listLoadedModels() {
      if (!runner.isRunning()) return []
      return MOCK_MODELS.filter((model) => !unloadedModels.has(model.id)).map((model) => ({
        ...model,
        idleMs: 0,
        unloading: false,
      }))
    },

    /**
     * The mock has no session to drop, so the row simply goes - which is what
     * the real backend arrives at one region boundary later. `false` for a row
     * that has already gone, the same answer `src-tauri/src/models.rs` gives.
     */
    async unloadModel({ id }) {
      const known = MOCK_MODELS.some((model) => model.id === id)
      if (!known || unloadedModels.has(id) || !runner.isRunning()) return false
      unloadedModels.add(id)
      return true
    },

    /**
     * The accelerator setting, on a machine the mock does not have.
     *
     * A **fixed** answer, and deliberately the honest shape of one: the mock
     * has no ONNX Runtime to ask, so rather than inventing a machine's worth of
     * availability it reports one particular machine in full. Every provider
     * the real backend can name is a row, so a panel written against this one
     * is written against the real shape. No `delay`, for `listLoadedModels`'s
     * reason.
     *
     * **The machine is a Windows one, and that is the whole point of the
     * choice.** It used to be the Apple machine this project was measured on,
     * where nothing is unmeasured, nothing is declined and no provider is
     * forced - so `noteKey` and `declinedKey` were null on every row and the
     * two branches of the Acceleration panel that render them could not be seen
     * at all. Those branches are where every Windows behaviour lives, this
     * repository cannot execute on Windows, and a browser run against this mock
     * is therefore the only place the behaviour can be looked at before it
     * ships. So the machine here has:
     *
     *   - **DirectML, forced and unmeasured.** The timings are Apple's; on
     *     Direct3D the choice rests on how the provider works rather than on
     *     anything anyone timed, which is exactly what `accel.chosen.unmeasured`
     *     says and what `measured: false` on every row here means.
     *   - **CUDA in the build and not on the machine.** A different remedy from
     *     "not in this runtime" - an install rather than a download - and the
     *     picker's disabled entries are the only place a user reads it.
     *   - **A provider refused for want of memory, with the two figures.** The
     *     inpainter wants more than this machine has room for, so the forced
     *     provider is declined and it lands on the CPU. The byte counts travel
     *     beside the key, and the panel formats them.
     *   - **A provider refused for being the wrong shape.** The language
     *     checker would be split across DirectML and run slower, so it is
     *     declined too - a decline with a reason and no figures, which is the
     *     other half of that branch.
     */
    async listAccelerators() {
      // id, available, measured, active, reasonKey when it is not available.
      // Nothing is measured: the timing table was taken on Apple silicon and
      // this machine is not that.
      const providers = [
        ['cpu', true, false, true, null],
        ['coreml', false, false, false, 'accel.declined.unavailable'],
        ['directml', true, false, true, null],
        // In this runtime build, and NVIDIA's own parts are not installed.
        ['cuda', false, false, false, 'accel.declined.missingRuntime'],
        ['tensorrt', false, false, false, 'accel.declined.unavailable'],
        ['rocm', false, false, false, 'accel.declined.unavailable'],
        ['openvino', false, false, false, 'accel.declined.unavailable'],
        // Loaded, usable, and not what this platform's placements pick.
        ['webgpu', true, false, false, null],
        ['xnnpack', false, false, false, 'accel.declined.unavailable'],
      ]
      // A user who went looking for the graphics card, which is what makes the
      // declines below reachable: a decline is a *forced* provider not being
      // used, and `auto` forces nothing.
      const preference = 'directml'
      return {
        preference,
        providers: providers.map(([id, available, measured, active, reasonKey]) => ({
          id,
          labelKey: `accel.${id}`,
          available,
          reasonKey,
          measured,
          active,
          selected: id === preference,
        })),
        models: [
          ['models.kind.textDetector', 'directml', 'accel.chosen.unmeasured'],
          ['models.kind.balloonDetector', 'directml', 'accel.chosen.unmeasured'],
          // Declined for its shape rather than for the machine's size: a
          // provider with no kernel for one of the graph's operators splits the
          // model instead of failing, and the split runs slower than the CPU
          // does whole.
          [
            'models.kind.scriptGate',
            'cpu',
            null,
            'accel.declined.partitioned',
            'directml',
            null,
            null,
          ],
          // The memory decline, with the two figures the sentence cannot carry.
          // 5.62 GB is what this rung was measured to need; 3 GB is what this
          // machine had room for.
          [
            'models.kind.inpainter',
            'cpu',
            'accel.chosen.unmeasured',
            'accel.declined.memory',
            'directml',
            6_034_997_248,
            3_221_225_472,
          ],
          // The rescue reader. On the CPU wherever it runs - a graphics
          // provider builds both its graphs and is still the wrong answer, see
          // `accel::OCR` - so it is not a decline and carries no note. It is
          // listed whether or not the weights are installed, because the panel
          // answers "where would each model run", not "what is loaded".
          ['models.kind.ocr', 'cpu'],
        ].map(
          ([
            modelKey,
            id,
            noteKey = null,
            declinedKey = null,
            declinedId = null,
            neededBytes = null,
            roomBytes = null,
          ]) => ({
            modelKey,
            acceleratorId: id,
            labelKey: `accel.${id}`,
            noteKey,
            declinedKey,
            declinedId,
            neededBytes,
            roomBytes,
          }),
        ),
      }
    },

    /**
     * The model catalogue, on a machine that has almost everything.
     *
     * **One row is deliberately missing**, and it is the redraw model. The
     * gating this whole call exists for - an engine whose weights are absent
     * is not offered at all - is invisible in a browser if the mock reports
     * every file present, and an interface nobody can see is an interface
     * nobody checks. So `inpainter` is not installed here, the Layers picker
     * and the Shapes row show two rungs rather than three, and Settings shows
     * one Download button with something to download.
     *
     * The rescue reader's three rows are missing for a different reason: it is
     * optional in the catalogue itself (`requiredBy: []`), nothing is gated on
     * it, and absent is the state every machine starts in. It gates no engine
     * either way, which is the property `featuresFrom` should be seen holding.
     *
     * The sizes and the kind keys are the real ones from
     * `src-tauri/src/weights.rs`; a mock that invented plausible numbers would
     * be a second catalogue to disagree with the first.
     */
    async listModels({ retryStore = false } = {}) {
      await delay(timing.method)
      // `retryStore` is accepted and does nothing here, which is the honest
      // answer rather than a shrug: it asks the native side to offer the token
      // to a credential store that refused once, and a browser
      // has no store to have refused. Named rather than ignored so that the
      // mock's signature is the seam's.
      void retryStore
      return {
        models: MOCK_CATALOGUE.map((model) => ({
          ...model,
          installed: !state.missingModels.has(model.id),
          path: state.missingModels.has(model.id) ? null : `${MOCK_MODELS_DIR}/${model.fileName}`,
          readOnly: false,
          sha256Ok: state.verifiedModels.get(model.id) ?? null,
          downloading: state.downloads.has(model.id),
          partialBytes: state.partials.get(model.id) ?? null,
        })),
        runtime: {
          installed: !state.missingModels.has(MOCK_RUNTIME_ID),
          path: state.missingModels.has(MOCK_RUNTIME_ID) ? null : `${MOCK_RUNTIME_DIR}/libonnxruntime.dylib`,
          readOnly: false,
          downloading: state.downloads.has(MOCK_RUNTIME_ID),
          partialBytes: state.partials.get(MOCK_RUNTIME_ID) ?? null,
          ...runtimeFlavour(),
          // What is *installed*, which the native side reads from a record the
          // install writes beside the libraries. The mock has
          // one build and installs it, so the two never differ here and the
          // "installed X, chosen Y" line is not drawn in a browser - the same
          // honesty as the flavour picker below.
          ...(state.missingModels.has(MOCK_RUNTIME_ID)
            ? { installedFlavour: null, installedVersion: null }
            : { installedFlavour: 'stock', installedVersion: '1.28.0' }),
          // One flavour, because the machine the mock reports is an Apple one
          // and macOS publishes one build. The picker is therefore *not* drawn
          // in a browser, which is the honest thing: a Windows-only control
          // faked here would be a control nobody could check against a real
          // answer.
          flavours: [
            {
              id: 'stock',
              ortVersion: '1.28.0',
              bytes: MOCK_RUNTIME_BYTES,
              isDefault: true,
              userInstalled: [],
            },
          ],
          available: true,
        },
        modelsDir: MOCK_MODELS_DIR,
        runtimeDir: MOCK_RUNTIME_DIR,
        hasToken: typeof state.settings.hfToken === 'string' && state.settings.hfToken.length > 0,
        // A browser has no credential store, and the mock keeps the token in
        // the same settings object every other preference lives in - which is
        // exactly the `fileNoStore` fallback the native side reports on a build
        // with no keychain, so the mock reports it honestly rather than
        // claiming a keychain it does not have. `fileStoreUnavailable` is the
        // other fallback and the mock can never truthfully give it: there is no
        // store here to have been unreachable.
        tokenStore: 'fileNoStore',
        // And with no unreachable store there is no reason to give for one.
        // `null` is what the native side answers for both other locations too.
        tokenStoreReason: null,
      }
    },

    /**
     * A download, as timers.
     *
     * Progress arrives on the event channel and nowhere else, exactly as the
     * command does it: this resolves the moment the first tick is scheduled,
     * and the row is driven from `model-progress` after that. Cancelling ends
     * it the same way a failure does - one `done` event carrying an error -
     * so the interface has one path back to "not installed" rather than three.
     */
    async downloadModel({ id }) {
      return startDownload(id)
    },

    async cancelDownload({ id }) {
      const handle = state.downloads.get(id)
      if (handle === undefined) return false
      timers.clearTimeout(handle)
      state.downloads.delete(id)
      emit({ type: 'model-progress', id, downloaded: 0, total: null, done: true, error: 'cancelled' })
      return true
    },

    /**
     * The mock owns every copy it reports - `readOnly` is false on every row -
     * so `readOnlyElsewhere` is an answer it can never truthfully give and does
     * not. What it does mirror is the distinction that matters here: a row that
     * was already gone says so instead of failing.
     */
    async deleteModel({ id }) {
      await delay(timing.method)
      if (!MOCK_CATALOGUE.some((model) => model.id === id)) throw new Error(`no such model: ${id}`)
      if (state.missingModels.has(id)) return 'notFound'
      state.missingModels.add(id)
      state.verifiedModels.delete(id)
      return 'deleted'
    },

    /**
     * Throw away what a stopped download left, and say whether there was any.
     *
     * The refusal is the one that matters and it is not an error: a transfer in
     * flight is writing into those bytes, so the press answers `false` and the
     * row - refreshed on the same press - shows the download it was competing
     * with.
     */
    async discardPartial({ id }) {
      await delay(timing.method)
      const known = id === MOCK_RUNTIME_ID || MOCK_CATALOGUE.some((model) => model.id === id)
      if (!known) throw new Error(`no such model: ${id}`)
      if (state.downloads.has(id)) return false
      return state.partials.delete(id)
    },

    /** The explicit re-digest. Always agrees with the pin here; the real one may not. */
    async verifyModel({ id }) {
      await delay(timing.method)
      if (state.missingModels.has(id)) return false
      state.verifiedModels.set(id, true)
      return true
    },

    /**
     * An installed runtime is not a refusal: the only way to change build is to
     * fetch the chosen one over the top of the one that is there, so the press
     * is always honoured. See `download_runtime` in `src-tauri/src/weights.rs`.
     */
    async downloadRuntime() {
      return startDownload(MOCK_RUNTIME_ID, true)
    },

    async deleteRuntime() {
      await delay(timing.method)
      // The native command removes the whole runtimes tree, which holds the
      // `.part` of a transfer in flight - so a running download refuses the
      // press rather than deleting the bytes out from under it.
      if (state.downloads.has(MOCK_RUNTIME_ID)) return 'busy'
      if (state.missingModels.has(MOCK_RUNTIME_ID)) return 'notFound'
      state.missingModels.add(MOCK_RUNTIME_ID)
      return 'deleted'
    },

    async cleanAnyway({ regionId, engine }) {
      await delay(timing.method)
      const found = findRegion(regionId)
      if (!found) return null
      const result = cleanRegionAnyway(found.region, found.page, toolContext, engine)
      notifyAll(result.notices)
      return {
        region: snapshot(found.region),
        mask: snapshot(result.mask),
        pageStatus: found.page.status,
      }
    },

    /**
     * The refusals `src-tauri/src/exporting.rs` decides, decided here in the
     * same order and answered in the same shape. A mock that only ever said
     * yes would leave every refusal path in the interface unexercised outside a
     * Tauri window.
     *
     * Two of the backend's refusals are not here: a PSD of an indexed or
     * sub-8-bit page (`refusedLayeredMode`) and of a page over 30 000 px
     * (`refusedLayeredSize`) are decided from the manifest's per-source mode,
     * depth and dimensions, which the mock's pages do not carry.
     */
    async exportChapter({
      chapterId,
      format = 'PNG',
      destination = 'new-folder',
      masks = 'flattened',
      layout = 'per-page',
    }) {
      const found = findChapter(chapterId)
      if (!found) return null

      /** @param {string} reasonKey @param {Object} [params] */
      const refuse = async (reasonKey, params = {}) => {
        await delay(timing.method)
        notify(reasonKey, { format, ...params }, 'warn')
        return { status: 'refused', reasonKey }
      }

      const container = EXPORT_FORMATS[String(format).toUpperCase()]
      if (!container) {
        const lossy = /^(JPE?G|WEBP)$/i.test(String(format))
        const layered = /^PSB$/i.test(String(format))
        return refuse(
          lossy
            ? 'notice.export.refusedLossyFormat'
            : layered
              ? 'notice.export.refusedLayeredFormat'
              : 'notice.export.refusedUnknownFormat',
        )
      }

      // A mask file inside a reader's archive would be read as a page.
      if (masks === 'separate-layer' && container === 'cbz') {
        return refuse('notice.export.refusedMaskLayers')
      }

      const stitched = layout === 'stitched'
      if (stitched && container === 'cbz') return refuse('notice.export.refusedStitchedArchive')
      if (stitched && container === 'psd') return refuse('notice.export.refusedStitchedLayered')
      if (stitched && found.project.mode !== 'longstrip') {
        return refuse('notice.export.refusedStitchPaginated')
      }

      // Two sentinels and an absolute path; a relative one has no directory
      // both ends of the seam would agree it is relative *to*.
      if (destination === 'source-folder') {
        return refuse('notice.export.refusedOverwrite', { path: found.project.sourcePath })
      }
      if (destination !== 'new-folder' && !isAbsolutePath(destination)) {
        return refuse('notice.export.refusedDestination')
      }

      await delay(timing.export)
      const folder =
        destination === 'new-folder'
          ? `${found.project.sourcePath}_cleaned/ch${found.chapter.number}`
          : destination
      const written = found.chapter.pages.filter((page) => page.status !== 'skipped')

      if (stitched) {
        const path = `${folder}/ch${found.chapter.number}.${container}`
        const count = gutterPixels(written)
        notify('notice.export.stitched', { count, format, path })
        return { status: 'exported', fileCount: 1, path, gutterPixels: count }
      }

      const path = container === 'cbz' ? `${folder}/ch${found.chapter.number}.cbz` : folder
      notify('notice.export.finished', { count: written.length, format, path })
      return { status: 'exported', fileCount: written.length, path }
    },

    async readSettings() {
      await delay(timing.method)
      return withoutToken(snapshot(state.settings))
    },

    async writeSettings(patch) {
      await delay(timing.method)
      Object.assign(state.settings, patch)
      return withoutToken(snapshot(state.settings))
    },

    async about() {
      await delay(timing.method)
      return aboutInfo()
    },
  }
}
