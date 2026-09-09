/**
 * The backend seam.
 *
 * Everything the interface needs from a backend goes through this module, and
 * only this module. No component imports `mock.js`; components import
 * `getBackend()` from here and get whichever implementation is selected, so
 * swapping the mock for Tauri `invoke` calls and a Tauri channel touches one
 * file - this one.
 *
 * Two rules the interface exists to enforce:
 *
 * 1. **Every method is async.** A real `invoke` returns a promise; nothing may
 *    come back synchronously, or the interface would come to depend on timing
 *    the real backend cannot provide.
 * 2. **Progress arrives only on the event channel.** `subscribe(handler)` is
 *    shaped like a Tauri channel: one handler, many events, never a callback
 *    per call. A run reports itself with `page-started`, `region-done`,
 *    `page-done` and `run-finished`; everything transient is a `notice`.
 *
 * Every user-visible string this layer produces is an i18n key plus
 * parameters. No English text crosses the seam.
 *
 * Nothing in `src/lib/api` imports from `svelte` or touches the DOM.
 */

import { createMockBackend } from './mock.js'
import { createTauriBackend, isTauri } from './tauri.js'

/* ------------------------------------------------------------------ */
/* Data shapes                                                         */
/* ------------------------------------------------------------------ */

/**
 * A translatable time like "2 h ago", carried as data rather than as text so
 * the interface renders it in the user's language. What must never cross the
 * seam is the formatted *string* - a real backend has a stored timestamp and
 * a clock and nothing else, so it is the one that picks the bucket. The
 * mock's values are fixture data, chosen by hand; that is a property of the
 * mock, not of this contract.
 *
 * @typedef {Object} RelativeTime
 * @property {string} key - i18n key, e.g. `time.relative.hoursAgo`
 * @property {Object} params - e.g. `{ count: 2 }`
 */

/**
 * `Project` (src/lib/model/types.js) plus what a backend owns. Home is a
 * two-level hierarchy - Projects → Chapters - so a project always carries its
 * chapters, and a chapter always carries its pages.
 *
 * @typedef {import('../model/types.js').Project & {
 *   sourcePath: string,
 *   lastOpened: RelativeTime,
 *   starred: boolean,
 *   interruptedJob: { chapterId: string, pageIndex: number }|null,
 *   conversion: { from: string, to: string }|null
 * }} ApiProject
 */

/**
 * A chapter's `sourcePath` is the folder its own pages were read from, and it
 * is a different fact from the project's: a project points at a folder of
 * scans, and each chapter reads one folder inside it - or somewhere else
 * entirely. It is `''` when the backend cannot say, which is what a chapter
 * created before `createChapter` carried a path has. An empty string is not a
 * licence to substitute the project's path; the two are different questions and
 * a wrong answer to this one names files the chapter does not hold.
 *
 * `review` is the chapter's **review index**: one
 * short record per region needing review, chapter-wide, in page order. It is
 * carried rather than derived because the review set is the one thing in the
 * interface that is honestly chapter-wide - stepping to the next issue may land
 * eleven pages away - and deriving it from the regions is what used to keep
 * every region of every chapter resident.
 *
 * @typedef {Object} ReviewRef
 * @property {string} id - the region's id
 * @property {string} pageId
 * @property {number} pageIndex
 * @property {string} reasonKey - the `review.reason.*` key
 */

/**
 * @typedef {import('../model/types.js').Chapter & {
 *   number: number,
 *   lastOpened: RelativeTime,
 *   sourcePath: string,
 *   sourceFormat: string,
 *   noTextDetected: boolean,
 *   inputReports: NoticeSpec[],
 *   review: ReviewRef[]
 * }} ApiChapter
 */

/**
 * `panels` and `layout` are stand-in image data for the mock canvas, not
 * information a real backend would send.
 *
 * **`regions` is the windowed part**. A page
 * outside the resident window arrives as a header: `regions: []`,
 * `resident: false`, and `regionCount` saying how many it would have had.
 * `resident` exists because `regions: []` otherwise means two different things
 * - "no regions" and "not loaded yet" - and a Layers panel that cannot tell
 * them apart draws an empty list for a page full of masks.
 *
 * `doneCount` and `reviewCount` ride alongside it for the same reason: the
 * Pages list draws a `done / total` ratio and a `✓ n` mark for **every** page
 * of the chapter, and a count taken off `regions` reads zero for every page but
 * the three in the window. The backend reads all three out of the manifest, so
 * they are right the moment a project is reopened.
 *
 * @typedef {import('../model/types.js').Page & {
 *   number: number,
 *   file: string,
 *   sourceSha: string,
 *   width: number,
 *   height: number,
 *   regionCount: number,
 *   doneCount: number,
 *   reviewCount: number,
 *   resident: boolean,
 *   layout: number,
 *   panels: Array<{x: number, y: number, w: number, h: number}>
 * }} ApiPage
 */

/**
 * `kind` and `text` are stand-in image data (what the mock canvas paints in
 * place of a scan). `tool` records which tool last committed the region, for
 * the "reopen in tool" action.
 *
 * @typedef {import('../model/types.js').Region & {
 *   sourceSha: string,
 *   detected: boolean,
 *   kind: 'bubble'|'sfx'|'outside',
 *   text: string,
 *   tool?: string
 * }} ApiRegion
 */

/**
 * The undo journal's index - everything the interface holds about a history.
 * The deltas themselves stay on disk.
 *
 * @typedef {Object} HistoryView
 * @property {number} cursor - how many entries are in the past
 * @property {Array<{seq: number, label: string}>} entries
 */

/**
 * @typedef {Object} NoticeSpec
 * @property {string} key - i18n key
 * @property {Object} params - named placeholders; a value may itself be an i18n key (`reasonKey`, `causeKey`, `rungKey`, `fillModeKey`, `modeKey`)
 * @property {'info'|'warn'} tone
 */

/* ------------------------------------------------------------------ */
/* Events                                                              */
/* ------------------------------------------------------------------ */

/**
 * A page has been taken off the queue and is being cleaned. The Pages list
 * shows it as `●`.
 *
 * Not one of the four event types the brief names; added because the Pages
 * list is the progress indicator and there is otherwise no
 * way to say "this page is cleaning now". `runClean` also returns the queue
 * in order, so a consumer that ignores this event can still derive it.
 *
 * @typedef {Object} PageStartedEvent
 * @property {'page-started'} type
 * @property {string} runId
 * @property {string} chapterId
 * @property {string} pageId
 * @property {number} pageIndex
 */

/**
 * One region finished. Emitted for every region of a page, in page order,
 * before that page's `page-done`.
 *
 * @typedef {Object} RegionDoneEvent
 * @property {'region-done'} type
 * @property {string} runId
 * @property {string} chapterId
 * @property {string} pageId
 * @property {number} pageIndex
 * @property {ApiRegion} region - the region in its finished state, mask and all
 */

/**
 * A page finished. The page's mark ticks over in the Pages list.
 *
 * @typedef {Object} PageDoneEvent
 * @property {'page-done'} type
 * @property {string} runId
 * @property {string} chapterId
 * @property {string} pageId
 * @property {number} pageIndex
 * @property {ApiPage} page - the page in its finished state
 */

/**
 * The run stopped, either because the queue emptied or because it was
 * cancelled. A cancelled run keeps every completed region and leaves the job
 * incomplete; `nextPageIndex` is where a resume
 * would pick up.
 *
 * @typedef {Object} RunFinishedEvent
 * @property {'run-finished'} type
 * @property {string} runId
 * @property {string} chapterId
 * @property {'completed'|'cancelled'} reason
 * @property {number} pagesQueued
 * @property {number} pagesCleaned
 * @property {number} regionsCleaned
 * @property {number|null} nextPageIndex - set only when cancelled
 */

/**
 * A transient notice for the bottom-left stack. `key` and
 * `params` are i18n data; this layer never emits English text.
 *
 * @typedef {Object} NoticeEvent
 * @property {'notice'} type
 * @property {string} id - stable key for the notice stack
 * @property {string} key
 * @property {Object} params
 * @property {'info'|'warn'} tone
 */

/**
 * A model or the ONNX Runtime being fetched.
 *
 * The sixth event type, and it is one for the same reason `page-started` is:
 * progress arrives on the event channel and nowhere else, because a Tauri
 * `invoke` cannot return a streaming result and a 207 MB download has to be
 * watchable and cancellable while it runs.
 *
 * `id` is a catalogue row's id or `'runtime'`. `total` is the response's
 * `Content-Length` and may be absent. **Exactly one event per download carries
 * `done`**, whatever happened: `error` is null for a file that arrived and
 * verified, and a string for a failure, a digest mismatch or a cancellation -
 * the row goes back to "not installed" through the same path for all three.
 *
 * @typedef {Object} ModelProgressEvent
 * @property {'model-progress'} type
 * @property {string} id
 * @property {number} downloaded
 * @property {number|null} total
 * @property {boolean} done
 * @property {string|null} error
 */

/**
 * @typedef {PageStartedEvent|RegionDoneEvent|PageDoneEvent|RunFinishedEvent|NoticeEvent|ModelProgressEvent} BackendEvent
 */

/* ------------------------------------------------------------------ */
/* The interface                                                       */
/* ------------------------------------------------------------------ */

/**
 * @typedef {Object} RunHandle
 * @property {string|null} runId - null when nothing was queued
 * @property {Array<{chapterId: string, pageId: string, pageIndex: number}>} pages - the queue, in order
 * @property {boolean} [alreadyRunning]
 */

/**
 * `pageStatus` is the status of the page the region sits on, as the backend
 * holds it *after* the edit. Applying a tool to a region on an unclean page
 * makes it a cleaned page, so a caller that replaced only the region would draw
 * a full track under a "not cleaned" mark - the same reason `deleteMask`,
 * `rerunMask` and `cleanAnyway` all report it. Present on `'applied'` only.
 *
 * @typedef {Object} ApplyResult
 * @property {'applied'|'needs-confirmation'|'blocked'|'run-started'|'not-found'} status
 * @property {ApiRegion} [region]
 * @property {import('../model/types.js').Mask} [mask]
 * @property {string} [pageStatus]
 * @property {{ kind: 'cloud-transmission'|'cloud-cost', regionId: string, estimatedCost: number }} [confirmation]
 * @property {string|null} [runId]
 * @property {Array<{chapterId: string, pageId: string, pageIndex: number}>} [pages]
 */

/**
 * `exportChapter` refuses more than one thing, and every refusal is the same
 * shape: `{status: 'refused', reasonKey}`, with the sentence in the catalogue
 * and the backend announcing it on the notice stack. Callers branch on
 * `reasonKey`, never on `status` alone - only
 * `notice.export.refusedOverwrite` has a way out the interface can offer
 * (choose another folder), and the rest are answered by changing what was
 * asked for.
 *
 * The refusals are: a lossy format (`refusedLossyFormat`), PSB
 * (`refusedLayeredFormat`), a format string the backend does not know
 * (`refusedUnknownFormat`), `masks: 'separate-layer'` into a CBZ
 * (`refusedMaskLayers`), a destination that is neither sentinel nor an
 * absolute path (`refusedDestination`), a stitched export of a paginated
 * project (`refusedStitchPaginated`), into a CBZ (`refusedStitchedArchive`) or
 * as PSD (`refusedStitchedLayered`), a PSD of a page PSD cannot carry
 * (`refusedLayeredMode`, `refusedLayeredSize`), and the five
 * `notice.export.stitchRefused.*` a strip whose pages disagree produces.
 * The seam contract is the list this restates.
 *
 * `masks` means two things by format. For PNG and TIFF, `'separate-layer'`
 * writes a mask file beside each page - `001.png` and `001_mask.png`. For PSD
 * it is the layered document: the untouched page as the Background, one
 * masked layer per region in a `Cleaned` group; `'flattened'` is one
 * Background layer of the cleaned page.
 *
 * The whole surface. Implementations: `mock.js` today, a Tauri adapter later.
 *
 * The region-level edits - `deleteMask`, `rerunMask`, `cleanAnyway` - return
 * the page's status alongside the region, because some of them move it:
 * cleaning a gate-skipped region on an unclean page makes it a cleaned page,
 * and deleting the last mask on a page takes that back. A caller that replaced
 * only the region would draw a full track under a "not cleaned" mark. Their
 * undo half, `restoreRegion`, takes the status back the same way.
 *
 * @typedef {Object} Backend
 * @property {(handler: (event: BackendEvent) => void) => (() => void)} subscribe - returns an unsubscribe function
 * @property {() => Promise<ApiProject[]>} listProjects
 * @property {(spec: {name: string, mode: 'single'|'longstrip', sourcePath?: string, readingDirection?: 'rtl'|'ltr'}) => Promise<ApiProject>} createProject
 * @property {(spec: {projectId: string, name: string, number?: number, sourcePath?: string}) => Promise<ApiChapter|null>} createChapter - `number` is the chapter's number and is stored verbatim; omitted, the backend numbers it `max + 1`. `sourcePath` omitted means "wherever the project's own folder implies", and a backend must refuse rather than give one folder to two chapters
 * @property {(spec: {projectId: string, chapterId: string, convert?: boolean}) => Promise<{project: ApiProject, chapter: ApiChapter, pendingConversion: {from: string, to: string, fileCount: number}|null}|null>} openChapter - the chapter arrives as page **headers** plus its review index; `loadPages` brings a window's regions
 * @property {(spec: {chapterId: string, indices: number[]}) => Promise<ApiPage[]>} loadPages - the regions of a window of pages. Indices that are not there are skipped, never refused
 * @property {(spec: {chapterId: string}) => Promise<HistoryView>} historyLoad - the chapter's undo journal as an index: a cursor and one `{seq, label}` per entry, no payloads
 * @property {(spec: {chapterId: string, entry: Object}) => Promise<HistoryView>} historyPush - record a delta that has already been applied; truncates the redo stack, caps the journal, and answers with the index as it now stands
 * @property {(spec: {chapterId: string, direction: 'undo'|'redo'}) => Promise<{cursor: number, entry: Object|null}>} historyMove - move the journal's cursor and answer with the one delta to replay
 * @property {(spec: {projectId: string, name: string}) => Promise<ApiProject|null>} renameProject
 * @property {(spec: {projectId: string}) => Promise<boolean>} deleteProject
 * @property {(spec: {projectId: string, chapterId: string, sourceFiles?: boolean}) => Promise<boolean>} deleteChapter - always removes the chapter's row and the library's own files for it (its manifest, masks and patches). `sourceFiles: true` also deletes the folder of scans it was read from - except the project's own folder, which a backend must refuse to remove so that deleting one chapter cannot empty the project. `false` means the chapter was not there
 * @property {(spec: {projectId: string, chapterId?: string}) => Promise<{project: ApiProject, chapter: ApiChapter, resumedFrom: number, runId: string|null, pages: Array<Object>}|null>} resumeJob
 * @property {(spec: {scope: 'page'|'chapter'|'project', chapterId: string, pageIndex?: number, engineCeiling?: string, bubbleEngine?: string, outsideEngine?: string, outsideBubbles?: 'review'|'clean'}) => Promise<RunHandle>} runClean - the two engine picks name a rung outright (`fill`, `denoise`, `lama`) and are applied per region by whether it sits inside a speech balloon; each is a *starting* rung the ladder may still escalate past, never a ceiling. `outsideBubbles` is the opt-in for text outside a balloon: `'clean'` sends every such region to the ladder on `outsideEngine`'s rung with no script read; anything else holds it for review as before
 * @property {(spec?: {runId?: string}) => Promise<string|null>} cancelRun
 * @property {(spec: {tool: string, params?: Object, chapterId?: string, pageIndex?: number, regionId?: string}) => Promise<ApplyResult>} applyTool
 * @property {(spec: {chapterId: string, pageIndex: number, bbox: {x: number, y: number, w: number, h: number}, tool: string, params?: Object}) => Promise<{region: ApiRegion, pageStatus: string}|null>} createRegion - a region drawn by hand, with a hand mask already committed
 * @property {(spec: {maskId: string}) => Promise<{region: null, pageStatus: string}|null>} deleteMask - deleting a mask deletes the row: the text under it comes back and the region goes off the page, so there is no region to answer with. `null` (rather than `{region: null}`) means the mask was not found
 * @property {(spec: {regionId: string, region: ApiRegion|null, pageStatus?: string}) => Promise<ApiRegion|null>} restoreRegion - put a region back as it was; the undo half of every region-level edit. `region: null` means it was not there
 * @property {(spec: {maskId: string, kind: 'stronger'|'simpler'|'cycleFill'|'reopenInTool'|'retry'|'engine', engine?: string}) => Promise<{region: ApiRegion, mask: import('../model/types.js').Mask, reopenTool: string|null, pageStatus: string}|null>} rerunMask - `engine` names the rung `kind: 'engine'` runs at; `'retry'` re-runs the rung the mask already used
 * @property {(spec: {regionId: string, engine?: string}) => Promise<{region: ApiRegion, mask: import('../model/types.js').Mask, pageStatus: string}|null>} cleanAnyway - `engine` is the starting rung: the user's `fill`/`redraw` pick for this kind of text, or a rung named outright. The automatic pass never cleans an out-of-balloon region; this is where the pick for one bites
 * @property {(opts: {chapterId: string, format?: string, destination?: 'new-folder'|'source-folder'|string, masks?: 'flattened'|'separate-layer', layout?: 'per-page'|'stitched'}) => Promise<{status: 'exported'|'refused', fileCount?: number, path?: string, reasonKey?: string, gutterPixels?: number}|null>} exportChapter - `format` is `'PNG' | 'TIFF' | 'PSD' | 'CBZ'`; `destination` also takes an absolute path; `layout: 'stitched'` is longstrip only and never PSD; `masks: 'separate-layer'` is a mask file beside each raster page or a layer per region in a PSD, and is refused for CBZ; `gutterPixels` comes back on a stitched export alone
 * @property {() => Promise<Object>} readSettings
 * @property {(patch: Object) => Promise<Object>} writeSettings
 * @property {() => Promise<{available: boolean, reasonKey: string|null}>} sidecarAvailable - whether rung 3a (the FLUX sidecar) can be offered on this machine. `reasonKey` is null when there is nothing to say, which is the ordinary case of nothing installed
 * @property {() => Promise<Array<{id: string, label: string}>>} listSidecarModels - list available model directories discovered under the sidecar weights root
 * @property {() => Promise<{appVersion: string, facts: Array<{labelKey: string, value: string}>}>} about
 * @property {() => Promise<LoadedModel[]>} listLoadedModels - what is in memory **right now**. A poll, not a subscription: the answer changes with the work rather than with an event, and an empty array is the ordinary state of an application that is not cleaning anything
 * @property {(spec: {id: number}) => Promise<boolean>} unloadModel - ask for one back. `true` means the request is recorded, **not** that the memory is free: a backend drops the session at its next safe point, and anything the work still needs loads again
 * @property {() => Promise<Accelerators>} listAccelerators - what this machine can run models on, which provider each model will land on under the current setting, and why. Not a poll: the answer moves when the runtime is downloaded or the `accelerator` setting is written
 * @property {(spec?: {retryStore?: boolean}) => Promise<ModelsView>} listModels - the model catalogue and what of it is on this machine. Cheap by construction - presence and size, never a digest. `retryStore` is "Settings › Models has just been opened" and nothing else: it lets the once-per-process token migration be attempted again, for a keychain unlocked since launch. A refresh or a poll must not pass it
 * @property {(spec: {id: string}) => Promise<DownloadStart>} downloadModel - start one. `'alreadyRunning'` and `'alreadyInstalled'` say why one did not start, which is what a second window's stale row needs to hear. Progress arrives as `model-progress`, never as this promise
 * @property {(spec: {id: string}) => Promise<boolean>} cancelDownload - ask one to stop; it ends with a `done` event carrying an error, the same way a failure does
 * @property {(spec: {id: string}) => Promise<DeleteOutcome>} deleteModel - remove it from the app-data models directory **only**. `'readOnlyElsewhere'` is a copy this app does not own and `'notFound'` a row that was already stale; a successful delete also drops any session that weight was loaded into
 * @property {(spec: {id: string}) => Promise<boolean>} verifyModel - digest an installed weight against its pin. Deliberately explicit: it is seconds of work, which is why `listModels` does not do it
 * @property {(spec: {id: string}) => Promise<boolean>} discardPartial - throw away the unfinished download the row reports as `partialBytes`, and say whether there was one. `false` is also what a transfer in flight answers: its `.part` is a file being written and is not deleted out from under it
 * @property {() => Promise<DownloadStart>} downloadRuntime - fetch and unpack the ONNX Runtime build this platform is set to; reports under the id `runtime`
 * @property {() => Promise<DeleteOutcome>} deleteRuntime - remove it from the app-data runtimes directory only, with the same three answers `deleteModel` gives
 */

/**
 * Why a Download press did not start a download, or that it did.
 *
 * Both refusals are another window's doing: a second Settings dialog draws its
 * rows from a `listModels` taken before this one pressed anything, so its
 * Download button can be offered for something already running or already
 * here. Neither is an error and neither is a rejection - the remedy is a
 * refreshed list, which the interface asks for on the answer.
 *
 * @typedef {'started'|'alreadyRunning'|'alreadyInstalled'} DownloadStart
 */

/**
 * What a Delete press found.
 *
 * `'notFound'` is a stale row - nothing of that name on any search path -
 * and `'readOnlyElsewhere'` is a copy outside the directory the app owns, or
 * one the filesystem will not let go of. Both are the row's `readOnly` and
 * `installed` seen a moment later than they were drawn, so both are sentences
 * rather than failures.
 *
 * `'busy'` is a delete refused because a download is writing into the same
 * directory - the runtime's row is the only one that can reach it, because
 * `deleteRuntime` removes the tree a transfer's `.part` lives in.
 *
 * @typedef {'deleted'|'notFound'|'readOnlyElsewhere'|'busy'} DeleteOutcome
 */

/**
 * The accelerator setting's whole subject.
 *
 * `preference` is what is stored - `auto`, `cpu`, or a provider `id` - and is
 * never absent: an unwritten or unreadable value reads as `auto`, which is what
 * the backend will actually do.
 *
 * Every string is a key or an id, never English: `labelKey` names the provider,
 * `reasonKey` says why one is unusable, `noteKey` says a choice was made without
 * a measurement on this hardware, and `declinedKey` says a chosen provider was
 * not used. `neededBytes` and `roomBytes` travel *beside* the key rather than
 * inside the sentence, because a byte count is not translatable.
 *
 * @typedef {Object} Accelerators
 * @property {string} preference - `auto`, `cpu`, or a provider id
 * @property {Array<{id: string, labelKey: string, available: boolean, reasonKey: string|null, measured: boolean, active: boolean, selected: boolean}>} providers
 * @property {Array<{modelKey: string, acceleratorId: string, labelKey: string, noteKey: string|null, declinedKey: string|null, declinedId: string|null, neededBytes: number|null, roomBytes: number|null}>} models
 */

/**
 * The model catalogue and what of it this machine has.
 *
 * `requiredBy` names **engines and features** in the interface's own
 * vocabulary - `autoClean`, `lama` - so that `state/capabilities` can
 * decide which controls to draw from the rows themselves rather than from a
 * second table beside them. An engine named by no row needs no weights and is
 * therefore always available, which is `fill` and `denoise`.
 *
 * `installed` is presence **and size**, never a digest: hashing 200 MB every
 * time Settings opens is not affordable. `sha256Ok` is null until something
 * calls `verifyModel`, which is the explicit re-check.
 *
 * `readOnly` is a copy found outside the directory the app writes to - a
 * developer's checkout, `$MANGA_CLEANER_MODELS`, or the installer's own - and
 * the Delete button is not offered for one.
 *
 * `partialBytes` is the unfinished download beside a row - the `.part` a
 * cancelled or failed transfer left, which the next Download resumes from and
 * which `discardPartial` throws away. `null` when there is none, which is the
 * ordinary state; on the runtime row it is everything its download directory is
 * holding rather than one file - both artefacts' partial transfers and any
 * complete archive a failed unpack left, which is kept for a re-unpack and is
 * the same kind of remainder.
 *
 * `hasToken` says whether a Hugging Face token is stored. **The token itself
 * never crosses the seam in this direction.** `tokenStore` says *where* one is
 * kept - `'keychain'` for the operating system's credential store,
 * `'fileNoStore'` for a build that has none, `'fileStoreUnavailable'` for a
 * store that is there and would not answer. Both file cases fall back to
 * `settings.json` and they are separate because they are opposite news: one is
 * permanent, the other is usually a keychain the user can unlock. Ids, not
 * sentences, and the difference between three notes in Settings.
 *
 * `tokenStoreReason` says *why* an unreachable store was unreachable -
 * `'locked'`, `'unreachable'`, `'ambiguous'`, `'unknown'` - and is `null` for
 * the other two locations, which have nothing to explain. An id again: the
 * platform's own words for a locked keychain are prose, and a note that could
 * only say "something went wrong" sent the user looking.
 *
 * The runtime row is the ONNX Runtime rather than a weight, and it carries what
 * a *choice* needs: `flavour` is the build that would be downloaded - the
 * stored `runtimeFlavour` where this platform publishes it, the platform's
 * default otherwise, and **not** necessarily the flavour of whatever is already
 * on disk, which nothing short of loading the library can tell. `bytes` is that
 * build's whole download, both artefacts where there are two. `flavours` is
 * every build published for this platform, default first; one entry means there
 * is nothing to choose and the picker is not drawn, which is macOS and Linux
 * aarch64.
 *
 * `installedFlavour` and `installedVersion` are the other question, and they
 * are the build **that is actually here**, read from the record an install
 * writes beside the libraries. `null` is *unknown* - a runtime unpacked before
 * that record existed, or one found outside the folder the app owns - rather
 * than *none*, so the interface says nothing rather than claiming a build.
 *
 * @typedef {Object} ModelsView
 * @property {Array<{id: string, fileName: string, bytes: number, kindKey: string, requiredBy: string[], installed: boolean, path: string|null, readOnly: boolean, sha256Ok: boolean|null, downloading: boolean, partialBytes: number|null}>} models
 * @property {{installed: boolean, path: string|null, readOnly: boolean, downloading: boolean, version: string|null, flavour: string|null, bytes: number|null, flavours: Array<{id: string, ortVersion: string, bytes: number, isDefault: boolean, userInstalled: string[]}>, available: boolean, installedFlavour: string|null, installedVersion: string|null, partialBytes: number|null}} runtime
 * @property {string|null} modelsDir
 * @property {string|null} runtimeDir
 * @property {boolean} hasToken
 * @property {'keychain'|'fileNoStore'|'fileStoreUnavailable'} tokenStore
 * @property {'locked'|'unreachable'|'ambiguous'|'unknown'|null} tokenStoreReason
 */

/**
 * One thing the backend has loaded, for the tab in the corner of the editor.
 *
 * `bytes` is an estimate and `basis` says which kind - `measured` from a
 * running process, `weights` from the size of the model file, `reported` by the
 * loaded thing itself. The interface uses `basis` to format the size: `measured`
 * renders as an exact size (`models.value.size`), while `weights` and
 * `reported` render as approximate (`models.value.sizeApprox`).
 *
 * `id` is **not stable across loads**. A model unloaded and opened again is a
 * new row, because it is a new session - which is exactly what lets the
 * interface tell "still loaded" from "gone and back".
 *
 * @typedef {Object} LoadedModel
 * @property {number} id
 * @property {string} kindKey - e.g. `models.kind.inpainter`
 * @property {number} bytes
 * @property {'measured'|'weights'|'reported'} basis
 * @property {string} deviceKey - e.g. `accel.coreml`
 * @property {boolean} gpu
 * @property {number} idleMs - how long since anything used it
 * @property {boolean} unloading - an unload was asked for and has not reached a safe point yet
 */

/* ------------------------------------------------------------------ */
/* The selector                                                        */
/* ------------------------------------------------------------------ */

/** @type {Backend|null} */
let instance = null

/**
 * Force the mock even inside a Tauri window. Set on `globalThis` before the
 * first `getBackend()`; the flag exists so the built application can be run
 * against fixture data - for a demo, for a screenshot, or to tell an interface
 * bug apart from a backend one.
 */
const FORCE_MOCK = '__MANGA_CLEANER_FORCE_MOCK__'

/**
 * The single backend instance.
 *
 * Inside a Tauri window this is the adapter, **with the mock behind it**: the
 * adapter answers what the core can answer and forwards the rest, so the
 * interface is whole at every stage of the backend being built rather than only
 * at the end. Outside one - a browser, a test - it is the mock alone, because
 * there is nothing to invoke.
 *
 * @returns {Backend}
 */
export function getBackend() {
  if (!instance) {
    const mock = createMockBackend()
    instance = isTauri() && !globalThis[FORCE_MOCK] ? createTauriBackend({ fallback: mock }) : mock
  }
  return instance
}

/**
 * Replaces the selected implementation. For tests and for a later Tauri
 * adapter; the interface never calls this.
 *
 * @param {Backend|null} backend - pass null to fall back to the default
 * @returns {Backend|null}
 */
export function setBackend(backend) {
  instance = backend
  return instance
}
