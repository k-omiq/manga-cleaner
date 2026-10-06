# Qwen-Image-Edit-2511 as a cloud-only render model: plan

Status: shipped on 2026-09-29 as the 4-bit SDNQ base plus the 4-step Lightning LoRA on an L40S (see "Benchmark, 2026-09-29" and "What was built"). Written 2026-09-27 on `codex/cloud-integration`; the plan below is kept as written.
External facts were checked on 2026-09-27; anything marked UNVERIFIED still needs a spike.

## Goal

Offer Qwen-Image-Edit-2511 as a third cloud render model beside FLUX.2 Klein 4B and 9B.
It runs only in the user's own Modal or Beam deployment. There is no local sidecar path.
One deployment still serves one model, as today.

## What the model is

| Fact | Value | Source |
|---|---|---|
| Repo, pin | `Qwen/Qwen-Image-Edit-2511` @ `6f3ccc0b56e431dc6a0c2b2039706d7d26f22cb9` | HF API |
| License | Apache-2.0 | model card |
| Parts | 20B MMDiT (`QwenImageTransformer2DModel`, `zero_cond_t: true`), Qwen2.5-VL-7B encoder, VAE | repo config |
| bf16 size | transformer 40.9 GB, encoder 16.6 GB, VAE 0.25 GB, about 57.7 GB | HF blobs |
| Pipeline | `QwenImageEditPlusPipeline`, diffusers 0.37.0 or later (we pin 0.39.0) | diffusers PR 12839 |
| Card sampling | 40 steps, `true_cfg_scale=4.0`, `negative_prompt=" "` | model card |
| Mode | Instruction editing of 1..N images. No `mask_image`. | pipeline source |
| Output | Fully regenerated. Unedited pixels are not kept, and 2511 still drifts by a few pixels. | community guide, Qwen-Image issue 229 |

Two things follow and shape the whole plan:

1. **80 transformer passes per crop is too slow.** 40 steps with true CFG means two 20B passes per step.
   The Lightning LoRA (`lightx2v/Qwen-Image-Edit-2511-Lightning` @ `d74eba145674fd7e31b949324e148e21e7118abd`)
   runs 4 steps at `true_cfg_scale=1.0`, so 4 passes. That is about 20 times less GPU work. We plan for Lightning.
2. **The client already composites through the mask.** Klein is not mask-conditioned either
   (`native_mask_conditioning=False` for every recipe). Our crop, context, hint and tone-aligned
   composite in `crates/cleaner-core/src/engines/render.rs` already handle a model that redraws the whole crop.
   Drift is the new risk: a 2 px shift shows as a seam at the mask edge. See "Resolution" below.

## Decisions

### Quantization and weights

| Option | Size | Loads in our stack | Verdict |
|---|---|---|---|
| bf16 | 57.7 GB | yes | Needs an 80 GB GPU (A100-80, H100). Cost is 2 to 5 times L40S. Reference only. |
| **SDNQ uint4 SVD r32**, `Disty0/Qwen-Image-Edit-2511-SDNQ-uint4-svd-r32` @ `a285fceef1439d72d60533e83cb6d8921748a666` | 17.3 GB (transformer 11.6, encoder 5.4, VAE 0.25) | yes: diffusers + `sdnq`, the same stack Klein uses | **Chosen base.** Same author and format as our Klein pins, so `weights.py`, the seed function and the loader barely change. |
| GGUF Q4_K_M (unsloth) | 13.2 GB transformer | diffusers `from_single_file` | Dequantizes on every forward, so slow. Encoder is not covered. Fallback only. |
| Nunchaku int4 / fp4 | 11.5 to 14.6 GB | community repo only (QuantFunc), made for ComfyUI | No official 2511 build. Loading in diffusers is UNVERIFIED. fp4 needs Blackwell. Rejected for now. |
| FP8 (torchao or lightx2v scaled fp8) | about 20.5 GB transformer | torchao on the fly, sm89+ | Plain fp8 plus Lightning gives grid artifacts. Heavier than SDNQ. Rejected. |
| bnb NF4 | UNVERIFIED | yes | Slow on-the-fly load, "a couple of minutes" extra. Rejected. |

**Lightning on the SDNQ base is the one open question.** Three ways, in order of preference:

- **A. Runtime LoRA.** Load the Disty0 SDNQ base, then `load_lora_weights` the 849 MB Lightning LoRA and fuse it at load.
  Works only if SDNQ quantized layers accept a LoRA. UNVERIFIED. Cheapest if it works: two pinned repos, nothing we host.
- **B. Build the fused checkpoint at seed time.** The seed function downloads bf16 plus the LoRA, fuses, SDNQ-quantizes and saves to the volume.
  Nothing we host, but the seed needs about 64 GB of RAM or a GPU, downloads 58 GB, and its output bytes are not pinnable in advance.
- **C. Build it once ourselves and publish it** to a Hugging Face repo we own, pinned by commit.
  Simplest runtime, but we become a weights host with its own license and upkeep duties.

Plan: spike A first. If A fails or looks worse than a fused build, compare B and C then.

Checked 2026-09-27 on the Hugging Face API:

- Disty0 has a Lightning SDNQ build only for the first Qwen-Image-Edit (`Disty0/Qwen-Image-Edit-Lightning-SDNQ-uint4-svd-r32`, 17.29 GB, from `vladmandic/Qwen-Lightning-Edit`). There is none for 2511 or 2509.
- Community repos already hold 2511 with Lightning merged in, in bf16 diffusers layout: `Ilus-AI/Qwen-Image-Edit-2511-Lightning` (full pipeline, 57.72 GB) and `sitatech/Qwen-Image-Edit-2511-Lightning` (transformer only, 40.86 GB).
  With one of these, option B skips the fuse step. The seed downloads only the merged transformer, SDNQ-quantizes it, and takes the encoder and VAE from the Disty0 repo. Both repos have almost no downloads and unknown authors, so verify their weights against our own fuse before trusting them.
- `kanttouchthis/Qwen-Image-Edit-2511-ComfyUI-SDNQ` has a Lightning SDNQ uint4 r128 single file, in ComfyUI format. Loading it in diffusers is UNVERIFIED.

### Backend

diffusers + `sdnq` in the existing worker image. vLLM-Omni, SGLang-Diffusion, LightX2V and ComfyUI all serve 2511.
They are built for long-lived multi-request servers. We run one container that scales to zero, so their startup cost buys nothing.
The pins stay `diffusers==0.39.0` and `sdnq==0.2.4` unless the spike shows the Disty0 repo needs newer.
diffusers 0.40 added a native SDNQ backend, and that is a separate upgrade.

### GPU

Estimated VRAM with SDNQ: about 17.3 GB weights plus activations at about 1 MP. UNVERIFIED until the spike measures it.

| Provider | Candidate | $/h | Plan |
|---|---|---|---|
| Modal | L40S 48 GB | 1.95 | **Required GPU** unless the spike shows a better one. |
| Modal | L4 / A10 24 GB | 0.80 / 1.10 | Only if the peak fits with the encoder moved off the GPU after encoding. Measure. |
| Modal | H100 80 GB | 3.95 | Add to the allowlist only if it is at least 2 times faster than L40S, so cost per crop is not worse. |
| Beam | RTX5090 32 GB | 1.09 | Required GPU on Beam. Beam's GPU docs and pricing page disagree about H100, so leave it out. |

This needs `required_gpu` per model instead of one `gpu_for_9b` scalar.

### Resolution and drift

The pipeline resizes the reference image to about 1 MP with sides that are multiples of 32, and the VL input to about 384 by 384.
If our crop reaches the pipeline at any other size, it is resampled twice and the output shifts.

Plan: the Qwen recipe gets its own work size and stride.

- Stride 32, not 16.
- Work size chosen so that `calculate_dimensions(1024*1024, w/h)` returns exactly our size. Then the pipeline does not resize.
- Pass `height` and `width` explicitly.
- The spike measures drift against the input with a phase-correlation check on 20 real crops. Target: under 0.5 px mean.

### Prompt and the mask

- The prompt stays on the server, one per recipe, never on the wire. Start with an explicit instruction, for example
  "Remove all text, speech and sound effects. Keep the drawing, screentone and line art unchanged." The spike tunes it.
  No official wording exists.
- The mask is not an input to `QwenImageEditPlusPipeline`. The client composites through it, as for Klein.
- Spike variants to try:
  - Pass the hint (stored lettering) as a second image: "Picture 2 shows the text to remove."
  - `QwenImageEditInpaintPipeline`, which blends latents through the mask on each step. It was made for the first Qwen-Image-Edit, and one user saw 2509 ignore the mask. UNVERIFIED with 2511.
  - The training-free anchoring in arXiv 2603.27790, which improved manga text removal for 2509. Only if the plain version loses structure.

## Codebase changes

The seam map is below, grouped into work that makes the code model-agnostic first and then the Qwen work.
File references are to `codex/cloud-integration`.

### Phase 1: make the cloud path model-agnostic (no Qwen yet, Klein unchanged)

1. **One model spec owns every per-model fact.** Extend `ProductionModel` in `deploy/cloud/common/manifest.py`
   with: label, `required_gpu` per provider, worker memory GiB, runner kind, prompt, pinned sampling, stride, work size,
   license, and whether it is cloud-only. Then remove the copies:
   - `driver_base.normalize_options` `gpu_for_9b` and the inline `required_gpu` ternary in `build_plan`.
   - `worker_memory = 24 if 9B else 12` in `modal/app.py`, `beam/app.py`, `modal_driver.py`, `beam_driver.py`.
   - The 9B-vs-4B label ternary in `driver_base.build_plan`.
   - The single `RECIPE_PROMPT` and `WORK_LONG_SIDE`.
2. **The client takes sampling from the recipe.** `service.rs` builds every request with `wire_sampling()`, the local Klein
   constants. The server refuses any other sampling. `/model-info` should publish the pinned seed, steps and guidance, and the
   client should send them back. `cloud_wire.rs` is already model-agnostic.
3. **The client takes stride and work size from the recipe.** `render.rs` uses `LATENT_STRIDE 16` and `cloud_clean.rs`
   keeps its own `working_size`. Both should read the recipe. Bump the preprocessing version for recipes with a new stride.
4. **Provenance names the model, not FLUX.** `service.rs` sets `Engine::Flux` and `ladder.rung.flux` on every cloud patch.
   Record the cloud model id instead, and decide whether the `flux_render@1` grant becomes a generic cloud render grant.
   A grant rename touches consent records, so it needs a migration that keeps old grants valid.
5. **The cost estimate is per recipe.** `cloud_clean.rs` has `SECONDS_PER_WORK_MEGAPIXEL 10` and `COLD_START_SECONDS 55`
   (both Klein guesses) and picks memory with `model_id.contains("9B")`. Move these into the recipe or `/model-info`.
6. **Recipe parity tests allow a cloud-only recipe.** `test_recipe_parity.py` expects a local sidecar twin for each recipe.

Exit: all existing tests pass, and a Klein deployment behaves byte-for-byte as before.

### Phase 2: the Qwen runner and weights

1. Add the model to `PRODUCTION_MODELS` with the pinned file list and byte total from the chosen weights (A, B or C).
   `snapshot_download` fetches the whole repo, so the list must match the repo exactly.
2. Add a Qwen runner beside `SdnqFluxRunner` in `flux.py`, or in a new `qwen.py`: load with SDNQ, apply Lightning,
   call with `true_cfg_scale`, explicit `height` and `width`, and the per-recipe prompt. `filter_call_kwargs` silently drops
   unknown kwargs, so add a test that the Qwen call really receives `true_cfg_scale`.
3. Pick the runner per model in `WorkerRuntime` instead of the fixed `runner_factory`.
4. Worker sizing: host RAM 32 GiB (UNVERIFIED, measure the load peak). Seed memory stays 4 GiB for plain downloads, and is more for option B.
   Seed time for 17.3 GB fits the 3600 s seed timeout. The 30 min apply budget may not cover the first download, and the existing resume path handles that.
5. Cold start: loading 17 GB from the volume. Measure it. If it is over about 90 s, revisit the Modal CPU memory snapshot, which is off today (open issue in `docs/findings.md`).
6. The job timeout stays 600 s. With 4 steps a crop should take seconds, not minutes. Measure.

### Phase 3: provisioner and app

1. The plan offers the Qwen model with its label, license, required GPU and weights size, all from the spec.
   Replace the "pinned FLUX.2 Klein weights" and "about a minute" texts with per-model ones.
2. Modal and Beam allowlists and price tables gain any GPU the spike adds. Beam has no price for A10G or RTX5090 today.
3. `src/lib/model/pipelines.js`: the `qwen-image-edit-2511` row exists as "Soon". Give it `cloudModel`.
   Add the model id to `src/lib/model/model-names.js`. Update `DEFAULT_WEIGHTS_GB` fallbacks and the mock in `src/lib/api/mock.js`.
4. Consent and i18n text that names FLUX for every cloud render names the deployed model instead.
5. Docs: `docs/cloud-provisioning.md` (also stale today: it says options accept only `gpu` and `idle_seconds`, and it omits 9B),
   `docs/cloud-api.md`, `docs/release-evidence-matrix.md`.

### Phase 4: evidence

A live Modal run on the real scans in `~/dev/120 noisy png`, not fixtures. Same crops through Klein 4B, Klein 9B and Qwen.
Record in `docs/findings.md`: quality side by side, drift, cold start, warm seconds per crop, VRAM peak, host RAM peak, cost per page.
Unmeasured items go under "What has not been measured".

## Phase 0: the spike (do this first)

A throwaway Modal app outside the product code, on the user's `k-omiq` workspace, deleted after. Budget: about $10.

Questions and exit criteria:

| # | Question | Pass |
|---|---|---|
| 1 | Does Disty0 SDNQ 2511 load with diffusers 0.39.0 and sdnq 0.2.4? | Loads and renders |
| 2 | Does the Lightning LoRA apply on the SDNQ base (option A)? | Output at 4 steps matches bf16 + LoRA by eye on 10 crops |
| 3 | VRAM and host RAM peak on L40S and L4, with and without moving the encoder off the GPU | L4 is a candidate only if the peak is under 22 GB |
| 4 | Warm seconds per 1 MP crop on L40S, L4, H100 | Keeps a GPU only if its cost per crop is within 1.5 times the best |
| 5 | Cold start from the volume | Number recorded |
| 6 | Drift with exact-size input | Under 0.5 px mean |
| 7 | Text removal quality on 20 real crops vs Klein 4B and 9B | Clearly better on screentone or large SFX, or it is not worth shipping |
| 8 | Hint as a second image, and the inpaint pipeline | Keep one only if it clearly beats plain |

**Stop point:** if question 7 fails, stop. Qwen is 3 times the weights, needs a larger GPU and costs more per crop.
It must earn its place on real pages. `docs/milestone-ledger.md` already records that the Qwen row waits on a common benchmark.

## Risks

- LoRA on SDNQ does not work (question 2). Falls back to option B or C, each with a real cost.
- 2511 still drifts. Mitigated by exact-size input and the mask composite. Worst case it needs a small alignment step before compositing.
- The Lightning LoRA license is not yet checked. Check before shipping.
- Qwen on manga: a 2026 paper finds 2509 loses structure on manga. The spike has to show 2511 does not.
- Bigger idle cost: an L40S idle tail costs 2.4 times an L4 tail. The idle window setting matters more for this model.

## Benchmark, 2026-09-27

Harness: `spikes/cloud-crops` (Rust; exports the exact preprocessing V2 crop, hint and write weights the app
sends, and composites answers back through `PreparedRender::composite`) and `spikes/cloud-crops/bench`
(`bench.py`, a throwaway Modal app; `metrics.py`, local scoring). Weights pinned as in the tables above;
image pins as the production worker plus peft and torchvision. Seed 1 unless noted.

**Set.** 20 regions from the real scans in `~/dev/120 noisy png` (12 routed Inpaint, 6 FillAndDenoise,
2 Fill) and 9 from two user-supplied pages with sound effects drawn over art. The app's text detector
does not see those sound effects at all, so 5 of the 9 have hand masks made the way a brushed mask is
(`MASKS_DIR`), which is what the app does for a box over undetected lettering.

**Scores** (`metrics.py`; every crop also checked by eye on contact sheets):
text left on the lettering by the app's own detector; `bg_gap`, how far the edit's tone is from the page
around the lettering (catches repainted or erased balloons); `ink_left`, share of the dark lettering
pixels still dark (catches a sound effect left in place, which the detector misses); drift by phase
correlation outside the write mask.

| Variant | GPU | s/crop | bg_gap | Sound effects (ink left, 5 hand regions) | By eye |
|---|---|---|---|---|---|
| Klein 4B (production recipe) | L40S | 1.2 | 14.4 | left the big ド stroke and part of ャッ | grey blotches in white balloons |
| Klein 9B (production recipe) | L40S | 2.6 | 10.8 | left both ド strokes and part of ャッ | clean balloons |
| Qwen bf16, 40 steps, cfg 4, "去除图中所有文字，保持其他内容不变。" | H100 | 32.5 | 7.7 | left the big ド stroke and part of ャッ; removed the rest | clean balloons; best rebuild of speed lines, wall and hair |
| same, seed 2 | H100 | 32.5 | 11.5 | also left チ | seed matters on sound effects |
| Qwen, lettering blanked white first | H100 | 32.6 | 13.9 | removed the big ド stroke | left white blobs on ガラッ and ghost tone in balloons: unusable |

Rejected on the way: a 768 px work size (the pipeline's reference image is always about 1 MP, so the
answer lands on a different grid: median drift 7.9 px, max 210 px); the hint as a second image (text left
on 85% of crops); longer English prompts (erase the balloon itself); "keep the speech bubbles" (clean
balloons, but invents balloons over sound effects and puts coloured noise on a grey page); a red outline
round the region (no better than plain).

**What it says.** At full precision Qwen is as clean as Klein 9B on ordinary lettering and better on
sound effects over art, where it removes more and redraws what is behind them more convincingly. It is
not reliable there: it misses different strokes on different seeds. It costs about 12 times the GPU
time of Klein 9B on a GPU twice the price: about $0.036 per crop on an H100 against $0.0014 for Klein 9B
on an L40S, before load and idle time.

**Not measured, and it decides the plan.** Everything above is bf16 at 40 steps. Whether the 4-bit
SDNQ base and the Lightning LoRA (4 or 8 steps) keep the sound-effect advantage is the question that
makes Qwen shippable or not, and it was not run: the account ran out of credits. Next run, cheapest
first: SDNQ uint4 at 40 steps and with Lightning 8 and 4 steps on the L40S, on the 9 sound-effect crops
and 10 balloons, with the Chinese prompt and two seeds. About $3 at the measured rates.

Spend: about $20 of Modal credit, most of it 4 parallel H100s per round. The test volume was deleted.

## Benchmark, 2026-09-29

Same 29 crops as above (`mix`: 20 from `~/dev/120 noisy png`, 9 from the two sound-effect pages),
on the `onlybixi` Modal workspace. Harness: a throwaway Modal app in the session scratchpad, one L40S
container per group of variants, answers streamed back as they finish. Weights: Disty0 SDNQ uint4
SVD r32 @ `a285fce`, Lightning @ `d74eba1` (4-step and 8-step bf16 files), loaded as PEFT adapters
with `peft==0.21.1`, the Lightning scheduler, `true_cfg_scale=1.0`, no negative prompt, the crop
resized to the pipeline's own 1 MP / 32 px grid and back.

Composites here are a plain alpha blend of the answer through the crop's write weights, for every
variant alike: the Rust `composite` step now stops at page 19, because today's detector finds a
different region there than the export of 2026-09-27 did. So `bg_gap` differs from the table above
(which used the app's composite with tone alignment); compare rows within this table only. Text
detector scores were not run.

| Variant | s/crop | bg_gap | bg_gap p90 | ink left on sound effects | drift median / max px |
|---|---|---|---|---|---|
| Klein 4B (production) | 1.2 | 17.5 | 53.4 | 0.18 | 0.01 / 0.0 |
| Klein 9B (production) | 2.6 | 11.2 | 24.0 | 0.28 | 0.01 / 0.1 |
| bf16, 40 steps, cfg 4, Chinese prompt (2026-09-27, H100) | 32.5 | 9.0 | 26.4 | 0.17 | 0.03 / 0.6 |
| 4-bit + Lightning 4, Chinese prompt | 7.3 | 7.0 | 10.8 | 0.17 | 0.01 / 5.1 |
| same, seed 2 / seed 3 | 7.3 | 12.1 / 4.0 | 23.4 / 8.9 | 0.21 / 0.08 | |
| **4-bit + Lightning 4, "Remove all text, including sound effect lettering. Keep everything else unchanged."** | 7.3 | **3.8** | 9.7 | 0.07 | 0.01 / 5.1 |
| same, seed 2 / seed 3 | 7.5 | 14.8 / 3.9 | 20.7 / 8.1 | 0.18 / 0.08 | |
| 4-bit + Lightning 8, Chinese prompt | 14.2 | 7.9 | 19.4 | 0.17 | 0.01 / 5.0 |
| "keep the speech bubbles" (English, Chinese, 4 or 8 steps) | 7.3 to 14.2 | 5.0 to 5.7 | 8.8 to 12.2 | 0.04 to 0.08 | |
| "fill with the surrounding background" (English, Chinese) | 7.4 | 50 to 55 | 110 to 120 | | up to 103 |
| "Remove all text and lettering from the image. …" | 7.5 | 12.5 | 31.4 | 0.17 | |
| "…all Japanese text and onomatopoeia in the manga…" (Chinese) | 7.4 | 14.0 | 45.6 | 0.17 | |

By eye, on contact sheets:

- 4-bit with Lightning is as clean as bf16 at 40 steps. The quantization and distillation cost
  nothing visible on this set, at a quarter of the GPU time and on a GPU half the price.
- The chosen English prompt is the only variant with no failed crop at seed 1. It leaves the
  balloons of `27_04` and `06_01` clean where Klein 9B and bf16 leave grey blotches, and it removes
  the ガラッ and チャン sound effects over art. The Chinese prompt at seed 1 wrote new kana into the
  screentone box of `26_07`.
- "Keep the speech bubbles" scores low on ink left only because it paints new white bubbles over
  sound effects (`p2a_01`, `p2b_05`). Rejected, as at bf16.
- "Fill with the background" repaints whole balloons grey and shifts crops by up to 103 px. Rejected.
- 8 steps is no better than 4 and takes twice as long.
- The seed matters more than the prompt: seed 2 is worse and seed 3 better for both leading prompts.
  The client pins seed 1 for every recipe, so the recipe is the English prompt at seed 1. Moving the
  seed would need sampling per recipe on the wire, and three seeds on 29 crops are too few to pick one.
- The 5.1 px drift maximum is one crop (`p2b_04`), on the 4-step Lightning runs only.

Resources on the L40S: 17.1 GiB VRAM after load, 21.5 GiB peak while rendering (so an L4 has under
1 GiB to spare, not tried), 23.6 GiB peak host memory during load, 29 to 44 s load from a warm
volume. The worker gets 32 GiB of host memory.

Live, through the product path: the provisioner helper planned, applied, seeded (18.1 GB), deployed
and health-checked a Qwen installation (`mc-qwenlive1`). The first live render failed to load: the
production GPU image lacked torchvision, which the Qwen2-VL processor requires (the benchmark image
had it). Fixed in both images; after a resume, real crops sent through the gateway's wire API
rendered with valid result digests and pixel-identical to the benchmark answers: the first job
75.5 s from a cold container, later jobs about 15 s end to end with 2 s status polling. The
installation was then cleaned up with the helper.

Spend on `onlybixi`: see the note in `docs/findings.md`.

**Prompt changed, 2026-09-30.** The recipe now sends a longer wording that works well for another
person with a masked image-edit service: erase the lettering and sound effects, rebuild the line art,
shading, screentones and colour underneath, add nothing, and keep the framing
(`QWEN_PROMPT` in `deploy/cloud/common/manifest.py`). It was picked by its results elsewhere, not
scored on the 29 crops above; the scores above belong to the earlier English prompt.

**FukidashiErase added, 2026-10-01 (recipe v2).** On a black-and-white page the 2026-09-30 prompt
painted peach and orange blobs, since it asks to continue the "colour". The recipe now also loads
`tori29umai/QwenImageEdit2511_LoRA` `QIE2511_FukidashiErase_V1` (@ `3ca05a1`, 18,753,872 bytes,
Apache-2.0, a manga balloon-text LoRA) beside Lightning, both at weight 1.0, and sends the LoRA's
training prompt changed to keep the balloon and to name sound effects. Scores and pictures:
`docs/research/manga-inpaint-models.md`.

## What was built

- `deploy/cloud/common/manifest.py`: `ProductionModel` now owns label, runner, required GPU per
  provider, worker memory and pinned adapters; `MODEL_PROD_QWEN`, `RECIPE_PROD_QWEN`
  (`mc-qwen-image-edit-2511-v3` since 2026-10-02, v2 since 2026-10-01; preprocessing 2.0.0, seed 1 with up to two retries on the next seeds, 4 steps, guidance 1.0, the same wire
  sampling as Klein) and `QWEN_PROMPT`.
- `deploy/cloud/common/qwen.py`: `QwenEditRunner`. `worker.py` picks the runner from the model.
- `deploy/cloud/common/weights.py`: the seed also downloads each pinned adapter into
  `<snapshot>/adapters`, inside the one marker.
- Modal and Beam apps: worker memory from the spec, `peft` and `torchvision` in the GPU image.
- Provisioner: required GPU, label and worker memory from the spec, not from 9B checks.
- Client: `RenderRate` in `cloud_clean.rs` gives the Qwen estimate its own grid, rate, cold start
  and memory. The crop, hint, composite, wire checks and 16 px stride are unchanged: the worker does
  the resize to the model's grid, as the Klein worker does.
- UI: the Qwen cleaner row is offered through cloud setup, and a Qwen patch reads
  "☁ Qwen-Image-Edit-2511 · Cloud".

