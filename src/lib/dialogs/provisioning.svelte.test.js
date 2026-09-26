import { expect, it, vi } from 'vitest'
import { removeEndpoint } from './provisioning.svelte.js'

vi.mock('../state/cloud.svelte.js', () => ({ refreshCloudReadiness: vi.fn() }))

function backend() {
  const calls = []
  let config = { modalProfiles: { studio: { id: 'studio', endpointUrl: 'https://example.modal.run/mc/v1' } }, beamProfiles: {}, selectedTarget: { type: 'modal', profile_id: 'studio' } }
  return {
    calls,
    readInferenceConfig: async () => config,
    deleteCloudSecret: vi.fn(async ({ role }) => {
      expect(config.modalProfiles.studio).toBeTruthy()
      calls.push(role)
    }),
    writeInferenceConfig: vi.fn(async ({ config: next }) => { calls.push('config'); config = next }),
  }
}

it('forgets runtime and billing credentials before removing their origin-bound profile', async () => {
  const api = backend()
  expect(await removeEndpoint({ provider: 'modal', profileId: 'studio' }, api)).toBe(true)
  expect(api.calls).toEqual(['runtime', 'setup', 'config'])
  expect((await api.readInferenceConfig()).selectedTarget).toEqual({ type: 'local' })
})

it('keeps the profile accessible when credential removal fails', async () => {
  const api = backend()
  api.deleteCloudSecret.mockRejectedValue(new Error('keychain unavailable'))
  await expect(removeEndpoint({ provider: 'modal', profileId: 'studio' }, api)).rejects.toThrow('keychain unavailable')
  expect(api.writeInferenceConfig).not.toHaveBeenCalled()
  expect((await api.readInferenceConfig()).modalProfiles.studio).toBeTruthy()
})
