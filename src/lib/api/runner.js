/**
 * The simulated run scheduler.
 *
 * A run walks its queue one page at a time, emitting `region-done` for each
 * region and then `page-done`, so the Pages list ticks over as the progress
 * indicator (there is no progress bar). Nothing is
 * returned synchronously: the caller gets a run id and the queue, and every
 * result arrives on the event channel, exactly as a Tauri channel would
 * deliver it.
 *
 * Cancel keeps completed regions and leaves the job incomplete,
 * which is why the in-flight page falls back to
 * `unclean` rather than being rolled back.
 *
 * Every delay comes from the injected `timers`, so tests drive it with fake
 * timers and a demo can slow it down with a speed factor.
 */

import { isOutsideHeld, isQueueable } from './tools.js'

/**
 * @typedef {Object} QueueEntry
 * @property {string} projectId
 * @property {string} chapterId
 * @property {import('../model/types.js').Page} page
 */

/**
 * @typedef {Object} RunnerDeps
 * @property {(event: Object) => void} emit
 * @property {{ setTimeout: Function, clearTimeout: Function }} timers
 * @property {{ region: number, pageTail: number }} timing - already scaled by the speed factor
 * @property {(region: import('../model/types.js').Region, page: import('../model/types.js').Page, ctx: Object) => void} cleanRegion
 * @property {(summary: Object) => void} onFinished
 */

/**
 * @param {RunnerDeps} deps
 * @returns {{ start: Function, cancel: Function, isRunning: () => boolean, activeRunId: () => string|null }}
 */
export function createRunner(deps) {
  let active = null
  let counter = 0

  const schedule = (fn, ms) => {
    active.timer = deps.timers.setTimeout(fn, ms)
  }

  function stepPage() {
    if (!active) return
    if (active.index >= active.queue.length) return finish('completed')
    const entry = active.queue[active.index]
    const { page } = entry
    page.status = 'cleaning'
    // Snapshot the queue of regions once: cleaning one removes it from the
    // pending set, so a filter recomputed per step would skip every other one.
    // With the outside-bubble opt-in on, the regions the gate would have held
    // back as "text outside a speech bubble" are queued too (`isOutsideHeld`).
    const optedIn = (region) => active.outsideBubbles === 'clean' && isOutsideHeld(region)
    active.pending = page.regions.filter((region) => isQueueable(region) || optedIn(region))
    deps.emit({
      type: 'page-started',
      runId: active.runId,
      chapterId: entry.chapterId,
      pageId: page.id,
      pageIndex: page.index,
    })
    stepRegion(0)
  }

  function stepRegion(i) {
    if (!active) return
    const entry = active.queue[active.index]
    const { page } = entry
    if (i >= active.pending.length) {
      return schedule(() => finishPage(entry), deps.timing.pageTail)
    }
    schedule(() => {
      if (!active) return
      const region = active.pending[i]
      deps.cleanRegion(region, page, {
        engineCeiling: active.engineCeiling,
        bubbleEngine: active.bubbleEngine,
        outsideEngine: active.outsideEngine,
      })
      active.regionsCleaned += 1
      deps.emit({
        type: 'region-done',
        runId: active.runId,
        chapterId: entry.chapterId,
        pageId: page.id,
        pageIndex: page.index,
        region,
      })
      stepRegion(i + 1)
    }, deps.timing.region)
  }

  function finishPage(entry) {
    if (!active) return
    const { page } = entry
    page.status = 'cleaned'
    active.pagesCleaned += 1
    deps.emit({
      type: 'page-done',
      runId: active.runId,
      chapterId: entry.chapterId,
      pageId: page.id,
      pageIndex: page.index,
      page,
    })
    active.index += 1
    schedule(stepPage, 0)
  }

  /**
   * @param {'completed'|'cancelled'} reason
   */
  function finish(reason) {
    const run = active
    active = null
    deps.timers.clearTimeout(run.timer)
    const summary = {
      type: 'run-finished',
      runId: run.runId,
      chapterId: run.chapterId,
      reason,
      pagesQueued: run.queue.length,
      pagesCleaned: run.pagesCleaned,
      regionsCleaned: run.regionsCleaned,
      nextPageIndex: reason === 'cancelled' ? (run.queue[run.index]?.page.index ?? null) : null,
    }
    deps.emit(summary)
    deps.onFinished({ ...summary, queue: run.queue, stoppedAt: run.index })
  }

  return {
    /**
     * @param {QueueEntry[]} queue
     * @param {{ chapterId: string, engineCeiling?: string }} options
     * @returns {{ runId: string, pages: Array<{chapterId: string, pageId: string, pageIndex: number}> }}
     */
    start(queue, options) {
      counter += 1
      active = {
        runId: `run-${counter}`,
        chapterId: options.chapterId,
        engineCeiling: options.engineCeiling ?? 'lama',
        bubbleEngine: options.bubbleEngine,
        outsideEngine: options.outsideEngine,
        outsideBubbles: options.outsideBubbles ?? 'review',
        queue,
        index: 0,
        timer: null,
        pagesCleaned: 0,
        regionsCleaned: 0,
      }
      schedule(stepPage, 0)
      return {
        runId: active.runId,
        pages: queue.map((entry) => ({
          chapterId: entry.chapterId,
          pageId: entry.page.id,
          pageIndex: entry.page.index,
        })),
      }
    },

    /** Stops the run, keeping every region completed so far. */
    cancel() {
      if (!active) return null
      const entry = active.queue[active.index]
      if (entry) {
        const { page } = entry
        page.status = page.regions.every((region) => region.outcome !== 'pending')
          ? 'cleaned'
          : 'unclean'
      }
      const runId = active.runId
      finish('cancelled')
      return runId
    },

    isRunning: () => active !== null,
    activeRunId: () => (active ? active.runId : null),
  }
}
