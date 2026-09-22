# Manga Cleaner — Cloud Security Architecture & Backend Foundation

**Milestone Status:** P3a Delivered (Backend Security, Profiles, Credentials, Policy & Grant Foundation)
**Remaining Milestone Scope:** P3b (Backend Region Binding Integration & Consent Copy), P4–P11 (Durable Transport, Adapters, Provisioning, Batching, Release Gates)

---

## 1. Threat Model & Security Invariants

The Manga Cleaner cloud inference architecture is governed by strict fail-closed security and data-minimization boundaries designed to protect user privacy, prevent unauthorized execution, and eliminate credential exposure:

1. **Crop-Only Transmission & Zero Coordinate Leakage:** Only tightly bounded raster crops snapped to `LATENT_STRIDE = 16` and context padding are transmitted. Full pages, page-relative coordinates (`x`, `y`, `page_x`, `page_y`, `strip_rect`, `on_page`), region IDs, and local filesystem paths are strictly forbidden in wire payloads (`/mc/v1`).
2. **Local Default Target:** The default execution target is always [`ExecutionTarget::Local`]. Remote cloud execution requires explicit user configuration and backend grant issuance.
3. **Simultaneous Versioned Public Profiles:** Both Beam and Modal profiles can be configured and persisted simultaneously in dedicated `inference.json`. Public configurations enforce `deny_unknown_fields` and never contain secrets.
4. **URL & Host Hygiene (`reqwest::Url`):** All remote endpoints must use HTTPS with valid hostnames. Embedded user credentials (`@`), query parameters (`?`), URL fragments (`#`), control characters, surrounding whitespace, and conservative IP literals / loopback domains (`localhost`, `127.0.0.1`, `[::1]`, `0.0.0.0`, `10.0.0.0/8`, `192.168.0.0/16`, `172.16.0.0/12`, `169.254.0.0/16`, WHATWG octal/hex/decimal IPs, IPv4-mapped IPv6, and `.local`/`.localhost`/`.internal`/`.arpa`) are strictly rejected following trailing-dot normalization. (DNS resolution and connect-time network verification remain P3b scope).
5. **Canonical Origin Fingerprinting:** Profile URLs are canonicalized (`https://<host>` or `https://<host>:<port>`) and hashed to generate a deterministic SHA-256 fingerprint for binding secrets and grants.
6. **Origin-Bound Credential Isolation:** Cloud API keys and tokens are stored in the OS credential manager or an explicit in-memory session-only store, bound directly to the profile's canonical origin fingerprint. Changing a profile's endpoint URL prevents reuse of old credentials.
7. **Role Separation:** Cloud credentials are partitioned into separate scopes (`Setup`, `Runtime`, `ModelDownload`) that cannot substitute for or overwrite one another.
8. **Compiler-Barrier Zeroization:** [`SecretValue`] uses the `zeroize` crate (`#[derive(Zeroize, ZeroizeOnDrop)]`) to prevent compiler dead-store elimination, never derives `Serialize`, and formats as `"[REDACTED]"`.
9. **Write-Only Credential Surface & Sanitized Errors:** Secret management commands are write-only and query endpoints return safe metadata summaries ([`SecretSummary`]) without ever returning secret strings or echoing raw backend errors to JavaScript.
10. **Generic Settings Guard:** The generic settings writer recursively rejects all cloud secret/token keys to prevent accidental leakage into `settings.json`.
11. **Backend Authorization Grants:** Remote diffusion requires an unexpired, unrevoked, attempt-limited grant issued by the backend over an immutable scope (`provider`, `profile_id`, `canonical_endpoint_fingerprint`, `source_hash`, `crop_sha256`, `crop_bounds`, `mask_hash`, `revision`, `recipe`, and `region_ids: Vec<String>`). Grants expire at `now >= deadline`, detect system clock rollback, and are automatically revoked when profiles or credentials are modified.

---

## 2. Public Profile Storage (`inference.json`)

Public configuration is isolated from generic application preferences and persisted atomically to `inference.json` under the app config directory.

```json
{
  "schemaVersion": 1,
  "selectedTarget": {
    "type": "local"
  },
  "beamProfiles": {
    "beam-prod": {
      "id": "beam-prod",
      "name": "Beam GPU Cluster",
      "endpointUrl": "https://api.beam.cloud/endpoint1",
      "canonicalOrigin": "https://api.beam.cloud",
      "canonicalOriginFingerprint": "...",
      "createdAtMs": 1726780000000,
      "updatedAtMs": 1726780000000
    }
  },
  "modalProfiles": {
    "modal-prod": {
      "id": "modal-prod",
      "name": "Modal FLUX Schnell",
      "endpointUrl": "https://user-app.run.modal.com",
      "canonicalOrigin": "https://user-app.run.modal.com",
      "canonicalOriginFingerprint": "...",
      "createdAtMs": 1726780000000,
      "updatedAtMs": 1726780000000
    }
  }
}
```

### Schema & ID Validation Rules

- `schema_version`: Must equal `CURRENT_SCHEMA_VERSION` (1). Version 0 or future versions fail closed.
- `profile_id`: Bounded length (1..=64 chars), ASCII alphanumeric, `-`, and `_`.
- `endpoint_url`: Validated by `validate_https_endpoint` using `reqwest::Url`.
- Bounds: Maximum 32 profiles per provider, maximum 128 characters per profile name.

---

## 3. Credential Isolation & Role Architecture

Credentials are stored in the OS credential store under service `com.mangacleaner.studio.cloud` with account user format `<provider>:<profile_id>:<canonical_origin_fingerprint>:<role>`.

| Role | Intended Scope | Storage Lifetime |
|---|---|---|
| `Setup` | Workspace/account token for deployment helper inspection and resource creation | OS Keyring or ephemeral session |
| `Runtime` | Least-privilege proxy token for `/mc/v1` crop inference | OS Keyring or ephemeral session |
| `ModelDownload` | Token for weight snapshot downloads | OS Keyring or ephemeral session |

---

## 4. Backend Authorization Grant Service

The [`GrantService`] provides backend-enforced admission control to guarantee that no paid cloud inference request can occur without validated authorization.

```text
Backend Workflow:
User Action -> Backend Region Preparation -> Validate Eligibility -> Mint Grant (opaque 256-bit nonce)
                                                                           |
                                                                    Atomic Mutex
                                                                           |
Execute Remote HTTP -> Consume Grant (validate scope + increment attempt) -> Dispatch /mc/v1
```

### Scope Binding Invariants

Grants bind the exact immutable execution context:
- `provider`, `profile_id` & `canonical_endpoint_fingerprint`
- `source_hash`, `crop_sha256` & `mask_hash`
- `crop_bounds` ([`cleaner_core::mask::Rect`])
- `revision` & `recipe` ([`cleaner_core::engines::render::RenderRecipe`])
- `region_ids: Vec<String>`
- `max_attempts` & `expires_at_ms`

Any mismatch between the grant's scope and the execution payload fails consumption immediately with [`GrantError::ScopeMismatch`].

---

## 5. Tauri Application Permissions & Capabilities

Custom commands are governed by explicit application manifest declarations in `build.rs` and capability declarations under `src-tauri/permissions/cloud.toml` and `src-tauri/capabilities/cloud.json`.

Exposed Command Surface:
- `read_inference_config`: Read public configuration from `inference.json`.
- `write_inference_config`: Validate and write public configuration to `inference.json`.
- `store_cloud_secret`: Write-only credential storage into OS keyring or session store.
- `delete_cloud_secret`: Delete credentials from OS keyring and session store.
- `get_cloud_secret_summary`: Safe summary of credential presence and backend.

---

## 6. Milestone Register & Remaining Work

| Phase | Description | Status |
|---|---|---|
| **P3a** | Backend security foundation, public profiles, secrets, policy, grants, and settings guards | **Completed** |
| **P3b** | Backend region binding integration, frontend consent dialogs, i18n copy updates | Pending |
| **P4** | Durable attempt journal, local result caching, crash recovery, and cancellation | Pending |
| **P5** | First real cloud crop via Modal staging adapter | Pending |
| **P6** | Beam runtime parity and token permission verification | Pending |
| **P7–P8** | Automatic provisioning helpers for Modal and Beam | Pending |
| **P9** | Complete account connection and lifecycle UX | Pending |
| **P10** | Cloud-assisted chapter workflows and conservative routing | Pending |
| **P11** | End-to-end integration and release gating | Pending |
