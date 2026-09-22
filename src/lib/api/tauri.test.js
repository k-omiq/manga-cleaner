import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import {
  createTauriBackend,
  implementedMethods,
  isCloudExecutionReady as isTauriCloudExecutionReady,
  isCloudExecutionRegistered,
  isTauri,
  SEAM_METHODS,
  TAURI_PENDING_CLOUD_COMMANDS,
  TAURI_REGISTERED_CLOUD_COMMANDS,
} from './tauri.js'
import {
  getBackend,
  isCloudExecutionReady,
  isCloudExecutionRegistered as isBackendCloudExecutionRegistered,
  setBackend,
} from './backend.js'

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

  describe('inference and cloud secrets command mapping', () => {
    it('maps readInferenceConfig to read_inference_config command', async () => {
      const invoke = vi.fn().mockResolvedValue({
        schemaVersion: 1,
        selectedTarget: { type: 'local' },
        beamProfiles: {},
        modalProfiles: {},
      })
      const fallback = recordingFallback()
      const backend = createTauriBackend({ fallback, invoke })

      const result = await backend.readInferenceConfig()
      expect(invoke).toHaveBeenCalledWith('read_inference_config')
      expect(result).toEqual({
        schemaVersion: 1,
        selectedTarget: { type: 'local' },
        beamProfiles: {},
        modalProfiles: {},
      })
      expect(fallback.calls.some((c) => c.method === 'readInferenceConfig')).toBe(false)
    })

    it('maps writeInferenceConfig to write_inference_config command with { config } argument', async () => {
      const config = {
        schemaVersion: 1,
        selectedTarget: { type: 'beam', profile_id: 'beam-prod' },
        beamProfiles: {
          'beam-prod': {
            id: 'beam-prod',
            name: 'Beam Prod',
            endpointUrl: 'https://api.beam.cloud/ep',
            canonicalOrigin: 'https://api.beam.cloud',
            canonicalOriginFingerprint: 'fp123',
            createdAtMs: 100,
            updatedAtMs: 200,
          },
        },
        modalProfiles: {},
      }
      const invoke = vi.fn().mockResolvedValue(config)
      const fallback = recordingFallback()
      const backend = createTauriBackend({ fallback, invoke })

      const result = await backend.writeInferenceConfig({ config })
      expect(invoke).toHaveBeenCalledWith('write_inference_config', { config })
      expect(result).toEqual(config)
      expect(JSON.stringify(result)).not.toContain('secret')
      expect(fallback.calls.some((c) => c.method === 'writeInferenceConfig')).toBe(false)
    })

    it('maps storeCloudSecret with exact parameter structure', async () => {
      const summary = {
        provider: 'beam',
        profileId: 'beam-prod',
        role: 'runtime',
        present: true,
        backend: 'keyring',
      }
      const invoke = vi.fn().mockResolvedValue(summary)
      const fallback = recordingFallback()
      const backend = createTauriBackend({ fallback, invoke })

      const result = await backend.storeCloudSecret({
        provider: 'beam',
        profileId: 'beam-prod',
        role: 'runtime',
        secret: 'raw-secret-token-value',
        sessionOnly: false,
      })

      expect(invoke).toHaveBeenCalledWith('store_cloud_secret', {
        provider: 'beam',
        profileId: 'beam-prod',
        role: 'runtime',
        secret: 'raw-secret-token-value',
        sessionOnly: false,
      })
      expect(result).toEqual(summary)
      expect(result).not.toHaveProperty('secret')
      expect(fallback.calls.some((c) => c.method === 'storeCloudSecret')).toBe(false)
    })

    it('maps storeCloudSecret with tokenId for modal runtime', async () => {
      const summary = {
        provider: 'modal',
        profileId: 'modal-prod',
        role: 'runtime',
        present: true,
        backend: 'keyring',
      }
      const invoke = vi.fn().mockResolvedValue(summary)
      const fallback = recordingFallback()
      const backend = createTauriBackend({ fallback, invoke })

      const result = await backend.storeCloudSecret({
        provider: 'modal',
        profileId: 'modal-prod',
        role: 'runtime',
        secret: 'raw-secret-token-value',
        tokenId: 'ak-genuine-token-id',
        sessionOnly: false,
      })

      expect(invoke).toHaveBeenCalledWith('store_cloud_secret', {
        provider: 'modal',
        profileId: 'modal-prod',
        role: 'runtime',
        secret: 'raw-secret-token-value',
        tokenId: 'ak-genuine-token-id',
        sessionOnly: false,
      })
      expect(result).toEqual(summary)
      expect(result).not.toHaveProperty('secret')
      expect(result).not.toHaveProperty('tokenId')
      expect(fallback.calls.some((c) => c.method === 'storeCloudSecret')).toBe(false)
    })

    it('maps deleteCloudSecret with exact parameter structure', async () => {
      const summary = {
        provider: 'modal',
        profileId: 'modal-dev',
        role: 'setup',
        present: false,
        backend: 'keyring',
      }
      const invoke = vi.fn().mockResolvedValue(summary)
      const fallback = recordingFallback()
      const backend = createTauriBackend({ fallback, invoke })

      const result = await backend.deleteCloudSecret({
        provider: 'modal',
        profileId: 'modal-dev',
        role: 'setup',
      })

      expect(invoke).toHaveBeenCalledWith('delete_cloud_secret', {
        provider: 'modal',
        profileId: 'modal-dev',
        role: 'setup',
      })
      expect(result).toEqual(summary)
      expect(fallback.calls.some((c) => c.method === 'deleteCloudSecret')).toBe(false)
    })

    it('maps getCloudSecretSummary with exact parameter structure', async () => {
      const summary = {
        provider: 'modal',
        profileId: 'modal-dev',
        role: 'model_download',
        present: true,
        backend: 'session',
      }
      const invoke = vi.fn().mockResolvedValue(summary)
      const fallback = recordingFallback()
      const backend = createTauriBackend({ fallback, invoke })

      const result = await backend.getCloudSecretSummary({
        provider: 'modal',
        profileId: 'modal-dev',
        role: 'model_download',
      })

      expect(invoke).toHaveBeenCalledWith('get_cloud_secret_summary', {
        provider: 'modal',
        profileId: 'modal-dev',
        role: 'model_download',
      })
      expect(result).toEqual(summary)
      expect(fallback.calls.some((c) => c.method === 'getCloudSecretSummary')).toBe(false)
    })

    it('propagates rejected invoke errors without falling back to mock for all 5 methods', async () => {
      const invoke = vi.fn().mockRejectedValue(new Error('Tauri command failure'))
      const fallback = recordingFallback()
      const backend = createTauriBackend({ fallback, invoke })

      await expect(backend.readInferenceConfig()).rejects.toThrow('Tauri command failure')
      await expect(
        backend.writeInferenceConfig({
          schemaVersion: 1,
          selectedTarget: { type: 'local' },
          beamProfiles: {},
          modalProfiles: {},
        }),
      ).rejects.toThrow('Tauri command failure')
      await expect(
        backend.storeCloudSecret({
          provider: 'beam',
          profileId: 'p1',
          role: 'runtime',
          secret: 's1',
        }),
      ).rejects.toThrow('Tauri command failure')
      await expect(
        backend.deleteCloudSecret({
          provider: 'beam',
          profileId: 'p1',
          role: 'runtime',
        }),
      ).rejects.toThrow('Tauri command failure')
      await expect(
        backend.getCloudSecretSummary({
          provider: 'beam',
          profileId: 'p1',
          role: 'runtime',
        }),
      ).rejects.toThrow('Tauri command failure')

      // Assert that none of the 5 methods fell back to fallback mock implementation
      expect(fallback.calls.filter((c) =>
        [
          'readInferenceConfig',
          'writeInferenceConfig',
          'storeCloudSecret',
          'deleteCloudSecret',
          'getCloudSecretSummary',
        ].includes(c.method),
      )).toHaveLength(0)
    })
  })

  describe('cloud lifecycle command mapping and registration', () => {
    it('maps prepareCloudConsent with exact parameters', async () => {
      const proposal = {
        proposalId: 'prop-123',
        profileId: 'beam-prod',
        provider: 'beam',
        endpointUrl: 'https://api.beam.cloud/ep',
        canonicalOriginFingerprint: 'fp123',
        profileEpoch: 1,
        cropSha256: 'crop123',
        hintSha256: 'hint123',
        sourceHash: 'src123',
        maskHash: 'mask123',
        regionRevision: 1,
        rect: { x: 0, y: 0, w: 100, h: 100 },
        recipe: { recipeId: 'sdnq-v1', preprocessingVersion: '1.0.0', modelId: 'flux', modelRevision: 'rev', nativeMaskConditioning: false },
        intent: { action: 'cleanAnyway' },
        createdAtMs: 1000,
        expiresAtMs: 2000,
      }
      const invoke = vi.fn().mockResolvedValue(proposal)
      const fallback = recordingFallback()
      const backend = createTauriBackend({ fallback, invoke })

      const req = {
        target: { type: 'beam', profile_id: 'beam-prod' },
        intent: { action: 'cleanAnyway' },
        chapterId: 'ch-1',
        pageIndex: 0,
        regionId: 'reg-1',
      }
      const res = await backend.prepareCloudConsent(req)
      expect(invoke).toHaveBeenCalledWith('prepare_cloud_consent', req)
      expect(res).toEqual(proposal)
      expect(fallback.calls.some((c) => c.method === 'prepareCloudConsent')).toBe(false)
    })

    it('maps confirmCloudConsent with exact parameters', async () => {
      const grant = {
        nonce: 'grant-nonce-xyz',
        scope: {
          provider: 'beam',
          profileId: 'beam-prod',
          endpointFingerprint: 'fp123',
          cropSha256: 'crop123',
          maskHash: 'mask123',
          revision: 1,
          recipe: { recipeId: 'sdnq-v1', preprocessingVersion: '1.0.0', modelId: 'flux', modelRevision: 'rev', nativeMaskConditioning: false },
          operationDigest: 'opdig123',
        },
        issuedAtMs: 1000,
        expiresAtMs: 2000,
        allowedAttempts: 1,
        usedAttempts: 0,
      }
      const invoke = vi.fn().mockResolvedValue(grant)
      const fallback = recordingFallback()
      const backend = createTauriBackend({ fallback, invoke })

      const req = {
        proposalId: 'prop-123',
        intent: { action: 'cleanAnyway' },
      }
      const res = await backend.confirmCloudConsent(req)
      expect(invoke).toHaveBeenCalledWith('confirm_cloud_consent', req)
      expect(res).toEqual(grant)
      expect(fallback.calls.some((c) => c.method === 'confirmCloudConsent')).toBe(false)
    })

    it('maps submitCloudAttempt with exact parameters', async () => {
      const submission = {
        attemptId: 'att-123',
        handle: 'h-456',
        status: 'accepted',
        requestDigest: 'reqdig123',
        autoRetryable: false,
      }
      const invoke = vi.fn().mockResolvedValue(submission)
      const fallback = recordingFallback()
      const backend = createTauriBackend({ fallback, invoke })

      const req = {
        attemptId: 'att-123',
        grantNonce: 'grant-nonce-xyz',
        proposalId: 'prop-123',
      }
      const res = await backend.submitCloudAttempt(req)
      expect(invoke).toHaveBeenCalledWith('submit_cloud_attempt', req)
      expect(res).toEqual(submission)
      expect(fallback.calls.some((c) => c.method === 'submitCloudAttempt')).toBe(false)
    })

    it('maps getCloudAttemptStatus with exact parameters', async () => {
      const status = {
        attemptId: 'att-123',
        handle: 'h-456',
        status: 'completed',
        reportedCostUsd: 0.002,
        acknowledged: true,
      }
      const invoke = vi.fn().mockResolvedValue(status)
      const fallback = recordingFallback()
      const backend = createTauriBackend({ fallback, invoke })

      const req = { attemptId: 'att-123', handle: 'h-456' }
      const res = await backend.getCloudAttemptStatus(req)
      expect(invoke).toHaveBeenCalledWith('get_cloud_attempt_status', req)
      expect(res).toEqual(status)
      expect(fallback.calls.some((c) => c.method === 'getCloudAttemptStatus')).toBe(false)
    })

    it('maps getCloudAttemptResult with exact parameters', async () => {
      const result = {
        attemptId: 'att-123',
        handle: 'h-456',
        resultDigest: 'resdig123',
        reportedCostUsd: 0.002,
        width: 100,
        height: 100,
        cached: true,
      }
      const invoke = vi.fn().mockResolvedValue(result)
      const fallback = recordingFallback()
      const backend = createTauriBackend({ fallback, invoke })

      const req = { attemptId: 'att-123', handle: 'h-456' }
      const res = await backend.getCloudAttemptResult(req)
      expect(invoke).toHaveBeenCalledWith('get_cloud_attempt_result', req)
      expect(res).toEqual(result)
      expect(fallback.calls.some((c) => c.method === 'getCloudAttemptResult')).toBe(false)
    })

    it('maps cancelCloudAttempt with exact parameters', async () => {
      const cancelRes = {
        handle: 'h-456',
        status: 'cancel_requested',
        acknowledged: true,
      }
      const invoke = vi.fn().mockResolvedValue(cancelRes)
      const fallback = recordingFallback()
      const backend = createTauriBackend({ fallback, invoke })

      const req = { attemptId: 'att-123', handle: 'h-456' }
      const res = await backend.cancelCloudAttempt(req)
      expect(invoke).toHaveBeenCalledWith('cancel_cloud_attempt', req)
      expect(res).toEqual(cancelRes)
      expect(fallback.calls.some((c) => c.method === 'cancelCloudAttempt')).toBe(false)
    })

    it('maps reconcileCloudRecovery with exact parameters and defaults', async () => {
      const recovery = {
        decision: 'resume_polling',
        attemptId: 'att-123',
        handle: 'h-456',
        autoRetryable: false,
      }
      const invoke = vi.fn().mockResolvedValue(recovery)
      const fallback = recordingFallback()
      const backend = createTauriBackend({ fallback, invoke })

      const res1 = await backend.reconcileCloudRecovery({ attemptId: 'att-123' })
      expect(invoke).toHaveBeenCalledWith('reconcile_cloud_recovery', { attemptId: 'att-123' })
      expect(res1).toEqual(recovery)

      const res2 = await backend.reconcileCloudRecovery()
      expect(invoke).toHaveBeenCalledWith('reconcile_cloud_recovery', {})
      expect(res2).toEqual(recovery)
      expect(fallback.calls.some((c) => c.method === 'reconcileCloudRecovery')).toBe(false)
    })

    it('maintains semantic split between registration truth and execution readiness', () => {
      // Registration truth: all 14 remote execution lifecycle commands are registered in Tauri
      expect(isCloudExecutionRegistered()).toBe(true)
      expect(isBackendCloudExecutionRegistered()).toBe(true)
      expect(TAURI_PENDING_CLOUD_COMMANDS).toEqual([])
      expect(TAURI_REGISTERED_CLOUD_COMMANDS).toEqual([
        'read_inference_config',
        'write_inference_config',
        'store_cloud_secret',
        'delete_cloud_secret',
        'get_cloud_secret_summary',
        'check_cloud_connection',
        'get_cloud_model_info',
        'prepare_cloud_consent',
        'confirm_cloud_consent',
        'submit_cloud_attempt',
        'get_cloud_attempt_status',
        'get_cloud_attempt_result',
        'cancel_cloud_attempt',
        'reconcile_cloud_recovery',
      ])

      // Execution readiness: must remain false until an authoritative backend capability exists
      // Asserts that caller-supplied attestation and forged truthy objects cannot bypass readiness
      expect(isCloudExecutionReady()).toBe(false)
      expect(isTauriCloudExecutionReady()).toBe(false)
      expect(isCloudExecutionReady(null)).toBe(false)
      expect(isCloudExecutionReady(undefined)).toBe(false)
      expect(isCloudExecutionReady({})).toBe(false)
      expect(isCloudExecutionReady(true)).toBe(false)
      expect(isTauriCloudExecutionReady(true)).toBe(false)
      expect(isCloudExecutionReady({ stagingAvailable: true })).toBe(false)
      expect(isCloudExecutionReady({ runtimeAvailable: true })).toBe(false)
      expect(isCloudExecutionReady({ integrationReady: true })).toBe(false)
      expect(isCloudExecutionReady({ stagingAvailable: true, runtimeAvailable: true })).toBe(false)
      expect(isCloudExecutionReady({ livePrerequisitesMet: true })).toBe(false)
      expect(isTauriCloudExecutionReady({ livePrerequisitesMet: true })).toBe(false)
      expect(
        isCloudExecutionReady({
          stagingAvailable: true,
          runtimeAvailable: true,
          integrationReady: true,
        }),
      ).toBe(false)
      expect(
        isTauriCloudExecutionReady({
          stagingAvailable: true,
          runtimeAvailable: true,
          integrationReady: true,
        }),
      ).toBe(false)
      expect(
        isCloudExecutionReady({
          modalStaging: true,
          beamStaging: true,
          runtimeAvailable: true,
          integrationReady: true,
        }),
      ).toBe(false)
      expect(
        isTauriCloudExecutionReady({
          modalStaging: true,
          beamStaging: true,
          runtimeAvailable: true,
          integrationReady: true,
        }),
      ).toBe(false)
    })
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
