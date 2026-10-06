# Page denoise: implementation plan

Status: plan, not started. Written 2026-09-28 on `codex/cloud-integration`.

## Goal

Many raw manga pages are blurry and grainy. Add a whole-page denoise feature that:

- runs on a whole chapter, started from the chapter list (right click a chapter, or its menu button, then **Denoise...**);
- lets the user pick settings and chain more than one engine;
- has a **Sharpen** step (upscale 2x with a model, then scale back to the original size) for blur, and **Grain** and **JPEG** steps for noise;
- uses the **cleaned** page (source plus visible patches) as input when a page has been cleaned;
- runs locally or on the cloud;
- is offered in onboarding right after the cloud step, and can be declined, in which case nothing is downloaded.

## Naming

The old `Engine::Denoise` (rung 1, "Denoise fill", a bilateral filter inside one region's mask) has been removed: pages are denoised whole, so Fill is now one flat colour, and a saved `denoise` patch, pick or ceiling reads as `fill`. The new feature is **page denoise** in code (`page_denoise`) and **Denoise** in the chapter menu. It is not a `Patch` and does not get an `Engine` variant.

## Core design decision: denoise is the last stage

Page denoise always runs over the page as it currently looks: `composite(source, visible patches)` (`crates/cleaner-core/src/composite.rs:41`, used the same way by `render_job` in `src-tauri/src/tile.rs:329`). Its output is a page-level result stored beside the patches, not a new source.

- **Input identity.** Each denoised page records the `page_appearance` digest (`src-tauri/src/tile.rs:629`) of its input, plus the recipe and model hashes.
- **Staleness.** If the page appearance changes (a patch is added, edited, hidden, reordered, or the source changes), the denoised result is **stale**. A stale result is never shown as current and never exported. The chapter list and editor show a "Denoise out of date" badge with a re-run action.
- **Denoise, then clean.** If a user denoises first and cleans later, the cleaning edits the undenoised page and the denoise result goes stale. Re-running denoise then uses the cleaned composite. This keeps one rule for every order of operations and keeps patches computed against the real source, which the integrity checks depend on.
- **Editor.** The editor edits the undenoised composite. A "Show denoised" toggle displays the fresh result for review.

Deferred: a "denoise as new base" mode where detection and cleaning read the denoised page. It needs a second source identity per page and changes the source hash invariants. Record it under "What has not been measured" in `docs/findings.md` if we ship without it.

## Recipes: more than one engine

A recipe is an ordered list of 1 to 3 steps. Each step has a kind, an engine, and settings. A per-step **strength** (0 to 100%) blends the step output with its input.

| Step | Purpose | Engines (v1 candidates) | Settings |
| --- | --- | --- | --- |
| Sharpen | Reduce blur | waifu2x swin_unet `art_scan` `scale2x` (or `noise{N}_scale2x` to fold in grain), Real-CUGAN `up2x-conservative` | scale 2x or 4x, strength |
| Grain | Reduce scan grain | waifu2x `art_scan` `noise0..3` (1x) | level 0 to 3, strength |
| JPEG | Remove JPEG blocks and ringing | 1x MangaJPEG HQ / LQ (ESRGAN, cloud only) | HQ or LQ, strength |

Sharpen downscale: convert to linear light, Lanczos-3 downscale to exactly the source width and height, convert back. Output dimensions always equal input dimensions, so patch geometry, export sizes and the `tile://` layout never change. (MangaJaNaiConverterGui uses linear-light downscaling for the same screentone reason; our own before/after check is in Phase 0.)

Presets: **Light grain**, **Heavy grain**, **JPEG cleanup**, **Blurry and grainy** (Sharpen with `noise2_scale2x`, then nothing), **Custom**. Final defaults come from Phase 0.

## Source format rules

Match the model-rung rules in `crates/cleaner-core/src/image/mod.rs:65` and `docs/features.md:160-176`:

- Grayscale: expand to 3 channels, run, fold back to luma. The page stays grayscale.
- 16-bit: run in float32, write back at 16 bits.
- Alpha: copied unchanged, never filtered.
- 1-bit, indexed, CMYK: declined in v1, with the reason shown per page. The run continues with the other pages.

## Models

| Model | Source | License | Notes |
| --- | --- | --- | --- |
| waifu2x swin_unet `art_scan` (noise0-3, scale2x, noise{N}_scale2x) | nunif export (`waifu2x/export_onnx.py`) or the `deepghs/waifu2x_onnx` mirror (`20250502/onnx_models/swin_unet/art_scan/`) | MIT | Needs an 8 px reflection pad. Swin ops may fall back to CPU on CoreML (unverified). |
| waifu2x cunet `art` | same | MIT | Plain conv, safer for CoreML and DirectML. 28 px pad. Fallback if swin is slow. |
| Real-CUGAN up2x conservative | bilibili/ailab Real-CUGAN export | MIT | ONNX export reportedly needs a fixed input shape, so one file per tile size. Optional, Phase 0 decides. |
| 1x MangaJPEG HQ / LQ | OpenModelDB (Bunzero++), `.pth` | CC-BY-NC-SA-4.0 | **Cloud only.** The worker already has PyTorch, so it loads the `.pth` directly with spandrel. No ONNX conversion and no hosted copy. |

The project is non-commercial (user decision, 2026-09-28), so NC licenses are acceptable. `docs/model-rights-decision.md` and the "Models and licensing" section of `docs/findings.md` still exclude CC-BY-NC and must be updated in the same change that adds these models. GPL and "academic use only" models (for example APISR) stay excluded.

Every model ships as a pinned `ModelPackage` row in `src-tauri/src/weights.rs:206` with URL, SHA-256, byte size, `kind_key` and `required_by: ["pageDenoise"]`, mirrored in `scripts/fetch-models.sh`. Denoise models are optional packages: they are downloaded only when the user picks local denoise.

## Phases

### Phase 0: spike on real scans (no app code)

In the scratchpad, with Python and onnxruntime, on the real scans in `~/dev/120 noisy png`:

1. Export or download each candidate model. Record its SHA-256, size, input layout, value range and padding.
2. Run each preset on every page. Produce side-by-side crops at 100% for screentone areas, line art, and flat areas.
3. Time each model per page on CPU and CoreML (Mac), and note which ops fall back to CPU.
4. Compare Lanczos in linear light with area and Lanczos in gamma space for the Sharpen downscale.
5. Pick the v1 engines, presets and defaults. Record the evidence in `docs/findings.md`.

Exit: the user has seen the comparisons and approved the v1 engine list and defaults.

### Phase 1: core engine (`crates/cleaner-core/src/page_denoise/`)

- `recipe.rs`: `Recipe`, `Step`, `StepKind`, validation, and a stable `recipe_id` digest.
- `tiles.rs`: whole-page tiling with per-model reflection padding, overlap and feathered blending. Reuse `engines/model.rs` helpers where they fit (`tile_origins`, `page_crop`, `ceiling_for`). Tile size must be fixed per model, for Real-CUGAN.
- `run.rs`: open sessions through `accel::open_session`, run steps in order, apply strength, and do the linear-light Lanczos downscale.
- Format handling from "Source format rules" above.
- Tests: tile seams (a flat page stays flat), output size equals input size, alpha untouched, gray stays gray, declined formats, and golden output for a small fixture per model.

### Phase 2: storage, staleness, display, export

- Store results in the project sidecar (`<job>.mtclean.d/denoise/<source_idx>.png`) with a record in the project manifest: `source_idx`, input appearance digest, `recipe_id`, model SHA-256s, target (local or cloud), and time.
- Add `denoised` to the `tile://` variants (`src-tauri/src/tile.rs`) and serve it only when fresh.
- Staleness check: compare the stored digest with `page_appearance` on load and after each edit.
- Export (`src-tauri/src/exporting.rs:1989-2017`): today it checks that every changed pixel lies inside `permitted_region(patches)`. Keep that check on the undenoised composite, then apply the fresh denoise result as a separately declared whole-page stage. If some pages are stale, the export dialog offers **Re-run denoise** or **Export those pages without denoise**. It never exports a stale result.

### Phase 3: chapter run

- Add `RunMode::Denoise` in `src-tauri/src/run.rs:726` and reuse the scheduler (`plan`, `execute`, `walk`), the single active-run slot, cancel (`cancel_run`) and resume (`resume_job`, with the recipe added to `CapturedRunPolicy`).
- Page queue options: all pages, only pages without a fresh result (default), or only cleaned pages.
- Progress and failures go through the existing `events.rs` events. Per-page decline or failure never stops the run.

### Phase 4: UI

- **Chapter menu** (`src/lib/home/actions.js:105`): add `denoise` to `chapterMenuItems` and `runChapterAction`. Add a `contextmenu` handler on `ChapterRow.svelte` that opens the same menu at the pointer and blocks the native menu, plus Shift+F10 and the Menu key for keyboard users. Check this in WKWebView, not only the Chromium mock.
- **Denoise dialog** (`src/lib/dialogs/DenoiseDialog.svelte`): preset picker, step list (add, remove, reorder, engine, level, strength), **Where to run** (Local or Cloud, where Cloud shows the cost estimate), page scope, and a one-page before/after preview before the chapter run starts.
- **Start path from Home.** Today only `editor.svelte.js:1477` starts runs. Add a small run starter in Home state that calls `runClean({ scope: 'chapter', mode: 'denoise', chapterId, recipe })`.
- **Progress on the chapter row**: pages done out of total, and a cancel button. Today progress exists only in the editor (`RunIndicator.svelte`).
- **Badges**: "Denoised" and "Denoise out of date" on the chapter row and in the editor page strip.
- **Editor**: "Show denoised" toggle.
- **Settings** (`SettingsDialog.svelte`): Denoise section with the target (Local, Cloud, Off), installed denoise models with download and remove, and the default preset.
- **Strings**: `src/lib/i18n/en.js`, under `home.action.denoise` and a new `denoise.*` namespace.

### Phase 5: onboarding and settings key

- New settings key `denoiseTarget`: `"local" | "cloud" | "off"`, validated in `src-tauri/src/settings.rs` like `cleanTarget` (`settings.rs:135`).
- New step `denoise` in `FIRST_LAUNCH_STEPS` (`src/lib/dialogs/firstlaunch.js:30`) between `cloud` and `community`, with a branch in `FirstLaunchDialog.svelte:263`.
- The step asks where to run denoise:
  - Cloud set up (`session.cloudAllowed`): **Local**, **Cloud**, **Don't use denoise**.
  - Cloud not set up: **Local** or **Don't use denoise**. Cloud is not shown.
- **Local** adds the denoise packages to the `downloads` step queue and shows their total size. **Cloud** and **Don't use denoise** download nothing.
- With `off`, the chapter menu still shows **Denoise...**. It opens a short setup screen (pick Local or Cloud) instead of the settings dialog.

### Phase 6: cloud

- **Worker**: a `denoise_pages` function in `deploy/cloud/modal/app.py`, on the existing L4 default (`deploy/cloud/modal/settings.py:26`), running the **same ONNX files** with onnxruntime-gpu so local and cloud output match. Add a recipe entry with pinned model hashes in `deploy/cloud/common/manifest.py`. Batch several pages per call to spread the cold-start cost.
- **Desktop**: follow the cloud clean flow (`src-tauri/src/inference/cloud_clean.rs`: prepare, confirm, start, cancel) with a `CloudDenoiseProposal` giving page count, estimated GPU seconds and a cost range. Keep the consent gate (`cloud_allowed` and the grant service), PNG upload with header validation (`crates/cleaner-core/src/cloud_wire.rs:734`), and journaling.
- **Upload limit**: the 16 MiB PNG cap applies to one page at original size. The 2x intermediate stays on the worker.
- **Tests**: a parity test (same page, local and cloud, max pixel difference under a set bound) and worker unit tests in `deploy/cloud/tests/`.
- **Credits**: estimate each live Modal round before running it, using Phase 0 timings. Delete test volumes after.

### Phase 7: docs

Update `docs/features.md` (new feature and engine table), `docs/model-rights-decision.md` (NC and share-alike models, attribution), `docs/findings.md` (Phase 0 evidence and anything deferred), and the third-party notices or About screen with the model attributions the licenses require.

## Risks

- Swin-based waifu2x may run partly on CPU under CoreML and DirectML. The cunet model is the fallback. Phase 0 measures this.
- Screentone damage: no model is proven safe on our scans yet. Phase 0 is the gate.
- Whole-page denoise touches every pixel, so the export integrity check needs the separate declared stage from Phase 2. Do not loosen `permitted_region`.
- Re-running after edits costs time or cloud credits. The badge and page-scope default ("only pages without a fresh result") keep this visible and small.
- MangaJPEG loading through spandrel on the Modal worker is untested.

## Open decisions for the user

1. **Model files: use existing downloads, no own hosting.** We train nothing. waifu2x already ships as ONNX on the `deepghs/waifu2x_onnx` mirror, so `weights.rs` pins those files by revision and SHA-256 and downloads them directly. MangaJPEG runs on the cloud only (user decision, 2026-09-28): the Modal worker loads the original `.pth` with spandrel, pinned by SHA-256. The JPEG step is therefore offered only when the target is Cloud.
2. **Denoise as new base** (detection and cleaning read the denoised page): defer (recommended), or include in v1.
