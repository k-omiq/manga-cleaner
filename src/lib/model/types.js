/**
 * Domain model typedefs shared across `src/lib/model`.
 *
 * This describes the in-memory shape the UI reads and Task 3's mock engine
 * produces. It is close to, but distinct from, the persisted `.mtclean` job
 * manifest: the manifest is a flat list of
 * `patches` and `regions_untouched` keyed by `source_idx`, written for one
 * job. This model groups the same information under the Projects → Chapters
 * → Pages → Regions hierarchy the UI needs (Home is a two-level hierarchy by
 * user ruling; a project owns chapters, a chapter owns pages).
 *
 * `Provenance` mirrors the persisted record's field names verbatim
 * so a patch's provenance round-trips without translation.
 *
 * No runtime code lives here - typedefs only, imported by JSDoc comments
 * elsewhere via `import('./types.js').Foo`.
 */

/**
 * A patch's reproducibility record. Every field name matches
 * the persisted record exactly.
 *
 * @typedef {Object} Provenance
 * @property {string} engine - rung id that produced this mask, e.g. 'lama'
 * @property {string} engine_version
 * @property {string|null} model_sha256
 * @property {string} execution_provider
 * @property {Record<string, unknown>} params_snapshot - thresholds, dilation, radii actually used
 * @property {string} mask_sha256
 * @property {string} source_sha256
 * @property {CloudProvenance|null} cloud
 * @property {string} created - ISO 8601 timestamp
 */

/**
 * @typedef {Object} CloudProvenance
 * @property {string} provider
 * @property {string} model
 * @property {string} request_id
 * @property {string} tier
 * @property {number} cost
 */

/**
 * Pipeline telemetry for a mask attempt that reached the cloud rung,
 * independent of `Provenance.cloud` - this is set even when the attempt was
 * rejected and the mask's final provenance records the rung-2 fallback
 * which is exactly the case review needs to surface.
 *
 * @typedef {Object} CloudOutcome
 * @property {boolean} accepted
 * @property {'safety-filter'|'transport-error'|'parameter-test'|'residual-test'|'structural'|null} rejectionCause
 */

/**
 * One cleaning attempt on a Region. Hand-drawn and automatic masks are the
 * same shape - there is no separate "manual mask" type.
 *
 * @typedef {Object} Mask
 * @property {string} id
 * @property {string} regionId
 * @property {number} sequence - monotonic per-region revision number; higher is newer
 * @property {'match-surround'|'reconstruct'|'solid'} fillMode
 * @property {number} elapsedMs
 * @property {boolean} fittingReconstructed - planar fit failed and a model reconstructed the area
 * @property {CloudOutcome|null} cloudOutcome
 * @property {Provenance} provenance
 */

/**
 * A detected (or hand-drawn) text area on a page. `mask` is the current /
 * latest revision; earlier revisions are not kept here (never silently
 * overwritten - but retaining history is a
 * concern for the persistence layer, not this in-memory shape).
 *
 * @typedef {Object} Region
 * @property {string} id
 * @property {string} pageId
 * @property {{x: number, y: number, w: number, h: number}} bbox
 * @property {'auto'|'hand'} source
 * @property {'pending'|'cleaned'|'declined'|'gate-skipped'} outcome
 * @property {'low-confidence'|'outside-bubble'|'not-japanese'|null} gateSkipCause - `not-japanese` is a *confident* refusal: the gate read the script and it was not CJK
 * @property {string|null} declineReason
 * @property {boolean} unusuallyLarge
 * @property {Mask|null} mask - null unless outcome === 'cleaned'
 */

/**
 * @typedef {Object} Page
 * @property {string} id
 * @property {string} chapterId
 * @property {number} index - 0-based position within the chapter / strip
 * @property {'unclean'|'cleaning'|'cleaned'|'skipped'} status
 * @property {string|null} skipReason
 * @property {Region[]} regions
 */

/**
 * @typedef {Object} Chapter
 * @property {string} id
 * @property {string} projectId
 * @property {string} name
 * @property {number} order
 * @property {Page[]} pages
 */

/**
 * @typedef {Object} Project
 * @property {string} id
 * @property {string} name
 * @property {'single'|'longstrip'} mode
 * @property {'rtl'|'ltr'} readingDirection - per-project, RTL by default
 * @property {string} created
 * @property {string} appVersion
 * @property {Chapter[]} chapters
 */

export {}
