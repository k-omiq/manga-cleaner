# Cloud Frontend API Mapping & Profile Settings UI (P3c1 / P3c2)

This document defines the bounded frontend API mapping layer and Profile Settings UI connecting the Svelte frontend to the Tauri backend for cloud inference configuration and profile management.

## 1. Tauri Command Surface

The backend exposes five explicit Tauri commands (`src-tauri/src/inference/commands.rs`):

| Seam Method | Tauri Command | Arguments | Return Shape |
|---|---|---|---|
| `readInferenceConfig()` | `read_inference_config` | *none* | `Promise<InferenceConfig>` |
| `writeInferenceConfig({ config })` | `write_inference_config` | `{ config: InferenceConfig }` | `Promise<InferenceConfig>` |
| `storeCloudSecret(spec)` | `store_cloud_secret` | `{ provider, profileId, role, secret, sessionOnly? }` | `Promise<SecretSummary>` |
| `deleteCloudSecret(spec)` | `delete_cloud_secret` | `{ provider, profileId, role }` | `Promise<SecretSummary>` |
| `getCloudSecretSummary(spec)` | `get_cloud_secret_summary` | `{ provider, profileId, role }` | `Promise<SecretSummary>` |

## 2. Data Shapes & Seam Contract

### `InferenceConfig` (`inference.json`)
Public inference configuration holding execution target and validated provider profiles:
- `schemaVersion`: `number` (currently `1`)
- `selectedTarget`: `ExecutionTarget` (`{ type: 'local' } | { type: 'beam', profile_id: string } | { type: 'modal', profile_id: string }`)
- `beamProfiles`: `Record<string, CloudProfile>`
- `modalProfiles`: `Record<string, CloudProfile>`

### `CloudProfile`
Public profile metadata without secret credentials:
- `id`: `string` (bounded identifier, 1..=64 ASCII alphanumeric, `-`, `_`)
- `name`: `string` (display name, 1..=128 characters)
- `endpointUrl`: `string` (strict `https://` URL)
- `canonicalOrigin`: `string` (normalized origin `https://<host>[:<port>]`)
- `canonicalOriginFingerprint`: `string` (SHA-256 hex digest of canonical origin)
- `createdAtMs`: `number`
- `updatedAtMs`: `number`

### `SecretSummary`
Safe public summary describing credential existence without exposing secret material:
- `provider`: `'beam' | 'modal'`
- `profileId`: `string`
- `role`: `'setup' | 'runtime' | 'model_download'`
- `present`: `boolean`
- `backend`: `'keyring' | 'session' | 'unavailable'`

## 3. Profile Settings UI (P3c2)

The `InferenceSettings.svelte` component integrates as the **Inference** tab in `SettingsDialog.svelte`:

1. **Default Execution Target Selection:**
   - Offers `Local (default)` as the primary target.
   - Populates dynamically with all configured Modal and Beam profiles.
   - Changes persist via `writeInferenceConfig({ config })`.

2. **Simultaneous Dual-Provider Profiles:**
   - Displays separate, dedicated sections for **Modal profiles** and **Beam profiles**.
   - Allows adding, editing, and deleting named profiles for both providers simultaneously.
   - Preserves non-modified profiles across providers during mutations.

3. **Target Integrity on Deletion:**
   - If a deleted profile is currently set as `selectedTarget`, the target automatically falls back to `{ type: 'local' }`.

4. **Endpoint Origin Change Warning:**
   - Editing a profile's endpoint URL to a new origin displays a prominent warning explaining that previously stored credentials remain bound to the old origin fingerprint and must be replaced separately.

5. **Safe Error Masking:**
   - Backend write and read errors display generic, localized messages and never echo raw error messages (which could expose sensitive endpoint URLs or parameters).

6. **Remote Execution Disclaimer:**
   - Explicitly informs the user that remote execution is unavailable in the current build and that selecting or configuring a profile does not transmit crops, contact remote hosts, or authorize paid work.
   - Contains no invented health check, fake ping, or misleading readiness indicators.

## 4. Core Invariants

1. **Zero Plaintext Secrets:** Plaintext API keys or tokens are NEVER stored in `inference.json`, `settings.json`, or frontend state/`localStorage`. Credential input is decoupled from public profile configuration.
2. **Write-Only Secrets:** Secret strings travel into the backend to OS keyring / session store; only safe `SecretSummary` confirmations return.
3. **No Mock Fallback for Production Methods:** In a Tauri window, all inference methods map directly to `invoke`. If an invoke rejects, the error propagates immediately without fallback to mock.
4. **Rust-Owned Validation:** Schema, URL, and character bounds validation remains strictly in Rust core; frontend performs safe client-side pre-validation and displays localized errors.
5. **Browser Mock Isolation:** Browser mock maintains an isolated in-memory clone of public config (defaulting to `{ selectedTarget: { type: 'local' }, beamProfiles: {}, modalProfiles: {} }`). Secret operations in mock explicitly reject without faking OS storage or simulating readiness.
6. **No Fake Readiness / GPU Availability:** Connection states and backend hardware readiness are never spoofed or simulated.
