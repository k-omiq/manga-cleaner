/**
 * The first-launch offer's *state*, and the download run it drives.
 *
 * Why a module and not the component: the dialog can leave the screen while
 * the run is going on. It is mounted beside the modal stack rather than on it,
 * so a dialog raised over it takes the screen - and when the
 * offer held its own progress, waiters and selection, that unmount abandoned a
 * transfer's report mid-run and came back with the ticks reset and the bytes
 * quoted again from the boot snapshot. Everything that must outlive a mount
 * therefore lives here: the plan, the selection, what has arrived, what
 * failed, and the sequence itself. `FirstLaunchDialog.svelte` only draws it.
 *
 * The subscription is owned here too, for the same reason, and it is opened
 * with the offer and closed with it rather than with a component.
 *
 * **It adds no seam method.** The run is `downloadRuntime` and then
 * `downloadModel` per row, one at a time, with progress arriving on the
 * process-wide `model-progress` channel Settings reads as well. The settings
 * the later steps change go through the session's own setters and
 * `writeSettings`, which is the path Settings takes.
 */

import { getBackend } from '../api/backend.js'
import {
  backendSettingsPatch,
  markFirstLaunchOffered,
  session,
  setCloudAllowed,
  setFluxModel,
} from '../state/session.svelte.js'
import { setDialogOutsideStack } from '../shortcuts.js'
import { capabilities, loadCapabilities } from '../state/capabilities.svelte.js'
import {
  RUNTIME_ID,
  FIRST_LAUNCH_STEPS,
  defaultFluxModel,
  downloadQueue,
  firstLaunchPlan,
  initialSelection,
  plannedBytes,
  runtimeReady,
} from './firstlaunch.js'

/**
 * The offer, as the dialog reads it.
 *
 * `open` is what `App.svelte` mounts on and what the keyboard layer is told
 * about; everything else is what the dialog draws.
 */
export const firstLaunch = $state({
  open: false,
  /** Which of `FIRST_LAUNCH_STEPS` is on screen. @type {string} */
  step: 'welcome',
  /** @type {import('./firstlaunch.js').FirstLaunchPlan|null} */
  plan: null,
  /** Which ids the press will fetch. @type {Record<string, boolean>} */
  selection: {},
  /** What each download has reported while it runs. @type {Record<string, {downloaded: number, total: number|null}>} */
  progress: {},
  /** The ids that have arrived since the offer opened. @type {Record<string, boolean>} */
  finished: {},
  /** The failure that stopped the sequence, if one did. @type {{id: string, message: string}|null} */
  failure: null,
  /** Whether the sequence ended because the user cancelled the transfer in flight. */
  stopped: false,
  paused: false,
  /** Whether the whole queue arrived. */
  sequenceDone: false,
  running: false,
  /** The id being fetched, which is the one Cancel would stop. @type {string|null} */
  current: null,
  /** Whether the cloud step is showing the provisioner rather than its two lines. */
  provisioning: false,
  /**
   * Whether the provisioner is creating or removing things in the user's
   * account. The setup offers no way out then but the provisioner's own: the
   * helper would carry on with nothing left to read its answer.
   */
  provisionerBusy: false,
  /**
   * The cloud GPU a setup in this offer finished, as much of it as the step
   * says back, and whether it answered its first check. Never the endpoint or
   * a credential: the provisioner keeps those. Kept whether or not it
   * answered, so the step does not offer a second setup, which would be a
   * second installation in the account.
   *
   * @type {{provider: 'modal'|'beam', name: string|null, healthy: boolean}|null}
   */
  cloud: null,
  /** Whether that setup finished and the permission it turns on could not be saved. */
  cloudSaveFailed: false,
  /** The runtime's answer to `listAccelerators`, once it can be asked. @type {import('../api/backend.js').Accelerators|null} */
  accelerators: null,
  /** Whether that question was asked and refused, which `null` alone cannot say. */
  acceleratorsFailed: false,
  /** What the AI redraw helper lists, when there is one. @type {Array<{id: string, label: string}>} */
  sidecarModels: [],
})

/**
 * One resolver per id in flight, settled by the single `done` event every
 * download ends with - a file that arrived, a failure, or a cancellation, all
 * three through the same path.
 *
 * @type {Map<string, (error: string|null) => void>}
 */
const waiting = new Map()

/** Torn down with the offer. @type {(() => void)|null} */
let unsubscribe = null

/**
 * Whether Cancel was pressed. Held outside the run because a press can land in
 * the window between asking for a download and being told it started, which is
 * exactly where a cancellation used to be lost.
 */
let cancelRequested = false

/**
 * Open the offer over a catalogue answer.
 *
 * Idempotent: a second call while it is open changes nothing, because the plan
 * the user is looking at - and the run underneath it - must not be rebuilt by
 * a stray relaunch of the check.
 *
 * `force` is Settings' "Run setup again": without it an offer already made is
 * not made twice. A replay while an earlier run is still fetching keeps that
 * run and its plan, because rebuilding the plan under it would quote bytes
 * already on their way and drop the waiters the run is parked on.
 *
 * @param {import('../api/backend.js').ModelsView} view
 * @param {{force?: boolean}} [options]
 * @returns {boolean} whether the offer is now open
 */
export function offerFirstLaunch(view, { force = false } = {}) {
  if (firstLaunch.open) return true
  if (!force && session.firstLaunchOffered) return false
  if (!firstLaunch.running) {
    const plan = firstLaunchPlan(view)
    firstLaunch.plan = plan
    firstLaunch.selection = initialSelection(plan)
    firstLaunch.progress = {}
    firstLaunch.finished = {}
    firstLaunch.failure = null
    firstLaunch.stopped = false
    firstLaunch.paused = false
    firstLaunch.sequenceDone = false
    firstLaunch.current = null
    cancelRequested = false
  }
  firstLaunch.step = FIRST_LAUNCH_STEPS[0]
  firstLaunch.provisioning = false
  firstLaunch.provisionerBusy = false
  firstLaunch.cloud = null
  firstLaunch.cloudSaveFailed = false
  firstLaunch.accelerators = null
  firstLaunch.acceleratorsFailed = false
  firstLaunch.sidecarModels = []
  listen()
  firstLaunch.open = true
  // The offer is a dialog the shortcut table cannot see, because it is not on
  // the modal stack. Telling the table is what stops `,` opening Settings
  // underneath it.
  setDialogOutsideStack(true)
  return true
}

/** @param {string} step - one of `FIRST_LAUNCH_STEPS`; anything else is ignored */
export function setFirstLaunchStep(step) {
  if (FIRST_LAUNCH_STEPS.includes(/** @type {any} */ (step))) firstLaunch.step = step
}

export function nextFirstLaunchStep() {
  const index = FIRST_LAUNCH_STEPS.indexOf(/** @type {any} */ (firstLaunch.step))
  if (index >= 0 && index < FIRST_LAUNCH_STEPS.length - 1) firstLaunch.step = FIRST_LAUNCH_STEPS[index + 1]
}

export function prevFirstLaunchStep() {
  const index = FIRST_LAUNCH_STEPS.indexOf(/** @type {any} */ (firstLaunch.step))
  if (index > 0) firstLaunch.step = FIRST_LAUNCH_STEPS[index - 1]
}

/**
 * Close it, and remember that the user was asked.
 *
 * Every way out comes here - Skip setup, Escape, and the last step's button -
 * because what the flag records is that the offer was *made*. A run still in
 * flight is not cancelled by closing: the transfers belong to the backend and
 * Settings › Models is watching the same events.
 */
export function dismissFirstLaunch() {
  markFirstLaunchOffered()
  firstLaunch.open = false
  firstLaunch.provisioning = false
  firstLaunch.provisionerBusy = false
  setDialogOutsideStack(false)
  if (!firstLaunch.running) stopListening()
}

/**
 * Tick or clear one of the optional engines.
 *
 * @param {string} id
 * @param {boolean} wanted
 */
export function setFirstLaunchTick(id, wanted) {
  firstLaunch.selection = { ...firstLaunch.selection, [id]: wanted }
}

/**
 * The selection minus whatever has already arrived.
 *
 * The plan is the boot snapshot and its `installed` flags are as old as it is,
 * so a row fetched a minute ago is still `installed: false` there - and a
 * button that went on promising 333 MB after four of the six had arrived would
 * be quoting a price for goods already delivered. The tick itself is left
 * alone: the selection is what the user asked for and this is what is left of
 * it.
 *
 * @returns {Record<string, boolean>}
 */
export function pendingSelection() {
  return Object.fromEntries(
    Object.entries(firstLaunch.selection).map(([id, wanted]) => [
      id,
      wanted && firstLaunch.finished[id] !== true,
    ]),
  )
}

/** What the press would fetch, in order. @returns {string[]} */
export function pendingQueue() {
  return firstLaunch.plan ? downloadQueue(firstLaunch.plan, pendingSelection()) : []
}

/** What the press promises, in bytes. @returns {number} */
export function pendingBytes() {
  return firstLaunch.plan ? plannedBytes(firstLaunch.plan, pendingSelection()) : 0
}

/**
 * The press.
 *
 * The flag is set here as well as on `Not now`, because what it records is
 * that the user was asked.
 *
 * Three orderings in this loop are load-bearing, and each of them was a race
 * before it was written this way:
 *
 * 1. **The waiter is registered before the download is asked for.** A `done`
 *    event can beat the `invoke` reply - a cached file verifies in
 *    microseconds - and a waiter registered afterwards would wait for an event
 *    that has already been and gone.
 * 2. **`alreadyRunning` waits.** It means another window is fetching this very
 *    artefact, and that transfer ends with the same single `done` event; not
 *    waiting for it moved on to the next download while the one before it was
 *    still writing, which is the parallelism this sequence exists to avoid.
 *    `alreadyInstalled` is the opposite - nothing is in flight and no event
 *    will ever come - so its waiter is discarded and the row is marked here.
 * 3. **A Cancel pressed while the start is in flight is honoured after it.**
 *    `cancelDownload` for an id the backend has not begun answers `false` and
 *    is lost, so the press is remembered and re-sent once there is a transfer
 *    to stop.
 */
export async function startFirstLaunchDownloads() {
  if (firstLaunch.running || !firstLaunch.plan) return
  markFirstLaunchOffered()
  firstLaunch.failure = null
  firstLaunch.stopped = false
  firstLaunch.paused = false
  firstLaunch.sequenceDone = false
  firstLaunch.running = true
  cancelRequested = false
  listen()
  const backend = getBackend()
  try {
    // What is left of the selection: a second press after a cancellation
    // starts where the first one stopped rather than fetching what arrived.
    for (const id of pendingQueue()) {
      firstLaunch.current = id
      const arrival = waitForDone(id)
      /** @type {import('../api/backend.js').DownloadStart} */
      let outcome
      try {
        outcome = id === RUNTIME_ID
          ? await backend.downloadRuntime()
          : await backend.downloadModel({ id })
      } catch (error) {
        discard(id)
        firstLaunch.failure = { id, message: String(error) }
        return
      }
      if (outcome === 'alreadyInstalled') {
        discard(id)
        firstLaunch.finished = { ...firstLaunch.finished, [id]: true }
        continue
      }
      // A press that landed while this download was being asked for.
      if (cancelRequested) await backend.cancelDownload({ id })
      const error = await arrival
      // A cancellation is a failure the user chose, so it is not reported as
      // one: the run stops and says where the rest of them live.
      if (error === 'cancelled') {
        firstLaunch.stopped = true
        firstLaunch.paused = true
        return
      }
      if (error) {
        firstLaunch.failure = { id, message: error }
        return
      }
    }
    firstLaunch.sequenceDone = true
  } finally {
    firstLaunch.current = null
    firstLaunch.running = false
    cancelRequested = false
    // A download changes which engines the editor may offer, which is the
    // whole point of the press.
    await loadCapabilities()
    // A run that outlived the dialog has nothing left to report to.
    if (!firstLaunch.open) stopListening()
  }
}

/**
 * Stop the transfer in flight. Its `done` event ends the sequence.
 *
 * The request is remembered whether or not there is something to cancel yet,
 * because the press can arrive before the backend has answered that it
 * started one.
 */
export async function cancelFirstLaunchDownload() {
  if (!firstLaunch.running) return
  cancelRequested = true
  const id = firstLaunch.current
  if (!id) return
  await getBackend().cancelDownload({ id })
}

/** Pause the active transfer. The backend keeps its partial file for resume. */
export async function pauseFirstLaunchDownloads() {
  await cancelFirstLaunchDownload()
}

/* ------------------------------------------------------------------ */
/* The settings the later steps change                                 */
/* ------------------------------------------------------------------ */

/**
 * Change one setting the way Settings does: the session's own setter, then
 * the backend half of the whole session, then the capabilities that read it.
 *
 * Unlike Settings, a refused write puts the session back. The step shows the
 * value it holds as the choice that was made, and a choice the backend did
 * not keep would be shown as kept.
 *
 * @template T
 * @param {(value: T) => void} apply - a session setter
 * @param {T} next
 * @param {T} previous - what `apply` restores when the write is refused
 * @returns {Promise<boolean>} whether the backend kept it
 */
export async function saveFirstLaunchSetting(apply, next, previous) {
  apply(next)
  try {
    await getBackend().writeSettings(backendSettingsPatch())
  } catch {
    apply(previous)
    return false
  }
  await loadCapabilities()
  return true
}

/**
 * Ask the runtime which processors it can run on, when it can be asked
 * (`runtimeReady`). Until then the defaults step offers Automatic alone and
 * says why.
 */
export async function loadFirstLaunchAccelerators() {
  if (!runtimeReady(firstLaunch.plan, firstLaunch.finished, firstLaunch.current)) return
  try {
    firstLaunch.accelerators = await getBackend().listAccelerators()
    firstLaunch.acceleratorsFailed = false
  } catch {
    firstLaunch.accelerators = null
    firstLaunch.acceleratorsFailed = true
  }
}

/**
 * What the AI redraw helper can run, and the first choice when none was made.
 *
 * The first choice is Settings' (`defaultFluxModel`), and it is written
 * through rather than held in the session alone: the backend falls back to
 * its own default when the setting is empty, and that default need not be a
 * model this helper has.
 */
export async function loadFirstLaunchSidecarModels() {
  if (!capabilities.sidecar) {
    firstLaunch.sidecarModels = []
    return
  }
  try {
    const models = await getBackend().listSidecarModels()
    firstLaunch.sidecarModels = Array.isArray(models) ? models : []
  } catch {
    firstLaunch.sidecarModels = []
    return
  }
  const chosen = defaultFluxModel(firstLaunch.sidecarModels)
  if (!session.fluxModel && chosen) await saveFirstLaunchSetting(setFluxModel, chosen, '')
}

/** Show the provisioner in place of the cloud step's two lines. */
export function openFirstLaunchProvisioner() {
  firstLaunch.provisioning = true
}

/**
 * Back to the two lines, which say what the setup did if it finished.
 *
 * The provisioner goes with them, so nothing is left to be busy: a
 * provisioner that closed itself mid-run must not leave the setup with no
 * way out.
 */
export function closeFirstLaunchProvisioner() {
  firstLaunch.provisioning = false
  firstLaunch.provisionerBusy = false
}

/**
 * The provisioner's `onbusychange`: true while it creates or removes things
 * in the user's account, false once that has ended however it ended.
 *
 * @param {boolean} busy
 */
export function setFirstLaunchProvisionerBusy(busy) {
  firstLaunch.provisionerBusy = busy === true
}

/**
 * The provisioner's `onconfigured`.
 *
 * The provisioner saves the endpoint and selects it itself (IC-5), so what is
 * left here is the cloud permission, which "Set up now" promises to turn on.
 * It never throws: the setup did succeed, and a provisioner told otherwise
 * would offer to run it again. A refused write is said on the step instead.
 *
 * Only for an endpoint that answered its first check. One that did not is
 * kept and said back, so the step does not offer the setup again, but the
 * permission stays as it was: on, it would promise cloud cleaning nobody has
 * seen work. The person tests it and turns it on in Settings > Cloud.
 *
 * An answer that names no saved profile has left nothing to run on, so the
 * permission stays as it was: on alone, it would promise cloud cleaning that
 * cannot happen.
 *
 * @param {{provider?: string, profileId?: string, name?: string, healthy?: boolean}|null|undefined} info
 */
export async function configureFirstLaunchCloud(info) {
  if (typeof info?.profileId !== 'string' || !info.profileId.trim()) return
  const provider = info?.provider === 'beam' ? 'beam' : 'modal'
  const name = typeof info?.name === 'string' && info.name.trim() ? info.name.trim() : null
  const healthy = info?.healthy === true
  firstLaunch.cloud = { provider, name, healthy }
  firstLaunch.cloudSaveFailed = false
  if (!healthy) return
  const previous = session.cloudAllowed
  firstLaunch.cloudSaveFailed = !(await saveFirstLaunchSetting(setCloudAllowed, true, previous))
}

/* ------------------------------------------------------------------ */
/* The event channel                                                   */
/* ------------------------------------------------------------------ */

function listen() {
  if (unsubscribe) return
  unsubscribe = getBackend().subscribe((event) => {
    if (event.type !== 'model-progress') return
    if (!event.done) {
      firstLaunch.progress = {
        ...firstLaunch.progress,
        [event.id]: { downloaded: event.downloaded, total: event.total },
      }
      return
    }
    const { [event.id]: _gone, ...rest } = firstLaunch.progress
    firstLaunch.progress = rest
    if (!event.error) firstLaunch.finished = { ...firstLaunch.finished, [event.id]: true }
    waiting.get(event.id)?.(event.error ?? null)
    waiting.delete(event.id)
  })
}

function stopListening() {
  // A sequence parked on a `done` event that will now never reach it would
  // hold its promise for the life of the page.
  for (const resolve of waiting.values()) resolve('cancelled')
  waiting.clear()
  unsubscribe?.()
  unsubscribe = null
}

/** @param {string} id */
function waitForDone(id) {
  return /** @type {Promise<string|null>} */ (
    new Promise((resolve) => {
      waiting.set(id, resolve)
    })
  )
}

/** A waiter for a download that will never report. @param {string} id */
function discard(id) {
  waiting.get(id)?.(null)
  waiting.delete(id)
}

/**
 * Put the module back as it was. **Tests only** - an application opens the
 * offer once and closes it once.
 */
export function resetFirstLaunch() {
  stopListening()
  cancelRequested = false
  firstLaunch.open = false
  firstLaunch.step = 'welcome'
  firstLaunch.plan = null
  firstLaunch.selection = {}
  firstLaunch.progress = {}
  firstLaunch.finished = {}
  firstLaunch.failure = null
  firstLaunch.stopped = false
  firstLaunch.paused = false
  firstLaunch.sequenceDone = false
  firstLaunch.running = false
  firstLaunch.current = null
  firstLaunch.provisioning = false
  firstLaunch.provisionerBusy = false
  firstLaunch.cloud = null
  firstLaunch.cloudSaveFailed = false
  firstLaunch.accelerators = null
  firstLaunch.acceleratorsFailed = false
  firstLaunch.sidecarModels = []
  setDialogOutsideStack(false)
}
