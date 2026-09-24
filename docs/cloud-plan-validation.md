# Cloud Integration Plan Validation Report (Beam & Modal)

**Status:** Phase record of the P0 validation of the plan at commit `03fcaba`. The plan has since been carried out offline; see [features.md](features.md) (Cloud section), [cloud-frontend.md](cloud-frontend.md), [cloud-provisioning.md](cloud-provisioning.md), and [findings.md](findings.md).

**Target Plan:** [`manga-cleaner-cloud-master-plan.md`](../manga-cleaner-cloud-master-plan.md) | **Commit:** `03fcaba` | **Status:** Structurally Aligned

## 1. Executive Verdict & Test Baseline
- **Verdict:** Master plan architecture is **structurally aligned** with codebase realities. P0 compatibility contracts, wire specifications, and baseline diagnostics are complete.
- **Baseline Test Status:**
  - *Frontend (Reviewer 2):* `npm test` passed (48 files, 913 tests, two Node experimental localStorage warnings); `npm run build` exit 0 with no warnings. Logs at `/tmp/manga-cloud-p0-frontend.log`.
  - *Rust Core/Backend:* Ran `cargo test -p cleaner-core -- --test-threads=1`: 527 unit tests passed. Targeted baseline reproduction ran once to completion via `cargo test --target-dir /Users/caved/dev/manga-cleaner/target -p manga-cleaner --lib run::tests::the_shipped_picks_keep_bubble_text_off_the_inpainter -- --nocapture`: FAILED (exit 101, 26.98s; panic at `src-tauri/src/run/tests.rs:2456:13`: `c1-p001-r8 is bubble text and ran at Lama`). Full reproduction log at `/tmp/manga-cloud-p0-baseline.log`. Temporary symlinks (`models/`, `runtimes/`) were used for weight resolution and recorded as test setup artifacts.
  - *Diagnosis:* Confirmed pre-existing on `03fcaba`. In `clean_region_with_color`, region `c1-p001-r8` either fails the `quality::assess` metric on Fill/Denoise and escalates up the ladder to LaMa, or is classified by the balloon detector as `inside == false`. Retained as open baseline issue; no tests weakened or production code modified to force pass.
- **Feasibility & Execution Gates:** Packaged helper behavior, cloud runner residency, and GPU performance remain unmeasured release gates; no cloud runner exists today.
- **Deliverables Completed:** [`docs/cloud-verification.md`](cloud-verification.md) and [`docs/cloud-api.md`](cloud-api.md).

## 2. Verified Ground Truths & Constraints
- **Local Auto Ceiling:** [`src-tauri/src/run.rs:157-188`](../src-tauri/src/run.rs#L157-L188) enforces `HIGHEST_AUTOMATIC = Engine::Lama`. Local page batch walks never invoke FLUX or Cloud.
- **FLUX Crop & Blend Seam:** [`crates/cleaner-core/src/engines/flux.rs:164-236,348-418`](../crates/cleaner-core/src/engines/flux.rs#L164-L236) uses `LATENT_STRIDE = 16`, `CONTEXT_MIN = 24`, `CONTEXT_MAX = 80`. Crop prep and alpha blend are host-side.
- **Reference SDNQ Recipe:** [`sidecar/manga_cleaner_sidecar/backend/sdnq.py:76-109`](../sidecar/manga_cleaner_sidecar/backend/sdnq.py#L76-L109) provides the reference implementation (`WORK_LONG_SIDE = 768`, `LATENT_STRIDE = 16`). Proposed cloud runner adapts this to a resident diffusers pipeline with a pinned model revision.
- **Project Persistence:** [`crates/cleaner-core/src/patch.rs:68-103`](../crates/cleaner-core/src/patch.rs#L68-L103) (`CloudRecord`), [`crates/cleaner-core/src/project/mod.rs:48-68`](../crates/cleaner-core/src/project/mod.rs#L48-L68) (current format v2 reads v1/v2; planned v3 will handle incompatible writes).
- **Keyring & Network Security:** Existing HF-token implementation in [`src-tauri/src/settings.rs:87-148`](../src-tauri/src/settings.rs#L87-L148) can fall back to plaintext; proposed cloud credentials must use secure keyring or session-only memory fallback. Outbound network routing in Rust is proposed cloud architecture consistent with current CSP in [`src-tauri/tauri.conf.json:25-32`](../src-tauri/tauri.conf.json#L25-L32).
- **Already Covered by Master Plan:** Rung and capability decoupling ([`src/lib/model/ladder.js:11`](../src/lib/model/ladder.js#L11), [`src/lib/state/capabilities.svelte.js:121`](../src/lib/state/capabilities.svelte.js#L121)), engine/location separation, session-only token fallback, and pinned runner recipe requirements are already part of the master plan design.

## 3. Specific Frontend & Model Omissions
*(Planned P3 backend authorization will block unapproved paid execution; these items represent missing UI prompt/display touchpoints)*
- **Consent UI in Actions:** [`src/lib/editor/maskactions.svelte.js:220-289`](../src/lib/editor/maskactions.svelte.js#L220-L289) (`rerunMask`, `cleanAnyway`) and [`src/lib/editor/drawing.svelte.js:258-267`](../src/lib/editor/drawing.svelte.js#L258-L267) (`createRegion`) lack user confirmation UI prompts before triggering cloud paths.
- **Legacy Engine Assumptions:** [`src/lib/model/masks.js:155-157`](../src/lib/model/masks.js#L155-L157) (`reRunnable`) checks `mask.provenance.engine !== 'cloud'`, conflating engine identity with execution location.
- **Null Cost Display Semantics:** [`src/lib/model/masks.js:64-78`](../src/lib/model/masks.js#L64-L78) (`provenanceFacts`) requires formatting handling for nullable `cloud.cost` without displaying raw `null`.

## 4. Milestone Task & Acceptance Matrix (P0 to P11)

| Phase & Deps | Implementer Scope | Reviewer 1 Scope (Rust/Backend/Security) | Reviewer 2 Scope (Frontend/UX/Packaging) |
|---|---|---|---|
| **P0: Baseline** (None) | Freeze commit `03fcaba`, diagnose baseline failure, inventory entry points, wire DTOs, ADRs | Verify Rust baseline test results & fixes | Review frontend test suite & DTO definitions |
| **P1: Feasibility** (P0) | Validate Modal SDK & Beam helper packaging on macOS/Windows | Audit helper security & subprocess boundaries | Review setup prerequisites & OS constraints |
| **P2: Core Seam** (P0) | Extract crop prep/blend from [`engines/flux.rs`](../crates/cleaner-core/src/engines/flux.rs); `.mtclean` v3 migration tests | Verify pixel-exact local parity & v1/v2/v3 reads | Verify project loads without cost regressions |
| **P3: Auth & Secrets** (P2) | Keyring storage (session-only fallback); backend consent gates | Verify zero disk leaks; enforce backend gates | Add UI prompts to [`maskactions.svelte.js`](../src/lib/editor/maskactions.svelte.js) & [`drawing.svelte.js`](../src/lib/editor/drawing.svelte.js) |
| **P4: Durable Journal** (P2, P3) | Transactional journal for `/mc/v1` handles, digests, and cached patches | Test crash recovery, idempotency, no duplicate posts | Validate optimistic UI updates & handle tracking |
| **P5: Modal Crop** (P1-P4) | `/mc/v1` async client; Modal FLUX worker (adapted `sdnq.py` recipe); single crop | Verify HTTP timeout, polling, result SHA256 digest | Verify single-region inpaint UX, cancel, and errors |
| **P6: Beam Parity** (P1, P5) | Beam `/mc/v1` gateway integration & runtime token configuration | Verify `/mc/v1` contract compliance on Beam runner | Verify provider switching in settings without desync |
| **P7: Modal Auto-Setup** (P5) | Provisioning helper: credential auth, app deploy, endpoint discovery | Audit helper execution & token scope | Verify setup wizard step-by-step UI flow |
| **P8: Beam Auto-Setup** (P6, P7) | Beam workspace inspection, deployment, token configuration | Audit Beam deployment execution & token isolation | Verify Beam setup wizard & error diagnostics UI |
| **P9: Lifecycle UX** (P7, P8) | Profile management: update, repair, key rotation, resource deletion | Verify complete remote/local cleanup | Verify Settings management UI & delete prompts |
| **P10: Cloud Chapters** (P6, P3, P4) | Explicit 2nd-pass batch orchestration; per-region durability | Verify memory limits, concurrency throttle, cancel tokens | Verify chapter batch progress, cost preview, and cancel |
| **P11: Release Gates** (P9, P10) | Packaged artifact validation, security audit, offline fallback check | Verify macOS/Windows release builds, no secret leaks | Verify end-to-end user journeys & packaging |

## 5. Test Strategy & Next Actions
- **Wire Contract Fixtures:** Mock `/mc/v1` routes (`/health`, `/model-info`, `/jobs`, `/jobs/{handle}`, `/jobs/{handle}/result`, `/jobs/{handle}/cancel`) verifying async polling, auth errors, and timeouts defined in [`docs/cloud-api.md`](cloud-api.md).
- **Durability & Fault Injection:** Interrupt process between handle persistence and result receipt; verify P4 journal recovers result on restart without duplicate remote submission.
- **Release Checkpoints:** Manual-endpoint beta after P6; automatic-setup beta after P9; chapter cloud release after P10/P11.
- **Next Action:** P0 baseline and contracts complete. Proceed to P1 (deployment-tooling/permission feasibility experiments) and P2 (core render seam extraction in `engines/flux.rs` & `.mtclean` v3 migration tests). Baseline test failure at `src-tauri/src/run/tests.rs:2456` remains an open baseline issue.
