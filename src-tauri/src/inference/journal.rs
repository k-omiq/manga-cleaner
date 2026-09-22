//! Hardened local attempt journal and validated result cache for cloud inference (P4a).
//!
//! Provides a crash-resilient, transactional local state machine for tracking cloud diffusion
//! inference attempts, locking attempts across processes, validating and caching output PNGs,
//! and reconciling project attachment snapshots.
//!
//! ## Invariants
//!
//! 1. **Durable Intent Before Dispatch:** Persisted and fsynced before dispatch permit is granted.
//! 2. **Durable Dispatching Before Request:** Transitions to `Dispatching` and fsyncs before HTTP POST.
//! 3. **Crash During Intent/Dispatching Recovers as Ambiguous Unknown:** Process crash or interruption
//!    recovers as `Unknown` unless explicitly guaranteed by caller that no network transmission occurred.
//! 4. **Billing Safety — Unknown NEVER Auto-Retried:** `Unknown` attempts strictly forbid auto-retry.
//! 5. **Accepted Handle Persisted Before Polling:** Server handles from `202 Accepted` are persisted
//!    and fsynced before status polling or result fetching commences.
//! 6. **Existing Known Handle Reusable:** Status/polling/download failures preserve durable handle for retry.
//! 7. **Non-Terminal Cancellation Request:** `CancelRequested` remains non-terminal until remote completion
//!    or cancellation is confirmed by authoritative gateway status.
//! 8. **Mandatory Core Decode Validation Before Caching:** Result bytes MUST pass full
//!    [`cleaner_core::cloud_decode::decode_result_crop`] validation. Caller claims of validity are rejected.
//! 9. **Crash Recovery for Undiscoverable Results:** Result write executes in 3 steps: metadata first,
//!    PNG second, journal third. Recovery reconciles metadata+PNG via full decode to restore `ResultCached`.
//! 10. **Two-Phase Project Attachment:** Persists `AttachmentPending` with stable `patch_id` and result
//!     identity BEFORE external project commit. Recovery verifies project by same `patch_id` and never
//!     blindly attaches again.
//! 11. **Stale Attachment Rejection with Result Retention:** If project region snapshot (source, region,
//!     mask, revision) drifted, attachment fails closed (`StaleAttachment`), but validated result is retained.
//! 12. **Duplicate Commit Prevention:** Commits record the patch identity, ensuring idempotent commit.
//! 13. **Safe Opaque Identifiers:** Bounded `[a-zA-Z0-9_-]` to eliminate directory traversal.
//! 14. **Zero Secrets in Journal Storage:** No tokens, bearer credentials, or grant nonces are ever stored.
//! 15. **Cross-Process Exclusive RAII Locking:** Kernel file locks (`flock`/`LockFile`) release on crash;
//!     stale PID guessing is strictly rejected. Unsupported OS platforms fail closed.
//! 16. **Guard Root Binding:** Locks are bound to canonical journal roots; foreign guards fail closed.
//! 17. **Durability Ordering:** In-memory state never advances if disk persistence fails.
//! 18. **Windows Durability Limitation:** Windows directory sync is unverified/unsupported via Win32 File
//!     handles; directory entry durability relies on NTFS transaction journaling.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use cleaner_core::cloud_decode::{decode_result_crop, DecodedResultCrop, ResultDecodeError};
use cleaner_core::cloud_wire::{
    compute_request_digest, validate_ascii_id, validate_lowercase_hex_hash,
    JobRequestMetadata, ResultMetadata, ServiceLimits, WireRenderRecipe, WireValidationError,
    PROTOCOL_VERSION,
};
use cleaner_core::engines::render::{CloudProvider, LATENT_STRIDE};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const MAX_JOURNAL_JSON_BYTES: u64 = 128 * 1024;
pub const CURRENT_JOURNAL_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Error)]
pub enum JournalError {
    #[error("invalid identifier '{value}' in field '{field}': must be 1..=128 chars of [a-zA-Z0-9_-]")]
    InvalidIdentifier { field: &'static str, value: String },
    #[error("unsupported journal schema version: expected {expected}, got {actual}")]
    UnsupportedSchemaVersion { expected: u32, actual: u32 },
    #[error("foreign lock guard: guard root '{guard_root}' does not match journal root '{expected_root}'")]
    ForeignGuard { expected_root: String, guard_root: String },
    #[error("cloud wire contract validation error: {0}")]
    Wire(#[from] WireValidationError),
    #[error("cloud result decode error: {0}")]
    Decode(#[from] ResultDecodeError),
    #[error("attempt '{attempt_id}' is exclusively locked by another process or descriptor")]
    AttemptLocked { attempt_id: String },
    #[error("attempt '{attempt_id}' already exists")]
    AttemptAlreadyExists { attempt_id: String },
    #[error("attempt '{attempt_id}' not found")]
    AttemptNotFound { attempt_id: String },
    #[error("invalid lifecycle state transition from {current} to {attempted}")]
    InvalidTransition { current: String, attempted: String },
    #[error("stale attachment: snapshot mismatch on field '{field}': expected '{expected}', got '{actual}'")]
    StaleAttachment { field: &'static str, expected: String, actual: String },
    #[error("duplicate project commit: attempt already committed with patch id '{patch_id}'")]
    DuplicateCommit { patch_id: String },
    #[error("payload limit exceeded for {field}: {actual} bytes exceeds maximum {max} bytes")]
    LimitExceeded { field: &'static str, actual: u64, max: u64 },
    #[error("canonical request digest mismatch: expected {expected}, computed {computed}")]
    RequestDigestMismatch { expected: String, computed: String },
    #[error("cached result digest mismatch: expected {expected}, computed {computed}")]
    ResultDigestMismatch { expected: String, computed: String },
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON serialization/deserialization error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Validate that an identifier is bounded and safe for filesystem paths without traversal.
pub fn validate_safe_id(id: &str, field: &'static str) -> Result<(), JournalError> {
    if id.is_empty()
        || id.len() > 128
        || id == "."
        || id == ".."
        || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(JournalError::InvalidIdentifier { field, value: id.to_string() });
    }
    Ok(())
}

/// Cross-process exclusive file lock RAII guard bound to a canonical journal root.
pub struct AttemptLockGuard {
    attempt_id: String,
    canonical_root: PathBuf,
    file: File,
    lock_path: PathBuf,
}

impl AttemptLockGuard {
    pub fn attempt_id(&self) -> &str { &self.attempt_id }
    pub fn canonical_root(&self) -> &Path { &self.canonical_root }
    pub fn lock_path(&self) -> &Path { &self.lock_path }
}

impl Drop for AttemptLockGuard {
    fn drop(&mut self) { unlock_file(&self.file); }
}

#[cfg(unix)]
fn try_lock_file(file: &File) -> Result<(), std::io::Error> {
    use std::os::unix::io::AsRawFd;
    let fd = file.as_raw_fd();
    if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(unix)]
fn unlock_file(file: &File) {
    use std::os::unix::io::AsRawFd;
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN); }
}

#[cfg(windows)]
fn try_lock_file(file: &File) -> Result<(), std::io::Error> {
    use std::os::windows::io::AsRawHandle;
    let ret = unsafe { windows_sys::Win32::Storage::FileSystem::LockFile(file.as_raw_handle() as _, 0, 0, 1, 0) };
    if ret == 0 { return Err(std::io::Error::last_os_error()); }
    Ok(())
}

#[cfg(windows)]
fn unlock_file(file: &File) {
    use std::os::windows::io::AsRawHandle;
    unsafe { windows_sys::Win32::Storage::FileSystem::UnlockFile(file.as_raw_handle() as _, 0, 0, 1, 0); }
}

#[cfg(not(any(unix, windows)))]
fn try_lock_file(_file: &File) -> Result<(), std::io::Error> {
    Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "cross-process locking unsupported on this OS"))
}
#[cfg(not(any(unix, windows)))]
fn unlock_file(_file: &File) {}

fn durable_create_dir_all(path: &Path) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    let missing: Vec<_> = path.ancestors().take_while(|p| !p.exists()).map(Path::to_path_buf).collect();
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    for directory in missing.iter().rev() {
        File::open(directory)?.sync_all()?;
        if let Some(parent) = directory.parent() {
            File::open(if parent.as_os_str().is_empty() { Path::new(".") } else { parent })?.sync_all()?;
        }
    }
    Ok(())
}

fn atomic_write_file(target_path: &Path, bytes: &[u8]) -> Result<(), JournalError> {
    let parent = target_path.parent().ok_or_else(|| {
        JournalError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "no parent dir"))
    })?;
    durable_create_dir_all(parent)?;

    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let pid = std::process::id();
    let file_name = target_path.file_name().and_then(|s| s.to_str()).unwrap_or("tmp");
    let temp_path = parent.join(format!(".{}.tmp.{}_{}_{}", file_name, pid, nanos, count));

    let res = (|| -> Result<(), std::io::Error> {
        let mut f = OpenOptions::new().write(true).create_new(true).open(&temp_path)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        Ok(())
    })();

    if let Err(e) = res {
        let _ = fs::remove_file(&temp_path);
        return Err(JournalError::Io(e));
    }

    if let Err(e) = fs::rename(&temp_path, target_path) {
        let _ = fs::remove_file(&temp_path);
        return Err(JournalError::Io(e));
    }

    #[cfg(unix)]
    {
        let dir_file = File::open(parent)?;
        dir_file.sync_all()?;
    }
    // This implementation does not flush directory handles on Windows.
    // Power-loss durability there remains unverified.
    Ok(())
}

fn current_epoch_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

#[derive(Debug, Clone)]
pub struct CreateAttemptIntent {
    pub attempt_id: String, pub job_id: String, pub provider: CloudProvider,
    pub profile_id: String, pub endpoint_fingerprint: String,
    pub recipe_id: String, pub preprocessing_version: String,
    pub model_id: String, pub model_revision: String, pub native_mask_conditioning: bool,
    pub width: u32, pub height: u32, pub seed: u64, pub steps: u32, pub guidance_scaled: u32,
    pub source_image_hash: String, pub region_id: String, pub region_revision: u64,
    pub crop_sha256: String, pub hint_sha256: String, pub request_digest: String,
    pub chapter_id: Option<String>,
    pub page_index: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectRegionSnapshot {
    pub source_image_hash: String,
    pub region_id: String,
    pub region_revision: u64,
    pub crop_sha256: String,
    pub hint_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
pub enum AttemptPhase {
    Intent,
    Dispatching,
    Accepted { handle: String },
    CancelRequested { handle: String },
    ResultCached {
        handle: String, result_digest: String, cached_png_bytes: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")] reported_cost_usd: Option<f64>,
    },
    AttachmentPending {
        handle: String, patch_id: String, result_digest: String, cached_png_bytes: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")] reported_cost_usd: Option<f64>,
    },
    Committed {
        handle: String, patch_id: String, result_digest: String, cached_png_bytes: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")] reported_cost_usd: Option<f64>,
    },
    Unknown { reason: String },
    Cancelled {
        #[serde(default, skip_serializing_if = "Option::is_none")] handle: Option<String>,
        reason: String,
    },
    Failed {
        #[serde(default, skip_serializing_if = "Option::is_none")] handle: Option<String>,
        error_code: String, message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptRecord {
    pub schema_version: u32,
    pub attempt_id: String, pub job_id: String, pub provider: CloudProvider,
    pub profile_id: String, pub endpoint_fingerprint: String,
    pub recipe_id: String, pub preprocessing_version: String,
    pub model_id: String, pub model_revision: String, pub native_mask_conditioning: bool,
    pub source_image_hash: String, pub region_id: String, pub region_revision: u64,
    pub crop_sha256: String, pub hint_sha256: String,
    pub width: u32, pub height: u32, pub seed: u64, pub steps: u32, pub guidance_scaled: u32,
    pub request_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chapter_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_index: Option<u32>,
    pub created_at_ms: u64, pub updated_at_ms: u64,
    pub phase: AttemptPhase,
}

impl AttemptRecord {
    pub fn is_terminal(&self) -> bool {
        matches!(self.phase, AttemptPhase::Committed { .. } | AttemptPhase::Cancelled { .. } | AttemptPhase::Failed { .. } | AttemptPhase::Unknown { .. })
    }
    pub fn is_retryable(&self) -> bool { false }
    pub fn to_request_metadata(&self) -> JobRequestMetadata {
        JobRequestMetadata {
            protocol_version: PROTOCOL_VERSION.to_string(),
            job_id: self.job_id.clone(), attempt_id: self.attempt_id.clone(),
            recipe: WireRenderRecipe {
                recipe_id: self.recipe_id.clone(), preprocessing_version: self.preprocessing_version.clone(),
                model_id: self.model_id.clone(), model_revision: self.model_revision.clone(),
                native_mask_conditioning: self.native_mask_conditioning,
            },
            width: self.width, height: self.height, seed: self.seed, steps: self.steps,
            guidance_scaled: self.guidance_scaled, image_sha256: self.crop_sha256.clone(),
            hint_sha256: self.hint_sha256.clone(), request_digest: self.request_digest.clone(),
        }
    }
}

#[derive(Debug)]
pub struct DispatchPermit { attempt_id: String }
impl DispatchPermit { pub fn attempt_id(&self) -> &str { &self.attempt_id } }

#[derive(Debug, Clone, PartialEq)]
pub enum RecoveryDecision {
    AlreadyCommitted { patch_id: String },
    AttachmentPendingVerification { patch_id: String, result_digest: String },
    ResultCachedReadyForAttach { handle: String, result_digest: String },
    ResumePolling { handle: String },
    ResumeCancelPolling { handle: String },
    CorruptedResultRetainedForInspection { handle: String, reason: String },
    Terminal { message: String },
    AmbiguousUnknown { reason: String },
}

impl RecoveryDecision {
    pub fn is_auto_retryable(&self) -> bool { false }
}

#[derive(Debug, Clone)]
pub struct AttemptJournal {
    root_dir: PathBuf,
    limits: ServiceLimits,
}

impl AttemptJournal {
    pub fn new(root_dir: impl Into<PathBuf>, limits: ServiceLimits) -> Self {
        Self { root_dir: root_dir.into(), limits }
    }

    pub fn root_dir(&self) -> &Path { &self.root_dir }
    pub fn limits(&self) -> &ServiceLimits { &self.limits }
    fn canonical_root(&self) -> PathBuf { fs::canonicalize(&self.root_dir).unwrap_or_else(|_| self.root_dir.clone()) }
    fn attempt_dir(&self, id: &str) -> PathBuf { self.root_dir.join("attempts").join(id) }
    fn journal_path(&self, id: &str) -> PathBuf { self.attempt_dir(id).join("journal.json") }
    fn lock_path(&self, id: &str) -> PathBuf { self.attempt_dir(id).join("lock") }
    fn result_png_path(&self, id: &str) -> PathBuf { self.attempt_dir(id).join("result.png") }
    fn result_meta_path(&self, id: &str) -> PathBuf { self.attempt_dir(id).join("result_meta.json") }

    fn verify_guard(&self, guard: &AttemptLockGuard) -> Result<(), JournalError> {
        let canonical = self.canonical_root();
        if guard.canonical_root != canonical {
            return Err(JournalError::ForeignGuard {
                expected_root: canonical.to_string_lossy().to_string(),
                guard_root: guard.canonical_root.to_string_lossy().to_string(),
            });
        }
        validate_safe_id(&guard.attempt_id, "attempt_id")?;
        Ok(())
    }

    pub fn acquire_lock(&self, attempt_id: &str) -> Result<AttemptLockGuard, JournalError> {
        validate_safe_id(attempt_id, "attempt_id")?;
        let attempt_dir = self.attempt_dir(attempt_id);
        if !attempt_dir.exists() {
            return Err(JournalError::AttemptNotFound { attempt_id: attempt_id.to_string() });
        }
        let lock_path = self.lock_path(attempt_id);
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&lock_path)?;
        if try_lock_file(&file).is_err() {
            return Err(JournalError::AttemptLocked { attempt_id: attempt_id.to_string() });
        }
        Ok(AttemptLockGuard { attempt_id: attempt_id.to_string(), canonical_root: self.canonical_root(), file, lock_path })
    }

    pub fn create_intent(&self, intent: CreateAttemptIntent) -> Result<(AttemptRecord, AttemptLockGuard), JournalError> {
        for (id, f) in [(&intent.attempt_id, "attempt_id"), (&intent.job_id, "job_id"), (&intent.profile_id, "profile_id"), (&intent.region_id, "region_id")] {
            validate_safe_id(id, f)?;
        }
        for (h, f, len) in [
            (&intent.endpoint_fingerprint, "endpoint_fingerprint", 64), (&intent.source_image_hash, "source_image_hash", 64),
            (&intent.crop_sha256, "crop_sha256", 64), (&intent.hint_sha256, "hint_sha256", 64),
            (&intent.request_digest, "request_digest", 64),
        ] {
            validate_lowercase_hex_hash(h, f, len)?;
        }
        validate_ascii_id(&intent.recipe_id, "recipe_id", 1, 128)?;
        validate_ascii_id(&intent.preprocessing_version, "preprocessing_version", 1, 32)?;
        validate_ascii_id(&intent.model_id, "model_id", 1, 128)?;

        if intent.width % LATENT_STRIDE != 0 || intent.height % LATENT_STRIDE != 0 {
            return Err(WireValidationError::DimensionNotSnapped { width: intent.width, height: intent.height, stride: LATENT_STRIDE }.into());
        }

        let record = AttemptRecord {
            schema_version: CURRENT_JOURNAL_SCHEMA_VERSION,
            attempt_id: intent.attempt_id, job_id: intent.job_id, provider: intent.provider,
            profile_id: intent.profile_id, endpoint_fingerprint: intent.endpoint_fingerprint,
            recipe_id: intent.recipe_id, preprocessing_version: intent.preprocessing_version,
            model_id: intent.model_id, model_revision: intent.model_revision,
            native_mask_conditioning: intent.native_mask_conditioning,
            source_image_hash: intent.source_image_hash, region_id: intent.region_id, region_revision: intent.region_revision,
            crop_sha256: intent.crop_sha256, hint_sha256: intent.hint_sha256,
            width: intent.width, height: intent.height, seed: intent.seed, steps: intent.steps, guidance_scaled: intent.guidance_scaled,
            request_digest: intent.request_digest,
            chapter_id: intent.chapter_id,
            page_index: intent.page_index,
            created_at_ms: current_epoch_ms(), updated_at_ms: current_epoch_ms(),
            phase: AttemptPhase::Intent,
        };

        let req_meta = record.to_request_metadata();
        cleaner_core::cloud_wire::validate_job_request_metadata(&req_meta)?;
        let computed = compute_request_digest(&req_meta);
        if computed != record.request_digest {
            return Err(JournalError::RequestDigestMismatch { expected: record.request_digest.clone(), computed });
        }

        let attempt_dir = self.attempt_dir(&record.attempt_id);
        durable_create_dir_all(&attempt_dir)?;

        let lock_path = self.lock_path(&record.attempt_id);
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&lock_path)?;
        if try_lock_file(&file).is_err() {
            return Err(JournalError::AttemptLocked { attempt_id: record.attempt_id });
        }
        let guard = AttemptLockGuard { attempt_id: record.attempt_id.clone(), canonical_root: self.canonical_root(), file, lock_path };
        if self.journal_path(&record.attempt_id).exists() {
            return Err(JournalError::AttemptAlreadyExists { attempt_id: record.attempt_id });
        }

        self.write_record_atomic(&guard.attempt_id, &record)?;
        Ok((record, guard))
    }

    pub fn read_record_locked(&self, guard: &AttemptLockGuard) -> Result<AttemptRecord, JournalError> {
        self.verify_guard(guard)?;
        self.get_record(guard.attempt_id())
    }

    pub fn get_record(&self, attempt_id: &str) -> Result<AttemptRecord, JournalError> {
        validate_safe_id(attempt_id, "attempt_id")?;
        let journal_path = self.journal_path(attempt_id);
        if !journal_path.exists() {
            return Err(JournalError::AttemptNotFound { attempt_id: attempt_id.to_string() });
        }
        let file = File::open(&journal_path)?;
        let mut buf = Vec::new();
        file.take(MAX_JOURNAL_JSON_BYTES + 1).read_to_end(&mut buf)?;
        if buf.len() as u64 > MAX_JOURNAL_JSON_BYTES {
            return Err(JournalError::LimitExceeded { field: "journal.json", actual: buf.len() as u64, max: MAX_JOURNAL_JSON_BYTES });
        }
        let record: AttemptRecord = serde_json::from_slice(&buf)?;
        if record.schema_version != CURRENT_JOURNAL_SCHEMA_VERSION {
            return Err(JournalError::UnsupportedSchemaVersion { expected: CURRENT_JOURNAL_SCHEMA_VERSION, actual: record.schema_version });
        }
        if record.attempt_id != attempt_id {
            return Err(JournalError::InvalidIdentifier { field: "attempt_id", value: record.attempt_id });
        }
        for (id, f) in [(&record.attempt_id, "attempt_id"), (&record.job_id, "job_id"), (&record.profile_id, "profile_id"), (&record.region_id, "region_id")] {
            validate_safe_id(id, f)?;
        }
        for (h, f, len) in [
            (&record.endpoint_fingerprint, "endpoint_fingerprint", 64), (&record.source_image_hash, "source_image_hash", 64),
            (&record.crop_sha256, "crop_sha256", 64), (&record.hint_sha256, "hint_sha256", 64),
            (&record.request_digest, "request_digest", 64),
        ] {
            validate_lowercase_hex_hash(h, f, len)?;
        }
        validate_ascii_id(&record.recipe_id, "recipe_id", 1, 128)?;
        validate_ascii_id(&record.preprocessing_version, "preprocessing_version", 1, 32)?;
        validate_ascii_id(&record.model_id, "model_id", 1, 128)?;

        let req_meta = record.to_request_metadata();
        cleaner_core::cloud_wire::validate_job_request_metadata(&req_meta)?;
        let computed = compute_request_digest(&req_meta);
        if computed != record.request_digest {
            return Err(JournalError::RequestDigestMismatch { expected: record.request_digest.clone(), computed });
        }

        match &record.phase {
            AttemptPhase::Accepted { handle } | AttemptPhase::CancelRequested { handle } => {
                validate_safe_id(handle, "handle")?;
                validate_ascii_id(handle, "handle", 1, 128)?;
            }
            AttemptPhase::ResultCached { handle, result_digest, cached_png_bytes, reported_cost_usd } => {
                validate_safe_id(handle, "handle")?;
                validate_ascii_id(handle, "handle", 1, 128)?;
                validate_lowercase_hex_hash(result_digest, "result_digest", 64)?;
                if *cached_png_bytes == 0 { return Err(JournalError::Wire(WireValidationError::ResultByteLengthMismatch { actual: 0, declared: 1 })); }
                if let Some(c) = reported_cost_usd { if !c.is_finite() || *c < 0.0 { return Err(JournalError::Wire(WireValidationError::InvalidReportedCost(*c))); } }
            }
            AttemptPhase::AttachmentPending { handle, patch_id, result_digest, cached_png_bytes, reported_cost_usd }
            | AttemptPhase::Committed { handle, patch_id, result_digest, cached_png_bytes, reported_cost_usd } => {
                validate_safe_id(handle, "handle")?;
                validate_ascii_id(handle, "handle", 1, 128)?;
                validate_safe_id(patch_id, "patch_id")?;
                validate_lowercase_hex_hash(result_digest, "result_digest", 64)?;
                if *cached_png_bytes == 0 { return Err(JournalError::Wire(WireValidationError::ResultByteLengthMismatch { actual: 0, declared: 1 })); }
                if let Some(c) = reported_cost_usd { if !c.is_finite() || *c < 0.0 { return Err(JournalError::Wire(WireValidationError::InvalidReportedCost(*c))); } }
            }
            AttemptPhase::Cancelled { handle, .. } | AttemptPhase::Failed { handle, .. } => {
                if let Some(h) = handle { validate_safe_id(h, "handle")?; validate_ascii_id(h, "handle", 1, 128)?; }
            }
            AttemptPhase::Intent | AttemptPhase::Dispatching | AttemptPhase::Unknown { .. } => {}
        }

        Ok(record)
    }

    fn write_record_atomic(&self, attempt_id: &str, record: &AttemptRecord) -> Result<(), JournalError> {
        let json_bytes = serde_json::to_vec_pretty(record)?;
        if json_bytes.len() as u64 > MAX_JOURNAL_JSON_BYTES {
            return Err(JournalError::LimitExceeded { field: "journal.json", actual: json_bytes.len() as u64, max: MAX_JOURNAL_JSON_BYTES });
        }
        atomic_write_file(&self.journal_path(attempt_id), &json_bytes)
    }

    fn mutate_record<F>(&self, guard: &AttemptLockGuard, f: F) -> Result<AttemptRecord, JournalError>
    where
        F: FnOnce(&mut AttemptRecord) -> Result<(), JournalError>,
    {
        self.verify_guard(guard)?;
        let mut record = self.read_record_locked(guard)?;
        f(&mut record)?;
        record.updated_at_ms = current_epoch_ms();
        self.write_record_atomic(guard.attempt_id(), &record)?;
        Ok(record)
    }

    pub fn mark_dispatching(&self, guard: &AttemptLockGuard) -> Result<DispatchPermit, JournalError> {
        self.mutate_record(guard, |rec| {
            if rec.phase != AttemptPhase::Intent {
                return Err(JournalError::InvalidTransition { current: format!("{:?}", rec.phase), attempted: "mark_dispatching".into() });
            }
            rec.phase = AttemptPhase::Dispatching;
            Ok(())
        })?;
        Ok(DispatchPermit { attempt_id: guard.attempt_id().to_string() })
    }

    pub fn abort_intent_no_dispatch(&self, guard: &AttemptLockGuard) -> Result<AttemptRecord, JournalError> {
        self.mutate_record(guard, |rec| {
            if rec.phase != AttemptPhase::Intent {
                return Err(JournalError::InvalidTransition { current: format!("{:?}", rec.phase), attempted: "abort_intent_no_dispatch".into() });
            }
            rec.phase = AttemptPhase::Cancelled { handle: None, reason: "explicit pre-dispatch abort".into() };
            Ok(())
        })
    }

    pub fn record_dispatch_transport_error(&self, guard: &AttemptLockGuard) -> Result<AttemptRecord, JournalError> {
        self.mutate_record(guard, |rec| {
            if rec.phase != AttemptPhase::Dispatching {
                return Err(JournalError::InvalidTransition { current: format!("{:?}", rec.phase), attempted: "record_dispatch_transport_error".into() });
            }
            rec.phase = AttemptPhase::Unknown { reason: "submission interrupted; remote acceptance unknown".into() };
            Ok(())
        })
    }

    pub fn record_accepted_handle(&self, guard: &AttemptLockGuard, handle: &str) -> Result<AttemptRecord, JournalError> {
        validate_safe_id(handle, "handle")?;
        validate_ascii_id(handle, "handle", 1, 128)?;
        self.mutate_record(guard, |rec| {
            match &rec.phase {
                AttemptPhase::Dispatching => { rec.phase = AttemptPhase::Accepted { handle: handle.to_string() }; Ok(()) }
                AttemptPhase::Accepted { handle: existing } if existing == handle => Ok(()),
                other => Err(JournalError::InvalidTransition { current: format!("{:?}", other), attempted: "record_accepted_handle".into() }),
            }
        })
    }

    pub fn record_cancel_requested(&self, guard: &AttemptLockGuard) -> Result<AttemptRecord, JournalError> {
        self.mutate_record(guard, |rec| {
            match &rec.phase {
                AttemptPhase::Accepted { handle } => { rec.phase = AttemptPhase::CancelRequested { handle: handle.clone() }; Ok(()) }
                AttemptPhase::CancelRequested { .. } => Ok(()),
                other => Err(JournalError::InvalidTransition { current: format!("{:?}", other), attempted: "record_cancel_requested".into() }),
            }
        })
    }

    pub fn record_cancelled(&self, guard: &AttemptLockGuard) -> Result<AttemptRecord, JournalError> {
        self.mutate_record(guard, |rec| {
            let handle = match &rec.phase {
                AttemptPhase::Accepted { handle } | AttemptPhase::CancelRequested { handle } => Some(handle.clone()),
                other => return Err(JournalError::InvalidTransition { current: format!("{:?}", other), attempted: "record_cancelled".into() }),
            };
            rec.phase = AttemptPhase::Cancelled { handle, reason: "confirmed terminal cancellation".into() };
            Ok(())
        })
    }

    pub fn record_failed(&self, guard: &AttemptLockGuard) -> Result<AttemptRecord, JournalError> {
        self.mutate_record(guard, |rec| {
            let handle = match &rec.phase {
                AttemptPhase::Accepted { handle } | AttemptPhase::CancelRequested { handle } => Some(handle.clone()),
                other => return Err(JournalError::InvalidTransition { current: format!("{:?}", other), attempted: "record_failed".into() }),
            };
            rec.phase = AttemptPhase::Failed { handle, error_code: "remote_failure".into(), message: "remote job failed".into() };
            Ok(())
        })
    }

    pub fn cache_result(&self, guard: &AttemptLockGuard, result_bytes: &[u8], result_meta: &ResultMetadata) -> Result<DecodedResultCrop, JournalError> {
        self.verify_guard(guard)?;
        let mut record = self.read_record_locked(guard)?;
        let expected_handle = match &record.phase {
            AttemptPhase::Accepted { handle } | AttemptPhase::CancelRequested { handle } => handle.clone(),
            other => return Err(JournalError::InvalidTransition { current: format!("{:?}", other), attempted: "cache_result".into() }),
        };

        if result_meta.handle != expected_handle {
            return Err(WireValidationError::ResponseBindingMismatch {
                field: "handle",
                request_val: expected_handle,
                response_val: result_meta.handle.clone(),
            }.into());
        }
        if result_meta.attempt_id != record.attempt_id {
            return Err(WireValidationError::ResponseBindingMismatch {
                field: "attempt_id",
                request_val: record.attempt_id.clone(),
                response_val: result_meta.attempt_id.clone(),
            }.into());
        }

        let request_meta = record.to_request_metadata();
        let decoded = decode_result_crop(result_bytes, result_meta, &request_meta, &self.limits, &expected_handle)?;

        // Step 1: Write validated result metadata first
        let meta_bytes = serde_json::to_vec_pretty(result_meta)?;
        atomic_write_file(&self.result_meta_path(guard.attempt_id()), &meta_bytes)?;

        // Step 2: Write result PNG second
        atomic_write_file(&self.result_png_path(guard.attempt_id()), result_bytes)?;

        // Step 3: Write journal phase ResultCached third
        record.phase = AttemptPhase::ResultCached {
            handle: expected_handle,
            result_digest: result_meta.result_digest.clone(),
            cached_png_bytes: result_bytes.len() as u64,
            reported_cost_usd: result_meta.reported_cost_usd,
        };
        record.updated_at_ms = current_epoch_ms();
        self.write_record_atomic(guard.attempt_id(), &record)?;
        Ok(decoded)
    }

    pub fn read_cached_png(&self, guard: &AttemptLockGuard) -> Result<Vec<u8>, JournalError> {
        self.verify_guard(guard)?;
        let record = self.read_record_locked(guard)?;
        let (expected_digest, expected_bytes, expected_cost, handle) = match &record.phase {
            AttemptPhase::ResultCached { handle, result_digest, cached_png_bytes, reported_cost_usd }
            | AttemptPhase::AttachmentPending { handle, result_digest, cached_png_bytes, reported_cost_usd, .. }
            | AttemptPhase::Committed { handle, result_digest, cached_png_bytes, reported_cost_usd, .. } => {
                (result_digest.clone(), *cached_png_bytes, *reported_cost_usd, handle.clone())
            }
            other => return Err(JournalError::InvalidTransition { current: format!("{:?}", other), attempted: "read_cached_png".into() }),
        };

        let png_path = self.result_png_path(guard.attempt_id());
        if !png_path.exists() {
            return Err(JournalError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "cached result.png missing")));
        }

        let file = File::open(&png_path)?;
        let mut buf = Vec::new();
        file.take(self.limits.max_png_bytes.min(cleaner_core::cloud_decode::MAX_RESULT_ENCODED_BYTES).saturating_add(1)).read_to_end(&mut buf)?;
        if buf.len() as u64 > self.limits.max_png_bytes {
            return Err(JournalError::LimitExceeded { field: "result.png", actual: buf.len() as u64, max: self.limits.max_png_bytes });
        }

        if buf.len() as u64 != expected_bytes {
            return Err(WireValidationError::ResultByteLengthMismatch { actual: buf.len() as u64, declared: expected_bytes }.into());
        }

        let mut hasher = Sha256::new();
        hasher.update(&buf);
        let computed_digest = format!("{:x}", hasher.finalize());
        if computed_digest != expected_digest {
            return Err(JournalError::ResultDigestMismatch { expected: expected_digest, computed: computed_digest });
        }

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
            result_digest: expected_digest,
            reported_cost_usd: expected_cost,
            width: record.width,
            height: record.height,
            byte_length: expected_bytes,
        };

        decode_result_crop(&buf, &result_meta, &record.to_request_metadata(), &self.limits, &handle)?;
        Ok(buf)
    }

    pub fn prepare_attachment(
        &self,
        guard: &AttemptLockGuard,
        snapshot: &ProjectRegionSnapshot,
        patch_id: &str,
    ) -> Result<AttemptRecord, JournalError> {
        self.verify_guard(guard)?;
        validate_safe_id(patch_id, "patch_id")?;
        let mut record = self.read_record_locked(guard)?;

        if snapshot.source_image_hash != record.source_image_hash {
            return Err(JournalError::StaleAttachment { field: "source_image_hash", expected: record.source_image_hash.clone(), actual: snapshot.source_image_hash.clone() });
        }
        if snapshot.region_id != record.region_id {
            return Err(JournalError::StaleAttachment { field: "region_id", expected: record.region_id.clone(), actual: snapshot.region_id.clone() });
        }
        if snapshot.region_revision != record.region_revision {
            return Err(JournalError::StaleAttachment { field: "region_revision", expected: record.region_revision.to_string(), actual: snapshot.region_revision.to_string() });
        }
        if snapshot.crop_sha256 != record.crop_sha256 {
            return Err(JournalError::StaleAttachment { field: "crop_sha256", expected: record.crop_sha256.clone(), actual: snapshot.crop_sha256.clone() });
        }
        if snapshot.hint_sha256 != record.hint_sha256 {
            return Err(JournalError::StaleAttachment { field: "hint_sha256", expected: record.hint_sha256.clone(), actual: snapshot.hint_sha256.clone() });
        }

        self.read_cached_png(guard)?;

        if let AttemptPhase::Committed { handle: _, patch_id: ref existing, .. } = record.phase {
            if existing == patch_id { return Ok(record); }
            return Err(JournalError::DuplicateCommit { patch_id: existing.clone() });
        }
        if let AttemptPhase::AttachmentPending { handle: _, patch_id: ref existing, .. } = record.phase {
            if existing == patch_id { return Ok(record); }
            return Err(JournalError::DuplicateCommit { patch_id: existing.clone() });
        }

        let (handle, result_digest, cached_png_bytes, reported_cost_usd) = match &record.phase {
            AttemptPhase::ResultCached { handle, result_digest, cached_png_bytes, reported_cost_usd } => {
                (handle.clone(), result_digest.clone(), *cached_png_bytes, *reported_cost_usd)
            }
            other => return Err(JournalError::InvalidTransition { current: format!("{:?}", other), attempted: "prepare_attachment".into() }),
        };


        record.phase = AttemptPhase::AttachmentPending {
            handle, patch_id: patch_id.to_string(), result_digest, cached_png_bytes, reported_cost_usd,
        };
        record.updated_at_ms = current_epoch_ms();
        self.write_record_atomic(guard.attempt_id(), &record)?;
        Ok(record)
    }

    pub fn confirm_committed(&self, guard: &AttemptLockGuard, patch_id: &str) -> Result<AttemptRecord, JournalError> {
        self.verify_guard(guard)?;
        validate_safe_id(patch_id, "patch_id")?;
        let mut record = self.read_record_locked(guard)?;

        if let AttemptPhase::Committed { handle: _, patch_id: ref existing, .. } = record.phase {
            if existing == patch_id { return Ok(record); }
            return Err(JournalError::DuplicateCommit { patch_id: existing.clone() });
        }

        let (handle, result_digest, cached_png_bytes, reported_cost_usd) = match &record.phase {
            AttemptPhase::AttachmentPending { handle, patch_id: pending_id, result_digest, cached_png_bytes, reported_cost_usd } => {
                if pending_id != patch_id {
                    return Err(JournalError::DuplicateCommit { patch_id: pending_id.clone() });
                }
                (handle.clone(), result_digest.clone(), *cached_png_bytes, *reported_cost_usd)
            }
            other => return Err(JournalError::InvalidTransition { current: format!("{:?}", other), attempted: "confirm_committed".into() }),
        };

        record.phase = AttemptPhase::Committed {
            handle, patch_id: patch_id.to_string(), result_digest, cached_png_bytes, reported_cost_usd,
        };
        record.updated_at_ms = current_epoch_ms();
        self.write_record_atomic(guard.attempt_id(), &record)?;
        Ok(record)
    }

    pub fn recover_attempt(&self, guard: &AttemptLockGuard) -> Result<RecoveryDecision, JournalError> {
        self.verify_guard(guard)?;
        let mut record = self.read_record_locked(guard)?;
        let phase = record.phase.clone();
        match &phase {
            AttemptPhase::Intent | AttemptPhase::Dispatching => {
                let reason = "interrupted during intent/dispatch; execution status ambiguous".to_string();
                record.phase = AttemptPhase::Unknown { reason: reason.clone() };
                record.updated_at_ms = current_epoch_ms();
                self.write_record_atomic(guard.attempt_id(), &record)?;
                Ok(RecoveryDecision::AmbiguousUnknown { reason })
            }
            AttemptPhase::Accepted { handle } | AttemptPhase::CancelRequested { handle } => {
                let meta_path = self.result_meta_path(guard.attempt_id());
                let png_path = self.result_png_path(guard.attempt_id());
                if meta_path.exists() && png_path.exists() {
                    let reconcile_res = (|| -> Result<ResultMetadata, JournalError> {
                        let mf = File::open(&meta_path)?;
                        let mut mbuf = Vec::new();
                        mf.take(MAX_JOURNAL_JSON_BYTES + 1).read_to_end(&mut mbuf)?;
                        if mbuf.len() as u64 > MAX_JOURNAL_JSON_BYTES {
                            return Err(JournalError::LimitExceeded { field: "result_meta.json", actual: mbuf.len() as u64, max: MAX_JOURNAL_JSON_BYTES });
                        }
                        let res_meta: ResultMetadata = serde_json::from_slice(&mbuf)?;

                        let pf = File::open(&png_path)?;
                        let mut pbuf = Vec::new();
                        pf.take(self.limits.max_png_bytes.min(cleaner_core::cloud_decode::MAX_RESULT_ENCODED_BYTES).saturating_add(1)).read_to_end(&mut pbuf)?;

                        decode_result_crop(&pbuf, &res_meta, &record.to_request_metadata(), &self.limits, handle)?;
                        Ok(res_meta)
                    })();

                    match reconcile_res {
                        Ok(res_meta) => {
                            let cached_bytes = res_meta.byte_length;
                            record.phase = AttemptPhase::ResultCached {
                                handle: handle.clone(),
                                result_digest: res_meta.result_digest.clone(),
                                cached_png_bytes: cached_bytes,
                                reported_cost_usd: res_meta.reported_cost_usd,
                            };
                            record.updated_at_ms = current_epoch_ms();
                            self.write_record_atomic(guard.attempt_id(), &record)?;
                            return Ok(RecoveryDecision::ResultCachedReadyForAttach {
                                handle: handle.clone(),
                                result_digest: res_meta.result_digest,
                            });
                        }
                        Err(_) => {
                            return Ok(RecoveryDecision::CorruptedResultRetainedForInspection {
                                handle: handle.clone(),
                                reason: "cached result validation failed".into(),
                            });
                        }
                    }
                }

                match &record.phase {
                    AttemptPhase::Accepted { handle } => Ok(RecoveryDecision::ResumePolling { handle: handle.clone() }),
                    AttemptPhase::CancelRequested { handle } => Ok(RecoveryDecision::ResumeCancelPolling { handle: handle.clone() }),
                    _ => unreachable!(),
                }
            }
            AttemptPhase::ResultCached { handle, result_digest, .. } => Ok(RecoveryDecision::ResultCachedReadyForAttach {
                handle: handle.clone(),
                result_digest: result_digest.clone(),
            }),
            AttemptPhase::AttachmentPending { patch_id, result_digest, .. } => {
                Ok(RecoveryDecision::AttachmentPendingVerification {
                    patch_id: patch_id.clone(),
                    result_digest: result_digest.clone(),
                })
            }
            AttemptPhase::Committed { patch_id, .. } => Ok(RecoveryDecision::AlreadyCommitted { patch_id: patch_id.clone() }),
            AttemptPhase::Cancelled { reason, .. } => Ok(RecoveryDecision::Terminal { message: reason.clone() }),
            AttemptPhase::Failed { message, .. } => Ok(RecoveryDecision::Terminal { message: message.clone() }),
            AttemptPhase::Unknown { reason } => Ok(RecoveryDecision::AmbiguousUnknown { reason: reason.clone() }),
        }
    }

    pub fn list_attempt_ids(&self) -> Result<Vec<String>, JournalError> {
        let attempts_dir = self.root_dir.join("attempts");
        if !attempts_dir.exists() { return Ok(Vec::new()); }
        let mut ids = Vec::new();
        for entry in fs::read_dir(attempts_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    if validate_safe_id(name, "attempt_id").is_ok() { ids.push(name.to_string()); }
                }
            }
        }
        ids.sort();
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cleaner_core::cloud_decode::result_metadata_from_status;
    use cleaner_core::cloud_wire::{provisional_fixture_limits, JobRequestMetadata, JobStatusResponse};

    const TINY_IMAGE_BYTES: &[u8] = include_bytes!("../../../deploy/cloud/fixtures/tiny_image.png");
    const JOB_REQUEST_FIXTURE: &str = include_str!("../../../deploy/cloud/fixtures/job_request_metadata.json");
    const JOB_STATUS_COMPLETED_FIXTURE: &str = include_str!("../../../deploy/cloud/fixtures/job_status_completed.json");

    struct TestDir { path: PathBuf }
    impl TestDir {
        fn new(prefix: &str) -> Self {
            static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
            let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
            let path = std::env::temp_dir().join(format!("mc_journal_test_{}_{}_{}", prefix, nanos, count));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }
        fn path(&self) -> &Path { &self.path }
    }
    impl Drop for TestDir {
        fn drop(&mut self) { let _ = fs::remove_dir_all(&self.path); }
    }

    fn test_intent() -> CreateAttemptIntent {
        let req: JobRequestMetadata = serde_json::from_str(JOB_REQUEST_FIXTURE).unwrap();
        CreateAttemptIntent {
            attempt_id: req.attempt_id,
            job_id: req.job_id,
            provider: CloudProvider::Modal,
            profile_id: "test-modal-profile".to_string(),
            endpoint_fingerprint: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
            recipe_id: req.recipe.recipe_id,
            preprocessing_version: req.recipe.preprocessing_version,
            model_id: req.recipe.model_id,
            model_revision: req.recipe.model_revision,
            native_mask_conditioning: req.recipe.native_mask_conditioning,
            width: req.width,
            height: req.height,
            seed: req.seed,
            steps: req.steps,
            guidance_scaled: req.guidance_scaled,
            source_image_hash: "2e0a9a2d5f28563463f634d5b81ec2ef90d91afe472a0c6e2c6ef4082f93ec5f".to_string(),
            region_id: "reg-test-101".to_string(),
            region_revision: 42,
            crop_sha256: req.image_sha256,
            hint_sha256: req.hint_sha256,
            request_digest: req.request_digest,
            chapter_id: None,
            page_index: None,
        }
    }

    #[test]
    fn test_safe_opaque_id_validation() {
        assert!(validate_safe_id("attempt-001", "attempt_id").is_ok());
        assert!(validate_safe_id("reg_123-abc", "region_id").is_ok());
        assert!(validate_safe_id("", "id").is_err());
        assert!(validate_safe_id(".", "id").is_err());
        assert!(validate_safe_id("..", "id").is_err());
        assert!(validate_safe_id("../evil", "id").is_err());
        assert!(validate_safe_id("dir/file", "id").is_err());
        assert!(validate_safe_id("dir\\file", "id").is_err());
        assert!(validate_safe_id("id with spaces", "id").is_err());
        assert!(validate_safe_id("id\0null", "id").is_err());
        assert!(validate_safe_id(&"a".repeat(129), "id").is_err());
    }

    #[test]
    fn test_foreign_guard_rejected() {
        let td1 = TestDir::new("foreign_1");
        let td2 = TestDir::new("foreign_2");
        let j1 = AttemptJournal::new(td1.path(), provisional_fixture_limits());
        let j2 = AttemptJournal::new(td2.path(), provisional_fixture_limits());
        let intent = test_intent();

        let (_rec, guard1) = j1.create_intent(intent).unwrap();
        let err = j2.mark_dispatching(&guard1).unwrap_err();
        assert!(matches!(err, JournalError::ForeignGuard { .. }));
    }

    #[test]
    fn test_ambiguous_accept_disconnect_counted() {
        let td = TestDir::new("ambiguous_disconnect");
        let journal = AttemptJournal::new(td.path(), provisional_fixture_limits());
        let intent = test_intent();

        let (record, guard) = journal.create_intent(intent.clone()).unwrap();
        assert_eq!(record.phase, AttemptPhase::Intent);

        let permit = journal.mark_dispatching(&guard).unwrap();
        assert_eq!(permit.attempt_id(), intent.attempt_id);

        let mut submissions = 0;
        let mut fake_transport = |_permit: DispatchPermit| -> Result<(), ()> {
            submissions += 1;
            Err(()) // Server accepted, then connection closed before returning a handle.
        };
        assert!(fake_transport(permit).is_err());
        assert!(journal.record_failed(&guard).is_err());
        let updated = journal.record_dispatch_transport_error(&guard).unwrap();
        assert!(matches!(updated.phase, AttemptPhase::Unknown { .. }));
        assert!(!updated.is_retryable());

        let decision = journal.recover_attempt(&guard).unwrap();
        assert!(!decision.is_auto_retryable());
        assert!(matches!(decision, RecoveryDecision::AmbiguousUnknown { .. }));
        assert!(journal.mark_dispatching(&guard).is_err());
        assert_eq!(submissions, 1);
    }

    #[test]
    fn test_abort_intent_explicit_no_dispatch() {
        let td = TestDir::new("abort_no_dispatch");
        let journal = AttemptJournal::new(td.path(), provisional_fixture_limits());
        let intent = test_intent();

        let (_record, guard) = journal.create_intent(intent).unwrap();
        let aborted = journal.abort_intent_no_dispatch(&guard).unwrap();
        assert!(matches!(aborted.phase, AttemptPhase::Cancelled { handle: None, .. }));

        let decision = journal.recover_attempt(&guard).unwrap();
        assert!(matches!(decision, RecoveryDecision::Terminal { .. }));
        assert!(!decision.is_auto_retryable());
    }

    #[test]
    fn test_restart_known_handle_and_result_reuse() {
        let td = TestDir::new("restart_reuse");
        let limits = provisional_fixture_limits();
        let journal = AttemptJournal::new(td.path(), limits);
        let intent = test_intent();

        let (_record, guard) = journal.create_intent(intent.clone()).unwrap();
        journal.mark_dispatching(&guard).unwrap();
        let accepted = journal.record_accepted_handle(&guard, "handle-test-modal-999").unwrap();
        assert_eq!(accepted.phase, AttemptPhase::Accepted { handle: "handle-test-modal-999".to_string() });
        drop(guard);

        let journal2 = AttemptJournal::new(td.path(), limits);
        let guard2 = journal2.acquire_lock(&intent.attempt_id).unwrap();
        let decision = journal2.recover_attempt(&guard2).unwrap();
        assert_eq!(decision, RecoveryDecision::ResumePolling { handle: "handle-test-modal-999".to_string() });

        let status: JobStatusResponse = serde_json::from_str(JOB_STATUS_COMPLETED_FIXTURE).unwrap();
        let req_meta: JobRequestMetadata = serde_json::from_str(JOB_REQUEST_FIXTURE).unwrap();
        let result_meta = result_metadata_from_status(&status, &req_meta, "handle-test-modal-999").unwrap();

        let decoded = journal2.cache_result(&guard2, TINY_IMAGE_BYTES, &result_meta).unwrap();
        assert_eq!(decoded.width(), 16);
        assert_eq!(decoded.height(), 16);
        drop(guard2);

        let journal3 = AttemptJournal::new(td.path(), limits);
        let guard3 = journal3.acquire_lock(&intent.attempt_id).unwrap();
        let decision2 = journal3.recover_attempt(&guard3).unwrap();
        assert_eq!(
            decision2,
            RecoveryDecision::ResultCachedReadyForAttach {
                handle: "handle-test-modal-999".to_string(),
                result_digest: result_meta.result_digest,
            }
        );

        let cached_png = journal3.read_cached_png(&guard3).unwrap();
        assert_eq!(cached_png, TINY_IMAGE_BYTES);
    }

    #[test]
    fn test_invalid_lifecycle_transitions() {
        let td = TestDir::new("invalid_transitions");
        let limits = provisional_fixture_limits();
        let journal = AttemptJournal::new(td.path(), limits);
        let intent = test_intent();

        let (_record, guard) = journal.create_intent(intent).unwrap();

        let err = journal.record_accepted_handle(&guard, "handle-premature").unwrap_err();
        assert!(matches!(err, JournalError::InvalidTransition { .. }));

        let status: JobStatusResponse = serde_json::from_str(JOB_STATUS_COMPLETED_FIXTURE).unwrap();
        let req_meta: JobRequestMetadata = serde_json::from_str(JOB_REQUEST_FIXTURE).unwrap();
        let result_meta = result_metadata_from_status(&status, &req_meta, "handle-test-modal-999").unwrap();

        let err = journal.cache_result(&guard, TINY_IMAGE_BYTES, &result_meta).unwrap_err();
        assert!(matches!(err, JournalError::InvalidTransition { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn test_disk_write_failure_injection_preserves_in_memory_state() {
        let td = TestDir::new("write_failure_injection");
        let journal = AttemptJournal::new(td.path(), provisional_fixture_limits());
        let intent = test_intent();

        let (_record, guard) = journal.create_intent(intent.clone()).unwrap();
        let attempt_dir = journal.attempt_dir(&intent.attempt_id);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&attempt_dir, fs::Permissions::from_mode(0o555)).unwrap();
        }

        let dispatch_err = journal.mark_dispatching(&guard);

        #[cfg(unix)]
        {
            assert!(dispatch_err.is_err());
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&attempt_dir, fs::Permissions::from_mode(0o755)).unwrap();
        }

        let record_after = journal.read_record_locked(&guard).unwrap();
        assert_eq!(record_after.phase, AttemptPhase::Intent);
    }

    #[test]
    fn test_concurrent_ownership_two_lock_descriptors() {
        let td = TestDir::new("concurrent_ownership");
        let journal = AttemptJournal::new(td.path(), provisional_fixture_limits());
        let intent = test_intent();

        let (_record, guard1) = journal.create_intent(intent.clone()).unwrap();
        let guard2_res = journal.acquire_lock(&intent.attempt_id);
        assert!(matches!(guard2_res, Err(JournalError::AttemptLocked { .. })));

        drop(guard1);

        let guard2 = journal.acquire_lock(&intent.attempt_id).unwrap();
        assert_eq!(guard2.attempt_id(), intent.attempt_id);
    }

    #[test]
    fn test_cancellation_ack_nonterminal() {
        let td = TestDir::new("cancel_nonterminal");
        let journal = AttemptJournal::new(td.path(), provisional_fixture_limits());
        let intent = test_intent();

        let (_record, guard) = journal.create_intent(intent).unwrap();
        journal.mark_dispatching(&guard).unwrap();
        journal.record_accepted_handle(&guard, "handle-test-modal-999").unwrap();

        let cancel_req = journal.record_cancel_requested(&guard).unwrap();
        assert_eq!(cancel_req.phase, AttemptPhase::CancelRequested { handle: "handle-test-modal-999".to_string() });
        assert!(!cancel_req.is_terminal());

        let decision = journal.recover_attempt(&guard).unwrap();
        assert_eq!(decision, RecoveryDecision::ResumeCancelPolling { handle: "handle-test-modal-999".to_string() });

        let cancelled = journal.record_cancelled(&guard).unwrap();
        assert!(matches!(cancelled.phase, AttemptPhase::Cancelled { .. }));
        assert!(cancelled.is_terminal());
    }

    #[test]
    fn test_two_phase_project_commit_and_recovery() {
        let td = TestDir::new("two_phase_commit");
        let limits = provisional_fixture_limits();
        let journal = AttemptJournal::new(td.path(), limits);
        let intent = test_intent();

        let (_record, guard) = journal.create_intent(intent.clone()).unwrap();
        journal.mark_dispatching(&guard).unwrap();
        journal.record_accepted_handle(&guard, "handle-test-modal-999").unwrap();

        let status: JobStatusResponse = serde_json::from_str(JOB_STATUS_COMPLETED_FIXTURE).unwrap();
        let req_meta: JobRequestMetadata = serde_json::from_str(JOB_REQUEST_FIXTURE).unwrap();
        let result_meta = result_metadata_from_status(&status, &req_meta, "handle-test-modal-999").unwrap();
        journal.cache_result(&guard, TINY_IMAGE_BYTES, &result_meta).unwrap();

        let snapshot = ProjectRegionSnapshot {
            source_image_hash: intent.source_image_hash.clone(),
            region_id: intent.region_id.clone(),
            region_revision: intent.region_revision,
            crop_sha256: intent.crop_sha256.clone(),
            hint_sha256: intent.hint_sha256.clone(),
        };

        // Phase 1: Prepare attachment
        let pending = journal.prepare_attachment(&guard, &snapshot, "patch-alpha").unwrap();
        assert!(matches!(pending.phase, AttemptPhase::AttachmentPending { .. }));

        // Recovery during AttachmentPending
        drop(guard);
        let j_recover = AttemptJournal::new(td.path(), limits);
        let guard_rec = j_recover.acquire_lock(&intent.attempt_id).unwrap();
        let dec = j_recover.recover_attempt(&guard_rec).unwrap();
        assert_eq!(
            dec,
            RecoveryDecision::AttachmentPendingVerification {
                patch_id: "patch-alpha".to_string(),
                result_digest: result_meta.result_digest.clone(),
            }
        );

        let mut changed = snapshot.clone();
        changed.region_revision += 1;
        assert!(matches!(j_recover.prepare_attachment(&guard_rec, &changed, "patch-alpha"),
            Err(JournalError::StaleAttachment { field: "region_revision", .. })));
        changed = snapshot.clone();
        changed.source_image_hash = "f".repeat(64);
        assert!(matches!(j_recover.prepare_attachment(&guard_rec, &changed, "patch-alpha"),
            Err(JournalError::StaleAttachment { field: "source_image_hash", .. })));
        j_recover.prepare_attachment(&guard_rec, &snapshot, "patch-alpha").unwrap();

        // Phase 2: Confirm committed after the external project writer verifies patch-alpha.
        let committed = j_recover.confirm_committed(&guard_rec, "patch-alpha").unwrap();
        assert!(matches!(committed.phase, AttemptPhase::Committed { .. }));

        // Idempotent retry succeeds
        let idemp = j_recover.confirm_committed(&guard_rec, "patch-alpha").unwrap();
        assert_eq!(idemp.phase, committed.phase);

        // Different patch fails
        let dup = j_recover.confirm_committed(&guard_rec, "patch-beta").unwrap_err();
        assert!(matches!(dup, JournalError::DuplicateCommit { .. }));
    }

    #[test]
    fn test_wrong_source_same_revision_stale_attach() {
        let td = TestDir::new("wrong_source_stale");
        let limits = provisional_fixture_limits();
        let journal = AttemptJournal::new(td.path(), limits);
        let intent = test_intent();

        let (_record, guard) = journal.create_intent(intent.clone()).unwrap();
        journal.mark_dispatching(&guard).unwrap();
        journal.record_accepted_handle(&guard, "handle-test-modal-999").unwrap();

        let status: JobStatusResponse = serde_json::from_str(JOB_STATUS_COMPLETED_FIXTURE).unwrap();
        let req_meta: JobRequestMetadata = serde_json::from_str(JOB_REQUEST_FIXTURE).unwrap();
        let result_meta = result_metadata_from_status(&status, &req_meta, "handle-test-modal-999").unwrap();
        journal.cache_result(&guard, TINY_IMAGE_BYTES, &result_meta).unwrap();

        // Same revision (42), but source image hash changed!
        let wrong_source_snapshot = ProjectRegionSnapshot {
            source_image_hash: "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            region_id: intent.region_id.clone(),
            region_revision: intent.region_revision,
            crop_sha256: intent.crop_sha256.clone(),
            hint_sha256: intent.hint_sha256.clone(),
        };

        let err = journal.prepare_attachment(&guard, &wrong_source_snapshot, "patch-001").unwrap_err();
        assert!(matches!(err, JournalError::StaleAttachment { field: "source_image_hash", .. }));

        // Validated result remains in ResultCached!
        let rec = journal.read_record_locked(&guard).unwrap();
        assert!(matches!(rec.phase, AttemptPhase::ResultCached { .. }));
    }

    #[test]
    fn test_corrupt_cache_read_rejected() {
        let td = TestDir::new("corrupt_cache");
        let limits = provisional_fixture_limits();
        let journal = AttemptJournal::new(td.path(), limits);
        let intent = test_intent();

        let (_record, guard) = journal.create_intent(intent).unwrap();
        journal.mark_dispatching(&guard).unwrap();
        journal.record_accepted_handle(&guard, "handle-test-modal-999").unwrap();

        let status: JobStatusResponse = serde_json::from_str(JOB_STATUS_COMPLETED_FIXTURE).unwrap();
        let req_meta: JobRequestMetadata = serde_json::from_str(JOB_REQUEST_FIXTURE).unwrap();
        let result_meta = result_metadata_from_status(&status, &req_meta, "handle-test-modal-999").unwrap();
        journal.cache_result(&guard, TINY_IMAGE_BYTES, &result_meta).unwrap();

        // Tamper with cached result.png (same length, different byte)
        let png_path = journal.result_png_path(guard.attempt_id());
        let mut tampered = TINY_IMAGE_BYTES.to_vec();
        tampered[10] ^= 0xff;
        fs::write(&png_path, tampered).unwrap();

        let err = journal.read_cached_png(&guard).unwrap_err();
        assert!(matches!(err, JournalError::ResultDigestMismatch { .. }));
    }

    #[test]
    fn test_crash_between_result_write_and_journal_recovers() {
        let td = TestDir::new("crash_result_write");
        let limits = provisional_fixture_limits();
        let journal = AttemptJournal::new(td.path(), limits);
        let intent = test_intent();

        let (_record, guard) = journal.create_intent(intent.clone()).unwrap();
        journal.mark_dispatching(&guard).unwrap();
        journal.record_accepted_handle(&guard, "handle-test-modal-999").unwrap();

        let status: JobStatusResponse = serde_json::from_str(JOB_STATUS_COMPLETED_FIXTURE).unwrap();
        let req_meta: JobRequestMetadata = serde_json::from_str(JOB_REQUEST_FIXTURE).unwrap();
        let result_meta = result_metadata_from_status(&status, &req_meta, "handle-test-modal-999").unwrap();

        // Manually simulate crash after Step 1 & Step 2 (result_meta.json and result.png written)
        // while journal.json is still in Accepted phase
        let meta_bytes = serde_json::to_vec_pretty(&result_meta).unwrap();
        atomic_write_file(&journal.result_meta_path(guard.attempt_id()), &meta_bytes).unwrap();
        atomic_write_file(&journal.result_png_path(guard.attempt_id()), TINY_IMAGE_BYTES).unwrap();

        drop(guard);

        // Restart recovery discovers metadata + PNG, decodes them, and restores ResultCached!
        let j_recover = AttemptJournal::new(td.path(), limits);
        let guard_rec = j_recover.acquire_lock(&intent.attempt_id).unwrap();
        let dec = j_recover.recover_attempt(&guard_rec).unwrap();
        assert_eq!(
            dec,
            RecoveryDecision::ResultCachedReadyForAttach {
                handle: "handle-test-modal-999".to_string(),
                result_digest: result_meta.result_digest.clone(),
            }
        );

        let restored_rec = j_recover.read_record_locked(&guard_rec).unwrap();
        assert!(matches!(restored_rec.phase, AttemptPhase::ResultCached { .. }));
    }

    #[test]
    fn test_no_auto_retry_accepted_failure() {
        let td = TestDir::new("no_retry_fail");
        let journal = AttemptJournal::new(td.path(), provisional_fixture_limits());
        let intent = test_intent();

        let (_record, guard) = journal.create_intent(intent).unwrap();
        journal.mark_dispatching(&guard).unwrap();
        journal.record_accepted_handle(&guard, "handle-test-modal-999").unwrap();

        let failed = journal.record_failed(&guard).unwrap();
        assert!(matches!(failed.phase, AttemptPhase::Failed { .. }));
        assert!(!failed.is_retryable());

        let dec = journal.recover_attempt(&guard).unwrap();
        assert!(!dec.is_auto_retryable());
        assert!(matches!(dec, RecoveryDecision::Terminal { .. }));
    }
}
