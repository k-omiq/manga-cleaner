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
 * @typedef {'beam' | 'modal'} CloudProvider
 */

/**
 * @typedef {{ type: 'local' } | { type: 'beam', profile_id: string } | { type: 'modal', profile_id: string }} ExecutionTarget
 */

/**
 * @typedef {Object} RenderRecipe
 * @property {string} recipe_id
 * @property {string} preprocessing_version
 * @property {string} model_id
 * @property {string} model_revision
 * @property {boolean} native_mask_conditioning
 */

/**
 * @typedef {Object} CloudProvenance
 * @property {string} provider
 * @property {string} [profile_id]
 * @property {string} [job_id]
 * @property {string} request_id
 * @property {string} [attempt_id]
 * @property {string} [recipe_id]
 * @property {string} model
 * @property {string} [model_revision]
 * @property {string} [tier]
 * @property {number|null} [cost]
 * @property {number} [duration_ms]
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
 * Operation intent bound to backend authorization proposals.
 *
 * @typedef {Object} OperationIntent
 * @property {'applyTool'|'createRegion'|'rerunMask'|'cleanAnyway'} action
 * @property {string} [tool]
 * @property {Record<string, unknown>} [params]
 * @property {string} [maskId]
 * @property {string} [kind]
 * @property {string} [engine]
 * @property {string} [regionId]
 */

/**
 * Authoritative backend consent proposal prepared before spend/transmission confirmation.
 *
 * @typedef {Object} ConsentProposal
 * @property {string} proposalId
 * @property {string} profileId
 * @property {CloudProvider} provider
 * @property {string} endpointUrl
 * @property {string} canonicalOriginFingerprint
 * @property {number} profileEpoch
 * @property {string} cropSha256
 * @property {string} hintSha256
 * @property {string} sourceHash
 * @property {string} maskHash
 * @property {string|number} regionRevision
 * @property {{x: number, y: number, w: number, h: number}} rect
 * @property {RenderRecipe} recipe
 * @property {OperationIntent} intent
 * @property {number} createdAtMs
 * @property {number} expiresAtMs
 * @property {number|null} estimatedCostUsd
 */

/**
 * Scoped authorization bounds minted upon proposal confirmation.
 *
 * @typedef {Object} GrantScope
 * @property {CloudProvider} provider
 * @property {string} profileId
 * @property {string} endpointFingerprint
 * @property {string} cropSha256
 * @property {string} maskHash
 * @property {string|number} revision
 * @property {RenderRecipe} recipe
 * @property {string} operationDigest
 */

/**
 * Backend-issued, attempt-limited authorization grant.
 *
 * @typedef {Object} Grant
 * @property {string} nonce
 * @property {GrantScope} scope
 * @property {number} issuedAtMs
 * @property {number} expiresAtMs
 * @property {number} allowedAttempts
 * @property {number} usedAttempts
 */

/**
 * Safe public connection check result (ordinary reachability, never triggers GPU work).
 *
 * @typedef {Object} CloudConnectionStatus
 * @property {boolean} ok
 * @property {string} status
 * @property {CloudProvider} provider
 * @property {string} profileId
 * @property {number} [latencyMs]
 * @property {string} [message]
 */

/**
 * Wire model metadata and provisional limits.
 *
 * @typedef {Object} CloudModelInfo
 * @property {string} supportedProtocolVersion
 * @property {string} pinnedModelId
 * @property {string} pinnedModelRevision
 * @property {string} pinnedRecipeId
 * @property {Object} limits
 * @property {[number, number]} limits.maxDimensions
 * @property {number} limits.maxMegapixels
 * @property {number} limits.maxPngBytes
 * @property {number} limits.maxMultipartBytes
 * @property {number} limits.defaultWorkerDeadlineSec
 */

/**
 * Submission response for a durable remote attempt.
 *
 * @typedef {Object} CloudAttemptSubmission
 * @property {string} attemptId
 * @property {string|null} [handle]
 * @property {'accepted'|'dispatching'|'unknown'} status
 * @property {string} [requestDigest]
 * @property {boolean} [autoRetryable]
 * @property {string} [error]
 */

/**
 * Authoritative lifecycle status for an accepted remote attempt.
 *
 * @typedef {Object} CloudAttemptStatus
 * @property {string} attemptId
 * @property {string|null} handle
 * @property {'pending'|'running'|'completed'|'failed'|'cancelled'|'cancel_requested'|'unknown'} status
 * @property {number|null} reportedCostUsd
 * @property {boolean} [acknowledged]
 * @property {number} [createdAtMs]
 * @property {number} [startedAtMs]
 * @property {number} [finishedAtMs]
 */

/**
 * Validated output crop result retrieved from local cache or remote gateway.
 *
 * @typedef {Object} CloudAttemptResult
 * @property {string} attemptId
 * @property {string} handle
 * @property {string} resultDigest
 * @property {number|null} reportedCostUsd
 * @property {number} width
 * @property {number} height
 * @property {boolean} cached
 */

/**
 * Cancellation request acknowledgement.
 *
 * @typedef {Object} CloudCancelResult
 * @property {string} handle
 * @property {'cancel_requested'|'cancelled'} status
 * @property {boolean} acknowledged
 */

/**
 * Recovery decider outcome for uncommitted or interrupted attempts.
 *
 * @typedef {Object} CloudRecoveryDecision
 * @property {'ambiguous_unknown'|'resume_polling'|'resume_cancel_polling'|'result_cached_ready'|'attachment_pending'|'stale_attachment'|'already_committed'|'terminal'} decision
 * @property {string} [attemptId]
 * @property {string|null} [handle]
 * @property {string} [resultDigest]
 * @property {string} [patchId]
 * @property {boolean} [autoRetryable]
 * @property {string} [message]
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
