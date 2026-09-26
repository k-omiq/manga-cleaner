# Cloud-integration branch: phased implementation plan

**Planning baseline:** `codex/cloud-integration` at `bceb56900164adcc95155ce3ef979d67199d3b98`, 24 September 2026. **Status:** plan only; the model exports, text-shaped mode, and repeated-edit fix have not been implemented by this document.

## Scope and decision order

This plan reconciles the unstaged `Plan for models/` pack (Documents 01–08, the 34-page combined copy, `AGENT_START_HERE.md`, and Addendum 09), `docs/repeated-inpaint-plan.md`, and the new open items in `docs/findings.md`. The combined PDF repeats Documents 01–08. Addendum 09 supersedes the earlier CTD-first mask and DBNet++-first discovery recommendations, while retaining their geometry, policy, accessibility, persistence, validation, and licensing requirements. The repeated-inpaint plan addresses an existing edit-history defect and is a prerequisite for reliable new masks and cloud results.

The target new mode is **optional text-shaped cleaning**: ogkalu RT-DETR-v2 discovers ordinary text/bubbles, the exact Koharu SAM-TS-L high-resolution head supplies lettering pixels, and COO MTSv3 can add SFX candidates if it earns its cost. CTD stays as the legacy path and a measured baseline. Recognition and translation are unnecessary for the explicit all-text policy. The current cleaning option and old project defaults stay intact. A candidate polygon is never an erase mask.

The user-facing contract is `base lettering mask B -> source-pixel padding/grouping -> approved write support W -> exact preview -> bounded patch`. For every edit, pixels outside `W` equal the composite immediately below that edit. Model crops and reading context may be larger than `W`; they grant no extra write permission. Empty or unsafe masks become review items, never filled rectangles or silent model substitutions.

### What the branch already supplies

| Existing capability | Evidence and consequence |
| --- | --- |
| CTD ONNX, ogkalu RT-DETR-v2 and a Japanese script gate | `crates/cleaner-core/src/detect/mod.rs`, `balloon.rs`, `src-tauri/src/run.rs`; reuse RT-DETR rather than introducing a second ordinary detector. CTD's line map is not yet used. |
| Binary masks, mask-bounded patches, ordered compositor, bounded `composite_region`, strip/export infrastructure | `crates/cleaner-core/src/mask.rs`, `composite.rs`, `project/mod.rs`, `export/`; extend these contracts instead of making a second image stack. The current mask dilation has mixed square/disc semantics, so define the new radius explicitly. |
| Rust `ort` runtime and provider selection | `crates/cleaner-core/src/detect/mod.rs`, `accel.rs`, `registry.rs`; use these for optional ONNX models after measured provider qualification. |
| Offline Beam/Modal FLUX crop rendering, backend grants, journal, provisioning UI | `src-tauri/src/inference/`, `deploy/cloud/`, `provisioner/`; current `/mc/v1` does **not** run detection or segmentation. No real-account end-to-end test has been recorded. |
| Per-language detector choices in setup/Settings | `src/lib/model/pipelines.js`, `state/session.svelte.js`; these currently decide downloads, not run behavior. The proposed RT-DETR/COO/SAM-TS entry is `ready: false` with no files. |

### Why the current Models list looks crowded

The screenshot shows **files**, not seven separate detection decisions. The catalogue in `src-tauri/src/weights.rs` exposes each physical artifact as an individual Check/Delete row:

| Screenshot rows | Actual role | Dependency after the new mode is wired |
| --- | --- | --- |
| Text finder (~90 MiB) | CTD: legacy text boxes and segmentation evidence | Keep for legacy and comparison; the RT-DETR + SAM-TS path does not require it. |
| Speech bubble finder (~11 MiB) | ogkalu RT-DETR-v2: text/bubble candidates and enclosure | Shared discovery input; it is not the lettering erase mask. |
| Language checker + labels (~3.6 MiB together) | Script identification for the legacy Japanese gate | Required only when that policy is selected; all-text cleaning must run without it. |
| Japanese text reader, second part, characters (~440 MiB together) | One optional manga-ocr reader with encoder, decoder and vocabulary; rescues uncertain Japanese script decisions | Never a cleaning or SAM-TS prerequisite. Keep available for the optional legacy rescue/reading workflow, off the first-launch critical path. |

The current `BASE_DETECTION` constant makes the script gate appear mandatory for every ready detector, while the three OCR files appear separately and a combined `rtdetr-coo-samts` row mixes discovery and segmentation. More confusingly, `src-tauri/src/run.rs::open_gate` attaches manga-ocr whenever all three files are present, regardless of the detector row selected in Settings; the row currently controls downloads, not that runtime behavior. M4 replaces this with a small capability graph and one logical UI row per model/workflow. Physical files and their hashes remain inspectable in details; installing, checking and removing a multi-file model is an atomic, dependency-aware action rather than leaving one part missing. Do not delete installed OCR files or disable the old rescue merely to simplify the screen.

The old `manga-cleaner-cloud-master-plan.md` is historical. Its proposed cloud files and phases are substantially implemented offline; use `docs/cloud-work-log.md` and executable code for current status. Its P10 chapter cloud pass is absent, and P11 live/platform evidence is partial.

## Milestone sequence

Milestones M0–M6 form the safe path to an optional local text-shaped release. M7 qualifies cloud compute as a separate opt-in extension. M8 closes deployment and product evidence. Export research (M2) can run beside M1, but neither model nor cloud work can bypass the write boundary from M3. Each milestone ends in reviewable artifacts and an explicit go/no-go decision; unknown performance or licensing figures are not presumed acceptable.

### M0 — Pin, reproduce, and classify failures

**Deliverables**

- Record branch/build identity, CTD/RT-DETR/LaMa/runtime hashes, existing project format, enabled providers, and currently installed models. Preserve the unstaged source documents unchanged.
- Assemble rights-cleared, **unmarked** Japanese monochrome and Korean color-webtoon pages, bubble/SFX and no-text cases, long-strip joins, plus deterministic synthetic clean-background images. The supplied colored screenshots and videos are visual references, not ground truth.
- Run the current pipeline with a ledger per region: missed discovery, held by policy, script rejection, weak mask, bad inpainting, or late/stale edit. Capture raw/visible input, candidate, base/applied mask, patch, and export. Record separate baseline recall, false candidates, correction time, artwork exposure, local latency and peak memory.
- Establish the current test/benchmark command results and platform inventory; label macOS measurements as macOS only. Set numerical parity tolerances, quality promotion thresholds, and per-device budgets **after** baseline measurements.

**Exit:** every reported sample failure has a category and reproducible artifact; no screenshot or published checkpoint score is presented as an app benchmark. This becomes the comparison baseline for M2, M5 and M6.

### M1 — Correct repeated edits and cloud crop identity

**Why first:** `src-tauri/src/region.rs::edit_with_bench` fits/renders manual AI edits against decoded raw source. `src-tauri/src/inference/consent.rs` and `service.rs` also prepare cloud crops from raw source. The compositor already knows how to render ordered visible patches into a bounded region. A later stroke can therefore read old lettering or a cloud result can attach after an earlier visible patch changes.

**Deliverables**

1. Add deterministic two-stroke and neighbor-context fixtures. New B reads all visible lower patches; retry B excludes its own and every newer patch; hidden/deleted A does not enter the input. Cover automatic, AI, paint, clone, alpha/depth and strip-join predecessors.
2. Use one immutable **bounded** composited underlay for fit, edge/noise measurements, every local renderer, quality assessment and patch base. Include the entire model read footprint and every intersecting strip source page. Reject an undersized window rather than falling back to raw. Separate original-file identity from the composited input digest.
3. Prepare cloud consent, encoded request crop/hint, and attachment from the same underlay and verify its exact digest and relevant predecessor revisions before commit. A changed underlay makes the result stale; it does not trigger a second paid job. Preserve no-write-on-failure and the existing grant scope.
4. Serialize simultaneous gestures per page in gesture order; bind work to page, region, order, mask, source and input revisions. Cancel/delete/page-switch cannot resurrect or misplace a patch. A new cloud stroke becomes one undoable gesture, not a local create followed by an independent remote undo step.
5. Persist optional versioned read-footprint/input provenance for new patches. If an earlier edit changes a later patch's actual read input, mark **Needs review** and offer Keep, Rebuild, or Undo; never silently regenerate or bill. Old records remain readable and are marked unknown only when implicated. Make Try again mean replace against lower layers; a new stroke means refine the visible result. Disable or relabel retry on non-replayable paint/clone strokes.
6. After the correctness fixture passes, evaluate a bounded underlay cache keyed by source digest, predecessor revisions, crop and exclusive order ceiling. Keep it only if dense-page measurements justify its memory cost. Compare Koharu/our LaMa and boundary ramps on identical source/crop/mask inputs before changing the model or blend.

**Exit:** fake renderers receive the expected A→B input for overlap, adjacent context, retry, hidden A and cross-join cases; rapid edits preserve order; exact pixels outside each write mask match its underlay; preview and lossless export agree. Cloud races at proposal, consent, inference and attach fail closed. Benchmark 4000×6000 and dense histories, then choose bounded-window budgets. Treat model quality and seam tuning as separate visual reviews.

### M2 — Reproduce exact models and prove ONNX parity

This is a **build/research** lane, not a Python service in the shipped app. Use temporary reproducible GPU compute where local memory is inadequate; pin image, code, package, checkpoint and output digests. Do not route source scans to a cloud machine without explicit scope and consent.

**SAM-TS-L first.** Pin `mayocream/koharu-text-sam-ts-l` revision `5dd97423e0fbf2404264979136d47e8101144046`, full `model.safetensors` and recorded SHA-256 from Addendum 09. Strictly load its `vit_l` architecture with one attention-alignment layer, prompt length 12 and hierarchical detection disabled. Reproduce the published PyTorch output: RGB; longest side 1024 with PIL bilinear; top-left placement on RGB-128 canvas; model normalization once; high-resolution logits; threshold, remove padding, nearest-neighbor restore. Save prepared tensors, embeddings, logits, restored masks and environment. Start batch 1/static 1024/FP32. Export only the **inference subgraph needed to produce the high-resolution lettering mask**: adapted image encoder (`[1,3,1024,1024]` RGB float32 in, `[1,256,64,64]` embedding out), modal aligner and text-mask head (`[1,1,1024,1024]` logits out). Omit unreachable training, hierarchical, interactive-prompt or other outputs only after confirming they do not affect the reference mask. A generic SAM decoder is not equivalent. This is a *partial graph export from the full, strictly loaded checkpoint*, not an adapter-only weight export or a claim that the entire model must ship. SAM-TS mask generation has no OCR requirement; the app's separate manga-ocr encoder/decoder/vocabulary are excluded. Validate ONNX checker, opset/IR compatibility, external weights and exact consumed keys; compare stage outputs and final masks against FP32 PyTorch before exploring BF16/FP16. Record packaged graph sizes and peak memory to confirm that pruning actually saves resources.

**COO independently.** Pin `mayocream/coo-comic-onomatopoeia-safetensors`, `mtsv3/model.safetensors`, revision `b5d31460573b6f61c1d4bdaea5fe4e18425e6a61`, and the Koharu-compatible architecture. Export only its ResNet-50/FPN/proposal-map neural path; implement BGR/mean-subtraction, resize mapping and contour/polygon scoring in Rust with separate map and polygon parity checks. Explicitly explain unused recognition weights. Do not treat proposal polygons as lettering masks or reuse SAM preprocessing. If this export fails, preserve a named blocker and continue with RT-DETR + SAM-TS; DBNet++ and other checkpoints remain separate comparators.

**Exit:** reproducible source and artifact manifests, export scripts/reports, fixed-input parity tables and error examples. No ONNX adapter is called release-ready merely because it loads. Exact weights, source implementations, training lineage and redistribution rights must be reviewed before downloads or bundling are promoted; COO MTSv3's non-commercial terms need an explicit distribution/service decision. A different runtime/custom op/replacement model requires its own documented decision.

### M3 — Make an exact, versioned text-shaped mask plan

**Deliverables**

- Add an explicit `legacy | text_shape` geometry policy. Missing old fields mean legacy; unknown future mode fails visibly. Keep current global growth and defaults on the legacy branch. Represent detector bounds, refinement crop, immutable base mask, approved support, model hole, write alpha and engine context separately.
- Derive source-pixel round padding from the immutable base plus manual corrections on every change; define circle/pixel-center semantics. `0` means exact base; `2 -> 5 -> 2` reproduces the first 2. Keep disconnected fragments, punctuation and holes unless an explicitly previewed grouping operation changes them. No convex hull, filled bbox, hidden **write** margin or cumulative dilation. Keep the engine's *input hole* independent: the existing LaMa real-scan finding supports an edge-clamped approximately 5 px hole expansion to avoid redrawn glyphs, while final writes must still stay within the previewed `W`.
- Have fill, denoise, LaMa, local FLUX and existing cloud FLUX consume the same approved support or visibly decline that mode. Their reading context may grow; their committed patch mask and alpha may not. Confirm narrow source formats retain the existing explicit engine restrictions. Final compositor checks the approved support again.
- Persist stable region ID, source/algorithm/model identity, immutable base and correction revisions, support raster/hash, source-pixel parameters, mask-quality state and plan revision in versioned records. A prepared raster reference (prefer compressed bounded tiles over full-page JSON) is the preview authority; apply requires the same source and support hash. Keep Windows WebView2's whole-response/UI-thread behavior in the preview budget and measure rapid padding updates there. Padding shrink reveals the correct lower-layer pixels. Existing projects and saved output open without optional models.
- Preserve mixed legacy/text-shaped patches, undo/redo, reruns, long strips, source mode/depth/alpha/ICC, and flattened/layered export. Use M1's ordered underlay for every new-mode operation.

**Exit:** synthetic fixtures show zero changed decoded samples outside approved support for **every enabled engine**; preview/apply hashes match; stale plans and empty/unsafe masks cannot commit; old fixtures remain unchanged; save/reopen/undo/export and join tests pass. Measure a bounded 4000×6000 page rather than assuming full-page masks are cheap.

### M4 — Wire the workflow, policies, and review UI

**Deliverables**

- Make run snapshots carry the user's per-language detector choices; a skipped language actually skips at clean time. Give area mode, detector profile, mask model, all-text policy and outside-bubble permission independent fields. Split the current combined `rtdetr-coo-samts` roadmap entry into those roles when it becomes selectable, so optional COO never controls SAM readiness. An explicit all-text run bypasses recognition/script gating and does not require gate/OCR downloads; legacy runs retain their current gate. Preserve the current outside-bubble default and make any wider permission visible and resumable by a documented rule.
- Replace file-driven setup and Settings choices with a capability graph: **Find regions** (RT-DETR; CTD for legacy), **Shape the removal mask** (SAM-TS for the new mode), **Optional SFX finder** (COO), **Optional Japanese filtering/rescue** (script gate and manga-ocr), and **Rebuild background** (fill/LaMa/other engines). Show only the few workflow choices relevant to the selected policy, with one logical row and total installed/download size per model. Put component files, exact revisions, checksums, Check and Delete controls in expandable details. Preserve direct access to those controls for troubleshooting.
- Compute downloads and readiness from the selected workflow and optional capabilities, not a universal `BASE_DETECTION` list. The no-recognition path must never fetch, open or block on language-checker labels or any of the three manga-ocr files. Make the legacy OCR rescue an explicit optional run capability instead of letting installed files silently switch it on. Install/verify/remove related files as a unit, explain which installed workflows would be affected by removal, and avoid deleting shared RT-DETR weights while another selected workflow needs them. Audit `required_by`, `filesFor`, native `models_ready`, setup, Settings, `open_gate` and run-start checks together so UI and backend cannot disagree.
- Expose a backend prepared-mask command and exact raster preview in the editor. The tinted pixels show actual write permission; bbox is a separate locator. Provide source-pixel padding, add/remove mask correction, refresh/rebuild, held/uncertain states and reviewable unassociated SAM components. Mask hit testing may have a display-only tolerance; keyboard/Layers access remains available for tiny or disconnected components.
- Show specific outcomes: undiscovered, held by policy, script-rejected on legacy, mask needs correction, engine declined, optional COO absent, stale preview, bad reconstruction. Keep setup/Settings roadmap rows disabled until real artifacts and a working run path exist. Re-rate engine stars only from one common benchmark; do not advertise estimated model speed as measured.
- Make new-stroke versus Try again semantics and later-patch dependency review visible. Keep one gesture/one undo action and the page stationary when a control is used. Show failures of background model downloads outside Settings as well.

**Exit:** UI points to the same mask hash the backend applies, survives zoom/DPR and save/reopen, remains keyboard usable, and can return to the unchanged old workflow. Use a deterministic synthetic segmenter through native prepare/apply commands at this stage; per-language picks and skip must affect an actual native run, not only download selection. M5 then repeats end-to-end checks with real ONNX outputs.

### M5 — Integrate and benchmark the local ONNX path

**Deliverables**

- Add a segmenter interface independent of CTD's boxes/language labels. Integrate RT-DETR candidates, SAM-TS source-coordinate evidence and explicit candidate-to-fragment association. Plausible unassociated components stay reviewable. Use page/overlapping crops only when M0 shows a recall benefit; map crop and strip coordinates once and deduplicate without losing provenance or changing stable IDs.
- Register/pin/checksum optional SAM graphs in the existing model manager and Rust `ort` provider/session path. Load a large graph on demand and release it under pressure; avoid simultaneous CTD, SAM, COO and FLUX residency unless measured. Account for ONNX Runtime arenas that may remain allocated after a session is idle. On macOS measure physical/unified-memory footprint as well as RSS; on Windows/Linux use measured working-set limits because the macOS kernel-pressure signal has no equivalent there. Cache prepared tensors/embeddings only when revision keys and memory benefit are established. Keep CPU FP32 a correctness path; test each intended accelerator for partition/fallback and peak memory before selecting it by default.
- Make the new mode useful without COO. Then add MTSv3 as an independently switchable discovery profile, compare RT-DETR + SAM-TS versus RT-DETR + COO + SAM-TS, and retain COO only if incremental SFX recovery outweighs false candidates, memory, latency and permission cost. CTD line/orphan recovery, higher-resolution crops, PP-OCR detector, comic segmenter and DBNet++ are measured reserve experiments, not hidden prerequisites.
- Measure the partial SAM-TS package against the complete reference path: same mask decisions, fewer shipped parameters/artifacts where possible, and actual cold load, peak memory and steady latency. Preserve a reproducible map from full checkpoint keys to exported graph weights; do not infer that a smaller file is a faster graph or silently drop an adapter or text-head operation.

**Exit:** on M0's untouched holdout, report instance and complete-page recall, text-pixel recall, non-text exposure, protected-art damage, false candidates/page, correction time, and runtime/peak by Japanese/Korean, bubble/SFX, tiny/outlined/art-contact/longstrip slices. Human mask review and reconstruction review are separate. Record exact provider/graph/weight versions; no universal language or performance claim from Japanese-only published scores.

### M6 — Local release candidate and rollback

**Deliverables:** feature-gated optional mode with legacy default, model-missing and model-failure messaging, exact artifact permissions decision, migration/rollback fixtures, user documentation and a known-failure gallery. Exercise CPU and intended macOS/Windows/Linux providers on their actual machines; unavailable hardware is marked unverified. Run lossless export pixel comparisons, mixed-mode chapters, source format restrictions, cancellation, memory pressure, and packaged model downloads. Decide numeric padding default/range, mask threshold, grouping/glow policy, crop overlap, provider limits and final UI names from M5 evidence.

**Exit:** disabling the option leaves saved patches viewable, old-project output unchanged, and no path silently swaps a model, enlarges the edit, or sends data to a cloud endpoint. Release claims are limited to measured slices plus the testable zero-outside-support invariant.

### M7 — Optional cloud computational support

The existing Beam/Modal worker performs FLUX crop rendering only. Keep it as an explicit per-region, consented path and complete M1's composited-input fix before extending it. A cloud detector/segmenter needs a **new versioned capability and result contract**, not the current FLUX hint contract relabeled. Plan separate source-page/tile consent, disclosed upload extent (including surrounding art), model revision, output masks/candidates and coordinates, transfer bounds, authorization, journal/recovery, cost estimate or honest unknown, and cancellation. Confirm provider/tier input rights, training/review policy and retention before uploading copyrighted scans; a user cannot grant a provider rights they do not hold. Retain local mask-plan approval and final composition. A remote result cannot grant a wider write mask than the user previewed.

Choose remote SAM/COO inference only where measured local memory or latency justifies upload and GPU startup. Prefer one warm worker/session per explicit batch, bounded tiles and on-demand model residency; avoid duplicate uploads, multi-provider racing, automatic paid retries or implicit chapter uploads. Use user-owned Modal/Beam profiles and existing grants/journal; do not assume remote idempotency prevents billing. Review checkpoint and training-lineage rights for remote service use as well as local downloads. Keep local CPU/ONNX functionality independent of account availability. A cloud chapter pass is a separate opt-in milestone; it is not present in the branch.

**Exit:** shared contract tests plus **real-account** Modal and Beam staging on authorized sample crops, recorded cold/warm latency, peak GPU/host memory, transfer, provider bill, recovery/cancel behavior, and cleanup. Simulated tests alone do not qualify remote inference. Cloud model export/benchmark workers may be used during development without making Python or cloud a desktop dependency.

### M8 — Platform, packaging, and remaining product evidence

- Complete the current cloud path's outstanding live account/provider checks, shipped WKWebView event/consent flow, Windows frozen-helper build and child-process cancellation, Windows/Linux runtime/plugin/DirectML/CUDA tests, and measured cloud input limits. Exercise Modal token creation versus journal interruption, slow/expired renders, Beam map cleanup/unknown tasks, provider auth/retention and settings-write races; record actual provider bills and resource cleanup. These are release gates for the existing cloud path and prerequisites for widening cloud compute. Keep the present FLUX cloud path off by default with per-render approval.
- Rebench provisional setup/Settings stars across the same pages; replace `Soon` only after downloadable validated artifacts and actual integrations. Decide Big LaMa/Qwen/other FLUX rows through a separate model/product gate; they are not prerequisites for text-shaped cleaning.
- Measure Windows custom-protocol tile throughput and GPU peaks before claiming budgets. Check app-local CRT/runtime packaging and WebGPU plugin loading on real Windows/Linux machines. Exercise close-to-tray/single-instance there and add a monochrome macOS tray asset.
- Validate PSD output in real Photoshop and guard its 2 GB size ceiling; decide stitched/indexed/sub-8-bit PSD support separately. Document old pre-import chapter behavior, original-scan replacement semantics and ingest free-space handling. These are existing fidelity/UX follow-ups rather than blockers to M3's lossless mask invariant.
- Build targeted fixtures for current script-rescue, adopted-box, paper-reading and long-strip edge cases recorded as unmeasured in `docs/findings.md`; report when optional reader/model open fails. Measure sidecar release floor, CUDA backend and context clamp only if those paths remain product priorities.

**Exit:** a release evidence matrix labels each OS/provider combination tested, unsupported or still unknown; app claims and defaults match that evidence.

## Dependency and resource rules

`M0 -> M1 -> M3 -> M4 -> M5 -> M6`; `M0 -> M2 -> M5`; `M1 + M2 + M3 + qualified cloud contract -> M7`. M7 may run beside M5–M6 if local SAM measurements justify it, but it has its own consent and live-provider release gates. M8 can proceed in parallel, with its deployment gates required before platform-specific claims. COO export can run beside SAM export, while COO promotion waits for the RT-DETR + SAM baseline. M1 should land before any new rendering path inherits the existing raw-input behavior.

The current macOS findings show a roughly 2.07–2.45 GB whole-run footprint and an approximately 1.2 GB detector arena; these are starting measurements, **not** SAM/COO budgets. Set budgets from actual peak resident/GPU memory, cold and warm latency, provider graph partitioning, and longstrip stress on each target. Prefer bounded windows, sequential optional model sessions, source-coordinate mask artifacts, on-demand downloads and explicit unload. Do not batch 512² LaMa regions merely to use a GPU: the existing benchmark found no per-region gain. Avoid a mandatory Python inference service; limited pinned Python conversion/test tooling and the existing cloud provisioner are separate build/control concerns.

## Coverage and decisions held open

| Source | Covered by | Disposition |
| --- | --- | --- |
| Document 01 C01–C09, R09–R18 | M0, M3–M6 | All confirmed behavior and geometry/persistence requirements retained. Earlier R01/R02/R08 priorities are overridden by Addendum 09. |
| Documents 02–04 | M0, M2, M3, M5 | Code audit, candidate/mask distinction, multilingual and reserve detector experiments. CTD-led first mask and DBNet++-first path are historical baselines. |
| Documents 05–06 | M3–M4 | Versioned plan, old-project defaults, preview/apply identity, correction, accessibility and workflow. |
| Documents 07–08 | M0, M2, M5–M8 | Boundary/quality benchmark, deployment evidence and exact artifact permission gates. |
| Addendum 09 and `AGENT_START_HERE.md` | M2–M7 | Exact SAM/COO targets, ONNX-first export, no-recognition policy and overrides. |
| `docs/repeated-inpaint-plan.md` | M1, M3, M7 | P0 source/ordering/race fix, P1 dependency recovery, P2 same-input quality/caching. |
| New `docs/findings.md` items and the Models screenshot | M4, M5, M7–M8 | Pipeline stars, per-language wiring, workflow-based model catalogue, `Soon` models, tray/download notices, platform/cloud live gaps and other unmeasured follow-ups. Historical measured findings remain evidence, not new tasks. |

Still open until evidence/product review: public labels; numerical padding default/range; threshold and grouping; glow/outlines; target language/style support; COO go/no-go; per-device memory/latency and model download limits; exact redistribution rights; cloud detection consent and cost policy; custom-op/alternate-runtime fallback. The earlier taxonomy, optional OCR, PP-OCRv6, DBNet++, other segmenters, VLM review and custom training remain named experiments or deferred ideas, never prerequisites silently added to this release.
