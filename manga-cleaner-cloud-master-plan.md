# Manga Cleaner — User-Owned Cloud Inference and Automatic Deployment

## Master implementation plan

**Verification date:** 19 September 2026
**Repository:** `k-omiq/manga-cleaner`
**Reviewed commit:** `03fcaba708d80835f0f5591b6d0cffe9154ad021` (`03fcaba708d8`)
**Status:** Proposed implementation; no repository modifications, builds, test executions, account changes, or GPU deployments were performed for this plan.

### Evidence and scope

This plan is grounded in the source at the commit above, the supplied 507-line integration brief, and current official Beam, Modal, and Tauri documentation. Repository observations are identified separately from proposed implementation work. The source and documentation registers at the end provide the verification trail.

This was a static source/documentation review, not an executable validation. Checkout access was unavailable in the research environment. GPU memory use, performance, packaged deployment-helper behavior, hosted Beam restricted-token permissions, and real cancellation/recovery behavior remain implementation release gates, not verified results.

The supplied brief requires Local, Beam, and Modal in one build, user-owned accounts, crop-based inference, preservation of local behavior, and no blind retry that creates duplicate billable jobs. The subsequent discussion extends onboarding to account-credential-based automatic deployment. It does not authorize this research task to create or modify cloud resources.

## 1. Product outcome

A user can choose **Set up cloud inference**, provide an eligible account's credentials, inspect the resources and permissions proposed by the app, and approve deployment. Manga Cleaner creates its installation, prepares model storage, discovers the endpoint, configures runtime authentication, and checks compatibility. The user does not have to copy an endpoint manually in this flow.

**Connect an existing deployment** remains an advanced alternative. Both Beam and Modal profiles can remain configured, while Local remains available and is the default.

The setup credential is not automatically a permanently stored inference credential. Prefer generated, narrower runtime credentials where the provider supports and the implementation verifies them. Disclose broader access when narrower scope cannot be established. The app cannot acquire privileges the supplied credential does not possess or resolve provider account eligibility requirements by itself.

### Non-negotiable behavior

1. Existing local cleaning and its automatic LaMa ceiling do not change by default.
2. Detection, masks, crop preparation, quality evaluation, composition, undo, and export remain local.
3. Cloud execution initially implements FLUX only; provider selection is not another cleaning engine.
4. Upload only the necessary crop and request metadata, not whole projects, source paths, or arbitrary application directories.
5. Ambiguous submission is not permission to submit again or switch providers.
6. Save accepted remote handles and completed results durably before treating work as recoverable or complete.
7. Account credentials never silently fall back to plaintext configuration storage.
8. Connection checks do not silently run GPU work. Model warmup/inference probes require separate approval.
9. No unknown bill, cancellation, or quality rejection is represented as a guaranteed zero charge.
10. Deployment, update, and deletion affect only explicitly approved resources owned by this installation.

## 2. Repository findings that determine the design

| Verified source | What exists now | Consequence for this plan |
|---|---|---|
| `src-tauri/src/run.rs` | Page-level `Cleaner`/`Pipeline`; highest automatic engine is LaMa, highest local engine is FLUX. | Add a region-rendering boundary below `Cleaner`. Do not raise the automatic ceiling as an incidental provider change. |
| `src-tauri/src/region.rs` | Manual FLUX uses `Bench::flux`; legacy Cloud paths report unavailable. `apply_tool`, `create_region`, `rerun_mask`, and `clean_anyway` all matter. | Cloud is not already implemented. Route and authorize every relevant manual entry point consistently. |
| `crates/cleaner-core/src/engines/flux.rs` | Crop geometry, context/padding, sidecar inference, source-format checks, and local masked composition share one implementation. | Extract preparation and composition; preserve these safeguards rather than replacing them with whole-page editing. |
| `sidecar/manga_cleaner_sidecar/backend/sdnq.py` | Local SDNQ recipe includes small-crop working-resolution behavior and desktop offload policy; stated CUDA working-set assumptions are not measured results. | Port/test the recipe, but use a cloud-specific resident CUDA loader and measure its limits. |
| `src/lib/state/capabilities.svelte.js` | FLUX availability depends on local sidecar capability. | Remote FLUX must not require local Python, local FLUX weights, or a local GPU. Local detection still has its own requirements. |
| `src/lib/model/ladder.js` | Frontend rung vocabulary does not match the Rust manual FLUX path. | Reconcile engine vocabulary independently of execution location. Preserve Paint/Clone behavior. |
| `src/lib/api/tauri.js` and `backend.js` | Production mappings coexist with a mock fallback; backend cloud confirmation is not a working remote implementation. | Every new production method must invoke a real command or explicitly report unavailable, never simulate deployment success. |
| `src-tauri/src/run.rs` and core project persistence | `walk()` obtains a complete `PageOutcome` before committing its returned regions. | Cache each paid result before page completion. Use a separate, explicit per-region cloud pass for chapter v1. |
| `src-tauri/src/settings.rs` | Existing HF-token storage can fall back to plaintext settings. | Add cloud-specific secret handling; do not reuse that fallback for account or runtime billing credentials. |
| `src/lib/editor/cloudflow.svelte.js` | Unknown estimated cost becomes zero; cloud consent is frontend-oriented. | Add backend-enforced authorization and honest nullable cost semantics. |
| `src/lib/i18n/en.js` | Legacy consent/About copy names Google and makes inaccurate assumptions about request billing. | Replace it with actual provider, transmission, authorization, and usage-billing information before cloud requests ship. |
| Core `patch.rs` and `project/mod.rs` | `CloudRecord.cost` is numeric; current project format is v2 and reads v1. | Introduce explicit cloud provenance and migration tests; use v3 when writing incompatible nullable-cost/new schema shapes. |
| `.github/workflows/ci.yml` and `release.yml` | Main CI deliberately avoids model/runtime downloads. Release targets are Apple Silicon macOS and Windows x64. | Add separate paid staging tests and clean packaged-app tests; do not equate current CI with cloud/GPU validation or promise a Linux release. |

These observations concern executable code rather than stale comments. In particular, comments about automatic cloud behavior or per-region flushing must not override the actual call sequence.

## 3. Architecture decisions

### 3.1 Separate the engine from its execution location

```text
Engine:             Fill | Denoise | LaMa | FLUX | existing manual tools
Execution target:   Local | Beam(profile_id) | Modal(profile_id)
```

A new remote FLUX patch records FLUX as its engine and its provider in cloud provenance. Retain reading of legacy `Engine::Cloud`; do not assume an older cloud patch used FLUX. Do not silently replace FLUX with LaMa when a policy says “fallback to Local.” That is a separate quality/engine decision.

### 3.2 Two independent services

```text
Svelte UI — sanitized state, explicit user approvals
    |
Tauri / Rust
    |-- InferenceService
    |      |-- Local renderer --> existing loopback sidecar
    |      |-- Beam renderer  --> authenticated async gateway
    |      `-- Modal renderer --> authenticated async gateway
    |
    `-- ProvisioningService
           `-- isolated packaged helper
                  |-- official Beam deployment tooling
                  `-- official Modal Python SDK
```

`RegionRenderer` handles prepared crop execution. `CloudProvisioner` handles account inspection, resource plans, deployment, repair, rotation, and removal. Both names are proposed application interfaces, not existing repository or provider API types.

The core owns preparation and composition; Rust application modules own network transport, credentials, admission policy, remote handles, and durable recovery. Keep the existing local sidecar protocol local. Do not widen the WebView's Internet access just to support cloud HTTP calls.

### 3.3 Versioned shared wire contract

Both templates implement application-owned routes under `/mc/v1`:

| Route | Purpose |
|---|---|
| `GET /health` | Authenticated control-service identity/reachability, without GPU inference. |
| `GET /model-info` | API version, fixed model revision, recipe version, verified weight manifest, and validated request limits. |
| `POST /jobs` | Accept a bounded crop request and return an opaque remote handle. |
| `GET /jobs/{handle}` | Report known job state; use “pending” when more detail is unavailable. |
| `GET /jobs/{handle}/result` | Authenticated result download with size, geometry, and digest checks. |
| `POST /jobs/{handle}/cancel` | Request cancellation; distinguish acknowledgement from a terminal state. |
| `POST /warmup` | Optional explicit, potentially billable worker-readiness probe. |

These are not claims about native Beam/Modal REST route names. The gateways translate them to provider-native queue/call operations.

Use multipart PNG image + grayscale hint + JSON metadata initially. Metadata includes protocol version, logical job ID, request digest, pinned model/recipe identity, dimensions, seed/settings, and input hashes. Keep original page coordinates, filenames, and final compositing masks local where not needed remotely.

The current reference-image FLUX recipe does not provide native masked-diffusion conditioning. Advertise that honestly, even when the protocol accepts a mask/hint. Local composition is responsible for limiting the actual edit.

Set application-level encoded-byte, decoded-pixel, dimension, and execution limits. Initial values are hypotheses to validate against real crop shapes, not inferred from the approximately 5 GB checkpoint size. Reject unsupported requests rather than silently resize, tile, or alter source formats.

### 3.4 Persistence and provenance

Retain `.mtclean` as the project authority. Add an auxiliary transactional journal for inference attempts, remote handles, ownership, and cached results, plus a separate installation/resource journal for provisioning. These can share a local database implementation while retaining distinct schemas and lifecycles.

Distinguish logical job, submission attempt, request digest, native remote handle, and local patch revision. Identical image content is not automatically the same user-authorized request. Cache output before committing the patch, and retain enough identity to avoid attaching a late result to an edited or deleted region.

Local durability and unique database rows do not provide cross-provider exactly-once execution. Protect against two desktop instances owning the same remote attempt, using tested cross-process locking/leases or a deliberate single-instance policy.

## 4. Milestone overview and dependencies

| Phase | Milestone | Exit evidence |
|---|---|---|
| P0 | Baseline and design contract | Pinned inventory, baseline test report, explicit invariants and initial wire/schema decisions. |
| P1 | Deployment feasibility proved early | Account/permission/tooling matrix plus packaged helper results on shipped platforms. |
| P2 | Provider-independent rendering seam | Local regression fixtures unchanged; engine/location separation and format migration tests. |
| P3 | Secure profiles and backend authorization | Both public profiles persist, secrets stay out of settings, every paid path is backend-gated. |
| P4 | Durable remote lifecycle | Fault-injection proves no automatic duplicate submission after ambiguous acceptance. |
| P5 | First real cloud crop: Modal | Manual endpoint cleans one region and recovers a known job across restart. |
| P6 | Beam runtime parity | Same build supports both endpoints, shared contract tests, explicit runtime-token scope. |
| P7 | Automatic Modal setup | Account credential + approved plan produces a usable configured installation. |
| P8 | Automatic Beam setup | Same outcome with verified SDK/token/platform support and resumable setup. |
| P9 | Complete connection/update/removal UX | Users can inspect, repair, update, rotate, stop, forget, and safely uninstall. |
| P10 | Explicit cloud-assisted chapters | Approved 20–60-page sessions recover per-region and apply conservative routing. |
| P11 | Production release gates | Clean packaged-app, security, migration, GPU, recovery, and cleanup evidence. |

P1 and P2 can proceed in parallel after P0. P3 builds on the domain decisions in P2. P4 is required before a real paid inference path is enabled. P5 requires the relevant P1 feasibility results and P2–P4. After P5, Beam runtime work (P6) and Modal provisioning (P7) can proceed in parallel. P8 builds on P6 and the shared provisioning machinery from P7. P9 requires both provisioning implementations. P10 requires both runtime implementations and the authorization/recovery foundation. P11 validates the integrated result.

**Release checkpoints:** after P6, manual-endpoint cloud beta; after P9, automatic-setup beta; after P10 and P11, full cloud-assisted chapter release. Do not call the P5 milestone a complete two-provider product.

## P0 — Freeze the baseline and define the compatibility contract

**Goal:** Establish what must remain unchanged and give every later task a testable starting point.

**Work:** Record the reviewed SHA, application targets, current command/interface shapes, project format versions, and every manual/automatic cleaning entry point. Trace `startRun`, `applyTool`, `createRegion`, `rerunMask`, and `cleanAnyway` through the actual frontend/backend paths. Record the distinction between the source ink/export mask and the applied blend mask. Identify strip/page geometry and undo dependencies.

Run the existing frontend and Rust CI commands in an environment capable of building the repository. Record their results and any pre-existing failures; this plan has not run them. Establish fixture-based tests that do not need downloaded models, plus a separate inventory of model-dependent tests.

Write architecture decision records covering engine/location separation, local-only defaults, scoped backend consent, nullable cost/provenance compatibility, journal ownership, and the shared API versioning strategy. Define how an approved deployment plan is hashed/versioned so execution cannot silently change its resource scope.

**Existing touchpoints:** `run.rs`, `region.rs`, core `flux.rs`, `patch.rs`, `project/mod.rs`, frontend API/state modules, and both existing CI/release workflows.

**Deliverables:** `docs/cloud-verification.md`, `docs/cloud-api.md`, initial regression fixtures, and a tracked verification matrix.

**Acceptance:** Every planned entry point is tied to an existing source path and a test or explicit test gap. No local behavior, deployment permission, or cloud billing is changed in this phase.

## P1 — Prove deployment-tooling and permission feasibility before building the wizard

**Goal:** Eliminate the largest platform/account uncertainties early.

**Modal experiment:** With an explicitly authorized test account, use the official client to verify connectivity and workspace identity, inspect accessible environments, and deploy a minimal CPU-only authenticated endpoint using persistent deployment. Discover its URL programmatically. Create a runtime proxy token, apply environment restrictions where supported, verify permitted/denied operations, and remove only the experiment's resources.

**Beam experiment:** With an authorized test account, prove non-interactive deployment, endpoint discovery, resource enumeration, and cleanup using a pinned released SDK/CLI. Test the actual hosted behavior of restricted token types instead of assuming the source-code label establishes least privilege. Verify access needed for submit, status, result, and cancellation separately from deployment powers.

**Packaging experiment:** Run the helper from an installed test build on Apple Silicon macOS and Windows x64. Include trusted Python template source files in controlled staging rather than assuming a frozen binary preserves all SDK source-discovery behavior. Do not bundle torch or model weights in the provisioning helper.

Beam documents Windows setup through WSL. Prove a native packaged deployment path before claiming one. Otherwise define a clearly labeled, explicitly approved WSL-assisted setup or manual-endpoint fallback. Do not silently install WSL or require administrator changes. Normal HTTPS inference and deployment tooling are separate capabilities.

**Deliverables:** A pinned SDK/version matrix, minimal provider experiments, source-packaging proof, secret-redacted execution logs, and cleanup verification.

**Acceptance:** The matrix distinguishes supported, permission-limited, unverified, and unavailable cases. Repeated inspection is non-mutating. Experiments do not overwrite global CLI profiles. OAuth and complete programmatic account creation are not assumed.

## P2 — Extract rendering and stabilize the domain/wire contracts

**Goal:** Make cloud rendering replace only the expensive crop-generation step.

**Work:** Extract pure preparation and composition from core `flux.rs` into proposed `engines/render.rs`. Preserve stride/context padding, edge replication, source-mode handling, alpha, local quality assessment, mask distinctions, and exact output geometry. The existing local `Inpainter` calls the extracted functions; it retains its own residency and platform guards.

Introduce `PreparedRender`, `GeneratedCrop`, `RenderRecipe`, `ExecutionTarget`, provider/model identity, and typed failure/state representations. Reconcile frontend engine vocabulary without inserting provider names into the ladder. Do not change the default `HIGHEST_AUTOMATIC` ceiling.

Implement protocol schemas and test fixtures shared by both cloud templates. Lock model revision and preprocessing version separately from API version. Make claimed support for native mask conditioning explicit. Unknown capabilities fail before submission.

Extend provenance to record cloud provider/profile-safe identity, remote request identity, recipe/model revision, optional measurements, and nullable known cost. Preserve reading of legacy Cloud records. Because old schemas require numeric cost, version incompatible writes deliberately and test v1/v2 import plus new-version round trips and future-version refusal.

**Existing touchpoints:** core `flux.rs`, engines `mod.rs`, `patch.rs`, project `mod.rs`; `region.rs`, `library.rs`; frontend `model/types.js` and `model/ladder.js`.

**Acceptance:** Fake-renderer tests prove unchanged decoded pixel samples outside the approved mask, preserved alpha, correct padded crop mapping, and unchanged refusal of unsupported source modes. Undo/project exports do not trigger inference. Local regression fixtures remain valid without network access.

## P3 — Secure profiles, credentials, command permissions, and consent

**Goal:** Make it possible to configure both providers without exposing account keys or permitting accidental paid work.

**Work:** Add versioned public profiles and write-only credential commands. Store account setup credentials separately from runtime credentials and model-download secrets. Prefer OS credential storage; when unavailable, offer session-only operation rather than silent plaintext fallback. Do not place secret material in ordinary settings responses, localStorage, fixtures, logs, command-line arguments, or telemetry.

A pasted secret necessarily exists briefly in the input handler. Clear it after submitting it to the backend; never pretend the input never touched JavaScript. Return only a credential-present/scope summary.

Implement origin-bound authentication, HTTPS certificate validation, redirect restrictions, hostile-URL/DNS checks, bounded decode, and protected result downloads. Keep normal cloud HTTP in Rust. Add actual command permissions through the Tauri build application manifest and capability configuration, not capability JSON alone.

Introduce backend-issued authorization bound to operation scope, provider/profile, source/region revision, recipe, and allowed attempts. Check it on every reachable paid path, including region creation and reruns. Frontend `cloudAllowed` remains a preference, not the complete authorization mechanism.

Update real Tauri mappings and mock implementations deliberately. New production methods must never fall through to fake-success. Make FLUX availability execution-target-aware. Replace the Google and no-charge consent copy; unknown estimated/billed cost remains unknown.

**Existing touchpoints:** `settings.rs`, `lib.rs`, `build.rs`, `Cargo.toml`, `tauri.conf.json`; frontend `backend.js`, `tauri.js`, `mock.js`, `fixtures.js`, capabilities/session state, cloud flow/tool metadata, settings dialog, and `i18n/en.js`.

**Acceptance:** Both profiles persist simultaneously; automated scans find no cloud secret in settings/export/logs; blocked cloud permission prevents all transmission regardless of frontend tool path; wrong-origin redirects do not receive credentials; no paid GPU operation occurs during an ordinary connection check.

## P4 — Implement durable attempts, cancellation, and crash recovery

**Goal:** Establish billing-safe lifecycle behavior before a production GPU request is possible.

**Work:** Implement a local transactional attempt journal and result cache. Persist intent before dispatch and accepted handles immediately after receipt. If the app exits between remote acceptance and local receipt persistence, recover as ambiguous—not as safe to resubmit. Persist each validated output before patch attachment or page completion.

Use bounded network calls and a cancellation registry that can be reached without acquiring the job lock held by a running render. Initially preserve the existing single-writer project discipline. Do not casually release project locks around network work without snapshot/revision validation.

Add cross-process ownership for jobs/attempts. Store source, crop, mask, recipe, and target-region revision identities. Reject stale attachment while retaining downloaded output for recovery/review. Reconcile crash points between result write, journal update, project commit, and commit acknowledgement. Cancellation acknowledgement is distinct from cancellation completion.

Use a local fake gateway to inject acceptance followed by disconnect, expired results, malformed outputs, duplicate responses, transient polling errors, close/restart, disk-full conditions, and multiple app-instance races.

**Existing touchpoints:** `region.rs`, `run.rs`, `events.rs`, `library.rs`, frontend progress/state/API mappings. New inference journal, policy, HTTP, and command modules.

**Acceptance:** Automated fault tests count submissions. Ambiguous acceptance never creates an automatic replacement. Polling/download failures reuse the known handle. A crash after result persistence requires no new inference. Undo/redo uses saved patches. Recovery does not attach results to changed/deleted regions.

## P5 — First real cloud crop through Modal

**Goal:** Deliver the smallest useful cloud feature through a manually supplied endpoint.

**Work:** Build the common worker recipe and Modal adapter. Use a CPU ASGI control API protected by Modal proxy authentication and a GPU worker invoked asynchronously. Store the native function-call identity behind the application handle and use it for status/results/cancellation.

Prepare a pinned model snapshot once as an explicit setup step, verify its manifest, and load from local cached files at worker startup. Keep the model resident per worker; do not download weights on ordinary requests. Use a cloud-specific CUDA residency policy, not the desktop offload policy by default.

Begin with one concurrent crop and at most one GPU container, minimum zero. A short idle retention setting, such as 180 seconds, is a tunable starting point, not a guaranteed reservation or demonstrated optimum. Measure supported shapes and memory peaks before advertising limits. Configure execution/startup limits explicitly.

Add a minimal real endpoint/token configuration view and connection check. The crop returns through the shared validation/composition path and existing patch persistence. Keep local FLUX optional on the client.

**Existing touchpoints:** manual region dispatch, capability gating, editor tool application, inference commands/events. New `deploy/cloud/common`, `deploy/cloud/modal/app.py`, and Modal runtime adapter.

**Acceptance:** On a supported desktop without local Python/FLUX weights, one approved region is cleaned using the staging endpoint. Tests cover cold start, a reused warm worker, cancellation, malformed results, and recovery after restart. Outside-mask samples and alpha remain unchanged. Required local detection models remain separately accounted for.

## P6 — Beam runtime parity in the same build

**Goal:** Add Beam without creating a second image-processing implementation or a provider-specific fork of the editor.

**Work:** Implement a CPU control gateway backed by a native Beam GPU task queue. Reuse the shared request schema, recipe, model snapshot strategy, limits, and result validation. Keep platform authentication enabled. Translate native queue/task states into the common state model; distinguish queue expiry before execution from failure after work began.

Set retry behavior explicitly using the pinned SDK's verified parameter, not a guessed flag copied across SDK versions. Verify cancel semantics and result retrieval in staging. Treat presigned output links as secrets; keep them out of logs/frontend state and proxy them through the authenticated gateway where appropriate. Document output retention rather than equating URL expiration with deletion.

Record the runtime credential's actual permissions. If narrower hosted credentials cannot support the necessary operations, disclose broader required access or keep the relevant setup mode restricted. Do not disable authentication to paper over token-scope uncertainty.

**Existing touchpoints:** provider registry, capabilities, events, backend/frontend profile selection. New Beam runtime adapter and `deploy/cloud/beam/app.py`.

**Acceptance:** Beam and Modal stay configured simultaneously; runtime switching needs no rebuild; the same contract fixtures work against both. A failed result download never submits fresh inference. The credential-permission matrix and server retry behavior are backed by staging evidence.

## P7 — Automatic Modal provisioning from account credentials

**Goal:** Replace manual endpoint preparation with a recoverable in-app deployment workflow.

**Work:** Build a shared provisioning controller and packaged helper with a strict versioned message protocol. The frontend may request known operations, not arbitrary shell commands, file paths, or Python modules. Stage only allowlisted deployment templates and dependency manifests. Inject secrets through protected process communication or explicit SDK clients; redact all events.

Use the verified account client to inspect identity/access, select an existing environment or propose creation, and construct a deployment plan. Include resources, expected credential scope, worker/storage settings, requested permission changes, model acquisition, and possible usage charges. Deployment begins only after approval of that plan.

Use persistent application deployment, not an ephemeral run session. Create and seed the installation's volume, deploy the CPU/GPU services, discover the endpoint, create the runtime proxy token, and scope it where supported. Role changes are optional, separately shown, and limited to the caller's authority. Do not silently change workspace defaults or elevate privileges.

Persist installation names, resource IDs, template/model versions, endpoint, token reference, and the last durable stage. Re-running setup inspects and resumes the same installation. Keep the account credential only for the current operation unless the user explicitly chooses secure retention for future maintenance.

**Verified SDK families:** `Client.from_credentials`, workspace/environment inspection, `App.deploy`, function URL discovery, and workspace proxy-token management. Account-feature availability remains a runtime preflight, not a universal assumption.

**Acceptance:** A fresh eligible account credential plus approved plan yields a configured endpoint without console copying. Interrupted setup resumes without duplicate resources. Missing privileges produce a specific blocked operation. Successful setup leaves only the disclosed credentials/resources and can forget the local setup credential.

## P8 — Automatic Beam provisioning with platform-specific gates

**Goal:** Provide the same automatic-setup contract using Beam's official deployment tools.

**Work:** Implement the Beam provisioning driver in the isolated helper. Inspect account access; create only installation-namespaced volume, secret, worker, and gateway resources; seed weights explicitly; discover endpoint/deployment IDs; validate the shared API; and save the connection.

Use non-interactive commands or supported SDK equivalents with isolated configuration. Never alter the user's default `~/.beam/config.ini` profile. Match the resource journal and approval model from P7. Document each credential that remains on the desktop or in cloud secrets, its purpose, and removal behavior.

Gate restricted-token creation and use on the P1/P6 permission results. Do not market a token as endpoint-only unless denied-operation tests establish the claim. Maintain platform authentication and disclose any remaining broad access.

For Windows, use only the native helper path proven by P1, or a clearly labeled WSL-assisted path requiring explicit user participation. Keep manual endpoint connection available even when automatic deployment cannot be supported on a particular machine.

**Acceptance:** Setup can resume after each resource-creation stage without duplicates, recover endpoint discovery, and remove its own experimental installation. Missing account prerequisites produce actionable instructions, not speculative permission changes. Packaged-platform claims match tested evidence.

## P9 — Complete the account-connection and installation lifecycle UX

**Goal:** Turn provider plumbing into a manageable user-owned installation.

**Work:** Add an Inference settings section using the existing settings design language and small focused components. Show both provider cards, current target, advanced endpoint mode, setup status, credential scope, and last successful compatibility check. The setup wizard follows account check → reviewed plan → explicit approval → progress/recovery → ready.

Distinguish control-plane reachability, authenticated access, verified weights, and measured GPU readiness. Do not report a warm model because a CPU health endpoint responds. Offer a separate potentially billable warm/test action.

Implement repair, template/model update, credential rotation, stop service, remove local connection, and uninstall. Do not delete an entire environment/workspace as a convenience. Resource deletion requires ownership verification and a concrete plan; persistent model storage gets separate approval. Retain access needed to retrieve accepted jobs or explain the consequence before revocation.

Updates must preserve compatibility for outstanding jobs or drain them before changing result routes/tokens. Cancellation of provisioning stops new steps but does not imply earlier resources were removed. Show remaining resources and provide an explicit cleanup plan.

**Acceptance:** A user can understand resource scope and manage an installation without a terminal. A half-completed setup is visible and recoverable. Forgetting a local credential is not mislabeled as provider revocation. Logs reveal neither tokens nor presigned result URLs. Worker caps are not described as a guaranteed currency budget.

## P10 — Explicit cloud-assisted chapters and conservative failover

**Goal:** Support the requested 20–60-page workload without silently converting automatic local cleaning into paid cloud processing.

**Work:** Implement a separate cloud-assisted second pass over approved eligible regions. Preserve the default local automatic run. Build a plan that names target regions, provider chain, recipe, permitted attempts, and execution limits. Require backend authorization for that bounded session; do not upload review-gated content just because it was detected.

Process one crop at a time initially. Save each completed remote result immediately and commit each region independently through the existing project machinery. Keep the GPU worker warm naturally while requests continue. Do not upload an entire chapter as one remote task or prewarm both providers merely because both are configured.

Prepare ahead or group crops only when their inputs are independent and quality/latency measurements justify it. Edits that change another region's context must invalidate precomputed requests. Preserve longstrip/window coordinate mappings and export behavior.

Select the preferred provider before submission. Failover must preserve compatible model/recipe/output capabilities and respect the session's authorization. Local fallback means compatible local FLUX unless the user explicitly approved a different engine. Never race providers speculatively for one logical job.

**Acceptance:** 20-page and 60-page staged sessions can stop, restart, recover accepted jobs, reuse saved outputs, and continue without blind replay. Warm/cold measurements are factual, not invented progress. Unknown submissions block automatic replacement. Local-only operation remains unaffected offline.

## P11 — Integrated testing, packaging, and release

**Goal:** Establish a release claim that matches actual runtime, deployment, security, and platform evidence.

**Work:** Extend existing CI with CPU-only contract, authorization, migration, retry-state, helper-protocol, redaction, and provisioning-plan tests. Keep ordinary PR checks free of mandatory paid GPU resources. Add an explicitly triggered, credential-protected staging workflow for real Beam/Modal inference and provisioning, with resource cleanup and retained evidence.

Build signed packaged helpers for the actual release targets. Test on clean Apple Silicon macOS and Windows x64 installations, not just developer machines with Python, WSL, SDK credentials, or model caches. Verify updater behavior, helper-version compatibility, subprocess cleanup, and trusted-template staging.

Test source modes and large/small/edge crops, malformed/oversized outputs, disk-full recovery, stale region results, two-instance ownership, account revocation, expired results, quota failures, ambiguous submission, cancellation, worker preemption, interrupted deployment, resource rotation, and safe removal. Include real source-pixel comparison outside the applied mask and local-only regression tests.

Collect checkpoint provenance, pinned package/image identities, measured peak VRAM/host memory, cold initialization, warm latency, supported shape limits, actual token scopes, and cleanup records. Do not demand bit-identical generated content across different GPU stacks; require geometry, provenance, contract, and untouched-pixel invariants.

**Acceptance:** All release gates are documented as pass/fail/unsupported with evidence. No provider, operating-system setup mode, or token-scope guarantee ships solely on a source-code assumption. Publish manual-endpoint fallback and recovery documentation alongside automatic setup.

## 5. Mandatory retry/failover policy

| Situation | Allowed behavior |
|---|---|
| Validation fails before dispatch | Explain and fix; another provider is eligible only if the problem is a declared capability difference and the policy permits it. |
| Connection failure demonstrably before transmission | Bounded retry or preapproved compatible fallback. |
| Authoritative admission rejection, such as a gateway rate limit before enqueue | Respect retry information; compatible fallback only within approved policy. |
| Submit disconnect/timeout or ambiguous upstream error | Record `submission_unknown`; no automatic resubmit or provider switch. |
| Accepted job; status request fails | Retry status against the same saved handle. |
| Worker asleep, queue delay, or cold start | Wait within declared limits; these do not prove failure or justify duplicate inference. |
| Native queue expiry positively confirms no execution | New attempt can be allowed under the session policy. |
| Confirmed execution failure/timeout | Additional inference is another potentially billable attempt; require explicit authorization/policy. |
| Cancel requested or acknowledged | Continue reconciliation; do not assume work stopped or no charge occurred. |
| Result retrieval interrupted | Retrieve the same result again; never rerun the model automatically. |
| Output invalid or quality rejected | Preserve diagnostic evidence; do not claim it was free. Another render requires approved retry policy. |
| Source/region changed | Keep the recovered result detached; do not overwrite new edits. |
| Result retention expired | Explain that recovery is unavailable; new paid inference requires permission. |
| App restart | Reconcile the journal and local results before making any new submission. |

Modal documents platform restarts after preemption. Disabling application retries does not prove exactly-once remote execution. Likewise, a client-generated ID is not server-side idempotency. A stronger distributed admission ledger can be investigated later; do not use eventually visible model volumes or non-atomic map operations as a substitute.

## 6. Existing-file change register

All paths in this section were inspected at the reviewed commit. A filename below is not a claim that the change has already been implemented.

| Existing path | Planned changes | Phases |
|---|---|---|
| `crates/cleaner-core/src/engines/flux.rs` | Extract pure request preparation/composition; retain local execution/residency and format rules. | P2 |
| `crates/cleaner-core/src/engines/mod.rs` | Export new render contracts. | P2 |
| `crates/cleaner-core/src/patch.rs` | Typed cloud provenance, nullable cost, preserve legacy Cloud records. | P2 |
| `crates/cleaner-core/src/project/mod.rs` | Schema migration, durable patch/cache reconciliation tests, compatibility. | P2/P4 |
| `src-tauri/src/region.rs` | Route manual FLUX across execution targets; enforce authorization for apply/create/rerun/clean-anyway paths. | P3–P6 |
| `src-tauri/src/run.rs` | Preserve local ceiling; connect explicit batch flow and cancellation safely; do not promise current per-region remote durability. | P4/P10 |
| `src-tauri/src/library.rs` | Public provenance conversion and recovery attachment checks. | P2/P4 |
| `src-tauri/src/events.rs` | Sanitized inference/provisioning events with stable operation IDs and real states. | P3–P9 |
| `src-tauri/src/settings.rs` | Versioned public cloud preferences; prevent cloud secrets in generic settings writes without disturbing unrelated settings behavior. | P3 |
| `src-tauri/src/lib.rs` | Register application services and explicit commands. | P3–P10 |
| `src-tauri/Cargo.toml` | Reuse HTTP/keyring infrastructure; add journal/helper dependencies only when justified. | P3/P4/P7 |
| `src-tauri/build.rs` | Command permission manifest and helper packaging integration as needed. | P3/P7/P11 |
| `src-tauri/tauri.conf.json` | Package trusted helper/resources; retain restrictive frontend networking. | P7/P11 |
| `src/lib/api/backend.js` | Public profiles, execution selection, authorization, remote/provisioning lifecycle contracts. | P2–P10 |
| `src/lib/api/tauri.js` | Explicit real command mappings; no mock fallback for cloud mutations or results. | P3–P10 |
| `src/lib/api/mock.js` | Deterministic mock states for both providers, failed setup, unknown acceptance, cancellation, and recovery. | P3–P10 |
| `src/lib/api/fixtures.js` | Safe public defaults; no credentials; local-only default. | P3 |
| `src/lib/api/tauri-events.js` | Event types/dispatch compatibility and consumer tests. | P4/P7 |
| `src/lib/model/types.js` | JSDoc engine/location/provenance/state types and nullable cost. | P2/P3 |
| `src/lib/model/ladder.js` | Reconcile FLUX vocabulary without making providers new rungs. | P2 |
| `src/lib/state/capabilities.svelte.js` | Target-aware availability without local sidecar requirement for remote FLUX. | P3/P5 |
| `src/lib/state/session.svelte.js` | Public selection/preferences and authorization-state lifecycle only. | P3/P9 |
| `src/lib/state/editor.svelte.js` | Policy snapshots, progress/recovery, explicit cloud pass. | P4/P10 |
| `src/lib/editor/cloudflow.svelte.js` | Correct cost/transmission handling and backend authorization integration. | P3/P9 |
| `src/lib/editor/tools.js` | Cloud-spend detection based on execution target, not engine label alone. | P3 |
| `src/lib/editor/toolapply.svelte.js` | Route selected target, approval, progress, and existing undo behavior. | P3–P6 |
| `src/lib/editor/ToolBar.svelte` | Valid engine/target combinations without extra cloud-engine rungs. | P5/P9 |
| `src/lib/dialogs/SettingsDialog.svelte` | Integrate Inference settings and setup components. | P3/P9 |
| `src/lib/i18n/en.js` | Replace legacy Google/no-charge copy; add scoped consent, setup, recovery, and honest cost messages. | P3–P10 |
| `.github/workflows/ci.yml` | CPU-only new regression/contract/helper checks. | P0/P11 |
| `.github/workflows/release.yml` | Helper packaging/signing and release-target checks. | P1/P11 |

`sidecar/manga_cleaner_sidecar/backend/sdnq.py`, its requirements, and core `sidecar/mod.rs` are verified reference implementations. The plan does not require changing desktop loading/offload or the local loopback protocol merely to host the cloud model. Generated lockfiles are updated only when dependencies actually change.

## 7. Proposed new-file register

Every path in this section is NEW/PROPOSED, not an existing repository claim. Closely related modules can be consolidated during implementation without changing the responsibilities.

| Proposed path/group | Responsibility and important interfaces |
|---|---|
| `crates/cleaner-core/src/engines/render.rs` | `PreparedRender`, `GeneratedCrop`, `RenderRecipe`; pure prepare/compose helpers. |
| `src-tauri/src/inference/mod.rs` | `InferenceService`, `RegionRenderer`, provider registry and orchestration. |
| `src-tauri/src/inference/local.rs` | Existing local FLUX adapter; preserve sidecar lifecycle rather than duplicate it. |
| `src-tauri/src/inference/http.rs` | Restricted HTTP client, multipart encoding, bounded result decoding, typed protocol errors. |
| `src-tauri/src/inference/beam.rs` | Beam authentication/state translation over the common deployment contract. |
| `src-tauri/src/inference/modal.rs` | Modal proxy-auth/runtime adapter over the same contract. |
| `src-tauri/src/inference/config.rs` | Profile/schema validation and public configuration migration. |
| `src-tauri/src/inference/secrets.rs` | Write/read/delete secret material internally; public presence/scope summaries only. |
| `src-tauri/src/inference/policy.rs` | Provider compatibility, authorization, retry/failover decisions and attempt limits. |
| `src-tauri/src/inference/journal.rs` | Attempt ownership, durable handles, result cache/reconciliation. |
| `src-tauri/src/inference/commands.rs` | Explicit profile, test, authorization, cancellation and recovery Tauri commands. |
| `src-tauri/src/inference/batch.rs` | Explicit second-pass chapter plan/execution; per-region durability. |
| `src-tauri/src/provisioning/mod.rs` | `CloudProvisioner`, reviewed deployment plans and installation ownership. |
| `src-tauri/src/provisioning/helper.rs` | Allowlisted helper execution and versioned structured message protocol. |
| `src-tauri/src/provisioning/journal.rs` | Resource IDs, operation stages, update/cleanup plans, interrupted-setup reconciliation. |
| `src-tauri/src/provisioning/commands.rs` | Inspect/plan/deploy/repair/rotate/remove command surface. |
| `src-tauri/permissions/cloud.toml` and `src-tauri/capabilities/cloud.json` | Restrict custom commands to the intended local app context. |
| `provisioner/pyproject.toml` and `provisioner/requirements.lock` | Minimal pinned setup dependencies; no local GPU inference stack. |
| `provisioner/mc_provisioner/__main__.py` and `protocol.py` | Trusted helper entry and protocol validation/redaction. |
| `provisioner/mc_provisioner/modal_driver.py` | Explicit-client Modal inspection/deploy/token/lifecycle operations. |
| `provisioner/mc_provisioner/beam_driver.py` | Isolated Beam SDK/CLI inspection/deploy/token/lifecycle operations. |
| `provisioner/tests/` | Fake-provider/helper tests and opt-in actual account tests. |
| `scripts/build-provisioner.py` | Reproducible platform helper and template packaging. |
| `deploy/cloud/common/contract.py` | Strict schema, model/recipe allowlist, hashes and limits. |
| `deploy/cloud/common/api.py` | Shared application routes and backend hooks; authentication enforced per provider. |
| `deploy/cloud/common/flux.py` | Cloud CUDA model residency and versioned preprocessing/inference/postprocessing. |
| `deploy/cloud/common/seed.py` | Explicit pinned snapshot seeding, integrity verification and completeness manifest. |
| `deploy/cloud/beam/app.py` | Beam control API, task queue, model storage, secrets and result adaptation. |
| `deploy/cloud/modal/app.py` | Modal control API, GPU worker, persistent model storage and call adaptation. |
| `deploy/cloud/requirements-control.lock` and `requirements-gpu.lock` | Independently pinned CPU gateway and CUDA inference stacks. |
| `deploy/cloud/tests/test_contract.py` and `test_staging.py` | Shared CPU contract tests and explicitly enabled paid staging evidence. |
| `src/lib/state/inference.svelte.js` | Public profile/capability/test state, never persisted secrets. |
| `src/lib/state/provisioning.svelte.js` | Sanitized setup/lifecycle state and resume behavior. |
| `src/lib/dialogs/inference/InferenceSettings.svelte` and `ProviderSetup.svelte` | Focused settings and account-setup UI; follows existing controls/i18n. |
| `docs/cloud-api.md`, `cloud-inference.md`, `cloud-provisioning.md`, `cloud-verification.md` | Protocol, user setup, permissions/recovery, and evidence/limitations. |
| `.github/workflows/cloud-staging.yml` | Explicitly triggered provider verification with cleanup and protected credentials. |

## 8. Test commands and release evidence

The following commands are aligned with the inspected repository scripts/workflow. They are instructions for implementation validation, NOT commands executed during this review.

```bash
npm ci
npm test
npm run build

cargo check --workspace --exclude spike-strip-memory
cargo clippy -p cleaner-core -p manga-cleaner --no-deps --all-targets -- -D warnings
cargo test --workspace --exclude spike-strip-memory -- --test-threads=1
```

Add separate invocation paths for contract tests, helper-packaging smoke tests, model-dependent fixtures, and protected Beam/Modal staging tests. Record the exact deployed template SHA, model snapshot revision, provider SDK versions, container identities, credential-scope findings, and resource cleanup results with each staging run.

The release gate is not “all unit tests pass.” It is: tested local invariants + no false production mock success + verified account permissions + tested packaged setup + crash/retry safety + actual cloud rendering + truthful UI/billing language + safe cleanup.

## 9. Deferred scope

Defer speculative multi-provider racing, automatic load balancing, whole-chapter remote jobs, arbitrary user-supplied model code, general cloud infrastructure management, and hard monetary-budget promises. Small independent crop batches and wider GPU concurrency require measurement first.

Modal's OAuth credential constructor exists, but a public native-app authorization/registration/scoping flow was not established by this review. Do not ship a shared client secret in the desktop app. Investigate a suitable supported account-login flow later; account-token onboarding is the verified initial route.

A stronger remote admission/deduplication service may be added later, but it must have explicit guarantees and a durable coordination design. Native platform recovery can still restart work. Never advertise exactly-once billing from an idempotency-key field alone.

## 10. Final implementation priority

Start with **P0 and P1**, then extract the local rendering seam in **P2**. Complete secure authorization and durable attempt handling **before the first paid GPU request**. Deliver the first narrow end-to-end result through Modal, bring Beam to the same runtime contract, and only then make account provisioning a polished user-facing workflow.

The core product guarantee is: **the user owns the cloud installation, the app automates only approved operations, and a recoverable rendering workflow does not blindly spend again after uncertainty.**

## Source register

The following primary sources were read for this review. Repository links are pinned to the reviewed commit. Provider documentation reflects the pages available on the verification date and must be checked again when SDK versions are pinned for implementation.

### Repository evidence

| Reference | Pinned source |
|---|---|
| R01 | [`package.json`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/package.json) |
| R02 | [`src-tauri/src/run.rs`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/src-tauri/src/run.rs) |
| R03 | [`src-tauri/src/region.rs`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/src-tauri/src/region.rs) |
| R04 | [`crates/cleaner-core/src/engines/flux.rs`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/crates/cleaner-core/src/engines/flux.rs) |
| R05 | [`crates/cleaner-core/src/engines/mod.rs`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/crates/cleaner-core/src/engines/mod.rs) |
| R06 | [`crates/cleaner-core/src/sidecar/mod.rs`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/crates/cleaner-core/src/sidecar/mod.rs) |
| R07 | [`crates/cleaner-core/src/patch.rs`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/crates/cleaner-core/src/patch.rs) |
| R08 | [`crates/cleaner-core/src/project/mod.rs`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/crates/cleaner-core/src/project/mod.rs) |
| R09 | [`sidecar/manga_cleaner_sidecar/backend/sdnq.py`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/sidecar/manga_cleaner_sidecar/backend/sdnq.py) |
| R10 | [`sidecar/requirements-sdnq.txt`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/sidecar/requirements-sdnq.txt) |
| R11 | [`src-tauri/src/settings.rs`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/src-tauri/src/settings.rs) |
| R12 | [`src-tauri/src/library.rs`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/src-tauri/src/library.rs) |
| R13 | [`src-tauri/src/events.rs`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/src-tauri/src/events.rs) |
| R14 | [`src-tauri/src/lib.rs`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/src-tauri/src/lib.rs) |
| R15 | [`src-tauri/Cargo.toml`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/src-tauri/Cargo.toml) |
| R16 | [`src-tauri/build.rs`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/src-tauri/build.rs) |
| R17 | [`src-tauri/tauri.conf.json`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/src-tauri/tauri.conf.json) |
| R18 | [`src/lib/api/backend.js`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/src/lib/api/backend.js) |
| R19 | [`src/lib/api/tauri.js`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/src/lib/api/tauri.js) |
| R20 | [`src/lib/api/mock.js`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/src/lib/api/mock.js) |
| R21 | [`src/lib/api/fixtures.js`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/src/lib/api/fixtures.js) |
| R22 | [`src/lib/api/tauri-events.js`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/src/lib/api/tauri-events.js) |
| R23 | [`src/lib/model/types.js`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/src/lib/model/types.js) |
| R24 | [`src/lib/model/ladder.js`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/src/lib/model/ladder.js) |
| R25 | [`src/lib/state/capabilities.svelte.js`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/src/lib/state/capabilities.svelte.js) |
| R26 | [`src/lib/state/session.svelte.js`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/src/lib/state/session.svelte.js) |
| R27 | [`src/lib/state/editor.svelte.js`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/src/lib/state/editor.svelte.js) |
| R28 | [`src/lib/editor/cloudflow.svelte.js`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/src/lib/editor/cloudflow.svelte.js) |
| R29 | [`src/lib/editor/tools.js`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/src/lib/editor/tools.js) |
| R30 | [`src/lib/editor/toolapply.svelte.js`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/src/lib/editor/toolapply.svelte.js) |
| R31 | [`src/lib/editor/ToolBar.svelte`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/src/lib/editor/ToolBar.svelte) |
| R32 | [`src/lib/dialogs/SettingsDialog.svelte`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/src/lib/dialogs/SettingsDialog.svelte) |
| R33 | [`src/lib/i18n/en.js`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/src/lib/i18n/en.js) |
| R34 | [`.github/workflows/ci.yml`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/.github/workflows/ci.yml) |
| R35 | [`.github/workflows/release.yml`](https://raw.githubusercontent.com/k-omiq/manga-cleaner/03fcaba708d80835f0f5591b6d0cffe9154ad021/.github/workflows/release.yml) |

### Official provider and platform evidence

| Reference | Source |
|---|---|
| D01 | [Modal Client: explicit account credentials and OAuth constructor](https://modal.com/docs/sdk/py/latest/Client) |
| D02 | [Modal App: persistent programmatic deployment](https://modal.com/docs/sdk/py/latest/App) |
| D03 | [Modal Workspace: identity and proxy-token lifecycle](https://modal.com/docs/sdk/py/latest/Workspace) |
| D04 | [Modal Environment: creation, roles, and destructive deletion scope](https://modal.com/docs/sdk/py/latest/Environment) |
| D05 | [Modal Function: endpoint discovery](https://modal.com/docs/sdk/py/latest/Function) |
| D06 | [Modal RBAC: deployment permissions and token scope](https://modal.com/docs/guide/rbac) |
| D07 | [Modal proxy authentication](https://modal.com/docs/guide/webhook-proxy-auth) |
| D08 | [Modal asynchronous job pattern](https://modal.com/docs/guide/job-queue) |
| D09 | [Modal FunctionCall: retrieval and cancellation](https://modal.com/docs/sdk/py/latest/FunctionCall) |
| D10 | [Modal preemption and input restart](https://modal.com/docs/guide/preemption) |
| D11 | [Modal Volumes: persistence, commit and reload](https://modal.com/docs/guide/volumes) |
| D12 | [Modal scaling controls](https://modal.com/docs/guide/scale) |
| D13 | [Beam quickstart: agent/non-interactive setup and signup prerequisite](https://docs.beam.cloud/v2/getting-started/quickstart) |
| D14 | [Beam installation: platform-specific tooling](https://docs.beam.cloud/v2/getting-started/installation) |
| D15 | [Beam CLI: deployments, storage, secrets and lifecycle](https://docs.beam.cloud/v2/reference/cli) |
| D16 | [Beam SDK: explicit worker/runtime controls](https://docs.beam.cloud/v2/reference/py-sdk) |
| D17 | [Beam native task submission](https://docs.beam.cloud/v2/task-queue/running-tasks) |
| D18 | [Beam task status: queue expiry distinction](https://docs.beam.cloud/v2/task-queue/query-status) |
| D19 | [Beam keep-warm behavior](https://docs.beam.cloud/v2/endpoint/keep-warm) |
| D20 | [Beam volumes: cross-container visibility](https://docs.beam.cloud/v2/data/volume) |
| D21 | [Beam public endpoints: platform-authentication implications](https://docs.beam.cloud/v2/topics/public-endpoints) |
| D22 | [Beam official SDK token source: not proof of hosted least-privilege semantics](https://raw.githubusercontent.com/beam-cloud/beta9/main/sdk/src/beta9/cli/token.py) |
| D23 | [Tauri custom-command capabilities/permissions](https://v2.tauri.app/security/capabilities/) |
| D24 | [Tauri external helper packaging](https://v2.tauri.app/develop/sidecar/) |

### User-supplied requirements

U01: `Pasted text.txt`, 507 lines. Especially lines 5–29 (scope and ownership), 67–107 (simultaneous providers and safe failover), 124–138 (crop fidelity), 169–203 (caching and chapters), 407–420 (verified file plan), and 483–507 (milestones, source grounding, and no modifications). The account-level automatic-setup requirement comes from the subsequent messages in this conversation.
