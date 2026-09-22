//! Cloud secret storage, credential isolation, and role-separated access.
//!
//! Enforces zero-plaintext disk persistence for all cloud credentials (API tokens, workspace
//! keys, bearer secrets). Material is stored strictly in the platform OS credential manager
//! or, when explicitly requested, in an in-memory session-only fallback store.
//!
//! ## Invariants
//!
//! 1. **Zero Plaintext Fallback:** Cloud secrets NEVER fall back to plaintext files (`settings.json`,
//!    `inference.json`, or environment variables).
//! 2. **Canonical Origin Binding:** Every [`SecretKey`] binds the canonical origin fingerprint of
//!    the validated stored profile. Modifying a profile's endpoint URL prevents reuse of old credentials.
//! 3. **Role Separation:** `Setup`, `Runtime`, and `ModelDownload` tokens are stored and scoped
//!    separately and cannot overwrite or substitute for each other.
//! 4. **Compiler-Barrier Zeroization:** [`SecretValue`] derives [`zeroize::Zeroize`] and
//!    [`zeroize::ZeroizeOnDrop`], never implements [`serde::Serialize`], and formats as `"[REDACTED]"`
//!    in both [`std::fmt::Debug`] and [`std::fmt::Display`].
//! 5. **Session-Only Override Semantics:** Storing a session-only credential retains the value in
//!    volatile memory for the current process without mutating or clearing any existing persistent
//!    OS keyring entry.
//! 6. **Sanitized Public Errors:** Error messages never echo secret content or raw backend messages.
//! 7. **Safe Summary Only:** Frontend commands only receive existence/backend status summaries
//!    ([`SecretSummary`]) and cannot read back stored secret strings.
//! 8. **Abstract Secret Backend:** Testable via [`SecretStore`] trait with an in-memory test double
//!    without touching the host OS keyring during unit tests.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use cleaner_core::engines::render::CloudProvider;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Service name used for OS keyring entries.
pub const KEYRING_SERVICE: &str = "com.mangacleaner.studio.cloud";

/// The distinct roles a cloud credential can fulfill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretRole {
    /// Full workspace credential used during automated setup and provisioning.
    Setup,
    /// Bounded, least-privilege token used for `/mc/v1` crop inference execution.
    Runtime,
    /// Token used for pulling model weights or snapshot archives.
    ModelDownload,
}

impl SecretRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            SecretRole::Setup => "setup",
            SecretRole::Runtime => "runtime",
            SecretRole::ModelDownload => "model_download",
        }
    }
}

/// The storage backend where a secret is currently retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageBackendKind {
    /// Operating system credential store (macOS Keychain, Windows Credential Manager, Linux Secret Service).
    Keyring,
    /// Volatile in-memory store retained only for the current application session.
    Session,
    /// Credential store is unreachable or unavailable.
    Unavailable,
}

/// Unique key identifying a stored secret, bound to the profile's canonical origin fingerprint.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SecretKey {
    pub provider: CloudProvider,
    pub profile_id: String,
    pub canonical_origin_fingerprint: String,
    pub role: SecretRole,
}

impl SecretKey {
    pub fn new(
        provider: CloudProvider,
        profile_id: impl Into<String>,
        canonical_origin_fingerprint: impl Into<String>,
        role: SecretRole,
    ) -> Self {
        Self {
            provider,
            profile_id: profile_id.into(),
            canonical_origin_fingerprint: canonical_origin_fingerprint.into(),
            role,
        }
    }

    /// Construct the OS keyring account identifier.
    pub fn as_keyring_user(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.provider.as_str(),
            self.profile_id,
            self.canonical_origin_fingerprint,
            self.role.as_str()
        )
    }
}

/// An opaque, zeroize-on-drop wrapper around secret token bytes with compiler-barrier guarantees.
///
/// Intentionally does NOT implement [`serde::Serialize`] or [`serde::Deserialize`].
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct SecretValue {
    bytes: Vec<u8>,
}

impl SecretValue {
    pub fn new(secret: impl Into<String>) -> Self {
        Self {
            bytes: secret.into().into_bytes(),
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            bytes: bytes.to_vec(),
        }
    }

    pub fn expose_str(&self) -> Result<&str, std::str::Utf8Error> {
        std::str::from_utf8(&self.bytes)
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }
}

impl std::fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl std::fmt::Display for SecretValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

/// Versioned compound payload structure for Modal credentials.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModalCompoundSecret {
    v: u32,
    token_id: String,
    token_secret: String,
}

/// Encode Modal runtime credentials into a versioned opaque compound [`SecretValue`].
///
/// Strictly bounds and validates token ID and token secret without storing plaintext files.
pub fn encode_modal_runtime_secret(token_id: &str, token_secret: &str) -> Result<SecretValue, String> {
    if token_id.chars().any(|c| c.is_control()) {
        return Err("Modal token ID cannot contain control characters".to_string());
    }
    let tid = token_id.trim();
    if tid.is_empty() {
        return Err("Modal token ID cannot be empty".to_string());
    }
    if tid.len() > 256 {
        return Err("Modal token ID exceeds maximum length of 256 characters".to_string());
    }

    if token_secret.chars().any(|c| c.is_control()) {
        return Err("Modal token secret cannot contain control characters".to_string());
    }
    let tsec = token_secret.trim();
    if tsec.is_empty() {
        return Err("Modal token secret cannot be empty".to_string());
    }
    if tsec.len() > 1024 {
        return Err("Modal token secret exceeds maximum length of 1024 characters".to_string());
    }

    let compound = ModalCompoundSecret {
        v: 1,
        token_id: tid.to_string(),
        token_secret: tsec.to_string(),
    };

    let serialized = serde_json::to_string(&compound)
        .map_err(|e| format!("failed to serialize Modal compound credential: {e}"))?;

    Ok(SecretValue::new(serialized))
}

/// Decode Modal runtime credentials from an opaque compound [`SecretValue`].
///
/// Fails closed on legacy entries, incomplete entries, unsupported versions, or invalid JSON.
pub fn decode_modal_runtime_secret(secret: &SecretValue) -> Result<(String, SecretValue), String> {
    let raw_str = secret
        .expose_str()
        .map_err(|_| "Modal credential is not valid UTF-8".to_string())?;

    let parsed: ModalCompoundSecret = serde_json::from_str(raw_str)
        .map_err(|_| "legacy or invalid Modal credential format; please re-store credential".to_string())?;

    if parsed.v != 1 {
        return Err(format!(
            "unsupported Modal compound credential schema version {}",
            parsed.v
        ));
    }

    if parsed.token_id.chars().any(|c| c.is_control()) {
        return Err("corrupted Modal token ID in stored credential".to_string());
    }
    let tid = parsed.token_id.trim();
    if tid.is_empty() || tid.len() > 256 {
        return Err("corrupted Modal token ID in stored credential".to_string());
    }

    if parsed.token_secret.chars().any(|c| c.is_control()) {
        return Err("corrupted Modal token secret in stored credential".to_string());
    }
    let tsec = parsed.token_secret.trim();
    if tsec.is_empty() || tsec.len() > 1024 {
        return Err("corrupted Modal token secret in stored credential".to_string());
    }

    Ok((tid.to_string(), SecretValue::new(tsec)))
}

/// Safe public summary of credential presence and storage backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretSummary {
    pub provider: CloudProvider,
    pub profile_id: String,
    pub role: SecretRole,
    pub present: bool,
    pub backend: StorageBackendKind,
}

/// Pluggable credential storage interface.
pub trait SecretStore: Send + Sync {
    fn get_secret(&self, key: &SecretKey) -> Result<Option<SecretValue>, String>;
    fn set_secret(&self, key: &SecretKey, secret: &SecretValue) -> Result<(), String>;
    fn delete_secret(&self, key: &SecretKey) -> Result<(), String>;
    fn has_secret(&self, key: &SecretKey) -> Result<bool, String>;
    fn backend_kind(&self) -> StorageBackendKind;
    fn is_available(&self) -> bool;
}

/// Platform OS Keyring implementation backed by the `keyring` crate.
pub struct OsKeyringSecretStore;

impl OsKeyringSecretStore {
    fn entry(key: &SecretKey) -> Result<keyring::Entry, String> {
        let user = key.as_keyring_user();
        keyring::Entry::new(KEYRING_SERVICE, &user)
            .map_err(|_| "OS credential store initialization failed".to_string())
    }
}

impl SecretStore for OsKeyringSecretStore {
    fn get_secret(&self, key: &SecretKey) -> Result<Option<SecretValue>, String> {
        match Self::entry(key)?.get_password() {
            Ok(pwd) => Ok(Some(SecretValue::new(pwd))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(keyring::Error::NoStorageAccess(_)) => {
                Err("OS credential store is locked or inaccessible".to_string())
            }
            Err(keyring::Error::PlatformFailure(_)) => {
                Err("OS credential store service is unreachable".to_string())
            }
            Err(_) => Err("OS credential store read failed".to_string()),
        }
    }

    fn set_secret(&self, key: &SecretKey, secret: &SecretValue) -> Result<(), String> {
        let str_val = secret
            .expose_str()
            .map_err(|_| "credential contains invalid UTF-8 bytes".to_string())?;
        match Self::entry(key)?.set_password(str_val) {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoStorageAccess(_)) => {
                Err("OS credential store is locked or inaccessible".to_string())
            }
            Err(keyring::Error::PlatformFailure(_)) => {
                Err("OS credential store service is unreachable".to_string())
            }
            Err(_) => Err("OS credential store write failed".to_string()),
        }
    }

    fn delete_secret(&self, key: &SecretKey) -> Result<(), String> {
        match Self::entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(keyring::Error::NoStorageAccess(_)) => {
                Err("OS credential store is locked or inaccessible".to_string())
            }
            Err(keyring::Error::PlatformFailure(_)) => {
                Err("OS credential store service is unreachable".to_string())
            }
            Err(_) => Err("OS credential store deletion failed".to_string()),
        }
    }

    fn has_secret(&self, key: &SecretKey) -> Result<bool, String> {
        self.get_secret(key).map(|opt| opt.is_some())
    }

    fn backend_kind(&self) -> StorageBackendKind {
        StorageBackendKind::Keyring
    }

    fn is_available(&self) -> bool {
        cfg!(any(target_os = "macos", target_os = "windows", target_os = "linux"))
    }
}

/// In-memory volatile secret store used for tests and session-only fallback.
pub struct MemorySecretStore {
    store: Mutex<HashMap<SecretKey, SecretValue>>,
}

impl MemorySecretStore {
    pub fn new() -> Self {
        Self {
            store: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for MemorySecretStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretStore for MemorySecretStore {
    fn get_secret(&self, key: &SecretKey) -> Result<Option<SecretValue>, String> {
        let map = self
            .store
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        Ok(map.get(key).cloned())
    }

    fn set_secret(&self, key: &SecretKey, secret: &SecretValue) -> Result<(), String> {
        let mut map = self
            .store
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        map.insert(key.clone(), secret.clone());
        Ok(())
    }

    fn delete_secret(&self, key: &SecretKey) -> Result<(), String> {
        let mut map = self
            .store
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        map.remove(key);
        Ok(())
    }

    fn has_secret(&self, key: &SecretKey) -> Result<bool, String> {
        let map = self
            .store
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        Ok(map.contains_key(key))
    }

    fn backend_kind(&self) -> StorageBackendKind {
        StorageBackendKind::Session
    }

    fn is_available(&self) -> bool {
        true
    }
}

/// High-level manager coordinating OS keyring storage and session fallback.
pub struct SecretManager {
    primary: Box<dyn SecretStore>,
    session: MemorySecretStore,
}

impl SecretManager {
    pub fn new(primary: Box<dyn SecretStore>) -> Self {
        Self {
            primary,
            session: MemorySecretStore::new(),
        }
    }

    /// Isolated in-memory secret manager for unit tests.
    pub fn new_in_memory() -> Self {
        Self::new(Box::new(MemorySecretStore::new()))
    }

    /// Global standard secret manager.
    pub fn global() -> &'static SecretManager {
        static INSTANCE: OnceLock<SecretManager> = OnceLock::new();
        INSTANCE.get_or_init(|| SecretManager::new(Box::new(OsKeyringSecretStore)))
    }

    /// Store a cloud credential.
    ///
    /// When `session_only` is true, the credential is held in the volatile in-memory store
    /// for the current session without modifying or deleting any pre-existing persistent OS keyring entry.
    pub fn store_secret(
        &self,
        key: &SecretKey,
        secret: SecretValue,
        session_only: bool,
    ) -> Result<SecretSummary, String> {
        if secret.is_empty() {
            return Err("cannot store empty secret".to_string());
        }

        if session_only {
            self.session.set_secret(key, &secret)?;
            Ok(SecretSummary {
                provider: key.provider,
                profile_id: key.profile_id.clone(),
                role: key.role,
                present: true,
                backend: StorageBackendKind::Session,
            })
        } else if self.primary.is_available() {
            self.primary.set_secret(key, &secret)
                .map_err(|_| "OS credential store write failed".to_string())?;
            // Clean up session override copy if now storing persistently in OS keyring
            let _ = self.session.delete_secret(key);
            Ok(SecretSummary {
                provider: key.provider,
                profile_id: key.profile_id.clone(),
                role: key.role,
                present: true,
                backend: StorageBackendKind::Keyring,
            })
        } else {
            Err("OS credential store unavailable on this host; use session-only storage".to_string())
        }
    }

    /// Delete a cloud credential across both keyring and session stores.
    ///
    /// Never claims successful deletion if the OS credential manager failed.
    pub fn delete_secret(&self, key: &SecretKey) -> Result<SecretSummary, String> {
        // A failed persistent deletion must never be reported as complete.
        self.session.delete_secret(key)?;
        if !self.primary.is_available() {
            return Err("OS credential store unavailable; persistent deletion unconfirmed".into());
        }
        self.primary.delete_secret(key)
            .map_err(|_| "OS credential store deletion failed".to_string())?;

        Ok(SecretSummary {
            provider: key.provider,
            profile_id: key.profile_id.clone(),
            role: key.role,
            present: false,
            backend: StorageBackendKind::Keyring,
        })
    }

    /// Get safe summary of credential presence.
    pub fn get_summary(&self, key: &SecretKey) -> Result<SecretSummary, String> {
        if self.session.has_secret(key)? {
            return Ok(SecretSummary {
                provider: key.provider,
                profile_id: key.profile_id.clone(),
                role: key.role,
                present: true,
                backend: StorageBackendKind::Session,
            });
        }

        if self.primary.is_available() {
            match self.primary.has_secret(key) {
                Ok(present) => Ok(SecretSummary {
                    provider: key.provider,
                    profile_id: key.profile_id.clone(),
                    role: key.role,
                    present,
                    backend: StorageBackendKind::Keyring,
                }),
                Err(_) => Ok(SecretSummary {
                    provider: key.provider,
                    profile_id: key.profile_id.clone(),
                    role: key.role,
                    present: false,
                    backend: StorageBackendKind::Unavailable,
                }),
            }
        } else {
            Ok(SecretSummary {
                provider: key.provider,
                profile_id: key.profile_id.clone(),
                role: key.role,
                present: false,
                backend: StorageBackendKind::Unavailable,
            })
        }
    }

    /// Backend-only retrieval of secret material for authenticating `/mc/v1` requests.
    pub fn get_secret(&self, key: &SecretKey) -> Result<Option<SecretValue>, String> {
        if let Some(sec) = self.session.get_secret(key)? {
            return Ok(Some(sec));
        }
        if self.primary.is_available() {
            self.primary.get_secret(key)
                .map_err(|_| "OS credential store read failed".to_string())
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_value_redacted_in_debug_and_display() {
        let val = SecretValue::new("super-secret-token-123");
        assert_eq!(format!("{val:?}"), "[REDACTED]");
        assert_eq!(format!("{val}"), "[REDACTED]");
        assert_eq!(val.expose_str().unwrap(), "super-secret-token-123");
        assert_eq!(val.len(), "super-secret-token-123".len());
        assert!(!val.is_empty());
    }

    #[test]
    fn role_and_provider_isolation() {
        let store = MemorySecretStore::new();

        let beam_runtime = SecretKey::new(
            CloudProvider::Beam,
            "prof-1",
            "fingerprint-a",
            SecretRole::Runtime,
        );
        let beam_setup = SecretKey::new(
            CloudProvider::Beam,
            "prof-1",
            "fingerprint-a",
            SecretRole::Setup,
        );
        let modal_runtime = SecretKey::new(
            CloudProvider::Modal,
            "prof-1",
            "fingerprint-a",
            SecretRole::Runtime,
        );

        store
            .set_secret(&beam_runtime, &SecretValue::new("beam-runtime-token"))
            .unwrap();
        store
            .set_secret(&beam_setup, &SecretValue::new("beam-setup-token"))
            .unwrap();
        store
            .set_secret(&modal_runtime, &SecretValue::new("modal-runtime-token"))
            .unwrap();

        assert_eq!(
            store
                .get_secret(&beam_runtime)
                .unwrap()
                .unwrap()
                .expose_str()
                .unwrap(),
            "beam-runtime-token"
        );
        assert_eq!(
            store
                .get_secret(&beam_setup)
                .unwrap()
                .unwrap()
                .expose_str()
                .unwrap(),
            "beam-setup-token"
        );
        assert_eq!(
            store
                .get_secret(&modal_runtime)
                .unwrap()
                .unwrap()
                .expose_str()
                .unwrap(),
            "modal-runtime-token"
        );

        // Delete beam runtime does not affect others
        store.delete_secret(&beam_runtime).unwrap();
        assert!(store.get_secret(&beam_runtime).unwrap().is_none());
        assert!(store.get_secret(&beam_setup).unwrap().is_some());
        assert!(store.get_secret(&modal_runtime).unwrap().is_some());
    }

    #[test]
    fn endpoint_origin_fingerprint_change_prevents_credential_reuse() {
        let fake_keyring = Box::new(MemorySecretStore::new());
        let manager = SecretManager::new(fake_keyring);

        let orig_key = SecretKey::new(
            CloudProvider::Modal,
            "modal-prod",
            "origin-fp-endpoint-v1",
            SecretRole::Runtime,
        );

        // Store credential under origin v1
        manager
            .store_secret(&orig_key, SecretValue::new("modal-key-v1"), false)
            .unwrap();

        assert_eq!(
            manager
                .get_secret(&orig_key)
                .unwrap()
                .unwrap()
                .expose_str()
                .unwrap(),
            "modal-key-v1"
        );

        // User changes endpoint URL -> resulting in origin-fp-endpoint-v2
        let updated_key = SecretKey::new(
            CloudProvider::Modal,
            "modal-prod",
            "origin-fp-endpoint-v2",
            SecretRole::Runtime,
        );

        // Querying under new origin must NOT find old credential
        let summary = manager.get_summary(&updated_key).unwrap();
        assert!(!summary.present, "Old credentials must not leak to new endpoint origin");
        assert!(manager.get_secret(&updated_key).unwrap().is_none());
    }

    #[test]
    fn session_override_retains_persistent_credential() {
        let fake_keyring = Box::new(MemorySecretStore::new());
        let manager = SecretManager::new(fake_keyring);

        let key = SecretKey::new(
            CloudProvider::Modal,
            "modal-dev",
            "fingerprint-1234",
            SecretRole::Runtime,
        );

        // Store persistent credential
        manager
            .store_secret(&key, SecretValue::new("persistent-token"), false)
            .unwrap();

        // Store session-only override
        manager
            .store_secret(&key, SecretValue::new("session-token"), true)
            .unwrap();

        // Active query returns session token
        let active = manager.get_secret(&key).unwrap().unwrap();
        assert_eq!(active.expose_str().unwrap(), "session-token");
        assert_eq!(manager.get_summary(&key).unwrap().backend, StorageBackendKind::Session);
    }

    #[test]
    fn sanitized_error_messages_contain_no_secrets() {
        struct FailingStore(bool);
        impl SecretStore for FailingStore {
            fn get_secret(&self, _key: &SecretKey) -> Result<Option<SecretValue>, String> {
                Err("OS credential store read failed".to_string())
            }
            fn set_secret(&self, _key: &SecretKey, _secret: &SecretValue) -> Result<(), String> {
                Err(format!("backend echoed {}", _secret.expose_str().unwrap()))
            }
            fn delete_secret(&self, _key: &SecretKey) -> Result<(), String> {
                Err("backend echoed super-secret-raw-token".to_string())
            }
            fn has_secret(&self, _key: &SecretKey) -> Result<bool, String> {
                Err("OS credential store read failed".to_string())
            }
            fn backend_kind(&self) -> StorageBackendKind {
                StorageBackendKind::Keyring
            }
            fn is_available(&self) -> bool {
                self.0
            }
        }

        let manager = SecretManager::new(Box::new(FailingStore(true)));
        let key = SecretKey::new(
            CloudProvider::Beam,
            "beam-test",
            "fingerprint-1",
            SecretRole::Runtime,
        );
        let secret = SecretValue::new("super-secret-raw-token");

        let store_err = manager.store_secret(&key, secret, false).unwrap_err();
        assert!(!store_err.contains("super-secret-raw-token"));
        assert!(store_err.contains("OS credential store write failed"));

        let delete_err = manager.delete_secret(&key).unwrap_err();
        assert!(!delete_err.contains("super-secret-raw-token"));
        assert!(delete_err.contains("deletion failed"));
        let unavailable = SecretManager::new(Box::new(FailingStore(false)));
        assert!(unavailable.delete_secret(&key).unwrap_err().contains("unconfirmed"));
    }

    #[test]
    fn memory_secret_store_poison_recovery() {
        use std::sync::Arc;

        let store = Arc::new(MemorySecretStore::new());
        let key = SecretKey::new(
            CloudProvider::Modal,
            "modal-test",
            "fingerprint-test",
            SecretRole::Runtime,
        );

        // Force a thread to panic while holding the memory store mutex
        let store_clone = Arc::clone(&store);
        let _ = std::thread::spawn(move || {
            let _guard = store_clone.store.lock().unwrap();
            panic!("intentional memory store poison panic");
        })
        .join();

        // Safe recovery: operations must succeed normally
        store.set_secret(&key, &SecretValue::new("token123")).expect("set after poison");
        assert!(store.has_secret(&key).expect("has after poison"));
        assert_eq!(
            store.get_secret(&key).expect("get after poison").unwrap().expose_str().unwrap(),
            "token123"
        );
        store.delete_secret(&key).expect("delete after poison");
        assert!(!store.has_secret(&key).expect("has after delete"));
    }

    #[test]
    fn modal_compound_secret_roundtrip_and_no_summary_leak() {
        let tid = "ak-genuine-token-id-12345";
        let tsec = "as-genuine-secret-67890";

        let compound = encode_modal_runtime_secret(tid, tsec).expect("encode valid compound");
        let (decoded_id, decoded_sec) = decode_modal_runtime_secret(&compound).expect("decode valid compound");

        assert_eq!(decoded_id, tid);
        assert_eq!(decoded_sec.expose_str().unwrap(), tsec);

        // Prove summaries never contain secret or token ID
        let store = Box::new(MemorySecretStore::new());
        let manager = SecretManager::new(store);
        let key = SecretKey::new(
            CloudProvider::Modal,
            "modal-prof-1",
            "fingerprint-1234",
            SecretRole::Runtime,
        );

        let summary = manager.store_secret(&key, compound, false).expect("store secret");
        let serialized_summary = serde_json::to_string(&summary).expect("serialize summary");

        assert!(!serialized_summary.contains(tid), "Token ID must not leak into summary");
        assert!(!serialized_summary.contains(tsec), "Token secret must not leak into summary");
        assert!(summary.present);
        assert_eq!(summary.provider, CloudProvider::Modal);
    }

    #[test]
    fn modal_compound_secret_fails_closed_on_legacy_or_malformed() {
        // Legacy plain secret (e.g. from prior versions) fails closed
        let legacy_plain = SecretValue::new("just-a-plain-raw-secret");
        let err = decode_modal_runtime_secret(&legacy_plain).unwrap_err();
        assert!(err.contains("legacy or invalid Modal credential format"));

        // Empty token ID fails
        assert!(encode_modal_runtime_secret("", "valid-secret").is_err());
        assert!(encode_modal_runtime_secret("   ", "valid-secret").is_err());

        // Empty token secret fails
        assert!(encode_modal_runtime_secret("valid-id", "").is_err());
        assert!(encode_modal_runtime_secret("valid-id", "   ").is_err());

        // Control characters fail
        assert!(encode_modal_runtime_secret("valid-id\n", "valid-secret").is_err());
        assert!(encode_modal_runtime_secret("valid-id", "valid-secret\0").is_err());

        // Oversized fails
        let long_id = "a".repeat(257);
        assert!(encode_modal_runtime_secret(&long_id, "valid-secret").is_err());
        let long_sec = "b".repeat(1025);
        assert!(encode_modal_runtime_secret("valid-id", &long_sec).is_err());

        // Wrong version fails closed
        let bad_ver_json = SecretValue::new(r#"{"v":2,"token_id":"id","token_secret":"sec"}"#);
        assert!(decode_modal_runtime_secret(&bad_ver_json).is_err());

        // Missing field fails closed
        let incomplete_json = SecretValue::new(r#"{"v":1,"token_id":"id"}"#);
        assert!(decode_modal_runtime_secret(&incomplete_json).is_err());
    }
}
