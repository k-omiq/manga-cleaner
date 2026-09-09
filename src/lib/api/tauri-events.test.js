/**
 * The merged event stream, and the run scheduler seen through it.
 *
 * Two halves. The first tests the merge itself - that both sources reach one
 * handler, that unsubscribing stops both, and that the backend half is opened
 * once for the life of the window rather than once per handler.
 *
 * The second is the acceptance check the seam contract names: three of `mock.test.js`'s
 * tests exercise the run scheduler and are written against the *interface*
 * rather than the mock, so they are worth running against a real adapter. They
 * are re-run here through `createTauriBackend` itself - the adapter that ships,
 * with nothing overridden after construction - against a backend that answers
 * `run_clean` and `cancel_run` and emits over the channel. What that pins is
 * everything between the scheduler and the handler: the queue shape, the merge,
 * and the ordering surviving both. The scheduler's own ordering is pinned in
 * Rust, in `src-tauri/src/run/tests.rs`, against the scheduler the window runs.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { createEventStream } from './tauri-events.js'
import { createMockBackend } from './mock.js'
import { createTauriBackend, SEAM_METHODS } from './tauri.js'

/** A channel a test can push into, in place of Tauri's. */
function fakeChannel() {
  const channel = { onmessage: null, closed: false }
  return channel
}

/**
 * A channel factory that mints a **new** channel each time it is asked, and
 * keeps every one it made.
 *
 * Tauri's own `Channel` behaves this way - each one is a distinct IPC target -
 * and the distinction is what makes the assertion possible at all: a factory
 * that hands the same object back every time cannot tell a stream that was
 * never closed apart from one that was closed and reopened.
 */
function channelFactory() {
  const made = []
  const next = () => {
    made.push(fakeChannel())
    return made.at(-1)
  }
  next.made = made
  return next
}

/** A fallback that records subscriptions and can emit. */
function recordingFallback() {
  const handlers = new Set()
  const backend = /** @type {any} */ ({
    subscribed: 0,
    unsubscribed: 0,
    emit: (event) => handlers.forEach((h) => h(event)),
  })
  for (const method of SEAM_METHODS) {
    backend[method] = () => Promise.resolve(null)
  }
  backend.subscribe = (handler) => {
    backend.subscribed += 1
    handlers.add(handler)
    return () => {
      backend.unsubscribed += 1
      handlers.delete(handler)
    }
  }
  return backend
}

describe('the merged event stream', () => {
  it('one handler receives both the backend’s events and the fallback’s', async () => {
    const fallback = recordingFallback()
    const channel = fakeChannel()
    const call = vi.fn().mockResolvedValue(7)
    const subscribe = createEventStream({ call, fallback, channel: () => channel })

    const seen = []
    subscribe((event) => seen.push(event))

    channel.onmessage({ type: 'page-started', runId: 'run-1', pageIndex: 0 })
    fallback.emit({ type: 'notice', id: 'notice-1', key: 'notice.project.created' })
    channel.onmessage({ type: 'page-done', runId: 'run-1', pageIndex: 0 })

    expect(seen.map((event) => event.type)).toEqual(['page-started', 'notice', 'page-done'])
    expect(call).toHaveBeenCalledWith('subscribe_events', { channel })
  })

  /**
   * The whole reason the merge is a merge. Anything still served by the mock
   * keeps emitting, and `notice.project.created` is one of the notices that
   * went silent inside a Tauri window until this existed.
   */
  it('the fallback keeps emitting even when the channel never opens', () => {
    const fallback = recordingFallback()
    const subscribe = createEventStream({
      call: vi.fn(),
      fallback,
      channel: () => {
        throw new Error('outside a Tauri window')
      },
    })
    const seen = []
    subscribe((event) => seen.push(event))
    fallback.emit({ type: 'notice', id: 'notice-1', key: 'notice.export.finished' })
    expect(seen).toHaveLength(1)
  })

  it('unsubscribing detaches from both sources', async () => {
    const fallback = recordingFallback()
    const channel = fakeChannel()
    const subscribe = createEventStream({
      call: vi.fn().mockResolvedValue(7),
      fallback,
      channel: () => channel,
    })

    const seen = []
    const unsubscribe = subscribe((event) => seen.push(event))
    unsubscribe()

    channel.onmessage({ type: 'page-started' })
    fallback.emit({ type: 'notice' })
    expect(seen).toEqual([])
    expect(fallback.unsubscribed).toBe(1)
  })

  it('opens the backend stream once, before any handler has attached', () => {
    const fallback = recordingFallback()
    const channel = channelFactory()
    const call = vi.fn().mockResolvedValue(7)
    const subscribe = createEventStream({ call, fallback, channel })

    expect(channel.made).toHaveLength(1)
    expect(call.mock.calls.filter(([command]) => command === 'subscribe_events')).toHaveLength(1)

    subscribe(() => {})
    subscribe(() => {})
    expect(channel.made).toHaveLength(1)
    expect(call.mock.calls.filter(([command]) => command === 'subscribe_events')).toHaveLength(1)
  })

  /**
   * The blocker this replaced a reference count for. Home and the editor never
   * coexist (`src/lib/home/library.svelte.js`), so every Home↔editor navigation
   * took the handler count to zero - and with it the backend's sink, over one
   * IPC round trip each way, while a run was still emitting into the channel it
   * was given.
   */
  it('a subscriber attaching and detaching mid-run does not tear the sink down', async () => {
    const fallback = recordingFallback()
    const channel = channelFactory()
    const call = vi.fn().mockResolvedValue(7)
    const subscribe = createEventStream({ call, fallback, channel })

    // The editor subscribes, starts a run, and the user navigates to Home.
    const editor = subscribe(() => {})
    channel.made[0].onmessage({ type: 'page-started', runId: 'run-1', pageIndex: 0 })
    editor()
    await Promise.resolve()
    await Promise.resolve()

    expect(call).not.toHaveBeenCalledWith('unsubscribe_events', expect.anything())

    // Home subscribes, and the user navigates back.
    const home = subscribe(() => {})
    home()
    await Promise.resolve()
    await Promise.resolve()
    subscribe(() => {})

    expect(channel.made).toHaveLength(1)
    expect(call.mock.calls.filter(([command]) => command === 'subscribe_events')).toHaveLength(1)
    expect(call).not.toHaveBeenCalledWith('unsubscribe_events', expect.anything())
  })

  /**
   * The consequence, stated as the run sees it. A run that ends while the user
   * is on the other screen still ends: `run-finished` arrives on the channel the
   * run was given, and whoever is attached when it does receives it. Exactly one
   * `run-finished` per run is the seam's rule, and it holds across a navigation.
   */
  it('a run that ends while nobody is subscribed still delivers run-finished', () => {
    const fallback = recordingFallback()
    const channel = channelFactory()
    const subscribe = createEventStream({
      call: vi.fn().mockResolvedValue(7),
      fallback,
      channel,
    })

    const sink = channel.made[0]
    const editor = subscribe(() => {})
    sink.onmessage({ type: 'page-started', runId: 'run-1', pageIndex: 0 })
    editor()

    // Nobody is attached. The run carries on emitting into the sink it holds.
    sink.onmessage({ type: 'region-done', runId: 'run-1', pageIndex: 0 })
    sink.onmessage({ type: 'page-done', runId: 'run-1', pageIndex: 0 })

    const seen = []
    subscribe((event) => seen.push(event))
    sink.onmessage({ type: 'run-finished', runId: 'run-1', reason: 'completed' })

    expect(seen.map((event) => event.type)).toEqual(['run-finished'])
  })

  /** A handler that mutates what it is given must not reach anyone else's copy. */
  it('events are cloned before dispatch', () => {
    const fallback = recordingFallback()
    const channel = fakeChannel()
    const subscribe = createEventStream({
      call: vi.fn().mockResolvedValue(1),
      fallback,
      channel: () => channel,
    })
    const original = { type: 'region-done', region: { id: 'r1' } }
    let received = null
    subscribe((event) => {
      received = event
      event.region.id = 'tampered'
    })
    channel.onmessage(original)
    expect(received.region.id).toBe('tampered')
    expect(original.region.id).toBe('r1')
  })

  /**
   * A run belongs entirely to one implementation, which is what makes
   * the seam's per-run ordering survive a merge. Interleaving the other
   * source's notices must not move a run's own events relative to each other.
   */
  it('a run’s own events keep their order when the other source interleaves', () => {
    const fallback = recordingFallback()
    const channel = fakeChannel()
    const subscribe = createEventStream({
      call: vi.fn().mockResolvedValue(1),
      fallback,
      channel: () => channel,
    })
    const seen = []
    subscribe((event) => seen.push(event))

    channel.onmessage({ type: 'page-started', runId: 'run-1', pageIndex: 0 })
    fallback.emit({ type: 'notice', id: 'n1', key: 'notice.project.renamed' })
    channel.onmessage({ type: 'region-done', runId: 'run-1', pageIndex: 0 })
    fallback.emit({ type: 'notice', id: 'n2', key: 'notice.chapter.added' })
    channel.onmessage({ type: 'page-done', runId: 'run-1', pageIndex: 0 })
    channel.onmessage({ type: 'run-finished', runId: 'run-1', reason: 'completed' })

    const run = seen.filter((event) => event.runId === 'run-1').map((event) => event.type)
    expect(run).toEqual(['page-started', 'region-done', 'page-done', 'run-finished'])
    expect(seen.filter((event) => event.type === 'notice')).toHaveLength(2)
  })
})

/* ------------------------------------------------------------------ */
/* The acceptance check                                                */
/* ------------------------------------------------------------------ */

const TIMING = { method: 0, openChapter: 0, export: 0, region: 100, pageTail: 100, noticeStagger: 0 }
/** A chapter with 20 pages, none of them cleaned. */
const CHAPTER = 'tsuki-to-hane-ch107'
/** A chapter of 48 pages with no text detected on any of them. */
const EMPTY_CHAPTER = 'yoake-photobook-ch1'

/**
 * The adapter that ships, and nothing else.
 *
 * Every method here is `createTauriBackend`'s own - `runClean`, `cancelRun` and
 * `listProjects` off `IMPLEMENTED`, and `subscribe` off the one wiring line in
 * `tauri.js`. Nothing is reassigned after construction. An earlier version of
 * this harness overrode all four "so the acceptance check can run before that
 * merge lands", which stopped being true the moment it landed and left three
 * tests exercising `createMockBackend` through a fake channel: deleting the
 * adapter's own bodies did not fail any of them.
 *
 * The channel comes from `globalThis.__TAURI__.core.Channel`, which is where
 * `withGlobalTauri` puts it and therefore where the adapter looks. Standing a
 * fake there is what lets the shipped `subscribe` be exercised without a Tauri
 * window, and it is the *transport* that is faked - not the scheduler, which is
 * pinned in Rust.
 *
 * The backend behind `call` is a second mock engine standing in for the Rust
 * side: what these tests exercise is the queue shape and the event stream as a
 * component sees them.
 */
function adapterOverAChannel() {
  const engine = createMockBackend({ timing: TIMING })
  const fallback = createMockBackend({ timing: TIMING })

  const call = async (command, args = {}) => {
    switch (command) {
      case 'subscribe_events':
        return 1
      case 'unsubscribe_events':
        return true
      case 'run_clean':
        return engine.runClean(args)
      case 'cancel_run':
        return engine.cancelRun(args)
      case 'list_projects':
        return engine.listProjects()
      // A listing is headers, so a test that wants regions pages
      // them in - through the adapter, off the same engine.
      case 'load_pages':
        return engine.loadPages(args)
      default:
        return null
    }
  }

  const backend = createTauriBackend({ fallback, invoke: call })
  // The adapter opened its channel at construction; that is the one the engine
  // standing in for Rust writes into, for the life of this "window".
  const channel = openedChannels.at(-1)
  engine.subscribe((event) => channel.onmessage?.(event))

  const events = []
  backend.subscribe((event) => events.push(event))
  return { backend, events }
}

async function settle(promise) {
  await vi.runAllTimersAsync()
  return promise
}

async function begin(promise) {
  await vi.advanceTimersByTimeAsync(0)
  return promise
}

/** Every channel the adapter constructed off the fake global, in order. */
let openedChannels = []

beforeEach(() => {
  vi.useFakeTimers()
  openedChannels = []
  globalThis.__TAURI__ = {
    core: {
      Channel: class {
        constructor() {
          this.onmessage = null
          openedChannels.push(this)
        }
      },
    },
  }
})

afterEach(() => {
  vi.useRealTimers()
  delete globalThis.__TAURI__
})

describe('the run scheduler, through the adapter', () => {
  it('emits every page in order and finishes', async () => {
    const { backend, events } = adapterOverAChannel()

    const handle = await settle(backend.runClean({ scope: 'chapter', chapterId: CHAPTER }))
    expect(handle.pages).toHaveLength(20)

    const started = events.filter((e) => e.type === 'page-started').map((e) => e.pageIndex)
    const done = events.filter((e) => e.type === 'page-done').map((e) => e.pageIndex)
    const queued = handle.pages.map((page) => page.pageIndex)
    expect(started).toEqual(queued)
    expect(done).toEqual(queued)

    // Every region of a page is emitted between that page's start and its end.
    for (const pageIndex of queued) {
      const openedAt = events.findIndex(
        (e) => e.type === 'page-started' && e.pageIndex === pageIndex,
      )
      const closedAt = events.findIndex((e) => e.type === 'page-done' && e.pageIndex === pageIndex)
      const regions = events.filter(
        (e) => e.type === 'region-done' && e.pageId === events[closedAt].pageId,
      )
      expect(regions.length).toBeGreaterThan(0)
      for (const region of regions) {
        const at = events.indexOf(region)
        expect(at).toBeGreaterThan(openedAt)
        expect(at).toBeLessThan(closedAt)
      }
    }

    const finished = events.filter((e) => e.type === 'run-finished')
    expect(finished).toHaveLength(1)
    expect(finished[0]).toMatchObject({
      runId: handle.runId,
      reason: 'completed',
      pagesQueued: 20,
      pagesCleaned: 20,
      nextPageIndex: null,
    })
    expect(finished[0].regionsCleaned).toBe(events.filter((e) => e.type === 'region-done').length)
    expect(events.at(-1)).toMatchObject({ type: 'notice', key: 'notice.run.finished' })
  })

  it('keeps completed regions and stops emitting when cancelled mid-run', async () => {
    const { backend, events } = adapterOverAChannel()

    const handle = await begin(backend.runClean({ scope: 'chapter', chapterId: CHAPTER }))
    expect(handle.runId).not.toBeNull()

    await vi.advanceTimersByTimeAsync(900)
    const cleanedBefore = events.filter((e) => e.type === 'region-done').length
    expect(cleanedBefore).toBeGreaterThan(0)
    expect(events.some((e) => e.type === 'run-finished')).toBe(false)

    const cancelled = await begin(backend.cancelRun({ runId: handle.runId }))
    expect(cancelled).toBe(handle.runId)

    const finished = events.filter((e) => e.type === 'run-finished')
    expect(finished).toHaveLength(1)
    expect(finished[0].reason).toBe('cancelled')
    expect(finished[0].pagesCleaned).toBeLessThan(20)
    expect(finished[0].nextPageIndex).not.toBeNull()

    const eventCount = events.length
    await vi.advanceTimersByTimeAsync(60_000)
    expect(events).toHaveLength(eventCount)

    const projects = await settle(backend.listProjects())
    const chapter = projects.flatMap((project) => project.chapters).find((c) => c.id === CHAPTER)
    // A listing carries page headers only; the regions come from
    // a window, exactly as the editor asks for them.
    const loaded = await settle(
      backend.loadPages({ chapterId: CHAPTER, indices: chapter.pages.map((p) => p.index) }),
    )
    const cleanedRegions = loaded
      .flatMap((page) => page.regions)
      .filter((region) => region.outcome === 'cleaned')
    expect(cleanedRegions).toHaveLength(cleanedBefore)
    expect(chapter.pages.some((page) => page.status === 'unclean')).toBe(true)
    expect(chapter.pages.every((page) => page.status !== 'cleaning')).toBe(true)
  })

  it('reports the empty result when a chapter has no regions at all', async () => {
    const { backend, events } = adapterOverAChannel()

    await settle(backend.runClean({ scope: 'chapter', chapterId: EMPTY_CHAPTER }))

    expect(events.filter((e) => e.type === 'region-done')).toHaveLength(0)
    expect(events.filter((e) => e.type === 'page-done')).toHaveLength(48)

    const notices = events.filter((e) => e.type === 'notice')
    expect(notices).toHaveLength(1)
    expect(notices[0]).toMatchObject({
      key: 'notice.chapter.emptyResult',
      params: { regions: 0, pages: 48 },
      tone: 'warn',
    })
    expect(notices.some((e) => e.key === 'notice.run.finished')).toBe(false)
  })
})
