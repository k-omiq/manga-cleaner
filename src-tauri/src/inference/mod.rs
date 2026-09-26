//! Cloud inference security, configuration, credential management, and grant policy.

pub mod commands;
pub mod analysis;
pub mod config;
pub mod consent;
pub mod http;
pub mod journal;
pub mod policy;
pub mod secrets;
pub mod service;

pub use config::{
    compute_canonical_endpoint_fingerprint, validate_https_endpoint, validate_inference_config,
    validate_profile_id, BeamProfile, CloudProfile, InferenceConfig, ModalProfile,
    CURRENT_SCHEMA_VERSION, INFERENCE_CONFIG_FILE, MAX_ENDPOINT_URL_LEN, MAX_PROFILES_PER_PROVIDER,
    MAX_PROFILE_ID_LEN, MAX_PROFILE_NAME_LEN,
};
pub use consent::{
    compute_region_revision_fingerprint, compute_region_revision_hash, ConsentError,
    ConsentProposal, ConsentService, OperationIntent, PrepareProposalRequest,
    DEFAULT_PROPOSAL_TTL, MAX_CACHED_PROPOSALS, MAX_PROPOSAL_TTL, MIN_PROPOSAL_TTL,
};
pub use http::{
    BoundRuntimeCredential, CloudEndpointTarget, CloudHttpClient, HttpTransportError,
    RuntimeCredential,
};
pub use journal::{
    AttemptJournal, AttemptLockGuard, AttemptPhase, AttemptRecord, CreateAttemptIntent,
    JournalError, ProjectRegionSnapshot, RecoveryDecision,
};
pub use policy::{Grant, GrantError, GrantScope, GrantService};
pub use secrets::{
    MemorySecretStore, OsKeyringSecretStore, SecretKey, SecretManager, SecretRole, SecretStore,
    SecretSummary, SecretValue, StorageBackendKind, KEYRING_SERVICE,
};
pub use service::{InferenceService, InferenceServiceError, PollOptions};

/// The user's cloud permission switch: `cloudEngines` in the settings file.
///
/// Anything but an explicit `"allowed"` is off, including a key that was never
/// written and a settings file that cannot be read, so paid work fails closed.
/// Every command that can spend reads the switch through this one function.
pub fn cloud_allowed(app: &tauri::AppHandle) -> bool {
    crate::settings::read(app)
        .ok()
        .and_then(|settings| {
            settings
                .get("cloudEngines")
                .and_then(|value| value.as_str())
                .map(|value| value == "allowed")
        })
        .unwrap_or(false)
}
