/**
 * The event stream, merged.
 *
 * What happens to `subscribe` when the run becomes a
 * command: it becomes a Tauri `Channel` **merged with** the fallback's stream,
 * not a replacement. The reason is the same one that makes the whole adapter a
 * delegating one - a partial adapter serves what it implements and the mock
 * serves the rest, so while any method is still the mock's, the notices that
 * method owes have to keep arriving.
 *
 * The merge is a union and it cannot double-count: an event is produced by
 * exactly one implementation, because whichever one served the call is the one
 * that knows anything happened. The seam's ordering rules are stated per run
 * - every `region-done` inside its page's bracket, exactly one `run-finished`
 * - and they survive the merge for the same reason: **a run belongs entirely
 * to one implementation**, and neither a channel nor the mock reorders its own
 * output.
 *
 * Nothing here imports `@tauri-apps/api`. `withGlobalTauri` puts `Channel` on
 * the window beside `invoke`, and taking it from there keeps this module
 * loadable - and testable - in a plain browser and in vitest, where a Tauri
 * import would throw at load. That is the same rule `tauri.js` follows.
 */

/**
 * A channel, from wherever Tauri put it.
 *
 * @returns {{ onmessage: ((event: any) => void)|null }}
 */
function globalChannel() {
  const Channel = globalThis.__TAURI__?.core?.Channel ?? globalThis.__TAURI_INTERNALS__?.Channel
  if (typeof Channel !== 'function') {
    throw new Error('the Tauri event stream was constructed outside a Tauri window')
  }
  return new Channel()
}

/**
 * One `subscribe` over two sources.
 *
 * ## The backend half is opened once, and its lifetime is the window's
 *
 * It used to be reference-counted: opened on the first handler, closed with the
 * last. That is wrong for this application, and wrong in a way that only shows
 * up while a run is in flight.
 *
 * Home and the editor are the only two subscribers and they **never coexist**
 * (`src/lib/home/library.svelte.js`), and the editor's screen drops its handler
 * on teardown (`closeEditorChapter`), so every Home↔editor navigation takes the
 * handler count to zero and back to one. Reference-counting turned that into
 * `unsubscribe_events` followed by a fresh `subscribe_events` - two IPC round
 * trips, with the generation guard refusing the old channel from the first
 * instant and `events.rs`'s `emit` iterating no sink at all once the unregister
 * lands. A run started in the editor goes on emitting through that gap, and
 * whatever falls in it is gone: `region-done`, `page-done`, and - the one that
 * matters - `run-finished`. The seam's "exactly one `run-finished` per run" rule
 * then fails on a navigation the interface fully supports, and the run's
 * completion, along with the `notice.run.finished` that goes with it, is never
 * seen by any screen.
 *
 * So the sink is opened here, at construction, and never closed while the
 * window lives. Handlers attach and detach against a channel that does not
 * move. The cost is the one the old comment named - a channel the backend
 * writes into for the life of the window - and it is the correct cost: the run
 * thread emits for minutes with no call outstanding, and something has to be
 * holding the other end the whole time.
 *
 * Construction is once per window: `getBackend()` in `backend.js` memoises a
 * single adapter and `createTauriBackend` builds exactly one stream from it.
 * That is a deliberately *lower* place to put this than `App.svelte` - a
 * component's `$effect` is remounted by HMR during development, which would
 * reintroduce the same open/close churn against a live run, and the memoised
 * adapter survives it.
 *
 * @param {Object} options
 * @param {(command: string, args?: Object) => Promise<any>} options.call - the adapter's `invoke`
 * @param {import('./backend.js').Backend} options.fallback - still serving whatever the adapter does not
 * @param {() => {onmessage: ((event: any) => void)|null}} [options.channel] - injected for tests
 * @returns {(handler: (event: import('./backend.js').BackendEvent) => void) => (() => void)}
 */
export function createEventStream({ call, fallback, channel = globalChannel }) {
  /** @type {Set<(event: any) => void>} */
  const handlers = new Set()

  /**
   * Cloned once and shared, exactly as the mock does it: the seam says events
   * are structured-cloned before dispatch, and a handler that mutates what it
   * was given must not be able to reach the backend's own copy.
   *
   * With nobody attached the event is dropped here rather than cloned first.
   * That is the *only* thing that happens differently while no component is
   * subscribed - the sink is still registered, so the run is not interrupted
   * and whatever is attached when the next event arrives receives it.
   *
   * @param {any} event
   */
  const deliver = (event) => {
    if (handlers.size === 0) return
    const frozen = structuredClone(event)
    for (const handler of [...handlers]) handler(frozen)
  }

  /** @type {{onmessage: ((event: any) => void)|null}|null} */
  let stream = null
  try {
    stream = channel()
  } catch {
    // Outside a Tauri window there is no channel to open, and the fallback is
    // then the only implementation running anything. Failing here would take
    // its stream down with the one that does not exist.
    stream = null
  }
  if (stream) {
    stream.onmessage = deliver
    // The handle `unsubscribe_events` would take is deliberately not kept:
    // nothing shorter than the window owns this sink, so there is nothing here
    // that could correctly decide to close it.
    Promise.resolve(call('subscribe_events', { channel: stream })).catch(() => null)
  }

  return function subscribe(handler) {
    handlers.add(handler)
    const detach = fallback.subscribe(handler)

    let live = true
    return () => {
      if (!live) return
      live = false
      handlers.delete(handler)
      detach()
    }
  }
}
