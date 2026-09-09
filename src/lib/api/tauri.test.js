import { beforeEach, describe, expect, it, vi } from 'vitest'

import { createTauriBackend, implementedMethods, isTauri, SEAM_METHODS } from './tauri.js'
import { getBackend, setBackend } from './backend.js'

/**
 * A fallback that records every call and answers with something identifiable,
 * so a test can tell which side of the adapter served a method.
 */
function recordingFallback() {
  const calls = []
  const handlers = new Set()
  const backend = { calls, emit: (event) => handlers.forEach((h) => h(event)) }
  for (const method of SEAM_METHODS) {
    backend[method] = (...args) => {
      calls.push({ method, args })
      return Promise.resolve({ from: 'fallback', method })
    }
  }
  backend.subscribe = (handler) => {
    calls.push({ method: 'subscribe', args: [] })
    handlers.add(handler)
    return () => handlers.delete(handler)
  }
  backend.readSettings = (...args) => {
    calls.push({ method: 'readSettings', args })
    return Promise.resolve({ language: 'en', cloud: false, engineCeiling: 'lama' })
  }
  return backend
}

describe('the Tauri adapter', () => {
  /**
   * The adapter is only useful if it *is* a backend. A method missing from both
   * the command table and the delegation loop would be `undefined` at a call
   * site, which is a runtime crash in a component rather than a test failure.
   */
  it('answers every method the seam fixes', () => {
    const backend = createTauriBackend({ fallback: recordingFallback(), invoke: vi.fn() })
    for (const method of SEAM_METHODS) {
      expect(typeof backend[method], `${method} is not a function`).toBe('function')
    }
  })

  it('lists exactly the methods it does not delegate', async () => {
    const fallback = recordingFallback()
    const invoke = vi.fn().mockResolvedValue({})
    const backend = createTauriBackend({ fallback, invoke })

    for (const method of SEAM_METHODS) {
      if (method === 'subscribe') backend.subscribe(() => {})
      else await backend[method]({})
    }

    const delegated = new Set(fallback.calls.map((call) => call.method))
    // `readSettings` reaches the fallback too - for the defaults, not for the
    // value - so it is excluded from the delegation check by name rather than
    // by accident.
    const served = SEAM_METHODS.filter((m) => !delegated.has(m) || m === 'readSettings')
    expect(served.sort()).toEqual(implementedMethods())
  })

  it('sends about straight to the command and returns what it says', async () => {
    const invoke = vi
      .fn()
      .mockResolvedValue({ appVersion: '0.1.0', facts: [{ labelKey: 'about.fact.licence', value: 'GPL-3.0-or-later' }] })
    const backend = createTauriBackend({ fallback: recordingFallback(), invoke })

    await expect(backend.about()).resolves.toEqual({
      appVersion: '0.1.0',
      facts: [{ labelKey: 'about.fact.licence', value: 'GPL-3.0-or-later' }],
    })
    expect(invoke).toHaveBeenCalledWith('about')
  })

  /**
   * The core stores settings without knowing what one means, so on a first
   * launch it returns `{}`. An adapter that passed that through would hand the
   * interface a settings object with no settings in it.
   */
  it('merges the stored settings over the interface defaults', async () => {
    const fallback = recordingFallback()
    const invoke = vi.fn().mockResolvedValue({ cloud: true })
    const backend = createTauriBackend({ fallback, invoke })

    await expect(backend.readSettings()).resolves.toEqual({
      language: 'en',
      cloud: true,
      engineCeiling: 'lama',
    })
  })

  it('returns the whole snapshot after a write, not the patch', async () => {
    const fallback = recordingFallback()
    const invoke = vi.fn().mockResolvedValue({ cloud: true })
    const backend = createTauriBackend({ fallback, invoke })

    await expect(backend.writeSettings({ cloud: true })).resolves.toEqual({
      language: 'en',
      cloud: true,
      engineCeiling: 'lama',
    })
    expect(invoke).toHaveBeenCalledWith('write_settings', { patch: { cloud: true } })
  })

  it('empty stored settings leave the defaults alone', async () => {
    const backend = createTauriBackend({ fallback: recordingFallback(), invoke: vi.fn().mockResolvedValue({}) })
    await expect(backend.readSettings()).resolves.toEqual({
      language: 'en',
      cloud: false,
      engineCeiling: 'lama',
    })
  })

  /**
   * Events come from whichever implementation is running the job, and since
   * `run.rs` landed that is both of them: the run's four are the backend's and
   * the six region-level edits' notices are still the fallback's. A handler
   * that received only one side would leave the Pages list frozen with a run
   * apparently in progress, which is why `subscribe` is a merge and not a
   * replacement.
   *
   * This test is the fallback half. The backend half is next, and the merged
   * stream under a real run is `tauri-events.test.js`.
   */
  it('passes the fallback’s events through', () => {
    const fallback = recordingFallback()
    const backend = createTauriBackend({ fallback, invoke: vi.fn() })
    const seen = []
    const unsubscribe = backend.subscribe((event) => seen.push(event))

    fallback.emit({ type: 'page-started', pageIndex: 0 })
    unsubscribe()
    fallback.emit({ type: 'page-done', pageIndex: 0 })

    expect(seen).toEqual([{ type: 'page-started', pageIndex: 0 }])
  })

  /**
   * And the backend half, through the adapter's own `subscribe` rather than
   * through `createEventStream` directly - the channel is taken from
   * `globalThis.__TAURI__.core.Channel`, which is where `withGlobalTauri` puts
   * it and therefore the only place the shipped adapter looks.
   *
   * The channel is registered at construction and stays registered: a run
   * emits for minutes with no call outstanding, so an event that arrives while
   * no component is subscribed must find the sink still there.
   */
  it('passes the backend’s events through, on a channel it keeps open', async () => {
    const opened = []
    globalThis.__TAURI__ = {
      core: {
        Channel: class {
          constructor() {
            this.onmessage = null
            opened.push(this)
          }
        },
      },
    }
    try {
      const invoke = vi.fn().mockResolvedValue(7)
      const backend = createTauriBackend({ fallback: recordingFallback(), invoke })
      expect(opened).toHaveLength(1)
      expect(invoke).toHaveBeenCalledWith('subscribe_events', { channel: opened[0] })

      const seen = []
      const unsubscribe = backend.subscribe((event) => seen.push(event))
      opened[0].onmessage({ type: 'page-started', runId: 'run-1', pageIndex: 0 })
      unsubscribe()
      await Promise.resolve()
      await Promise.resolve()

      // Detaching the only handler stops delivery and nothing else: no second
      // channel, and the backend is never told to drop the sink.
      opened[0].onmessage({ type: 'run-finished', runId: 'run-1', reason: 'completed' })
      expect(seen).toEqual([{ type: 'page-started', runId: 'run-1', pageIndex: 0 }])
      expect(opened).toHaveLength(1)
      expect(invoke).not.toHaveBeenCalledWith('unsubscribe_events', expect.anything())

      // And a handler attached afterwards is on the same channel the run holds.
      backend.subscribe((event) => seen.push(event))
      opened[0].onmessage({ type: 'run-finished', runId: 'run-1', reason: 'completed' })
      expect(seen.at(-1)).toEqual({ type: 'run-finished', runId: 'run-1', reason: 'completed' })
    } finally {
      delete globalThis.__TAURI__
    }
  })

  /**
   * The delegation loop is still there and has nothing left to serve.
   *
   * Every method the seam fixes is a command now - the four region edits were
   * the last of them (`src-tauri/src/region.rs`) - so the assertion this test can make is
   * the one that is true: the fallback is reached for exactly two things, and
   * neither is a method it *serves*. `readSettings` reaches it for the
   * interface's defaults, which the command's stored snapshot is merged over,
   * and `subscribe` reaches it because the stream is a merge.
   */
  it('serves every seam method itself, and reaches the fallback only for the defaults', async () => {
    const fallback = recordingFallback()
    const backend = createTauriBackend({ fallback, invoke: vi.fn().mockResolvedValue({}) })

    for (const method of SEAM_METHODS) {
      if (method === 'subscribe') backend.subscribe(() => {})
      else await backend[method]({})
    }

    expect([...new Set(fallback.calls.map((call) => call.method))].sort()).toEqual([
      'readSettings',
      'subscribe',
    ])
    expect(implementedMethods()).toEqual(SEAM_METHODS.filter((m) => m !== 'subscribe').sort())
  })

  it('constructed outside a Tauri window, a command rejects rather than throwing', async () => {
    const fallback = recordingFallback()
    const backend = createTauriBackend({ fallback })
    // The seam declares every method async, so being built outside a window
    // has to surface as a rejected promise like any other backend failure -
    // never as a synchronous throw out of a call site that is awaiting one.
    await expect(backend.about()).rejects.toThrow(/outside a Tauri window/)
    await expect(backend.applyTool({ tool: 'brush', regionId: 'r1' })).rejects.toThrow(
      /outside a Tauri window/,
    )
  })
})

describe('selecting a backend', () => {
  beforeEach(() => {
    setBackend(null)
    delete globalThis.__TAURI_INTERNALS__
    delete globalThis.__TAURI__
    delete globalThis.__MANGA_CLEANER_FORCE_MOCK__
  })

  it('is the mock outside a Tauri window', async () => {
    expect(isTauri()).toBe(false)
    // The mock's own projects. Outside a Tauri window there is no `invoke` to
    // reach `list_projects` with, so this is the mock answering, not the
    // adapter falling through.
    await expect(getBackend().listProjects()).resolves.toEqual(expect.any(Array))
  })

  it('is the adapter inside one', async () => {
    globalThis.__TAURI_INTERNALS__ = { invoke: vi.fn().mockResolvedValue({ appVersion: '9.9.9', facts: [] }) }
    expect(isTauri()).toBe(true)
    await expect(getBackend().about()).resolves.toEqual({ appVersion: '9.9.9', facts: [] })
  })

  it('the force-mock flag wins inside a Tauri window', async () => {
    globalThis.__TAURI_INTERNALS__ = { invoke: vi.fn() }
    globalThis.__MANGA_CLEANER_FORCE_MOCK__ = true
    const about = await getBackend().about()
    expect(about.appVersion).not.toBe('9.9.9')
    expect(globalThis.__TAURI_INTERNALS__.invoke).not.toHaveBeenCalled()
  })
})
