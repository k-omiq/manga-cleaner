/**
 * The Tauri adapter - the real backend, as far as it goes.
 *
 * The seam contract fixes thirty-six methods and six events, and the
 * core cannot yet answer all of them. That is not a reason to wait: `setBackend`
 * exists so a partial adapter can serve what it implements while the mock
 * serves the rest, which is what lets the pipeline be built bottom-up against a
 * finished interface rather than integrated in one jump at the end.
 *
 * So this is a **delegating** adapter. Every method is either
 *
 *   - `invoke`'d against a Tauri command, or
 *   - forwarded to `fallback` unchanged,
 *
 * and `IMPLEMENTED` below is the list of which - one place to read, and one
 * place to edit when a command lands. A method missing from both is a bug the
 * tests catch: the adapter must satisfy the whole interface or it is not one.
 *
 * Nothing here imports `@tauri-apps/api`. `withGlobalTauri` puts `invoke` on the
 * window, and taking it from there keeps this module loadable - and testable -
 * in a plain browser and in vitest, where a Tauri import would throw at load.
 */

import { createEventStream } from './tauri-events.js'

/**
 * The Tauri commands this adapter uses, by seam method name.
 *
 * Everything not in here is the mock's, and stays the mock's until the core can
 * answer it honestly.
 *
 * All six region-level edits are commands now. `deleteMask` and `restoreRegion`
 * landed first because they need **no engine** - a mask is deleted by turning a
 * persisted `visible` flag off, which the composite already honours - and the
 * other four each run a rung over one region in `src-tauri/src/region.rs`.
 * Leaving any of them with the mock
 * meant the control silently did nothing in a real window: the mock holds
 * fixture chapters, so a region id from the library matches nothing in it and
 * every call answered `null`.
 *
 * `sidecarAvailable` is the one method the seam grew for rung 3a. It is a
 * question about the machine rather than about a project, and it is asked once
 * so that a rung nobody installed is never offered rather than offered and
 * refused.
 */
const IMPLEMENTED = Object.freeze({
  about: 'about',
  readSettings: 'read_settings',
  writeSettings: 'write_settings',
  listProjects: 'list_projects',
  createProject: 'create_project',
  createChapter: 'create_chapter',
  openChapter: 'open_chapter',
  loadPages: 'load_pages',
  historyLoad: 'history_load',
  historyPush: 'history_push',
  historyMove: 'history_move',
  renameProject: 'rename_project',
  deleteProject: 'delete_project',
  deleteChapter: 'delete_chapter',
  exportChapter: 'export_chapter',
  deleteMask: 'delete_mask',
  restoreRegion: 'restore_region',
  applyTool: 'apply_tool',
  createRegion: 'create_region',
  rerunMask: 'rerun_mask',
  cleanAnyway: 'clean_anyway',
  sidecarAvailable: 'sidecar_available',
  listSidecarModels: 'list_sidecar_models',
  runClean: 'run_clean',
  cancelRun: 'cancel_run',
  resumeJob: 'resume_job',
  listLoadedModels: 'list_loaded_models',
  unloadModel: 'unload_model',
  listAccelerators: 'list_accelerators',
  listModels: 'list_models',
  downloadModel: 'download_model',
  cancelDownload: 'cancel_download',
  deleteModel: 'delete_model',
  discardPartial: 'discard_partial',
  verifyModel: 'verify_model',
  downloadRuntime: 'download_runtime',
  deleteRuntime: 'delete_runtime',
})

/** Every method the seam contract fixes, so the adapter can be checked against it. */
export const SEAM_METHODS = Object.freeze([
  'subscribe',
  'listProjects',
  'createProject',
  'createChapter',
  'openChapter',
  'loadPages',
  'historyLoad',
  'historyPush',
  'historyMove',
  'renameProject',
  'deleteProject',
  'deleteChapter',
  'resumeJob',
  'runClean',
  'cancelRun',
  'applyTool',
  'createRegion',
  'deleteMask',
  'restoreRegion',
  'rerunMask',
  'cleanAnyway',
  'exportChapter',
  'sidecarAvailable',
  'listSidecarModels',
  'readSettings',
  'writeSettings',
  'about',
  'listLoadedModels',
  'unloadModel',
  'listAccelerators',
  'listModels',
  'downloadModel',
  'cancelDownload',
  'deleteModel',
  'discardPartial',
  'verifyModel',
  'downloadRuntime',
  'deleteRuntime',
])

/** Whether this page is running inside a Tauri window. */
export function isTauri() {
  return Boolean(globalThis.__TAURI_INTERNALS__ ?? globalThis.__TAURI__)
}

/**
 * `invoke`, from wherever Tauri put it.
 *
 * @returns {(command: string, args?: Object) => Promise<any>}
 */
function globalInvoke() {
  const fn = globalThis.__TAURI__?.core?.invoke ?? globalThis.__TAURI_INTERNALS__?.invoke
  if (typeof fn !== 'function') {
    throw new Error('the Tauri adapter was constructed outside a Tauri window')
  }
  return fn
}

/**
 * @param {Object} options
 * @param {import('./backend.js').Backend} options.fallback - serves every method not yet implemented
 * @param {(command: string, args?: Object) => Promise<any>} [options.invoke] - injected for tests
 * @returns {import('./backend.js').Backend}
 */
export function createTauriBackend({ fallback, invoke }) {
  // `async` so that being constructed outside Tauri surfaces as a rejected
  // promise like any other backend failure, rather than as a synchronous throw
  // from a method the seam declares async.
  const call = invoke ?? (async (command, args) => globalInvoke()(command, args))

  /**
   * Settings are stored by the core and *defaulted* by the interface.
   *
   * The core deliberately does not know what a setting means - it round-trips
   * opaque JSON - so on a first launch it
   * has nothing to return. The defaults are the fallback's, and the persisted
   * snapshot is merged over them. When the settings vocabulary gets a home
   * outside `fixtures.js`, this is the one line that moves.
   *
   * @param {Object} stored
   */
  const withDefaults = async (stored) => ({ ...(await fallback.readSettings()), ...stored })

  const implementations = {
    about: () => call(IMPLEMENTED.about),
    readSettings: async () => withDefaults(await call(IMPLEMENTED.readSettings)),
    writeSettings: async (patch) => withDefaults(await call(IMPLEMENTED.writeSettings, { patch })),

    // Straight passthrough. The commands answer in the seam's own shapes - the
    // conversions that used to justify a mapping layer happen in Rust, where
    // the data is: epoch seconds become ISO 8601 and pixel boxes become
    // percentages of the page, both in `src-tauri/src/library.rs`.
    listProjects: () => call(IMPLEMENTED.listProjects),
    createProject: (spec) => call(IMPLEMENTED.createProject, spec),
    createChapter: (spec) => call(IMPLEMENTED.createChapter, spec),
    openChapter: (spec) => call(IMPLEMENTED.openChapter, spec),
    // The resident window. Whole pages come back, so the page's
    // status travels with its regions.
    loadPages: ({ chapterId, indices }) =>
      call(IMPLEMENTED.loadPages, { chapterId, indices: indices ?? [] }),
    // The persisted undo journal. `historyLoad` answers the index
    // (cursor plus one `{seq, label}` per entry); `historyMove` moves the
    // cursor on disk and answers with the one delta to replay. The payloads
    // never all exist at once on this side, which is the whole point.
    historyLoad: ({ chapterId }) => call(IMPLEMENTED.historyLoad, { chapterId }),
    historyPush: ({ chapterId, entry }) => call(IMPLEMENTED.historyPush, { chapterId, entry }),
    historyMove: ({ chapterId, direction }) =>
      call(IMPLEMENTED.historyMove, { chapterId, direction }),
    renameProject: (spec) => call(IMPLEMENTED.renameProject, spec),
    deleteProject: (spec) => call(IMPLEMENTED.deleteProject, spec),
    deleteChapter: (spec) => call(IMPLEMENTED.deleteChapter, spec),
    exportChapter: (spec) => call(IMPLEMENTED.exportChapter, spec),

    // Named arguments rather than the spec object, because these two are the
    // only calls whose spec carries a field the command has no parameter for:
    // `restoreRegion`'s `pageStatus` is the *caller's* snapshot, and the
    // manifest's own record is the better copy of it.
    deleteMask: ({ maskId }) => call(IMPLEMENTED.deleteMask, { maskId }),
    restoreRegion: ({ regionId, region }) =>
      call(IMPLEMENTED.restoreRegion, { regionId, region: region ?? null }),

    // The four region edits that run an engine. Named arguments rather than
    // the spec object wherever the command's parameters are not the spec's:
    // `rerunMask` addresses a mask and the command takes the three fields it
    // acts on, and `cleanAnyway` carries the engine pick the tool window holds
    // for text outside a balloon, and this is where that row
    // finally bites.
    applyTool: ({ tool, params, chapterId, pageIndex, regionId }) =>
      call(IMPLEMENTED.applyTool, {
        tool,
        params: params ?? null,
        chapterId: chapterId ?? null,
        pageIndex: pageIndex ?? null,
        regionId: regionId ?? null,
      }),
    createRegion: ({ chapterId, pageIndex, bbox, tool, params }) =>
      call(IMPLEMENTED.createRegion, {
        chapterId,
        pageIndex,
        bbox,
        tool,
        params: params ?? null,
      }),
    rerunMask: ({ maskId, kind, engine }) =>
      call(IMPLEMENTED.rerunMask, { maskId, kind, engine: engine ?? null }),
    cleanAnyway: ({ regionId, engine }) =>
      call(IMPLEMENTED.cleanAnyway, { regionId, engine: engine ?? null }),
    sidecarAvailable: () => call(IMPLEMENTED.sidecarAvailable),
    listSidecarModels: () => call(IMPLEMENTED.listSidecarModels),

    // The loaded-models tab. `listLoadedModels` is a **poll**, so it takes no
    // argument and is deliberately the cheapest command in this table: it walks
    // a handful of rows under one mutex in `src-tauri/src/models.rs` and
    // touches no session. `unloadModel` records a request the run acts on at
    // its next region boundary, which is why it answers a boolean about the
    // row rather than about the memory.
    listLoadedModels: () => call(IMPLEMENTED.listLoadedModels),
    unloadModel: ({ id }) => call(IMPLEMENTED.unloadModel, { id }),

    // The accelerator setting. Not a poll: the answer changes when the runtime
    // is downloaded or the setting is written, and both are events the panel
    // already knows about. It carries the *reasons* as keys - which providers
    // this machine has, which one each model landed on, and why a forced one
    // was refused - because the earlier complaint was that the core
    // decided all of that and told nobody.
    listAccelerators: () => call(IMPLEMENTED.listAccelerators),

    // The model catalogue. `listModels` is the cheap one - a
    // `metadata` call per search path per weight and no digests, because a
    // dialog that hashed 200 MB to decide whether to draw a row would take
    // most of a minute to open.
    //
    // The four that *change* something answer as soon as the work is under
    // way, never when it is finished: a 207 MB download reports on the event
    // channel (`model-progress`) and is cancellable while it does, and a
    // command that resolved at the end could offer neither. `verifyModel` is
    // the one exception and is deliberately slow - it is the explicit
    // re-digest of a file somebody suspects.
    //
    // What they answer *with* is an id rather than a boolean: `downloadModel`
    // says `started` or which of the two races refused it, and `deleteModel`
    // says whether the file went, was never there, or belongs to somebody else.
    // The commands serialise those as camelCase
    // strings, so there is nothing to translate here.
    //
    // `retryStore` is the one argument any of them takes and it is not a
    // preference: it says "Settings › Models has just been opened", which is
    // the one moment worth re-offering the token to a credential store that
    // refused this process's first attempt. Absent on every
    // other call, including the refreshes this dialog makes when a download
    // ends, or the prompt-per-poll that row removed would be back.
    listModels: ({ retryStore } = {}) => call(IMPLEMENTED.listModels, { retryStore }),
    downloadModel: ({ id }) => call(IMPLEMENTED.downloadModel, { id }),
    cancelDownload: ({ id }) => call(IMPLEMENTED.cancelDownload, { id }),
    deleteModel: ({ id }) => call(IMPLEMENTED.deleteModel, { id }),
    // The bytes a cancelled transfer left behind, given back.
    discardPartial: ({ id }) => call(IMPLEMENTED.discardPartial, { id }),
    verifyModel: ({ id }) => call(IMPLEMENTED.verifyModel, { id }),
    downloadRuntime: () => call(IMPLEMENTED.downloadRuntime),
    deleteRuntime: () => call(IMPLEMENTED.deleteRuntime),

    runClean: (spec) => call(IMPLEMENTED.runClean, spec),
    cancelRun: (spec = {}) => call(IMPLEMENTED.cancelRun, spec),
    resumeJob: (spec) => call(IMPLEMENTED.resumeJob, spec),
  }

  const backend = {}
  for (const method of SEAM_METHODS) {
    backend[method] = Object.hasOwn(implementations, method)
      ? implementations[method]
      : (...args) => /** @type {any} */ (fallback)[method](...args)
  }

  /**
   * `subscribe` is both streams at once.
   *
   * The events a handler receives have to come from whichever implementation is
   * actually running the job. `runClean` is a command now and the mock still
   * serves the six region-level edits, so both produce events and a handler
   * that saw only one of them would leave the Pages list frozen with a run
   * apparently in progress. The seam's ordering rules still hold per run,
   * because a run belongs entirely to one implementation.
   */
  backend.subscribe = createEventStream({ call, fallback })

  return /** @type {import('./backend.js').Backend} */ (backend)
}

/** Which seam methods this adapter answers itself. For tests and for `about`. */
export function implementedMethods() {
  return Object.keys(IMPLEMENTED).sort()
}
