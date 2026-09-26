import { afterEach, describe, expect, it, vi } from 'vitest'
import { createMockBackend } from '../api/mock.js'
import { cloud, refreshCloudReadiness, stopCloud } from './cloud.svelte.js'
import { setCloudAllowed } from './session.svelte.js'

afterEach(() => {
  stopCloud()
  setCloudAllowed(false)
})

describe('cloud model metadata', () => {
  it('loads the selected endpoint model as a read-only readiness followup', async () => {
    const backend = createMockBackend({ timing: { method: 0 } })
    await backend.writeSettings({ cloudEngines: 'allowed' })
    setCloudAllowed(true)
    const endpointUrl = 'https://studio.example.test/mc/v1'
    await backend.writeInferenceConfig({ config: {
      schemaVersion: 1,
      selectedTarget: { type: 'modal', profile_id: 'studio' },
      beamProfiles: {},
      modalProfiles: { studio: {
        id: 'studio', name: 'Studio', endpointUrl,
        canonicalOrigin: new URL(endpointUrl).origin, canonicalOriginFingerprint: '',
        createdAtMs: 1, updatedAtMs: 1,
      } },
    } })
    await backend.storeCloudSecret({ provider: 'modal', profileId: 'studio', role: 'runtime', secret: 'token', tokenId: 'token' })
    const metadata = vi.spyOn(backend, 'getCloudModelInfo')

    await refreshCloudReadiness(backend)
    await vi.waitFor(() => expect(cloud.model?.id).toBe('Disty0/FLUX.2-klein-4B-SDNQ-4bit-dynamic'))
    expect(metadata).toHaveBeenCalledTimes(1)
    await refreshCloudReadiness(backend)
    expect(metadata).toHaveBeenCalledTimes(1)
  })

  it('ignores metadata from an endpoint deselected while its read was pending', async () => {
    const backend = createMockBackend({ timing: { method: 0 } })
    await backend.writeSettings({ cloudEngines: 'allowed' })
    setCloudAllowed(true)
    const profile = (id) => ({ id, name: id, endpointUrl: `https://${id}.example.test/mc/v1`,
      canonicalOrigin: `https://${id}.example.test`, canonicalOriginFingerprint: '',
      createdAtMs: 1, updatedAtMs: 1 })
    const config = (id) => ({ schemaVersion: 1, selectedTarget: { type: 'modal', profile_id: id },
      beamProfiles: {}, modalProfiles: { first: profile('first'), second: profile('second') } })
    await backend.writeInferenceConfig({ config: config('first') })
    for (const id of ['first', 'second']) {
      await backend.storeCloudSecret({ provider: 'modal', profileId: id, role: 'runtime', secret: 'token', tokenId: id })
    }
    let answerFirst
    const first = new Promise((resolve) => { answerFirst = resolve })
    vi.spyOn(backend, 'getCloudModelInfo').mockImplementation(({ profileId }) =>
      profileId === 'first' ? first : Promise.resolve({ pinnedModelId: 'Disty0/FLUX.2-klein-9B-SDNQ-4bit-dynamic-svd-r32' }))

    await refreshCloudReadiness(backend)
    await vi.waitFor(() => expect(backend.getCloudModelInfo).toHaveBeenCalledTimes(1))
    await backend.writeInferenceConfig({ config: config('second') })
    await refreshCloudReadiness(backend)
    await vi.waitFor(() => expect(cloud.model?.id).toBe('Disty0/FLUX.2-klein-9B-SDNQ-4bit-dynamic-svd-r32'))
    answerFirst({ pinnedModelId: 'Disty0/FLUX.2-klein-4B-SDNQ-4bit-dynamic' })
    await Promise.resolve()
    expect(cloud.model.id).toBe('Disty0/FLUX.2-klein-9B-SDNQ-4bit-dynamic-svd-r32')
  })
})
