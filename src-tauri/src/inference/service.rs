//! Coherent cloud inference service coordinating consent, credentials, HTTP transport,
//! attempt durability, and project attachment (P3/P4).
//!
//! Provides the top-level orchestration layer for user-owned cloud diffusion rendering,
//! ensuring strict backend gating, honest nullable cost provenance, tamper-evident authorization,
//! non-terminal cancellation, and transactional crash recovery.
//!
//! ## Invariants
//!
//! 1. **Strict Backend Gating:** No network dispatch or GPU inference is initiated unless
//!    `cloudEngines == "allowed"` in application settings and a valid backend [`Grant`] is consumed.
//! 2. **Durable Intent & Dispatching:** Every remote operation persists its intent and dispatching
//!    phase to the transactional journal before issuing an HTTP request.
//! 3. **Ambiguous Acceptance Never Auto-Retried:** Transport failures or process interruptions
//!    during submission recover strictly as `Unknown` and forbid automatic resubmission.
//! 4. **Known Handle Reuse:** Polling or download network faults preserve the durable remote handle
//!    for resume and never spawn a duplicate billable job.
//! 5. **Non-Terminal Cancellation Acknowledgement:** `CancelRequested` remains non-terminal until
//!    confirmed as cancelled or completed by authoritative gateway status.
//! 6. **Mandatory Result Decode Validation:** Output bytes are validated via full
//!    [`cleaner_core::cloud_decode::decode_result_crop`] before caching or project attachment.
//! 7. **Crash Discovery & Offline Recovery:** Cached outputs recover without resubmission.
//! 8. **Stale Revision Attachment Rejection:** Drifts in source pixels, region geometry, or mask
//!    hashes reject project attachment (`StaleAttachment`) while preserving cached results for inspection.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cleaner_core::cloud_decode::{decode_result_crop, DecodedResultCrop, ResultDecodeError};
use cleaner_core::cloud_wire::{
    compute_request_digest, JobExecutionStatus, JobRequestMetadata, ResultMetadata, ServiceLimits,
    TypedError, WireRenderRecipe, WireValidationError, PROTOCOL_VERSION,
};
use cleaner_core::engines::render::{CloudProvider, ExecutionTarget, PreparedRender, RenderRecipe};
use cleaner_core::fit::{self, EdgeMap};
use cleaner_core::ingest::sha256_hex;
use cleaner_core::patch::{CloudRecord, Engine};
use cleaner_core::project::Job;
use thiserror::Error;

use crate::inference::config::{compute_canonical_endpoint_fingerprint, InferenceConfig};
use crate::inference::consent::{
    compute_operation_digest, compute_region_revision_hash, encode_gray8_png, encode_rgb8_png,
    ConfirmProposalOptions, ConsentError, ConsentProposal, ConsentService, OperationIntent,
    PrepareProposalRequest,
};
use crate::inference::http::{
    BoundRuntimeCredential, CloudEndpointTarget, CloudHttpClient, HttpTransportError,
    RuntimeCredential,
};
use crate::inference::journal::{
    AttemptJournal, AttemptPhase, CreateAttemptIntent, JournalError, ProjectRegionSnapshot,
    RecoveryDecision,
};
use crate::inference::policy::{Grant, GrantError, GrantScope, GrantService};
use crate::inference::secrets::{
    decode_modal_runtime_secret, SecretKey, SecretManager, SecretRole,
};
use crate::library::{ApiRegion, Library};
use crate::run;

/// Errors arising from inference service operations.
#[derive(Debug, Error)]
pub enum InferenceServiceError {
    #[error("cloud execution is disabled in application settings")]
    CloudDisabled,

    #[error(
        "local execution target requested; cloud inference service only handles remote providers"
    )]
    LocalTargetNotAllowed,

    #[error("profile '{0}' not found for provider {1:?}")]
    ProfileNotFound(String, CloudProvider),

    #[error("cloud credential missing for profile '{0}'")]
    CredentialMissing(String),

    #[error("grant error: {0}")]
    Grant(#[from] GrantError),

    #[error("consent error: {0}")]
    Consent(#[from] ConsentError),

    #[error("attempt journal error: {0}")]
    Journal(#[from] JournalError),

    #[error("HTTP transport error: {0}")]
    Http(#[from] HttpTransportError),

    #[error("wire contract validation error: {0}")]
    Wire(#[from] WireValidationError),

    #[error("result decode error: {0}")]
    Decode(#[from] ResultDecodeError),

    #[error("remote job failed with error '{error_code}': {message}")]
    RemoteJobFailed { error_code: String, message: String },

    #[error("remote job was cancelled (handle '{0}')")]
    RemoteJobCancelled(String),

    #[error("polling timeout exceeded for job handle '{0}'")]
    PollingTimeout(String),

    #[error("stale project attachment: {0}")]
    StaleAttachment(String),

    #[error("job manifest error: {0}")]
    JobManifest(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Polling configuration for status retrieval.
#[derive(Debug, Clone)]
pub struct PollOptions {
    pub poll_interval: Duration,
    pub timeout: Duration,
    pub max_polls: usize,
}

impl Default for PollOptions {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_millis(500),
            timeout: Duration::from_secs(120),
            max_polls: 240,
        }
    }
}

fn current_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Coherent service managing the lifecycle of cloud inference requests.
pub struct InferenceService {
    journal: AttemptJournal,
    consent: Option<Arc<ConsentService>>,
    grants: Option<Arc<GrantService>>,
    secrets: Option<Arc<SecretManager>>,
    custom_client: Option<CloudHttpClient>,
}

impl InferenceService {
    /// Create a new InferenceService backed by default global authorities.
    pub fn new(journal_dir: impl Into<PathBuf>, limits: ServiceLimits) -> Self {
        Self {
            journal: AttemptJournal::new(journal_dir, limits),
            consent: None,
            grants: None,
            secrets: None,
            custom_client: None,
        }
    }

    /// Create an isolated InferenceService for testing.
    pub fn new_isolated(
        journal_dir: impl Into<PathBuf>,
        limits: ServiceLimits,
        consent: Arc<ConsentService>,
        grants: Arc<GrantService>,
        secrets: Arc<SecretManager>,
    ) -> Self {
        Self {
            journal: AttemptJournal::new(journal_dir, limits),
            consent: Some(consent),
            grants: Some(grants),
            secrets: Some(secrets),
            custom_client: None,
        }
    }

    /// Attach a custom/mock HTTP transport client for unit testing.
    pub fn with_custom_client(mut self, client: CloudHttpClient) -> Self {
        self.custom_client = Some(client);
        self
    }

    pub fn journal(&self) -> &AttemptJournal {
        &self.journal
    }

    fn resolve_consent(&self) -> &ConsentService {
        match &self.consent {
            Some(c) => c.as_ref(),
            None => ConsentService::global(),
        }
    }

    fn resolve_grants(&self) -> &GrantService {
        match &self.grants {
            Some(g) => g.as_ref(),
            None => GrantService::global(),
        }
    }

    fn resolve_secrets(&self) -> &SecretManager {
        match &self.secrets {
            Some(s) => s.as_ref(),
            None => SecretManager::global(),
        }
    }

    /// Step 1: Prepare an immutable authorization proposal from real disk assets.
    pub fn prepare_proposal(
        &self,
        request: PrepareProposalRequest,
        config: &InferenceConfig,
        cloud_allowed: bool,
        job_path: &Path,
    ) -> Result<ConsentProposal, InferenceServiceError> {
        self.resolve_consent()
            .prepare_proposal(request, config, cloud_allowed, job_path)
            .map_err(InferenceServiceError::Consent)
    }

    /// Step 2: Confirm a prepared proposal and issue a scoped authorization grant.
    pub fn confirm_proposal(
        &self,
        opts: ConfirmProposalOptions<'_>,
    ) -> Result<Grant, InferenceServiceError> {
        self.resolve_consent()
            .confirm_proposal(opts)
            .map_err(InferenceServiceError::Consent)
    }

    /// Build or resolve the HTTP transport client for a given provider and profile.
    pub fn build_http_client(
        &self,
        provider: CloudProvider,
        profile_id: &str,
        config: &InferenceConfig,
    ) -> Result<CloudHttpClient, InferenceServiceError> {
        if let Some(ref client) = self.custom_client {
            return Ok(client.clone());
        }

        let profile = match provider {
            CloudProvider::Beam => config.beam_profiles.get(profile_id),
            CloudProvider::Modal => config.modal_profiles.get(profile_id),
        }
        .ok_or_else(|| InferenceServiceError::ProfileNotFound(profile_id.to_string(), provider))?;

        let target = CloudEndpointTarget::new(provider, profile_id, &profile.endpoint_url)
            .map_err(InferenceServiceError::Http)?;

        let sec_key = SecretKey::new(
            provider,
            profile_id.to_string(),
            profile.canonical_origin_fingerprint.clone(),
            SecretRole::Runtime,
        );

        let secret_val = self
            .resolve_secrets()
            .get_secret(&sec_key)
            .map_err(|_| InferenceServiceError::CredentialMissing(profile_id.to_string()))?
            .ok_or_else(|| InferenceServiceError::CredentialMissing(profile_id.to_string()))?;

        let runtime_cred = match provider {
            CloudProvider::Beam => RuntimeCredential::BeamBearer(secret_val),
            CloudProvider::Modal => {
                let (token_id, token_secret) =
                    decode_modal_runtime_secret(&secret_val).map_err(|_| {
                        InferenceServiceError::CredentialMissing(profile_id.to_string())
                    })?;
                RuntimeCredential::ModalProxy {
                    token_id,
                    token_secret,
                }
            }
        };

        let bound_cred = BoundRuntimeCredential::new(&target, runtime_cred)
            .map_err(InferenceServiceError::Http)?;

        CloudHttpClient::new(target, bound_cred).map_err(InferenceServiceError::Http)
    }

    /// Execute a fully authenticated, durable remote cloud inference operation.
    #[allow(clippy::too_many_arguments)]
    pub fn execute_cloud_render(
        &self,
        grant_nonce: &str,
        job_path: &Path,
        page_index: u32,
        region_id: &str,
        target: &ExecutionTarget,
        recipe: &RenderRecipe,
        intent: &OperationIntent,
        config: &InferenceConfig,
        cloud_allowed: bool,
        poll_opts: &PollOptions,
    ) -> Result<ApiRegion, InferenceServiceError> {
        if !cloud_allowed {
            return Err(InferenceServiceError::CloudDisabled);
        }

        let provider = target
            .provider()
            .ok_or(InferenceServiceError::LocalTargetNotAllowed)?;
        let profile_id = target
            .profile_id()
            .ok_or(InferenceServiceError::LocalTargetNotAllowed)?;

        let profile = match provider {
            CloudProvider::Beam => config.beam_profiles.get(profile_id),
            CloudProvider::Modal => config.modal_profiles.get(profile_id),
        }
        .ok_or_else(|| InferenceServiceError::ProfileNotFound(profile_id.to_string(), provider))?;

        let canonical_endpoint_fingerprint =
            compute_canonical_endpoint_fingerprint(&profile.endpoint_url)
                .map_err(|e| InferenceServiceError::Consent(ConsentError::InvalidEndpoint(e)))?;

        // 1. Read local source & region under lock_job to extract exact crop, hint, and hashes,
        // validating page index resolution and patch record source match before remote client setup.
        let (
            crop_bounds,
            crop_png,
            crop_sha256,
            hint_png,
            hint_sha256,
            source_hash,
            mask_hash,
            revision,
            source_idx,
            page_w,
            page_h,
        ) = {
            let _lock = run::lock_job(job_path);
            let job = Job::open(job_path)
                .map_err(|e| InferenceServiceError::JobManifest(e.to_string()))?;

            let source_idx =
                Library::resolve_page(&job.project, page_index as usize).ok_or_else(|| {
                    InferenceServiceError::Consent(ConsentError::PageNotFound(
                        page_index,
                        "unknown".into(),
                    ))
                })?;

            let source_path = job
                .source_path(source_idx)
                .ok_or(InferenceServiceError::Consent(
                    ConsentError::SourceImageError,
                ))?;
            let source_bytes = std::fs::read(&source_path)?;
            let source_hash = sha256_hex(&source_bytes);
            let page = cleaner_core::image::decode(&source_bytes).map_err(|e| {
                InferenceServiceError::Consent(ConsentError::RenderPrepareFailed(e.to_string()))
            })?;

            let record = job
                .project
                .patches
                .iter()
                .find(|r| r.id == region_id)
                .ok_or_else(|| {
                    InferenceServiceError::Consent(ConsentError::RegionNotFound(
                        region_id.to_string(),
                    ))
                })?;

            if record.source_idx != source_idx {
                return Err(InferenceServiceError::Consent(ConsentError::PageMismatch));
            }

            let patch = job
                .load_patch(record)
                .map_err(|e| InferenceServiceError::JobManifest(e.to_string()))?;

            let seed = patch.mask.clone();
            if seed.is_empty() {
                return Err(InferenceServiceError::Consent(
                    ConsentError::RenderPrepareFailed("seed mask is empty".to_string()),
                ));
            }

            let noise = fit::page_noise_sigma(&page);
            let edges = EdgeMap::sobel(&page);
            let fitted = fit::fit(&page, &seed, 1.0, noise, &edges, true);

            let prepared = PreparedRender::prepare(&page, &fitted).map_err(|e| {
                InferenceServiceError::Consent(ConsentError::RenderPrepareFailed(e.to_string()))
            })?;

            let crop_bounds = prepared.crop();
            let crop_png = encode_rgb8_png(crop_bounds.w, crop_bounds.h, prepared.image_rgb8())?;
            let crop_sha256 = sha256_hex(&crop_png);

            let hint_png = encode_gray8_png(crop_bounds.w, crop_bounds.h, prepared.hint_gray8())?;
            let hint_sha256 = sha256_hex(&hint_png);

            let mask_hash = sha256_hex(&patch.mask.bits);
            let (revision, _revision_hash) =
                compute_region_revision_hash(&source_hash, record, &patch.mask, &patch.ink);

            (
                crop_bounds,
                crop_png,
                crop_sha256,
                hint_png,
                hint_sha256,
                source_hash,
                mask_hash,
                revision,
                source_idx,
                page.width,
                page.height,
            )
        };

        // 2. Build HTTP client first before creating durable intent or consuming grant
        let client = self.build_http_client(provider, profile_id, config)?;

        let operation_digest = compute_operation_digest(intent)?;

        // 3. Exact immutable GrantScope construction
        let scope = GrantScope {
            provider,
            profile_id: profile_id.to_string(),
            canonical_endpoint_fingerprint: canonical_endpoint_fingerprint.clone(),
            source_hash: source_hash.clone(),
            crop_sha256: crop_sha256.clone(),
            hint_sha256: hint_sha256.clone(),
            operation_digest,
            crop_bounds,
            mask_hash: mask_hash.clone(),
            revision,
            recipe: recipe.clone(),
            region_ids: vec![region_id.to_string()],
        };

        // 4. Create Intent & Acquire Journal Lock
        let attempt_id = format!("att-{}", &sha256_hex(grant_nonce.as_bytes())[0..24]);
        let job_id = format!(
            "job-{}",
            &sha256_hex(format!("{region_id}:{page_index}").as_bytes())[0..24]
        );

        let wire_recipe: WireRenderRecipe = recipe.clone().into();

        let req_meta = JobRequestMetadata {
            protocol_version: PROTOCOL_VERSION.to_string(),
            job_id: job_id.clone(),
            attempt_id: attempt_id.clone(),
            recipe: wire_recipe,
            width: crop_bounds.w,
            height: crop_bounds.h,
            seed: 42,
            steps: 4,
            guidance_scaled: 350,
            image_sha256: crop_sha256.clone(),
            hint_sha256: hint_sha256.clone(),
            request_digest: String::new(),
        };

        let request_digest = compute_request_digest(&req_meta);
        let mut req_meta = req_meta;
        req_meta.request_digest = request_digest.clone();

        let create_intent = CreateAttemptIntent {
            attempt_id: attempt_id.clone(),
            job_id: job_id.clone(),
            provider,
            profile_id: profile_id.to_string(),
            endpoint_fingerprint: canonical_endpoint_fingerprint,
            recipe_id: recipe.recipe_id.clone(),
            preprocessing_version: recipe.preprocessing_version.clone(),
            model_id: recipe.model_id.clone(),
            model_revision: recipe.model_revision.clone(),
            native_mask_conditioning: recipe.native_mask_conditioning,
            width: crop_bounds.w,
            height: crop_bounds.h,
            seed: 42,
            steps: 4,
            guidance_scaled: 350,
            source_image_hash: source_hash.clone(),
            region_id: region_id.to_string(),
            region_revision: revision,
            crop_sha256: crop_sha256.clone(),
            hint_sha256: hint_sha256.clone(),
            request_digest,
            chapter_id: None,
            page_index: Some(page_index),
        };

        let (_record, guard) = self.journal.create_intent(create_intent)?;

        // 5. Consume grant at the last safe point before marking Dispatching and network dispatch
        if let Err(e) = self.resolve_grants().validate_and_consume(grant_nonce, &scope) {
            let _ = self.journal.abort_intent_no_dispatch(&guard);
            return Err(InferenceServiceError::Grant(e));
        }

        // 6. Mark Dispatching immediately before network dispatch
        let _permit = match self.journal.mark_dispatching(&guard) {
            Ok(p) => p,
            Err(e) => {
                let _ = self.journal.abort_intent_no_dispatch(&guard);
                return Err(InferenceServiceError::Journal(e));
            }
        };
        // Release exclusive attempt lock before performing network operations (Property 6)
        drop(guard);

        // 7. Submit job over HTTP POST (executed without holding attempt lock - Property 6)
        let accepted =
            match client.submit_job(&req_meta, &crop_png, &hint_png, self.journal.limits()) {
                Ok(acc) => {
                    let guard = self.journal.acquire_lock(&attempt_id)?;
                    self.journal.record_accepted_handle(&guard, &acc.handle)?;
                    drop(guard);
                    acc
                }
                Err(e) => {
                    // Interrupted or network error during submit -> must record transport error
                    // so attempt transitions to Unknown and is NEVER auto-retried.
                    if let Ok(guard) = self.journal.acquire_lock(&attempt_id) {
                        let _ = self.journal.record_dispatch_transport_error(&guard);
                    }
                    return Err(InferenceServiceError::Http(e));
                }
            };

        // 6. Poll status until completion (executed without holding attempt lock - Property 6)
        let handle = accepted.handle;
        let start = Instant::now();
        let mut polls = 0;
        let mut final_status = None;

        while polls < poll_opts.max_polls && start.elapsed() < poll_opts.timeout {
            polls += 1;
            match client.get_job_status(&handle, &req_meta) {
                Ok(status_resp) => match status_resp.status {
                    JobExecutionStatus::Pending | JobExecutionStatus::Running => {
                        std::thread::sleep(poll_opts.poll_interval);
                    }
                    JobExecutionStatus::Completed => {
                        final_status = Some(status_resp);
                        break;
                    }
                    JobExecutionStatus::Failed => {
                        if let Ok(guard) = self.journal.acquire_lock(&attempt_id) {
                            let _ = self.journal.record_failed(&guard);
                        }
                        let err = status_resp.error.unwrap_or(TypedError {
                            error_code: "remote_failure".into(),
                            message: "remote job failed".into(),
                        });
                        return Err(InferenceServiceError::RemoteJobFailed {
                            error_code: err.error_code,
                            message: err.message,
                        });
                    }
                    JobExecutionStatus::Cancelled => {
                        if let Ok(guard) = self.journal.acquire_lock(&attempt_id) {
                            let _ = self.journal.record_cancelled(&guard);
                        }
                        return Err(InferenceServiceError::RemoteJobCancelled(handle));
                    }
                },
                Err(_) => {
                    // Polling fault: preserve known handle and retry polling
                    std::thread::sleep(poll_opts.poll_interval);
                }
            }
        }

        let status_resp =
            final_status.ok_or_else(|| InferenceServiceError::PollingTimeout(handle.clone()))?;

        // 7. Download result bytes (executed without holding attempt lock - Property 6)
        let result_meta = ResultMetadata {
            handle: handle.clone(),
            job_id: req_meta.job_id.clone(),
            attempt_id: req_meta.attempt_id.clone(),
            request_digest: req_meta.request_digest.clone(),
            recipe_id: req_meta.recipe.recipe_id.clone(),
            preprocessing_version: req_meta.recipe.preprocessing_version.clone(),
            model_id: req_meta.recipe.model_id.clone(),
            model_revision: req_meta.recipe.model_revision.clone(),
            native_mask_conditioning: req_meta.recipe.native_mask_conditioning,
            result_digest: status_resp.result_digest.clone().unwrap_or_default(),
            reported_cost_usd: status_resp.reported_cost_usd,
            width: req_meta.width,
            height: req_meta.height,
            byte_length: status_resp.result_bytes.unwrap_or(0),
        };

        let result_bytes =
            client.fetch_result_bytes(&handle, &result_meta, &req_meta, self.journal.limits())?;

        let guard = self.journal.acquire_lock(&attempt_id)?;
        let decoded_crop = self
            .journal
            .cache_result(&guard, &result_bytes, &result_meta)?;

        // 8. Two-phase project attachment
        let patch_id = format!("{}-p{:03}-{}", region_id, page_index, current_epoch_ms());
        let snapshot = ProjectRegionSnapshot {
            source_image_hash: source_hash.clone(),
            region_id: region_id.to_string(),
            region_revision: revision,
            crop_sha256: crop_sha256.clone(),
            hint_sha256: hint_sha256.clone(),
        };

        self.journal
            .prepare_attachment(&guard, &snapshot, &patch_id)?;

        let api_region = self.attach_patch_to_project(
            job_path,
            source_idx,
            region_id,
            &snapshot,
            &decoded_crop,
            &recipe.clone(),
            provider,
            profile_id,
            &handle,
            &req_meta.job_id,
            &req_meta.attempt_id,
            &req_meta.request_digest,
            &result_meta.result_digest,
            status_resp.reported_cost_usd,
            page_w,
            page_h,
        )?;

        self.journal.confirm_committed(&guard, &patch_id)?;
        drop(guard);
        Ok(api_region)
    }

    /// Internal helper to commit a composited patch into the project manifest.
    #[allow(clippy::too_many_arguments)]
    fn attach_patch_to_project(
        &self,
        job_path: &Path,
        source_idx: usize,
        region_id: &str,
        snapshot: &ProjectRegionSnapshot,
        decoded_crop: &DecodedResultCrop,
        recipe: &RenderRecipe,
        provider: CloudProvider,
        profile_id: &str,
        _handle: &str,
        job_id: &str,
        attempt_id: &str,
        request_digest: &str,
        _result_digest: &str,
        reported_cost_usd: Option<f64>,
        _page_w: u32,
        _page_h: u32,
    ) -> Result<ApiRegion, InferenceServiceError> {
        let _lock = run::lock_job(job_path);
        let mut job =
            Job::open(job_path).map_err(|e| InferenceServiceError::JobManifest(e.to_string()))?;

        // Re-verify snapshot on disk before mutating project
        let cur_source_path = job
            .source_path(source_idx)
            .ok_or_else(|| InferenceServiceError::JobManifest("source path missing".into()))?;
        let cur_source_bytes = std::fs::read(&cur_source_path)?;
        if sha256_hex(&cur_source_bytes) != snapshot.source_image_hash {
            return Err(InferenceServiceError::StaleAttachment(
                "source_image_hash changed".into(),
            ));
        }

        let cur_record = job
            .project
            .patches
            .iter()
            .find(|r| r.id == region_id)
            .ok_or_else(|| InferenceServiceError::JobManifest("region record missing".into()))?;

        if cur_record.source_idx != source_idx {
            return Err(InferenceServiceError::StaleAttachment(
                "patch record source index mismatch".into(),
            ));
        }

        let cur_patch = job
            .load_patch(cur_record)
            .map_err(|e| InferenceServiceError::JobManifest(e.to_string()))?;

        let (cur_rev, _) = compute_region_revision_hash(
            &snapshot.source_image_hash,
            cur_record,
            &cur_patch.mask,
            &cur_patch.ink,
        );

        if cur_rev != snapshot.region_revision {
            return Err(InferenceServiceError::StaleAttachment(
                "region_revision changed".into(),
            ));
        }

        let page = cleaner_core::image::decode(&cur_source_bytes).map_err(|e| {
            InferenceServiceError::Consent(ConsentError::RenderPrepareFailed(e.to_string()))
        })?;

        let noise = fit::page_noise_sigma(&page);
        let edges = EdgeMap::sobel(&page);
        let fitted = fit::fit(&page, &cur_patch.mask, 1.0, noise, &edges, true);

        let prepared = PreparedRender::prepare(&page, &fitted).map_err(|e| {
            InferenceServiceError::Consent(ConsentError::RenderPrepareFailed(e.to_string()))
        })?;

        let generated = decoded_crop.as_generated_crop();
        let rendered = prepared.composite(&generated).map_err(|e| {
            InferenceServiceError::Consent(ConsentError::RenderPrepareFailed(e.to_string()))
        })?;

        let cloud_record = CloudRecord {
            provider: match provider {
                CloudProvider::Beam => "beam".to_string(),
                CloudProvider::Modal => "modal".to_string(),
            },
            profile_id: Some(profile_id.to_string()),
            job_id: Some(job_id.to_string()),
            request_id: request_digest.to_string(),
            attempt_id: Some(attempt_id.to_string()),
            recipe_id: Some(recipe.recipe_id.clone()),
            model: recipe.model_id.clone(),
            model_revision: Some(recipe.model_revision.clone()),
            tier: None,
            cost: reported_cost_usd,
            duration_ms: None,
        };

        let mut updated_patch = cur_patch.clone();
        updated_patch.provenance.engine = Engine::Flux;
        updated_patch.provenance.cloud = Some(cloud_record);
        updated_patch.pixels = rendered.pixels;
        updated_patch.mask = rendered.mask;

        job.complete_region(source_idx, &updated_patch, None)
            .map_err(|e| InferenceServiceError::JobManifest(e.to_string()))?;
        crate::library::invalidate_manifest_cache(job_path);

        let chapter_id = job_path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("chapter");

        let (api_region, _) =
            crate::library::region_and_status(chapter_id, &job.project, source_idx, region_id)
                .ok_or_else(|| {
                    InferenceServiceError::JobManifest(
                        "failed to resolve region in manifest".to_string(),
                    )
                })?;

        Ok(api_region)
    }

    /// Request non-terminal cancellation of an in-flight cloud attempt.
    pub fn cancel_cloud_render(
        &self,
        attempt_id: &str,
        config: &InferenceConfig,
    ) -> Result<RecoveryDecision, InferenceServiceError> {
        let guard = self.journal.acquire_lock(attempt_id)?;
        let record = self.journal.read_record_locked(&guard)?;

        if record.is_terminal() {
            return Ok(RecoveryDecision::Terminal {
                message: format!("attempt is already in terminal phase: {:?}", record.phase),
            });
        }

        let handle = match &record.phase {
            AttemptPhase::Accepted { handle } | AttemptPhase::CancelRequested { handle } => {
                handle.clone()
            }
            _ => {
                return Err(InferenceServiceError::Journal(
                    JournalError::InvalidTransition {
                        current: format!("{:?}", record.phase),
                        attempted: "cancel_cloud_render".into(),
                    },
                ));
            }
        };

        // Transition to CancelRequested in journal
        self.journal.record_cancel_requested(&guard)?;
        // Drop lock before network cancellation (Property 6)
        drop(guard);

        let client = self.build_http_client(record.provider, &record.profile_id, config)?;
        let cancel_resp = client.cancel_job(&handle, &record.to_request_metadata())?;

        if cancel_resp.status != "cancel_requested" || !cancel_resp.acknowledged {
            return Err(InferenceServiceError::Wire(
                WireValidationError::NonterminalCancelStatus(cancel_resp.status),
            ));
        }

        // Poll until terminal status without holding attempt lock (Property 6)
        let req_meta = record.to_request_metadata();
        for _ in 0..10 {
            if let Ok(status) = client.get_job_status(&handle, &req_meta) {
                if status.status == JobExecutionStatus::Cancelled {
                    let guard = self.journal.acquire_lock(attempt_id)?;
                    self.journal.record_cancelled(&guard)?;
                    drop(guard);
                    return Ok(RecoveryDecision::Terminal {
                        message: "cancellation confirmed by gateway".into(),
                    });
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        Ok(RecoveryDecision::ResumeCancelPolling { handle })
    }

    /// Recover an attempt upon application restart or following a crash.
    pub fn recover_cloud_attempt(
        &self,
        attempt_id: &str,
        job_path: &Path,
        config: &InferenceConfig,
        poll_opts: &PollOptions,
    ) -> Result<RecoveryDecision, InferenceServiceError> {
        let guard = self.journal.acquire_lock(attempt_id)?;
        let decision = self.journal.recover_attempt(&guard)?;

        match decision {
            RecoveryDecision::AmbiguousUnknown { reason } => {
                // Ambiguous submission is NEVER auto-retried
                Ok(RecoveryDecision::AmbiguousUnknown { reason })
            }
            RecoveryDecision::ResumePolling { handle } => {
                let record = self.journal.read_record_locked(&guard)?;
                // Release lock before network polling and result download (Property 6)
                drop(guard);

                let client = self.build_http_client(record.provider, &record.profile_id, config)?;
                let req_meta = record.to_request_metadata();

                let mut final_status = None;
                for _ in 0..poll_opts.max_polls {
                    if let Ok(st) = client.get_job_status(&handle, &req_meta) {
                        if st.status == JobExecutionStatus::Completed {
                            final_status = Some(st);
                            break;
                        }
                    }
                    std::thread::sleep(poll_opts.poll_interval);
                }

                if let Some(status_resp) = final_status {
                    let result_meta = ResultMetadata {
                        handle: handle.clone(),
                        job_id: req_meta.job_id.clone(),
                        attempt_id: req_meta.attempt_id.clone(),
                        request_digest: req_meta.request_digest.clone(),
                        recipe_id: req_meta.recipe.recipe_id.clone(),
                        preprocessing_version: req_meta.recipe.preprocessing_version.clone(),
                        model_id: req_meta.recipe.model_id.clone(),
                        model_revision: req_meta.recipe.model_revision.clone(),
                        native_mask_conditioning: req_meta.recipe.native_mask_conditioning,
                        result_digest: status_resp.result_digest.clone().unwrap_or_default(),
                        reported_cost_usd: status_resp.reported_cost_usd,
                        width: req_meta.width,
                        height: req_meta.height,
                        byte_length: status_resp.result_bytes.unwrap_or(0),
                    };

                    let result_bytes = client.fetch_result_bytes(
                        &handle,
                        &result_meta,
                        &req_meta,
                        self.journal.limits(),
                    )?;

                    let guard = self.journal.acquire_lock(attempt_id)?;
                    self.journal
                        .cache_result(&guard, &result_bytes, &result_meta)?;
                    drop(guard);
                    return Ok(RecoveryDecision::ResultCachedReadyForAttach {
                        handle,
                        result_digest: result_meta.result_digest,
                    });
                }

                Ok(RecoveryDecision::ResumePolling { handle })
            }
            RecoveryDecision::ResultCachedReadyForAttach { result_digest, .. }
            | RecoveryDecision::AttachmentPendingVerification { result_digest, .. } => {
                let record = self.journal.read_record_locked(&guard)?;
                let handle = match &record.phase {
                    AttemptPhase::Accepted { handle }
                    | AttemptPhase::CancelRequested { handle }
                    | AttemptPhase::ResultCached { handle, .. }
                    | AttemptPhase::AttachmentPending { handle, .. }
                    | AttemptPhase::Committed { handle, .. } => handle.clone(),
                    _ => "recovered-handle".to_string(),
                };

                let snapshot = ProjectRegionSnapshot {
                    source_image_hash: record.source_image_hash.clone(),
                    region_id: record.region_id.clone(),
                    region_revision: record.region_revision,
                    crop_sha256: record.crop_sha256.clone(),
                    hint_sha256: record.hint_sha256.clone(),
                };

                let png_bytes = self.journal.read_cached_png(&guard)?;
                let result_meta = ResultMetadata {
                    handle: handle.clone(),
                    job_id: record.job_id.clone(),
                    attempt_id: record.attempt_id.clone(),
                    request_digest: record.request_digest.clone(),
                    recipe_id: record.recipe_id.clone(),
                    preprocessing_version: record.preprocessing_version.clone(),
                    model_id: record.model_id.clone(),
                    model_revision: record.model_revision.clone(),
                    native_mask_conditioning: record.native_mask_conditioning,
                    result_digest: result_digest.clone(),
                    reported_cost_usd: None,
                    width: record.width,
                    height: record.height,
                    byte_length: png_bytes.len() as u64,
                };

                let decoded = decode_result_crop(
                    &png_bytes,
                    &result_meta,
                    &record.to_request_metadata(),
                    self.journal.limits(),
                    &handle,
                )?;

                let patch_id = format!("{}-recovered", record.region_id);
                self.journal
                    .prepare_attachment(&guard, &snapshot, &patch_id)?;

                let source_idx = {
                    let job = Job::open(job_path)
                        .map_err(|e| InferenceServiceError::JobManifest(e.to_string()))?;
                    job.project
                        .patches
                        .iter()
                        .find(|r| r.id == record.region_id)
                        .map(|r| r.source_idx)
                        .unwrap_or(0)
                };

                let _api_region = self.attach_patch_to_project(
                    job_path,
                    source_idx,
                    &record.region_id,
                    &snapshot,
                    &decoded,
                    &RenderRecipe::new(
                        &record.recipe_id,
                        &record.preprocessing_version,
                        &record.model_id,
                        &record.model_revision,
                        record.native_mask_conditioning,
                    ),
                    record.provider,
                    &record.profile_id,
                    &handle,
                    &record.job_id,
                    &record.attempt_id,
                    &record.request_digest,
                    &result_digest,
                    None,
                    record.width,
                    record.height,
                )?;

                self.journal.confirm_committed(&guard, &patch_id)?;
                Ok(RecoveryDecision::AlreadyCommitted { patch_id })
            }
            other => Ok(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inference::config::validate_https_endpoint;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    use crate::inference::secrets::SecretValue;
    use cleaner_core::engines::render::CloudProvider;
    use cleaner_core::image::fixtures;
    use cleaner_core::image::Format;
    use cleaner_core::mask::{Mask, Rect};
    use cleaner_core::patch::{Patch, Provenance};
    use cleaner_core::project::{Project, StripMode};

    fn test_scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("mc-inference-service-tests")
            .join(name)
            .join(format!("{}", current_epoch_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn setup_test_job(root: &Path, patch_id: &str) -> (PathBuf, Rect) {
        let raws = root.join("raws");
        std::fs::create_dir_all(&raws).unwrap();
        let mut raster = fixtures::by_name("l8").raster;
        raster.width = 256;
        raster.height = 256;
        raster.data = vec![200; 256 * 256];
        let bytes = cleaner_core::image::encode(&raster, Format::Png).unwrap();
        let page_path = raws.join("001.png");
        std::fs::write(&page_path, &bytes).unwrap();

        let source_ref = cleaner_core::ingest::source_ref(&page_path, &bytes).unwrap();
        let manifest = root.join("job/chapter.mtclean");
        std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        let project = Project::new(
            manifest.parent().unwrap(),
            "test-app",
            StripMode::Single,
            &[source_ref],
        );
        let mut job = Job::create(&manifest, project).unwrap();

        let bounds = Rect::new(100, 100, 32, 32);
        let mut pixels = raster.clone();
        pixels.width = bounds.w;
        pixels.height = bounds.h;
        pixels.data = vec![200; (bounds.w * bounds.h) as usize];

        let patch = Patch {
            id: patch_id.to_string(),
            mask: Mask::filled(bounds),
            ink: Mask::filled(bounds),
            pixels,
            order: 1,
            visible: true,
            provenance: Provenance {
                engine: Engine::Fill,
                engine_version: "1.0".into(),
                model_sha256: None,
                execution_provider: "cpu".into(),
                params_snapshot: serde_json::json!({ "thickness": 2 }),
                mask_sha256: sha256_hex(&vec![255; (bounds.w * bounds.h) as usize]),
                source_sha256: sha256_hex(&bytes),
                cloud: None,
                created: 1_760_000_000,
            },
        };

        job.complete_region(0, &patch, None).unwrap();
        (manifest, bounds)
    }

    fn test_config() -> InferenceConfig {
        let mut cfg = InferenceConfig::default();
        let (origin, origin_fp) = validate_https_endpoint("https://modal.run/mc/v1").unwrap();
        cfg.modal_profiles.insert(
            "modal-prof-1".to_string(),
            crate::inference::config::CloudProfile {
                id: "modal-prof-1".to_string(),
                name: "Modal Test".to_string(),
                endpoint_url: "https://modal.run/mc/v1".to_string(),
                canonical_origin: origin,
                canonical_origin_fingerprint: origin_fp,
                created_at_ms: 100,
                updated_at_ms: 100,
            },
        );
        cfg
    }

    fn test_crop_bounds(manifest: &Path, region_id: &str) -> Rect {
        let job = Job::open(manifest).unwrap();
        let patch_rec = job
            .project
            .patches
            .iter()
            .find(|r| r.id == region_id)
            .unwrap();
        let patch = job.load_patch(patch_rec).unwrap();
        let source_path = job.source_path(0).unwrap();
        let source_bytes = std::fs::read(source_path).unwrap();
        let page = cleaner_core::image::decode(&source_bytes).unwrap();
        let noise = fit::page_noise_sigma(&page);
        let edges = EdgeMap::sobel(&page);
        let fitted = fit::fit(&page, &patch.mask, 1.0, noise, &edges, true);
        let prepared = PreparedRender::prepare(&page, &fitted).unwrap();
        prepared.crop()
    }

    /// Fault Test 1: Ambiguous acceptance is NEVER automatically resubmitted.
    #[test]
    fn test_fault_ambiguous_acceptance_never_resubmitted() {
        let scratch = test_scratch("ambiguous-never-resubmit");
        let (manifest, _bounds) = setup_test_job(&scratch, "reg-1");

        let submit_count = Arc::new(AtomicUsize::new(0));
        let sc_clone = Arc::clone(&submit_count);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        // Fake gateway: drops connection during POST /mc/v1/jobs to simulate transport disconnect
        thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let req_str = String::from_utf8_lossy(&buf);

                if req_str.starts_with("POST /mc/v1/jobs") {
                    sc_clone.fetch_add(1, Ordering::SeqCst);
                    // Drop TCP connection immediately before sending 202 Accepted
                    drop(stream);
                    break;
                }
            }
        });

        let endpoint_url = format!("http://127.0.0.1:{port}/mc/v1");
        let config = test_config();

        let grant_service = Arc::new(GrantService::new());
        let consent_service = Arc::new(ConsentService::new_isolated(Arc::clone(&grant_service)));
        let secret_mgr = Arc::new(SecretManager::new_in_memory());

        let target = ExecutionTarget::Modal {
            profile_id: "modal-prof-1".into(),
        };
        let recipe = RenderRecipe::new(
            "sdnq-v1",
            "1.0.0",
            "flux-schnell",
            "0123456789abcdef0123456789abcdef01234567",
            false,
        );
        let intent = OperationIntent::CleanAnyway;

        let proposal = consent_service
            .prepare_proposal(
                PrepareProposalRequest {
                    chapter_id: "c1".into(),
                    page_index: 0,
                    region_id: Some("reg-1".into()),
                    target: target.clone(),
                    recipe: recipe.clone(),
                    intent: intent.clone(),
                },
                &config,
                true,
                &manifest,
            )
            .unwrap();

        let grant = consent_service
            .confirm_proposal(ConfirmProposalOptions {
                proposal_id: &proposal.proposal_id,
                intent: &intent,
                config: &config,
                cloud_allowed: true,
                job_path: &manifest,
                grant_ttl: Duration::from_secs(60),
                max_attempts: 1,
            })
            .unwrap();

        // Custom client with loopback target
        let http_target = CloudEndpointTarget::new_test_target(
            CloudProvider::Modal,
            "modal-prof-1",
            &endpoint_url,
        )
        .unwrap();

        let bound_cred = BoundRuntimeCredential::new(
            &http_target,
            RuntimeCredential::ModalProxy {
                token_id: "tid".into(),
                token_secret: SecretValue::new("tsec"),
            },
        )
        .unwrap();

        let custom_client = CloudHttpClient::new_test_client(
            http_target,
            bound_cred,
            reqwest::blocking::Client::new(),
        );

        let service = InferenceService::new_isolated(
            scratch.join("journal"),
            cleaner_core::cloud_wire::provisional_fixture_limits(),
            consent_service,
            grant_service,
            secret_mgr,
        )
        .with_custom_client(custom_client);

        // Execution fails due to dropped connection during submit
        let exec_res = service.execute_cloud_render(
            &grant.nonce,
            &manifest,
            0,
            "reg-1",
            &target,
            &recipe,
            &intent,
            &config,
            true,
            &PollOptions::default(),
        );
        assert!(exec_res.is_err());

        // Exactly 1 submission attempt occurred
        assert_eq!(submit_count.load(Ordering::SeqCst), 1);

        // Attempt ID derived deterministically
        let attempt_id = format!("att-{}", &sha256_hex(grant.nonce.as_bytes())[0..24]);

        // On recovery: decider marks attempt AmbiguousUnknown and NEVER resubmits
        let recovery = service
            .recover_cloud_attempt(&attempt_id, &manifest, &config, &PollOptions::default())
            .unwrap();

        assert!(matches!(
            recovery,
            RecoveryDecision::AmbiguousUnknown { .. }
        ));
        assert!(!recovery.is_auto_retryable());

        // Gateway received ZERO additional submissions
        assert_eq!(submit_count.load(Ordering::SeqCst), 1);
    }

    /// Fault Test 2: Known handles are reused after polling/download faults without resubmitting.
    #[test]
    fn test_fault_known_handles_reused_after_polling_or_download_faults() {
        let scratch = test_scratch("known-handle-reuse");
        let (manifest, _bounds) = setup_test_job(&scratch, "reg-1");

        let submit_count = Arc::new(AtomicUsize::new(0));
        let poll_count = Arc::new(AtomicUsize::new(0));
        let sc_clone = Arc::clone(&submit_count);
        let pc_clone = Arc::clone(&poll_count);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let crop_bounds = test_crop_bounds(&manifest, "reg-1");

        let raster_res = cleaner_core::image::Raster {
            width: crop_bounds.w,
            height: crop_bounds.h,
            mode: cleaner_core::image::ColorMode::Rgb,
            depth: cleaner_core::image::BitDepth::Eight,
            icc: None,
            palette: None,
            trns: None,
            srgb_intent: None,
            data: vec![220; (crop_bounds.w * crop_bounds.h * 3) as usize],
        };
        let res_png = cleaner_core::image::encode(&raster_res, Format::Png).unwrap();
        let res_digest = sha256_hex(&res_png);
        let res_png_clone = res_png.clone();

        let req_info = Arc::new(std::sync::Mutex::new((
            String::new(),
            String::new(),
            String::new(),
        )));
        let req_info_clone = Arc::clone(&req_info);

        thread::spawn(move || {
            let find_field = |req: &str, field: &str| -> String {
                let needle = format!("\"{field}\":\"");
                if let Some(idx) = req.find(&needle) {
                    let rest = &req[idx + needle.len()..];
                    if let Some(end) = rest.find('"') {
                        return rest[..end].to_string();
                    }
                }
                String::new()
            };

            while let Ok((mut stream, _)) = listener.accept() {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                loop {
                    let n = match stream.read(&mut tmp) {
                        Ok(n) if n > 0 => n,
                        _ => break,
                    };
                    buf.extend_from_slice(&tmp[..n]);
                    let s = String::from_utf8_lossy(&buf);
                    if s.contains("GET ") && s.contains("\r\n\r\n") {
                        break;
                    }
                    if s.contains("POST ") && find_field(&s, "request_digest").len() == 64 {
                        break;
                    }
                    if buf.len() > 65536 {
                        break;
                    }
                }
                let req_str = String::from_utf8_lossy(&buf);

                if req_str.starts_with("POST /mc/v1/jobs") {
                    sc_clone.fetch_add(1, Ordering::SeqCst);
                    let job_id = find_field(&req_str, "job_id");
                    let attempt_id = find_field(&req_str, "attempt_id");
                    let digest = find_field(&req_str, "request_digest");
                    *req_info_clone.lock().unwrap() =
                        (job_id.clone(), attempt_id.clone(), digest.clone());

                    let json = format!(
                        r#"{{
                        "handle": "h-reuse-999",
                        "status": "pending",
                        "job_id": "{job_id}",
                        "attempt_id": "{attempt_id}",
                        "request_digest": "{digest}",
                        "recipe_id": "sdnq-v1",
                        "preprocessing_version": "1.0.0",
                        "model_id": "flux-schnell",
                        "model_revision": "0123456789abcdef0123456789abcdef01234567",
                        "native_mask_conditioning": false
                    }}"#
                    );
                    let resp = format!("HTTP/1.1 202 Accepted\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}", json.len(), json);
                    let _ = stream.write_all(resp.as_bytes());
                } else if req_str.starts_with("GET /mc/v1/jobs/h-reuse-999/result") {
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\n\r\n",
                        res_png_clone.len()
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.write_all(&res_png_clone);
                } else if req_str.starts_with("GET /mc/v1/jobs/h-reuse-999") {
                    let p = pc_clone.fetch_add(1, Ordering::SeqCst);
                    if p == 0 {
                        // First poll fails with 500 error
                        let resp =
                            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n";
                        let _ = stream.write_all(resp.as_bytes());
                    } else {
                        let (job_id, attempt_id, digest) = req_info_clone.lock().unwrap().clone();
                        // Subsequent poll succeeds with Completed
                        let json = format!(
                            r#"{{
                            "handle": "h-reuse-999",
                            "job_id": "{job_id}",
                            "attempt_id": "{attempt_id}",
                            "request_digest": "{digest}",
                            "recipe_id": "sdnq-v1",
                            "preprocessing_version": "1.0.0",
                            "model_id": "flux-schnell",
                            "model_revision": "0123456789abcdef0123456789abcdef01234567",
                            "native_mask_conditioning": false,
                            "status": "completed",
                            "result_digest": "{}",
                            "result_bytes": {}
                        }}"#,
                            res_digest,
                            res_png_clone.len()
                        );
                        let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}", json.len(), json);
                        let _ = stream.write_all(resp.as_bytes());
                    }
                }
            }
        });

        let endpoint_url = format!("http://127.0.0.1:{port}/mc/v1");
        let config = test_config();

        let grant_service = Arc::new(GrantService::new());
        let consent_service = Arc::new(ConsentService::new_isolated(Arc::clone(&grant_service)));
        let secret_mgr = Arc::new(SecretManager::new_in_memory());

        let target = ExecutionTarget::Modal {
            profile_id: "modal-prof-1".into(),
        };
        let recipe = RenderRecipe::new(
            "sdnq-v1",
            "1.0.0",
            "flux-schnell",
            "0123456789abcdef0123456789abcdef01234567",
            false,
        );
        let intent = OperationIntent::CleanAnyway;

        let proposal = consent_service
            .prepare_proposal(
                PrepareProposalRequest {
                    chapter_id: "c1".into(),
                    page_index: 0,
                    region_id: Some("reg-1".into()),
                    target: target.clone(),
                    recipe: recipe.clone(),
                    intent: intent.clone(),
                },
                &config,
                true,
                &manifest,
            )
            .unwrap();

        let grant = consent_service
            .confirm_proposal(ConfirmProposalOptions {
                proposal_id: &proposal.proposal_id,
                intent: &intent,
                config: &config,
                cloud_allowed: true,
                job_path: &manifest,
                grant_ttl: Duration::from_secs(60),
                max_attempts: 1,
            })
            .unwrap();

        let http_target = CloudEndpointTarget::new_test_target(
            CloudProvider::Modal,
            "modal-prof-1",
            &endpoint_url,
        )
        .unwrap();

        let bound_cred = BoundRuntimeCredential::new(
            &http_target,
            RuntimeCredential::ModalProxy {
                token_id: "tid".into(),
                token_secret: SecretValue::new("tsec"),
            },
        )
        .unwrap();

        let custom_client = CloudHttpClient::new_test_client(
            http_target,
            bound_cred,
            reqwest::blocking::Client::new(),
        );

        let service = InferenceService::new_isolated(
            scratch.join("journal"),
            cleaner_core::cloud_wire::provisional_fixture_limits(),
            consent_service,
            grant_service,
            secret_mgr,
        )
        .with_custom_client(custom_client);

        let poll_opts = PollOptions {
            poll_interval: Duration::from_millis(50),
            timeout: Duration::from_secs(5),
            max_polls: 10,
        };

        // Execution survives initial polling 500 error, continues polling, downloads result, and commits
        let api_region = service
            .execute_cloud_render(
                &grant.nonce,
                &manifest,
                0,
                "reg-1",
                &target,
                &recipe,
                &intent,
                &config,
                true,
                &poll_opts,
            )
            .unwrap();

        assert_eq!(api_region.outcome, "cleaned");
        assert_eq!(submit_count.load(Ordering::SeqCst), 1);
        assert!(poll_count.load(Ordering::SeqCst) >= 2);
    }

    /// Fault Test 3: Cancellation acknowledgement is verified to be nonterminal.
    #[test]
    fn test_fault_cancellation_acknowledgement_is_nonterminal() {
        let scratch = test_scratch("cancel-nonterminal");
        let (manifest, _bounds) = setup_test_job(&scratch, "reg-1");

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let crop_bounds = test_crop_bounds(&manifest, "reg-1");

        let req_meta = JobRequestMetadata {
            protocol_version: PROTOCOL_VERSION.to_string(),
            job_id: "job-1".to_string(),
            attempt_id: "att-cancel-test".to_string(),
            recipe: WireRenderRecipe {
                recipe_id: "sdnq-v1".into(),
                preprocessing_version: "1.0.0".into(),
                model_id: "flux-schnell".into(),
                model_revision: "0123456789abcdef0123456789abcdef01234567".into(),
                native_mask_conditioning: false,
            },
            width: crop_bounds.w,
            height: crop_bounds.h,
            seed: 42,
            steps: 4,
            guidance_scaled: 350,
            image_sha256: "0000000000000000000000000000000000000000000000000000000000000000".into(),
            hint_sha256: "0000000000000000000000000000000000000000000000000000000000000000".into(),
            request_digest: String::new(),
        };
        let request_digest = compute_request_digest(&req_meta);
        let mut req_meta = req_meta;
        req_meta.request_digest = request_digest.clone();
        let digest_clone = request_digest.clone();

        thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let req_str = String::from_utf8_lossy(&buf);

                if req_str.starts_with("POST /mc/v1/jobs/h-cancel-123/cancel") {
                    let json = format!(
                        r#"{{
                        "handle": "h-cancel-123",
                        "job_id": "job-1",
                        "attempt_id": "att-cancel-test",
                        "request_digest": "{digest_clone}",
                        "status": "cancel_requested",
                        "acknowledged": true
                    }}"#
                    );
                    let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}", json.len(), json);
                    let _ = stream.write_all(resp.as_bytes());
                } else if req_str.starts_with("GET /mc/v1/jobs/h-cancel-123") {
                    let json = format!(
                        r#"{{
                        "handle": "h-cancel-123",
                        "job_id": "job-1",
                        "attempt_id": "att-cancel-test",
                        "request_digest": "{digest_clone}",
                        "recipe_id": "sdnq-v1",
                        "preprocessing_version": "1.0.0",
                        "model_id": "flux-schnell",
                        "model_revision": "0123456789abcdef0123456789abcdef01234567",
                        "native_mask_conditioning": false,
                        "status": "cancelled"
                    }}"#
                    );
                    let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}", json.len(), json);
                    let _ = stream.write_all(resp.as_bytes());
                }
            }
        });

        let endpoint_url = format!("http://127.0.0.1:{port}/mc/v1");
        let config = test_config();

        let grant_service = Arc::new(GrantService::new());
        let consent_service = Arc::new(ConsentService::new_isolated(Arc::clone(&grant_service)));
        let secret_mgr = Arc::new(SecretManager::new_in_memory());

        let http_target = CloudEndpointTarget::new_test_target(
            CloudProvider::Modal,
            "modal-prof-1",
            &endpoint_url,
        )
        .unwrap();

        let bound_cred = BoundRuntimeCredential::new(
            &http_target,
            RuntimeCredential::ModalProxy {
                token_id: "tid".into(),
                token_secret: SecretValue::new("tsec"),
            },
        )
        .unwrap();

        let custom_client = CloudHttpClient::new_test_client(
            http_target,
            bound_cred,
            reqwest::blocking::Client::new(),
        );

        let service = InferenceService::new_isolated(
            scratch.join("journal"),
            cleaner_core::cloud_wire::provisional_fixture_limits(),
            consent_service,
            grant_service,
            secret_mgr,
        )
        .with_custom_client(custom_client);

        // Manually seed an Accepted attempt in journal
        let create_intent = CreateAttemptIntent {
            attempt_id: "att-cancel-test".into(),
            job_id: "job-1".into(),
            provider: CloudProvider::Modal,
            profile_id: "modal-prof-1".into(),
            endpoint_fingerprint:
                "0000000000000000000000000000000000000000000000000000000000000000".into(),
            recipe_id: "sdnq-v1".into(),
            preprocessing_version: "1.0.0".into(),
            model_id: "flux-schnell".into(),
            model_revision: "0123456789abcdef0123456789abcdef01234567".into(),
            native_mask_conditioning: false,
            width: crop_bounds.w,
            height: crop_bounds.h,
            seed: 42,
            steps: 4,
            guidance_scaled: 350,
            source_image_hash: "0000000000000000000000000000000000000000000000000000000000000000"
                .into(),
            region_id: "reg-1".into(),
            region_revision: 10,
            crop_sha256: "0000000000000000000000000000000000000000000000000000000000000000".into(),
            hint_sha256: "0000000000000000000000000000000000000000000000000000000000000000".into(),
            request_digest: request_digest.clone(),
            chapter_id: Some("chapter-1".into()),
            page_index: Some(0),
        };

        let (_rec, guard) = service.journal.create_intent(create_intent).unwrap();
        service.journal.mark_dispatching(&guard).unwrap();
        service
            .journal
            .record_accepted_handle(&guard, "h-cancel-123")
            .unwrap();
        drop(guard);

        let decision = service
            .cancel_cloud_render("att-cancel-test", &config)
            .unwrap();

        assert!(matches!(decision, RecoveryDecision::Terminal { .. }));

        let guard = service.journal.acquire_lock("att-cancel-test").unwrap();
        let record = service.journal.read_record_locked(&guard).unwrap();
        assert!(matches!(record.phase, AttemptPhase::Cancelled { .. }));
        assert!(record.is_terminal());
    }

    /// Fault Test 4: Cached results recover via decode validation without resubmitting.
    #[test]
    fn test_fault_cached_results_recover_without_resubmit() {
        let scratch = test_scratch("cached-result-recovery");
        let (manifest, _bounds) = setup_test_job(&scratch, "reg-1");

        let submit_count = Arc::new(AtomicUsize::new(0));
        let sc_clone = Arc::clone(&submit_count);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let req_str = String::from_utf8_lossy(&buf);
                if req_str.starts_with("POST /mc/v1/jobs") {
                    sc_clone.fetch_add(1, Ordering::SeqCst);
                }
            }
        });

        let crop_bounds = test_crop_bounds(&manifest, "reg-1");

        let _endpoint_url = format!("http://127.0.0.1:{port}/mc/v1");
        let config = test_config();

        let grant_service = Arc::new(GrantService::new());
        let consent_service = Arc::new(ConsentService::new_isolated(Arc::clone(&grant_service)));
        let secret_mgr = Arc::new(SecretManager::new_in_memory());

        let service = InferenceService::new_isolated(
            scratch.join("journal"),
            cleaner_core::cloud_wire::provisional_fixture_limits(),
            consent_service,
            grant_service,
            secret_mgr,
        );

        let job = Job::open(&manifest).unwrap();
        let patch_rec = job
            .project
            .patches
            .iter()
            .find(|r| r.id == "reg-1")
            .unwrap();
        let patch = job.load_patch(patch_rec).unwrap();
        let source_path = job.source_path(0).unwrap();
        let source_bytes = std::fs::read(source_path).unwrap();
        let source_hash = sha256_hex(&source_bytes);
        let (revision, _) =
            compute_region_revision_hash(&source_hash, patch_rec, &patch.mask, &patch.ink);

        let raster_res = cleaner_core::image::Raster {
            width: crop_bounds.w,
            height: crop_bounds.h,
            mode: cleaner_core::image::ColorMode::Rgb,
            depth: cleaner_core::image::BitDepth::Eight,
            icc: None,
            palette: None,
            trns: None,
            srgb_intent: None,
            data: vec![199; (crop_bounds.w * crop_bounds.h * 3) as usize],
        };
        let res_png = cleaner_core::image::encode(&raster_res, Format::Png).unwrap();
        let res_digest = sha256_hex(&res_png);

        let req_meta = JobRequestMetadata {
            protocol_version: PROTOCOL_VERSION.to_string(),
            job_id: "job-rec-1".to_string(),
            attempt_id: "att-rec-1".to_string(),
            recipe: WireRenderRecipe {
                recipe_id: "sdnq-v1".into(),
                preprocessing_version: "1.0.0".into(),
                model_id: "flux-schnell".into(),
                model_revision: "0123456789abcdef0123456789abcdef01234567".into(),
                native_mask_conditioning: false,
            },
            width: crop_bounds.w,
            height: crop_bounds.h,
            seed: 42,
            steps: 4,
            guidance_scaled: 350,
            image_sha256: "0000000000000000000000000000000000000000000000000000000000000000".into(),
            hint_sha256: "0000000000000000000000000000000000000000000000000000000000000000".into(),
            request_digest: String::new(),
        };
        let request_digest = compute_request_digest(&req_meta);
        let mut req_meta = req_meta;
        req_meta.request_digest = request_digest.clone();

        let create_intent = CreateAttemptIntent {
            attempt_id: "att-rec-1".into(),
            job_id: "job-rec-1".into(),
            provider: CloudProvider::Modal,
            profile_id: "modal-prof-1".into(),
            endpoint_fingerprint:
                "0000000000000000000000000000000000000000000000000000000000000000".into(),
            recipe_id: "sdnq-v1".into(),
            preprocessing_version: "1.0.0".into(),
            model_id: "flux-schnell".into(),
            model_revision: "0123456789abcdef0123456789abcdef01234567".into(),
            native_mask_conditioning: false,
            width: crop_bounds.w,
            height: crop_bounds.h,
            seed: 42,
            steps: 4,
            guidance_scaled: 350,
            source_image_hash: source_hash.clone(),
            region_id: "reg-1".into(),
            region_revision: revision,
            crop_sha256: "0000000000000000000000000000000000000000000000000000000000000000".into(),
            hint_sha256: "0000000000000000000000000000000000000000000000000000000000000000".into(),
            request_digest: request_digest.clone(),
            chapter_id: Some("chapter-1".into()),
            page_index: Some(0),
        };

        let (_rec, guard) = service.journal.create_intent(create_intent).unwrap();
        service.journal.mark_dispatching(&guard).unwrap();
        service
            .journal
            .record_accepted_handle(&guard, "h-rec-1")
            .unwrap();

        let result_meta = ResultMetadata {
            handle: "h-rec-1".into(),
            job_id: "job-rec-1".into(),
            attempt_id: "att-rec-1".into(),
            request_digest: request_digest.clone(),
            recipe_id: "sdnq-v1".into(),
            preprocessing_version: "1.0.0".into(),
            model_id: "flux-schnell".into(),
            model_revision: "0123456789abcdef0123456789abcdef01234567".into(),
            native_mask_conditioning: false,
            result_digest: res_digest,
            reported_cost_usd: Some(0.01),
            width: crop_bounds.w,
            height: crop_bounds.h,
            byte_length: res_png.len() as u64,
        };

        // Cache result
        service
            .journal
            .cache_result(&guard, &res_png, &result_meta)
            .unwrap();
        drop(guard);

        // Recover attempt: reconciles cached result and commits without resubmitting
        let decision = service
            .recover_cloud_attempt("att-rec-1", &manifest, &config, &PollOptions::default())
            .unwrap();

        assert!(matches!(
            decision,
            RecoveryDecision::AlreadyCommitted { .. }
        ));
        assert_eq!(submit_count.load(Ordering::SeqCst), 0);
    }

    /// Fault Test 5: Stale revision attachment is rejected and cached result is preserved.
    #[test]
    fn test_fault_stale_revision_attachment_rejected_and_result_retained() {
        let scratch = test_scratch("stale-revision-rejection");
        let (manifest, bounds) = setup_test_job(&scratch, "reg-1");

        let grant_service = Arc::new(GrantService::new());
        let consent_service = Arc::new(ConsentService::new_isolated(Arc::clone(&grant_service)));
        let secret_mgr = Arc::new(SecretManager::new_in_memory());

        let service = InferenceService::new_isolated(
            scratch.join("journal"),
            cleaner_core::cloud_wire::provisional_fixture_limits(),
            consent_service,
            grant_service,
            secret_mgr,
        );

        let job = Job::open(&manifest).unwrap();
        let patch_rec = job
            .project
            .patches
            .iter()
            .find(|r| r.id == "reg-1")
            .unwrap();
        let patch = job.load_patch(patch_rec).unwrap();
        let source_path = job.source_path(0).unwrap();
        let source_bytes = std::fs::read(source_path).unwrap();
        let source_hash = sha256_hex(&source_bytes);
        let (original_revision, _) =
            compute_region_revision_hash(&source_hash, patch_rec, &patch.mask, &patch.ink);

        let crop_bounds = test_crop_bounds(&manifest, "reg-1");

        let raster_res = cleaner_core::image::Raster {
            width: crop_bounds.w,
            height: crop_bounds.h,
            mode: cleaner_core::image::ColorMode::Rgb,
            depth: cleaner_core::image::BitDepth::Eight,
            icc: None,
            palette: None,
            trns: None,
            srgb_intent: None,
            data: vec![210; (crop_bounds.w * crop_bounds.h * 3) as usize],
        };
        let res_png = cleaner_core::image::encode(&raster_res, Format::Png).unwrap();
        let res_digest = sha256_hex(&res_png);

        let req_meta = JobRequestMetadata {
            protocol_version: PROTOCOL_VERSION.to_string(),
            job_id: "job-stale-1".to_string(),
            attempt_id: "att-stale-1".to_string(),
            recipe: WireRenderRecipe {
                recipe_id: "sdnq-v1".into(),
                preprocessing_version: "1.0.0".into(),
                model_id: "flux-schnell".into(),
                model_revision: "0123456789abcdef0123456789abcdef01234567".into(),
                native_mask_conditioning: false,
            },
            width: crop_bounds.w,
            height: crop_bounds.h,
            seed: 42,
            steps: 4,
            guidance_scaled: 350,
            image_sha256: "0000000000000000000000000000000000000000000000000000000000000000".into(),
            hint_sha256: "0000000000000000000000000000000000000000000000000000000000000000".into(),
            request_digest: String::new(),
        };
        let request_digest = compute_request_digest(&req_meta);
        let mut req_meta = req_meta;
        req_meta.request_digest = request_digest.clone();

        let create_intent = CreateAttemptIntent {
            attempt_id: "att-stale-1".into(),
            job_id: "job-stale-1".into(),
            provider: CloudProvider::Modal,
            profile_id: "modal-prof-1".into(),
            endpoint_fingerprint:
                "0000000000000000000000000000000000000000000000000000000000000000".into(),
            recipe_id: "sdnq-v1".into(),
            preprocessing_version: "1.0.0".into(),
            model_id: "flux-schnell".into(),
            model_revision: "0123456789abcdef0123456789abcdef01234567".into(),
            native_mask_conditioning: false,
            width: crop_bounds.w,
            height: crop_bounds.h,
            seed: 42,
            steps: 4,
            guidance_scaled: 350,
            source_image_hash: source_hash.clone(),
            region_id: "reg-1".into(),
            region_revision: original_revision,
            crop_sha256: "0000000000000000000000000000000000000000000000000000000000000000".into(),
            hint_sha256: "0000000000000000000000000000000000000000000000000000000000000000".into(),
            request_digest: request_digest.clone(),
            chapter_id: Some("chapter-1".into()),
            page_index: Some(0),
        };

        let (_rec, guard) = service.journal.create_intent(create_intent).unwrap();
        service.journal.mark_dispatching(&guard).unwrap();
        service
            .journal
            .record_accepted_handle(&guard, "h-stale-1")
            .unwrap();

        let result_meta = ResultMetadata {
            handle: "h-stale-1".into(),
            job_id: "job-stale-1".into(),
            attempt_id: "att-stale-1".into(),
            request_digest: request_digest.clone(),
            recipe_id: "sdnq-v1".into(),
            preprocessing_version: "1.0.0".into(),
            model_id: "flux-schnell".into(),
            model_revision: "0123456789abcdef0123456789abcdef01234567".into(),
            native_mask_conditioning: false,
            result_digest: res_digest,
            reported_cost_usd: Some(0.01),
            width: crop_bounds.w,
            height: crop_bounds.h,
            byte_length: res_png.len() as u64,
        };

        service
            .journal
            .cache_result(&guard, &res_png, &result_meta)
            .unwrap();
        drop(guard);

        // Now modify the project region mask on disk (changing region revision to Drifting)
        {
            let mut job = Job::open(&manifest).unwrap();
            let record = job
                .project
                .patches
                .iter()
                .find(|r| r.id == "reg-1")
                .unwrap();
            let mut patch = job.load_patch(record).unwrap();
            patch.mask = Mask::empty(bounds);
            job.complete_region(0, &patch, None).unwrap();
        }

        let guard = service.journal.acquire_lock("att-stale-1").unwrap();
        let decoded = decode_result_crop(
            &res_png,
            &result_meta,
            &req_meta,
            service.journal.limits(),
            "h-stale-1",
        )
        .unwrap();

        let snapshot = ProjectRegionSnapshot {
            source_image_hash: source_hash.clone(),
            region_id: "reg-1".into(),
            region_revision: original_revision,
            crop_sha256: "0000000000000000000000000000000000000000000000000000000000000000".into(),
            hint_sha256: "0000000000000000000000000000000000000000000000000000000000000000".into(),
        };

        // Attachment fails closed because revision on disk has drifted
        let attach_res = service.attach_patch_to_project(
            &manifest,
            0,
            "reg-1",
            &snapshot,
            &decoded,
            &RenderRecipe::new(
                "sdnq-v1",
                "1.0.0",
                "flux-schnell",
                "0123456789abcdef0123456789abcdef01234567",
                false,
            ),
            CloudProvider::Modal,
            "modal-prof-1",
            "h-stale-1",
            "job-stale-1",
            "att-stale-1",
            &compute_request_digest(&req_meta),
            &result_meta.result_digest,
            None,
            crop_bounds.w,
            crop_bounds.h,
        );

        assert!(matches!(
            attach_res,
            Err(InferenceServiceError::StaleAttachment(_))
        ));

        // Cached result PNG is preserved intact in the journal directory for review
        let cached_png = service.journal.read_cached_png(&guard).unwrap();
        assert_eq!(cached_png, res_png);
    }

    /// Unit Test: Local client construction failure (e.g. missing credentials) leaves grant unconsumed and retryable.
    #[test]
    fn test_grant_not_consumed_on_client_construction_failure() {
        let scratch = test_scratch("no-consume-client-failure");
        let (manifest, _bounds) = setup_test_job(&scratch, "reg-1");

        let config = test_config(); // contains "modal-prof-1"
        let grant_service = Arc::new(GrantService::new());
        let consent_service = Arc::new(ConsentService::new_isolated(Arc::clone(&grant_service)));
        let secret_mgr = Arc::new(SecretManager::new_in_memory());
        // NOTE: SecretManager intentionally does NOT have the secret stored for modal-prof-1!

        let target = ExecutionTarget::Modal {
            profile_id: "modal-prof-1".into(),
        };
        let recipe = RenderRecipe::new(
            "sdnq-v1",
            "1.0.0",
            "flux-schnell",
            "0123456789abcdef0123456789abcdef01234567",
            false,
        );
        let intent = OperationIntent::CleanAnyway;

        // Prepare & confirm proposal, issuing a grant with max_attempts = 1
        let proposal = consent_service
            .prepare_proposal(
                PrepareProposalRequest {
                    chapter_id: "c1".into(),
                    page_index: 0,
                    region_id: Some("reg-1".into()),
                    target: target.clone(),
                    recipe: recipe.clone(),
                    intent: intent.clone(),
                },
                &config,
                true,
                &manifest,
            )
            .unwrap();

        let grant = consent_service
            .confirm_proposal(ConfirmProposalOptions {
                proposal_id: &proposal.proposal_id,
                intent: &intent,
                config: &config,
                cloud_allowed: true,
                job_path: &manifest,
                grant_ttl: Duration::from_secs(60),
                max_attempts: 1,
            })
            .unwrap();

        let service = InferenceService::new_isolated(
            scratch.join("journal"),
            cleaner_core::cloud_wire::provisional_fixture_limits(),
            consent_service,
            Arc::clone(&grant_service),
            secret_mgr,
        );

        // Execute render: fails because secret is missing during client construction
        let res = service.execute_cloud_render(
            &grant.nonce,
            &manifest,
            0,
            "reg-1",
            &target,
            &recipe,
            &intent,
            &config,
            true,
            &PollOptions::default(),
        );

        assert!(matches!(res, Err(InferenceServiceError::CredentialMissing(_))));

        // Grant attempt MUST NOT be consumed (attempts_used == 0)
        let grant_state = grant_service.get_grant(&grant.nonce).expect("grant exists");
        assert_eq!(grant_state.attempts_used, 0);

        // Grant is still usable / valid for a retry
        assert!(grant_service.validate_and_consume(&grant.nonce, &grant.scope).is_ok());
    }

    /// Unit Test: Local revalidation failure leaves grant unconsumed and retryable.
    #[test]
    fn test_grant_not_consumed_on_local_revalidation_failure() {
        let scratch = test_scratch("no-consume-reval-failure");
        let (manifest, bounds) = setup_test_job(&scratch, "reg-1");

        let config = test_config();
        let grant_service = Arc::new(GrantService::new());
        let consent_service = Arc::new(ConsentService::new_isolated(Arc::clone(&grant_service)));
        let secret_mgr = Arc::new(SecretManager::new_in_memory());

        // Store valid credential in SecretManager
        let (_, origin_fp) = validate_https_endpoint("https://modal.run/mc/v1").unwrap();
        let key = SecretKey::new(
            CloudProvider::Modal,
            "modal-prof-1".to_string(),
            origin_fp,
            SecretRole::Runtime,
        );
        let compound = crate::inference::secrets::encode_modal_runtime_secret("tid", "tsec").unwrap();
        secret_mgr.store_secret(&key, compound, true).unwrap();

        let target = ExecutionTarget::Modal {
            profile_id: "modal-prof-1".into(),
        };
        let recipe = RenderRecipe::new(
            "sdnq-v1",
            "1.0.0",
            "flux-schnell",
            "0123456789abcdef0123456789abcdef01234567",
            false,
        );
        let intent = OperationIntent::CleanAnyway;

        let proposal = consent_service
            .prepare_proposal(
                PrepareProposalRequest {
                    chapter_id: "c1".into(),
                    page_index: 0,
                    region_id: Some("reg-1".into()),
                    target: target.clone(),
                    recipe: recipe.clone(),
                    intent: intent.clone(),
                },
                &config,
                true,
                &manifest,
            )
            .unwrap();

        let grant = consent_service
            .confirm_proposal(ConfirmProposalOptions {
                proposal_id: &proposal.proposal_id,
                intent: &intent,
                config: &config,
                cloud_allowed: true,
                job_path: &manifest,
                grant_ttl: Duration::from_secs(60),
                max_attempts: 1,
            })
            .unwrap();

        // Mutate the local disk state (e.g. empty mask) to cause local revalidation to fail
        {
            let mut job = Job::open(&manifest).unwrap();
            let patch_rec = job.project.patches.iter().find(|r| r.id == "reg-1").unwrap();
            let mut patch = job.load_patch(patch_rec).unwrap();
            patch.mask = Mask::empty(bounds);
            job.complete_region(0, &patch, None).unwrap();
        }

        let service = InferenceService::new_isolated(
            scratch.join("journal"),
            cleaner_core::cloud_wire::provisional_fixture_limits(),
            consent_service,
            Arc::clone(&grant_service),
            secret_mgr,
        );

        let res = service.execute_cloud_render(
            &grant.nonce,
            &manifest,
            0,
            "reg-1",
            &target,
            &recipe,
            &intent,
            &config,
            true,
            &PollOptions::default(),
        );

        // Should fail during local render preparation
        assert!(res.is_err());

        // Grant attempt MUST NOT be consumed (attempts_used == 0)
        let grant_state = grant_service.get_grant(&grant.nonce).expect("grant exists");
        assert_eq!(grant_state.attempts_used, 0);

        // Grant is still usable / unburned
        assert!(grant_service.validate_and_consume(&grant.nonce, &grant.scope).is_ok());
    }

    /// Unit Test: Network dispatch consumes exactly one attempt on the grant.
    #[test]
    fn test_one_grant_consumed_on_dispatch() {
        let scratch = test_scratch("one-consume-on-dispatch");
        let (manifest, _bounds) = setup_test_job(&scratch, "reg-1");

        let submit_count = Arc::new(AtomicUsize::new(0));
        let sc_clone = Arc::clone(&submit_count);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let crop_bounds = test_crop_bounds(&manifest, "reg-1");

        let raster_res = cleaner_core::image::Raster {
            width: crop_bounds.w,
            height: crop_bounds.h,
            mode: cleaner_core::image::ColorMode::Rgb,
            depth: cleaner_core::image::BitDepth::Eight,
            icc: None,
            palette: None,
            trns: None,
            srgb_intent: None,
            data: vec![220; (crop_bounds.w * crop_bounds.h * 3) as usize],
        };
        let res_png = cleaner_core::image::encode(&raster_res, Format::Png).unwrap();
        let res_digest = sha256_hex(&res_png);
        let res_png_clone = res_png.clone();

        let req_info = Arc::new(std::sync::Mutex::new((
            String::new(),
            String::new(),
            String::new(),
        )));
        let req_info_clone = Arc::clone(&req_info);

        thread::spawn(move || {
            let find_field = |req: &str, field: &str| -> String {
                let needle = format!("\"{field}\":\"");
                if let Some(idx) = req.find(&needle) {
                    let rest = &req[idx + needle.len()..];
                    if let Some(end) = rest.find('"') {
                        return rest[..end].to_string();
                    }
                }
                String::new()
            };

            while let Ok((mut stream, _)) = listener.accept() {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                loop {
                    let n = match stream.read(&mut tmp) {
                        Ok(n) if n > 0 => n,
                        _ => break,
                    };
                    buf.extend_from_slice(&tmp[..n]);
                    let s = String::from_utf8_lossy(&buf);
                    if s.contains("GET ") && s.contains("\r\n\r\n") {
                        break;
                    }
                    if s.contains("POST ") && find_field(&s, "request_digest").len() == 64 {
                        break;
                    }
                    if buf.len() > 65536 {
                        break;
                    }
                }
                let req_str = String::from_utf8_lossy(&buf);

                if req_str.starts_with("POST /mc/v1/jobs") {
                    sc_clone.fetch_add(1, Ordering::SeqCst);
                    let job_id = find_field(&req_str, "job_id");
                    let attempt_id = find_field(&req_str, "attempt_id");
                    let digest = find_field(&req_str, "request_digest");
                    *req_info_clone.lock().unwrap() =
                        (job_id.clone(), attempt_id.clone(), digest.clone());

                    let json = format!(
                        r#"{{
                        "handle": "h-test-dispatch-1",
                        "status": "pending",
                        "job_id": "{job_id}",
                        "attempt_id": "{attempt_id}",
                        "request_digest": "{digest}",
                        "recipe_id": "sdnq-v1",
                        "preprocessing_version": "1.0.0",
                        "model_id": "flux-schnell",
                        "model_revision": "0123456789abcdef0123456789abcdef01234567",
                        "native_mask_conditioning": false
                    }}"#
                    );
                    let resp = format!(
                        "HTTP/1.1 202 Accepted\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        json.len(),
                        json
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if req_str.contains("GET /mc/v1/jobs/h-test-dispatch-1/result") {
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\n\r\n",
                        res_png_clone.len()
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.write_all(&res_png_clone);
                } else if req_str.contains("GET /mc/v1/jobs/h-test-dispatch-1") {
                    let (job_id, attempt_id, digest) = req_info_clone.lock().unwrap().clone();
                    let json = format!(
                        r#"{{
                        "handle": "h-test-dispatch-1",
                        "job_id": "{job_id}",
                        "attempt_id": "{attempt_id}",
                        "request_digest": "{digest}",
                        "recipe_id": "sdnq-v1",
                        "preprocessing_version": "1.0.0",
                        "model_id": "flux-schnell",
                        "model_revision": "0123456789abcdef0123456789abcdef01234567",
                        "native_mask_conditioning": false,
                        "status": "completed",
                        "result_digest": "{}",
                        "result_bytes": {},
                        "reported_cost_usd": 0.01
                    }}"#,
                        res_digest,
                        res_png_clone.len()
                    );
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        json.len(),
                        json
                    );
                    let _ = stream.write_all(resp.as_bytes());
                }
            }
        });

        let endpoint_url = format!("http://127.0.0.1:{port}/mc/v1");
        let config = test_config();

        let grant_service = Arc::new(GrantService::new());
        let consent_service = Arc::new(ConsentService::new_isolated(Arc::clone(&grant_service)));
        let secret_mgr = Arc::new(SecretManager::new_in_memory());

        let target = ExecutionTarget::Modal {
            profile_id: "modal-prof-1".into(),
        };
        let recipe = RenderRecipe::new(
            "sdnq-v1",
            "1.0.0",
            "flux-schnell",
            "0123456789abcdef0123456789abcdef01234567",
            false,
        );
        let intent = OperationIntent::CleanAnyway;

        let proposal = consent_service
            .prepare_proposal(
                PrepareProposalRequest {
                    chapter_id: "c1".into(),
                    page_index: 0,
                    region_id: Some("reg-1".into()),
                    target: target.clone(),
                    recipe: recipe.clone(),
                    intent: intent.clone(),
                },
                &config,
                true,
                &manifest,
            )
            .unwrap();

        let grant = consent_service
            .confirm_proposal(ConfirmProposalOptions {
                proposal_id: &proposal.proposal_id,
                intent: &intent,
                config: &config,
                cloud_allowed: true,
                job_path: &manifest,
                grant_ttl: Duration::from_secs(60),
                max_attempts: 1,
            })
            .unwrap();

        let http_target = CloudEndpointTarget::new_test_target(
            CloudProvider::Modal,
            "modal-prof-1",
            &endpoint_url,
        )
        .unwrap();

        let bound_cred = BoundRuntimeCredential::new(
            &http_target,
            RuntimeCredential::ModalProxy {
                token_id: "tid".into(),
                token_secret: SecretValue::new("tsec"),
            },
        )
        .unwrap();

        let custom_client = CloudHttpClient::new_test_client(
            http_target,
            bound_cred,
            reqwest::blocking::Client::new(),
        );

        let service = InferenceService::new_isolated(
            scratch.join("journal"),
            cleaner_core::cloud_wire::provisional_fixture_limits(),
            consent_service,
            Arc::clone(&grant_service),
            secret_mgr,
        )
        .with_custom_client(custom_client);

        let res = service.execute_cloud_render(
            &grant.nonce,
            &manifest,
            0,
            "reg-1",
            &target,
            &recipe,
            &intent,
            &config,
            true,
            &PollOptions {
                poll_interval: Duration::from_millis(10),
                timeout: Duration::from_secs(5),
                max_polls: 50,
            },
        );

        match &res {
            Ok(_) => {}
            Err(e) => panic!("expected execute_cloud_render to succeed, got Err: {e:?}"),
        }
        assert_eq!(submit_count.load(Ordering::SeqCst), 1);

        // Grant attempt MUST be consumed exactly once (attempts_used == 1)
        let grant_state = grant_service.get_grant(&grant.nonce).expect("grant exists");
        assert_eq!(grant_state.attempts_used, 1);

        // Reusing the same single-use grant must fail closed with AttemptsExceeded
        assert_eq!(
            grant_service.validate_and_consume(&grant.nonce, &grant.scope),
            Err(GrantError::AttemptsExceeded {
                max_attempts: 1,
                attempts_used: 1,
            })
        );
    }

    fn setup_two_page_service_job(root: &Path, patch_id: &str) -> (PathBuf, Rect) {
        let raws = root.join("raws");
        std::fs::create_dir_all(&raws).unwrap();
        let mut raster = fixtures::by_name("l8").raster;
        raster.width = 256;
        raster.height = 256;
        raster.data = vec![200; 256 * 256];
        let bytes = cleaner_core::image::encode(&raster, Format::Png).unwrap();
        let page0_path = raws.join("001.png");
        let page1_path = raws.join("002.png");
        std::fs::write(&page0_path, &bytes).unwrap();
        std::fs::write(&page1_path, &bytes).unwrap();

        let source_ref0 = cleaner_core::ingest::source_ref(&page0_path, &bytes).unwrap();
        let source_ref1 = cleaner_core::ingest::source_ref(&page1_path, &bytes).unwrap();
        let manifest = root.join("job/chapter.mtclean");
        std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        let project = Project::new(
            manifest.parent().unwrap(),
            "test-app",
            StripMode::Single,
            &[source_ref0, source_ref1],
        );
        let mut job = Job::create(&manifest, project).unwrap();

        let bounds = Rect::new(100, 100, 32, 32);
        let mut pixels = raster.clone();
        pixels.width = bounds.w;
        pixels.height = bounds.h;
        pixels.data = vec![200; (bounds.w * bounds.h) as usize];

        let patch = Patch {
            id: patch_id.to_string(),
            mask: Mask::filled(bounds),
            ink: Mask::filled(bounds),
            pixels,
            order: 1,
            visible: true,
            provenance: Provenance {
                engine: Engine::Fill,
                engine_version: "1.0".into(),
                model_sha256: None,
                execution_provider: "cpu".into(),
                params_snapshot: serde_json::json!({ "thickness": 2 }),
                mask_sha256: sha256_hex(&vec![255; (bounds.w * bounds.h) as usize]),
                source_sha256: sha256_hex(&bytes),
                cloud: None,
                created: 1_760_000_000,
            },
        };

        job.complete_region(0, &patch, None).unwrap();
        (manifest, bounds)
    }

    #[test]
    fn test_execute_cloud_render_rejects_page_deletion_before_intent_grant_network() {
        let scratch = test_scratch("exec-page-del");
        let (manifest, _bounds) = setup_test_job(&scratch, "reg-del");

        let grant_service = Arc::new(GrantService::new());
        let consent_service = Arc::new(ConsentService::new_isolated(Arc::clone(&grant_service)));
        let secret_mgr = Arc::new(SecretManager::new_in_memory());
        let config = test_config();

        // Store valid credential in SecretManager so client construction succeeds and pre-POST revalidation is reached
        let (_, origin_fp) = validate_https_endpoint("https://modal.run/mc/v1").unwrap();
        let key = SecretKey::new(
            CloudProvider::Modal,
            "modal-prof-1".to_string(),
            origin_fp,
            SecretRole::Runtime,
        );
        let compound = crate::inference::secrets::encode_modal_runtime_secret("tid", "tsec").unwrap();
        secret_mgr.store_secret(&key, compound, true).unwrap();

        let target = ExecutionTarget::Modal {
            profile_id: "modal-prof-1".into(),
        };
        let recipe = RenderRecipe::new(
            "sdnq-v1",
            "1.0.0",
            "flux-schnell",
            "0123456789abcdef0123456789abcdef01234567",
            false,
        );
        let intent = OperationIntent::CleanAnyway;

        let proposal = consent_service
            .prepare_proposal(
                PrepareProposalRequest {
                    chapter_id: "c1".into(),
                    page_index: 0,
                    region_id: Some("reg-del".into()),
                    target: target.clone(),
                    recipe: recipe.clone(),
                    intent: intent.clone(),
                },
                &config,
                true,
                &manifest,
            )
            .unwrap();

        let grant = consent_service
            .confirm_proposal(ConfirmProposalOptions {
                proposal_id: &proposal.proposal_id,
                intent: &intent,
                config: &config,
                cloud_allowed: true,
                job_path: &manifest,
                grant_ttl: Duration::from_secs(60),
                max_attempts: 1,
            })
            .unwrap();

        let http_target = CloudEndpointTarget::new_test_target(
            CloudProvider::Modal,
            "modal-prof-1",
            "https://modal.run/mc/v1",
        )
        .unwrap();

        let bound_cred = BoundRuntimeCredential::new(
            &http_target,
            RuntimeCredential::ModalProxy {
                token_id: "tid".into(),
                token_secret: SecretValue::new("tsec"),
            },
        )
        .unwrap();

        let custom_client = CloudHttpClient::new_test_client(
            http_target,
            bound_cred,
            reqwest::blocking::Client::new(),
        );

        let service = InferenceService::new_isolated(
            scratch.join("journal"),
            cleaner_core::cloud_wire::provisional_fixture_limits(),
            consent_service,
            Arc::clone(&grant_service),
            secret_mgr,
        )
        .with_custom_client(custom_client);

        // Execute with deleted/out-of-bounds page_index
        let res = service.execute_cloud_render(
            &grant.nonce,
            &manifest,
            999,
            "reg-del",
            &target,
            &recipe,
            &intent,
            &config,
            true,
            &PollOptions::default(),
        );

        match &res {
            Err(InferenceServiceError::Consent(ConsentError::PageNotFound(999, _))) => {}
            other => panic!("expected ConsentError::PageNotFound(999, _), got: {other:?}"),
        }

        // Invariant: grant is NOT consumed and journal has NO attempt created
        let grant_state = grant_service.get_grant(&grant.nonce).unwrap();
        assert_eq!(grant_state.attempts_used, 0);
        assert!(service.journal().list_attempt_ids().unwrap().is_empty());
    }

    #[test]
    fn test_execute_cloud_render_rejects_page_reorder_and_patch_source_mismatch_before_intent_grant_network() {
        let scratch = test_scratch("exec-reorder-mismatch");
        let (manifest, _bounds) = setup_two_page_service_job(&scratch, "reg-reorder");

        let grant_service = Arc::new(GrantService::new());
        let consent_service = Arc::new(ConsentService::new_isolated(Arc::clone(&grant_service)));
        let secret_mgr = Arc::new(SecretManager::new_in_memory());
        let config = test_config();

        // Store valid credential in SecretManager so client construction succeeds and pre-POST revalidation is reached
        let (_, origin_fp) = validate_https_endpoint("https://modal.run/mc/v1").unwrap();
        let key = SecretKey::new(
            CloudProvider::Modal,
            "modal-prof-1".to_string(),
            origin_fp,
            SecretRole::Runtime,
        );
        let compound = crate::inference::secrets::encode_modal_runtime_secret("tid", "tsec").unwrap();
        secret_mgr.store_secret(&key, compound, true).unwrap();

        let target = ExecutionTarget::Modal {
            profile_id: "modal-prof-1".into(),
        };
        let recipe = RenderRecipe::new(
            "sdnq-v1",
            "1.0.0",
            "flux-schnell",
            "0123456789abcdef0123456789abcdef01234567",
            false,
        );
        let intent = OperationIntent::CleanAnyway;

        let proposal = consent_service
            .prepare_proposal(
                PrepareProposalRequest {
                    chapter_id: "c1".into(),
                    page_index: 0,
                    region_id: Some("reg-reorder".into()),
                    target: target.clone(),
                    recipe: recipe.clone(),
                    intent: intent.clone(),
                },
                &config,
                true,
                &manifest,
            )
            .unwrap();

        let grant = consent_service
            .confirm_proposal(ConfirmProposalOptions {
                proposal_id: &proposal.proposal_id,
                intent: &intent,
                config: &config,
                cloud_allowed: true,
                job_path: &manifest,
                grant_ttl: Duration::from_secs(60),
                max_attempts: 1,
            })
            .unwrap();

        // Reorder pages in manifest so page 0 now resolves to source 1, while region is on source 0
        {
            let mut job = Job::open(&manifest).unwrap();
            job.project.strip.order = vec![1, 0];
            job.flush().unwrap();
        }

        let http_target = CloudEndpointTarget::new_test_target(
            CloudProvider::Modal,
            "modal-prof-1",
            "https://modal.run/mc/v1",
        )
        .unwrap();

        let bound_cred = BoundRuntimeCredential::new(
            &http_target,
            RuntimeCredential::ModalProxy {
                token_id: "tid".into(),
                token_secret: SecretValue::new("tsec"),
            },
        )
        .unwrap();

        let custom_client = CloudHttpClient::new_test_client(
            http_target,
            bound_cred,
            reqwest::blocking::Client::new(),
        );

        let service = InferenceService::new_isolated(
            scratch.join("journal"),
            cleaner_core::cloud_wire::provisional_fixture_limits(),
            consent_service,
            Arc::clone(&grant_service),
            secret_mgr,
        )
        .with_custom_client(custom_client);

        let res = service.execute_cloud_render(
            &grant.nonce,
            &manifest,
            0,
            "reg-reorder",
            &target,
            &recipe,
            &intent,
            &config,
            true,
            &PollOptions::default(),
        );

        match &res {
            Err(InferenceServiceError::Consent(ConsentError::PageMismatch)) => {}
            other => panic!("expected ConsentError::PageMismatch, got: {other:?}"),
        }

        // Invariant: grant is NOT consumed and journal has NO attempt created
        let grant_state = grant_service.get_grant(&grant.nonce).unwrap();
        assert_eq!(grant_state.attempts_used, 0);
        assert!(service.journal().list_attempt_ids().unwrap().is_empty());
    }
}
