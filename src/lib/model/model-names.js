/** Product names shared by memory reporting, cloud provenance and engine selectors. */
const MODEL_NAMES = Object.freeze({
    'flux2-klein-4b': 'FLUX.2 Klein 4B',
    'flux2-klein-9b': 'FLUX.2 Klein 9B',
    'flux1-schnell': 'FLUX.1 [schnell]',
    'flux1-dev': 'FLUX.1 [dev]',
    'lama-manga': 'LaMa Manga',
    'sam-ts-l': 'SAM-TS-L',
    'Disty0/FLUX.2-klein-4B-SDNQ-4bit-dynamic': 'FLUX.2 Klein 4B',
    'Disty0/FLUX.2-klein-9B-SDNQ-4bit-dynamic-svd-r32': 'FLUX.2 Klein 9B',
  })

/** Unknown IDs do not get a guessed product name in a user-facing engine choice. */
export function knownModelName(id) {
  return typeof id === 'string' ? MODEL_NAMES[id] ?? null : null
}

export function displayModelName(id) {
  return knownModelName(id) ?? id
}

/** The current endpoint's verified ID, if the endpoint or a prior consent supplied one. */
export function currentCloudModelId(cloud) {
  const readiness = cloud?.readiness
  const profile = readiness?.profile
  const explicit = profile?.modelId ?? profile?.model_id
  if (typeof explicit === 'string' && explicit) return explicit
  const target = readiness?.target
  const remembered = cloud?.model
  return remembered && target?.type === remembered.provider && target?.profile_id === remembered.profileId &&
    (profile?.updatedAtMs ?? null) === (remembered.updatedAtMs ?? null)
    ? remembered.id
    : null
}

/** Put the model among local engines while making its remote execution visible. */
export function engineModelLabel(engine, modelId, fallback) {
  const name = knownModelName(modelId)
  if (engine === 'cloud') return name ? `☁ ${name} · Cloud` : `☁ ${fallback}`
  if (engine === 'flux') return name ? `${name} · Local` : fallback
  return fallback
}
