import { describe, expect, it } from 'vitest'
import { currentCloudModelId, engineModelLabel, knownModelName } from './model-names.js'

const FOUR = 'Disty0/FLUX.2-klein-4B-SDNQ-4bit-dynamic'
const NINE = 'Disty0/FLUX.2-klein-9B-SDNQ-4bit-dynamic-svd-r32'

describe('engine model names', () => {
  it('uses exact pinned cloud IDs and marks remote execution', () => {
    expect(knownModelName(FOUR)).toBe('FLUX.2 Klein 4B')
    expect(engineModelLabel('cloud', NINE, 'Cloud')).toBe('☁ FLUX.2 Klein 9B · Cloud')
    expect(engineModelLabel('cloud', 'unrecognized', 'Cloud')).toBe('☁ Cloud')
    expect(engineModelLabel('flux', 'flux2-klein-4b', 'FLUX')).toBe('FLUX.2 Klein 4B · Local')
  })

  it('does not use metadata from a different endpoint', () => {
    const state = {
      readiness: { target: { type: 'modal', profile_id: 'new' }, profile: null },
      model: { id: FOUR, provider: 'modal', profileId: 'old' },
    }
    expect(currentCloudModelId(state)).toBeNull()
    state.model.profileId = 'new'
    expect(currentCloudModelId(state)).toBe(FOUR)
    state.readiness.profile = { modelId: NINE }
    expect(currentCloudModelId(state)).toBe(NINE)
  })
})
