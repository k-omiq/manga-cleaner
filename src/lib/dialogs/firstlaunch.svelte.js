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
 * process-wide `model-progress` channel Settings reads as well.
 */

import { getBackend } from '../api/backend.js'
import { markFirstLaunchOffered } from '../state/session.svelte.js'
import { setDialogOutsideStack } from '../shortcuts.js'
import { loadCapabilities } from '../state/capabilities.svelte.js'
import { RUNTIME_ID, downloadQueue, firstLaunchPlan, initialSelection, plannedBytes } from './firstlaunch.js'

/**
 * The offer, as the dialog reads it.
 *
 * `open` is what `App.svelte` mounts on and what the keyboard layer is told
 * about; everything else is what the dialog draws.
 */
export const firstLaunch = $state({
  open: false,
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
  /** Whether the whole queue arrived. */
  sequenceDone: false,
  running: false,
  /** The id being fetched, which is the one Cancel would stop. @type {string|null} */
  current: null,
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
 * @param {import('../api/backend.js').ModelsView} view
 * @returns {boolean} whether the offer is now open
 */
export function offerFirstLaunch(view) {
  if (firstLaunch.open) return true
  const plan = firstLaunchPlan(view)
  if (!plan.offer) return false
  firstLaunch.plan = plan
  firstLaunch.selection = initialSelection(plan)
  firstLaunch.progress = {}
  firstLaunch.finished = {}
  firstLaunch.failure = null
  firstLaunch.stopped = false
  firstLaunch.sequenceDone = false
  firstLaunch.running = false
  firstLaunch.current = null
  cancelRequested = false
  listen()
  firstLaunch.open = true
  // The offer is a dialog the shortcut table cannot see, because it is not on
  // the modal stack. Telling the table is what stops `,` opening Settings
  // underneath it.
  setDialogOutsideStack(true)
  return true
}

/**
 * Close it, and remember that the user was asked.
 *
 * Both answers come here - `Not now`, Escape, the backdrop, and `Done` at the
 * end of a run - because what the flag records is that the offer was *made*.
 * A run still in flight is not cancelled by closing: the transfers belong to
 * the backend and Settings › Models is watching the same events.
 */
export function dismissFirstLaunch() {
  markFirstLaunchOffered()
  firstLaunch.open = false
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
  firstLaunch.plan = null
  firstLaunch.selection = {}
  firstLaunch.progress = {}
  firstLaunch.finished = {}
  firstLaunch.failure = null
  firstLaunch.stopped = false
  firstLaunch.sequenceDone = false
  firstLaunch.running = false
  firstLaunch.current = null
  setDialogOutsideStack(false)
}
