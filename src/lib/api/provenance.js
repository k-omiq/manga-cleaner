/**
 * Engine metadata and mask/provenance construction, shared by the fixtures
 * and by the mock engine's live results so a fixture mask and a mask made
 * during a session are indistinguishable in shape.
 *
 * Engine names, versions and the cloud provider are taken from
 * documented engine vocabulary rather than from the prototype's invented model
 * names. `params_snapshot` carries the constants actually used by the
 * pipeline.
 */

import { RUNGS } from '../model/ladder.js'

/**
 * @typedef {Object} EngineInfo
 * @property {string} version - `Provenance.engine_version`
 * @property {string|null} modelSha - `Provenance.model_sha256`; null for the two model-free rungs
 * @property {string} provider - `Provenance.execution_provider`
 * @property {[number, number]} elapsed - inclusive millisecond range for the simulated run
 * @property {'match-surround'|'reconstruct'|'solid'} fillMode - the mode this rung produces by default
 */

/** @type {Readonly<Record<string, EngineInfo>>} */
export const ENGINE_INFO = Object.freeze({
  fill: {
    version: 'planar-fill 3',
    modelSha: null,
    provider: 'cpu',
    elapsed: [2, 14],
    fillMode: 'match-surround',
  },
  denoise: {
    version: 'denoise 3',
    modelSha: null,
    provider: 'cpu',
    elapsed: [18, 90],
    fillMode: 'match-surround',
  },
  lama: {
    version: 'lama-manga onnx opset 17',
    modelSha: 'lama',
    provider: 'cpu',
    elapsed: [1000, 3000],
    fillMode: 'reconstruct',
  },
  // Rung 3a, the optional FLUX sidecar. Never on the automatic path and
  // never bundled; a mask made by one still has to be able
  // to name what made it, and the row's picker can name it on a machine that
  // has the sidecar installed.
  flux: {
    version: 'flux2-klein-4b',
    modelSha: null,
    provider: 'sidecar',
    elapsed: [10000, 60000],
    fillMode: 'reconstruct',
  },
  cloud: {
    version: 'gemini-3.1-flash-image',
    modelSha: null,
    provider: 'cloud',
    elapsed: [3000, 15000],
    fillMode: 'reconstruct',
  },
  paint: {
    version: 'brush-paint 1',
    modelSha: null,
    provider: 'cpu',
    elapsed: [2, 14],
    fillMode: 'solid',
  },
  clone: {
    version: 'clone-heal 1',
    modelSha: null,
    provider: 'cpu',
    elapsed: [5, 20],
    fillMode: 'match-surround',
  },
})

/** The five cloud rejection causes, in doc order. */
export const CLOUD_REJECTION_CAUSES = Object.freeze([
  'safety-filter',
  'transport-error',
  'parameter-test',
  'residual-test',
  'structural',
])

/** Cloud tier and price (NB2 @1K). */
export const CLOUD_TIER = '1K'
export const CLOUD_COST = 0.067
export const CLOUD_PROVIDER = 'google'

/** Constants the pipeline actually ran with. */
const PARAMS_SNAPSHOT = Object.freeze({
  min_mask_thickness: 4,
  mask_growth_step: 2,
  annulus_offset: [1, 5],
  mask_deviation_max: 8,
  denoise_trigger: 0.3,
  isolation_radius: 5,
  edit_margin: 6,
})

/**
 * The rung a region routes to, given a ceiling. Deterministic in the region
 * id, so the same region always comes back on the same rung - a re-run is a
 * re-run, not a dice roll.
 *
 * @param {number} hash - `hashString(regionId)`
 * @param {string} ceiling - highest rung permitted, a member of `RUNGS`
 * @returns {string} rung id
 */
export function routeRung(hash, ceiling) {
  const ceilingIndex = Math.max(0, RUNGS.indexOf(ceiling))
  // Most regions are flat paper and belong on rung 0.
  // Only a few reach the inpainter, because every one of those is a review
  // entry and the flag rate is capped at 5% of boxes.
  const bucket = hash % 32
  let index = 0
  if (bucket === 31 && ceilingIndex >= 4) index = 4
  else if (bucket >= 29) index = 2
  else if (bucket >= 22) index = 1
  return RUNGS[Math.min(index, ceilingIndex)]
}

/**
 * Caps a requested rung at a ceiling. Never raises it: a ceiling is an upper
 * bound on what a run may reach, not an instruction to reach it.
 *
 * @param {string} requested
 * @param {string} ceiling
 * @returns {string} rung id, the lower of the two
 */
export function capRung(requested, ceiling) {
  const top = RUNGS.length - 1
  const requestedIndex = RUNGS.indexOf(requested)
  const ceilingIndex = RUNGS.indexOf(ceiling)
  return RUNGS[
    Math.min(requestedIndex === -1 ? top : requestedIndex, ceilingIndex === -1 ? top : ceilingIndex)
  ]
}

/**
 * @typedef {Object} MaskSpec
 * @property {string} regionId
 * @property {number} sequence - monotonic revision number
 * @property {string} engine - rung id the mask's provenance records
 * @property {import('./rng.js').Rng} rng - source of the simulated hashes and elapsed time
 * @property {string} created - ISO 8601 timestamp
 * @property {string} sourceSha - the page's `source_sha256`
 * @property {'match-surround'|'reconstruct'|'solid'} [fillMode]
 * @property {number} [elapsedMs] - overrides the engine's simulated range
 * @property {boolean} [fittingReconstructed]
 * @property {import('../model/types.js').CloudOutcome|null} [cloudOutcome]
 * @property {boolean} [cloudBilled] - record a `Provenance.cloud` block (an accepted request)
 */

/**
 * Builds a `Mask` with a complete `Provenance` record.
 *
 * @param {MaskSpec} spec
 * @returns {import('../model/types.js').Mask}
 */
export function buildMask(spec) {
  const info = ENGINE_INFO[spec.engine] ?? ENGINE_INFO.fill
  const elapsedMs = spec.elapsedMs ?? spec.rng.int(info.elapsed[0], info.elapsed[1])
  const params_snapshot = { ...PARAMS_SNAPSHOT }
  if (spec.tool) {
    params_snapshot.tool = spec.tool
  }
  if (spec.source) {
    params_snapshot.source = spec.source
  }
  if (spec.fillMode) {
    params_snapshot.fill_mode = spec.fillMode
  }
  return {
    id: `${spec.regionId}-m${spec.sequence}`,
    regionId: spec.regionId,
    sequence: spec.sequence,
    fillMode: spec.fillMode ?? info.fillMode,
    elapsedMs,
    fittingReconstructed: spec.fittingReconstructed ?? false,
    cloudOutcome: spec.cloudOutcome ?? null,
    provenance: {
      engine: spec.engine,
      engine_version: info.version,
      model_sha256: info.modelSha ? spec.rng.sha256() : null,
      execution_provider: info.provider,
      params_snapshot,
      mask_sha256: spec.rng.sha256(),
      source_sha256: spec.sourceSha,
      cloud: spec.cloudBilled
        ? {
            provider: CLOUD_PROVIDER,
            model: ENGINE_INFO.cloud.version,
            request_id: `req-${spec.rng.sha256().slice(0, 16)}`,
            tier: CLOUD_TIER,
            cost: CLOUD_COST,
          }
        : null,
      created: spec.created,
    },
  }
}

/**
 * A context that can commit a mask. Fixture construction and a live session
 * differ only in where the revision number and the timestamp come from -
 * `pagebuilder.js` counts from a fixed epoch so launches stay byte-identical,
 * the mock engine reads its injected clock - so they expose the same shape and
 * share `commitMask`.
 *
 * @typedef {Object} CommitContext
 * @property {import('./rng.js').Rng} rng
 * @property {() => number} nextSequence - next mask revision number
 * @property {() => string} created - ISO 8601 timestamp for `Provenance.created`
 */

/**
 * Commits a cleaned mask to a region. The single definition of what cleaning
 * a region resets: the outcome becomes `cleaned`, and the gate-skip and
 * decline records are cleared, because a region that now has a mask was
 * neither skipped nor declined.
 *
 * @param {import('../model/types.js').Region} region
 * @param {CommitContext} ctx
 * @param {Partial<MaskSpec> & { tool?: string, source?: string }} spec - engine and any overrides passed to `buildMask`
 * @returns {import('../model/types.js').Mask} the committed mask
 */
export function commitMask(region, ctx, spec) {
  region.outcome = 'cleaned'
  region.gateSkipCause = null
  region.declineReason = null
  if (spec.tool) {
    region.tool = spec.tool
  }
  region.mask = buildMask({
    regionId: region.id,
    sequence: ctx.nextSequence(),
    rng: ctx.rng,
    created: ctx.created(),
    sourceSha: region.sourceSha ?? '',
    tool: spec.tool ?? region.tool,
    source: spec.source ?? region.source,
    ...spec,
  })
  return region.mask
}
