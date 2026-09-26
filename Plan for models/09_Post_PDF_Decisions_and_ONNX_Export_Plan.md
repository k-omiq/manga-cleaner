# Document 09 — Post-PDF decisions and ONNX export plan

**Version:** 1.0  |  **Research date:** 23 September 2026

**Decision:** target ONNX Runtime for a new, optional text-shaped cleaning mode. Use ogkalu RT-DETR-v2 for ordinary text/bubble discovery, a COO MTSv3 detection-only branch for extra SFX candidates, and the manga-fine-tuned Koharu SAM-TS-L high-resolution output for the lettering mask. Keep the legacy CTD path intact.

### 1. How to use this addendum

Read this first, then Documents 01-08 in the earlier documentation pack. This addendum supersedes their CTD-first model recommendations where stated below. It does not replace their unchanged safety, interface, persistence or testing requirements. The old files have not been edited.

**Status:** user requirements are confirmed; model and runtime choices below are the current recommended implementation target, not a claim that each technical detail received separate user approval. Production release remains conditional on export parity, cleanup quality, device tests and artifact permissions.

### 2. Requirements that remain unchanged

**Clean lettering, not transcripts.** Include ordinary bubble text and SFX. Do not require OCR, translation, recognized language or a successful text reading. Preserve existing optional OCR features outside this workflow.

**Add, do not replace.** Keep the current option and old-project behavior. Add a text-shaped option with adjustable padding around lettering, not a larger filled detection rectangle.

**Preview equals edit permission.** The highlighted mask is the complete allowed write area. Larger inference/inpainting context must not silently enlarge that area. Preserve manual correction, layers, undo/redo and source/export fidelity.

### 3. Exact identities: do not interchange these models

| Component | Job in the new mode |
| --- | --- |
| ogkalu/comic-text-and-bubble-detector | RT-DETR-v2: text regions and bubble context. Not CTD and not the removal mask. [N13-N14] |
| COO MTSv3 detection-only branch | Additional SFX regions. Use the Koharu-compatible proposal-network path as the initial export target. [N16-N18] |
| mayocream/koharu-text-sam-ts-l | Manga-fine-tuned SAM-TS-L: text-pixel mask. Not generic SAM and not paragraph segmentation. [N01] |
| CTD / Comic Text Detector | Preserve in legacy mode and benchmarks; not a mandatory mask dependency of the new mode. |

**No execution claim:** this session inspected source and prepared documentation. It did not export these checkpoints, run ONNX inference, benchmark the models, or change the application.


## Item-by-item decision delta

### 4. Decisions to carry into implementation

**AD09-01 — Mask model priority.** Promote the specific Koharu SAM-TS-L checkpoint from optional research comparison to the first mask-model integration target. CTD refinement is no longer a prerequisite for the new mode.

**AD09-02 — Region discovery.** Keep ogkalu RT-DETR-v2 and target COO MTSv3 for complementary SFX discovery. DBNet++ becomes a reserve comparison, not the first checkpoint selected for this implementation.

**AD09-03 — Export first.** Attempt reproducible ONNX export and Rust/ort deployment before introducing a permanent Python or LibTorch dependency. A missing published .onnx file does not establish incompatibility.

**AD09-04 — No mandatory CTD agreement.** Do not intersect the new mask with CTD as an acceptance rule. Do not blindly union broad CTD masks into it. Either operation can defeat the new mode’s purpose.

**AD09-05 — Unassociated text.** Retain plausible SAM-TS components outside detector regions as reviewable candidates. Do not automatically erase all such components or silently discard them.

**AD09-06 — Geometry is authoritative.** Store a base lettering mask. Derive padding from that base at original-image resolution. No repeated dilation of the last applied mask; no hidden engine margin after preview.

**AD09-07 — Keep decisions independent.** Detection profile, mask model, cleaning-area mode, text policy and outside-bubble permission remain separate. Missing COO must not disable a validated text-shaped mask mode.

**AD09-08 — All-text policy.** Offer an explicit no-recognition policy that can include SFX outside bubbles. Do not globally remove legacy language filtering or infer outside-bubble permission from the chosen engine.

**AD09-09 — No silent substitution.** Do not substitute stock SAM, another SAM-TS checkpoint, or current Koharu RF-DETR and call it the agreed model. Any replacement is a separately named experiment. [N01, N21]

**AD09-10 — Release is evidence-gated.** Treat the supplied screenshot as motivation, not a corpus benchmark or proof of ONNX parity. Japanese, Korean/color-webtoon and small bubble-text cases must be tested.

### 5. Explicit overrides in the earlier pack

| Earlier entry | New instruction |
| --- | --- |
| Document 01: R01, R02, R08 | Keep CTD for legacy/baseline; use Koharu mask first; target MTSv3 instead of DBNet++ first. |
| Document 01: D04, D07, O07 | MTSv3 and the specific Koharu SAM-TS-L checkpoint are active implementation targets, subject to gates. |
| Documents 03-04: CTD-led mask plan | Replace the primary mask source; retain mask safety, padding and provenance requirements. |
| Documents 05-08 | Retain existing contracts; add ONNX export/provider tests and exact new-artifact permission checks. |


## SAM-TS-L: ONNX export design

### 6. Feasibility and graph boundary

**Assessment: technically plausible; not yet validated.** The inspected forward path consists of an image encoder, learned modal aligner and text-mask decoder. These are tensor modules that can be wrapped for ONNX export. PyTorch supplies export diagnostics and custom operator translations if needed. The published release itself contains no ONNX graph. [N04, N09, N22]

Start with batch 1 and a fixed 1024 x 1024 canvas. Prefer two graphs for diagnosis and memory control; a single fused graph remains acceptable after equivalent testing. Suggested filenames and tensor names below are new design names, not existing downloadable artifacts.

| Proposed graph | Contract and retained computation |
| --- | --- |
| koharu_samts_encoder.onnx | Input: RGB float32 [1,3,1024,1024], values 0-255 after resize/gray padding. Normalize inside this wrapper. Output: embeddings [1,256,64,64]. Retain the learned encoder adapters. [N02, N05] |
| koharu_samts_text_head.onnx | Input: embeddings. Retain modal aligner, dense positional encoding and high-resolution text-mask decoder. Output: float32 high-resolution logits [1,1,1024,1024]. No clicks, text strings or OCR inputs. [N04, N06-N07] |

Exporting Meta’s standard SAM decoder alone is insufficient: its exporter targets a prompt encoder and mask decoder, while this model also needs its adapted image encoder, learned alignment and extra text-mask computation. Export the trained modules, not merely weights into the nearest-looking SAM architecture. [N05-N08]

### 7. Weight and configuration contract

Use the **full model.safetensors**, not adapter_model.safetensors alone. Construct the matching architecture and require strict loading before building wrappers. Target configuration: vit_l, one attention-alignment layer, prompt length 12, hierarchical detection disabled. [N01-N02]

```text
Repository: mayocream/koharu-text-sam-ts-l
Revision: 5dd97423e0fbf2404264979136d47e8101144046
File: model.safetensors
SHA-256:
bcd9525291677f467f0603509a0ca3df35711b4e3417cefce8da6bfc97164f45
```

### 8. Export engineering rules

Use evaluation mode and FP32 for the initial parity baseline. Wrap tensor-to-tensor calls rather than the original list-of-dictionaries interface. Replace training checkpoint wrappers with equivalent direct evaluation calls when needed, then verify equivalence. Preserve all trained adapters and the high-resolution head. [N04-N05]

Pin the exporter, ONNX IR/opset and ONNX Runtime versions. Choose an opset supported by the actual runtime/providers; do not equate the Rust ort API level with ONNX graph compatibility. Save export reports, operator lists and any decompositions. Support relative external-weight files in the model package. [N09, N11]


## Exact inference and COO export

### 9. Reference preprocessing is a release contract

Koharu’s supplied example converts to RGB, resizes the longest side to 1024 using PIL bilinear interpolation, and places the result at the top-left of a canvas filled with RGB 128. It thresholds the high-resolution output, crops away padding, then resizes the binary mask back with nearest-neighbor interpolation. Its CUDA path uses BF16 autocast. [N03]

Port the exact rounding, resampling, padding and channel normalization behavior. Do not reuse a black-padded stock-SAM preprocessor or normalize twice. Compare prepared input tensors before debugging the neural outputs.

For ONNX conversion parity, compare FP32 PyTorch and FP32 ONNX on identical tensors. Separately reproduce the published BF16 path to quantify precision differences. Resizing continuous logits before thresholding, soft masks and alternate tiling are potential improvements, not the identical reference algorithm; version and evaluate them separately.

### 10. COO also has a practical export target

Koharu’s inspected MTSv3 adapter runs **ResNet-50 + feature pyramid + segmentation-proposal head** and returns a dense probability map. Contours, scores, expansion and page-coordinate polygons are computed separately. It does not run the full text-recognition pipeline. This is a narrower export target than the complete upstream research application. [N16-N18]

```text
COO MTSv3 detection-only ONNX
  image tensor -> backbone/FPN -> proposal probability map
Rust postprocessing
  probability map -> candidate contours/polygons/scores
Koharu SAM-TS-L
  selected lettering pixels -> actual removal mask
```

Use mayocream/coo-comic-onomatopoeia-safetensors, file mtsv3/model.safetensors, revision b5d31460573b6f61c1d4bdaea5fe4e18425e6a61 as the initial implementation reference. Check consumed tensor names/shapes/values; explicitly account for unused recognition-related weights rather than ignoring arbitrary loading errors. [N16, N19]

Export a matching Python tensor-only proposal network, with learned weights mapped explicitly. Keep polygon extraction outside ONNX. Preserve this branch’s own BGR/mean-subtraction and resize/padding conventions; do not share SAM-TS preprocessing. Verify maps and polygons independently. [N17-N18]

**Important:** a proposal probability map or expanded SFX polygon is discovery evidence, not automatically a glyph-accurate removal mask. Nor should upstream full-pipeline benchmark scores be claimed for this subset without reproducing the evaluation.

### 11. Runtime direction

Ogkalu already publishes ONNX artifacts, and Manga Cleaner already uses Rust ort. Target that runtime for the two additions. Python is permitted for conversion/testing; it is not the intended end-user inference dependency. Avoid simultaneous loading of every large model by default. [N13, N15]


## Mask behavior and implementation order

### 12. New-mode mask contract

```text
Page -> RT-DETR / optional COO discovery
Page or tiles -> Koharu SAM-TS-L text evidence
  -> associate fragments with stable editable region IDs
  -> validated base lettering mask + manual corrections
  -> adjustable source-pixel padding
  -> prepared preview with mask hash/revision
  -> existing fill/inpainting engine
  -> compositor limited to that same prepared footprint
```

Keep processing crops, detection bounds, base masks and final write support separate. Rectangles may still be used for indexing, storage and interaction handles. They must not determine removal membership in the new mode.

Keep disconnected SFX marks, punctuation and relevant outlines/shadows. Do not fill a convex hull between fragments. Preserve holes unless the selected, previewed morphology changes them. Do not use broad dilation to make up for missing characters.

Padding changes rebuild from the immutable base and corrections. A larger inpainting context changes what the engine can inspect, not what it can overwrite. Any feathered support or engine-specific expansion must already be included in the preview.

If a mask is empty, implausibly broad or uncertain, return a correction/review state. Do not silently fall back to a filled rectangle or a different cleaning mode. A confirmed text region with a weak mask may need a local higher-resolution pass or manual correction.

### 13. Build sequence for the next agent

**A — Pin and reproduce.** Record the actual app commit. Pin the reference architectures and weights. Reproduce Koharu’s full-page PyTorch output and save prepared tensors, embeddings, logits and restored masks.

**B — Prove ONNX parity.** Export the encoder and text head separately, validate each graph and compare the complete reconstructed mask. Investigate specific failing operators rather than declaring the model generally non-exportable.

**C — Integrate the optional mask mode.** Add the model behind a segmenter interface and memory/session policy. Implement preview/apply identity, base-mask padding, bounded compositing and persistence without changing legacy defaults.

**D — Add COO independently.** Export its proposal network and port/verify postprocessing. Compare RT-DETR + SAM-TS against RT-DETR + COO + SAM-TS. Keep COO only if additional recall justifies false positives and runtime cost.

**E — Optimize and qualify.** Test full-page plus overlapping crops, then supported FP16/mixed precision. Defer INT8 until thin strokes and punctuation survive quality tests. Evaluate every supported device profile before making speed claims.

A permanent alternate runtime, custom native ONNX operator, replacement checkpoint or automatic fallback needs a recorded decision after a concrete blocker. None is pre-approved by this addendum.


## Acceptance gates and unresolved choices

### 14. Required checks before shipping

| Gate | Required evidence |
| --- | --- |
| Artifact identity | Exact model/repository revision, weight hashes, architecture revision, consumed keys and exporter environment. Missing weights fail visibly. |
| Numerical parity | Same-precision prepared inputs, embeddings and high-resolution logits; final-mask overlap and per-region disagreement. Freeze tolerances before release. |
| Mask quality | Independently reviewed lettering masks. Measure remaining text, erased non-text, boundary errors and manual correction effort, not overall background accuracy. |
| Representative pages | Japanese SFX and dialogue; Korean/color webtoons; outlines/glows; reversed text; tiny punctuation; text crossing art; long-strip joins; no-text negatives. |
| Exact write limit | Decoded output pixels outside the approved support equal the input/lower-layer composite. Verify fill, inpainting, preview, rerun and lossless export. |
| State compatibility | Legacy snapshots unchanged; old projects load as legacy; padding is idempotent; save/load, undo/redo and cancellation preserve the exact plan. |
| Deployment | CPU FP32 plus each intended accelerator/provider. Record peak memory, warm/cold latency, device transfers, graph partitioning and fallback behavior. |
| Permissions | Review the exact model, copied implementation and redistribution terms separately. ONNX conversion does not grant new rights. [N19-N20] |

### 15. Open decisions; do not invent settled defaults

Final UI names; padding default/range; production mask threshold; crop size/overlap; grouping and glow handling; numerical tolerance; per-device budgets; optional model download size limits; final COO go/no-go; custom-op or alternate-runtime fallback. The architecture target is chosen for implementation, not certified as the universal best combination.

Initial engineering choices in this document are batch-1/static-canvas export, an FP32 parity baseline, and a split SAM-TS graph. They may change with evidence while preserving the user-facing contract. Provider success is not implied by a valid ONNX file. [N10-N12]

### 16. Do not regress to these earlier assumptions

CTD and ogkalu are not the same model. A missing ONNX download does not mean no ONNX route. Generic SAM output does not reproduce Koharu SAM-TS-L. A detector polygon is not an erase mask. One comparison screenshot does not establish corpus superiority. Current Koharu main is not a pinned reproduction of the older stack. [N01, N14, N21-N22]

**Done means:** the optional mode removes more target lettering with acceptable artwork damage, its runtime is validated on intended devices, and every committed edit remains inside the displayed mask. Export success alone is not completion.


## Primary sources and provenance

Research checked on 23 September 2026. N02, N03 and N22 are linked to the observed immutable Koharu release. Other main-branch code links are research references, not deployment pins; the implementing agent must capture their actual commits before conversion. Source facts support the design assessment but do not establish a successful export.

**[N01] [Koharu SAM-TS-L model card and full-weight release](https://huggingface.co/mayocream/koharu-text-sam-ts-l).** Model identity, merged weights, capabilities and limitations.

**[N02] [Koharu SAM-TS-L architecture and preprocessing configuration](https://huggingface.co/mayocream/koharu-text-sam-ts-l/blob/5dd97423e0fbf2404264979136d47e8101144046/config.json).** Pinned architecture, normalization, 1024 input and weight checksum.

**[N03] [Koharu SAM-TS-L reference inference](https://huggingface.co/mayocream/koharu-text-sam-ts-l/blob/5dd97423e0fbf2404264979136d47e8101144046/inference.py).** Gray padding, PIL resize, threshold/crop/nearest output order and CUDA BF16.

**[N04] [Hi-SAM model assembly and forward path](https://raw.githubusercontent.com/ymy-k/Hi-SAM/main/hi_sam/modeling/hi_sam.py).** Image encoder, modal aligner, dense positional encoding and mask decoder.

**[N05] [Hi-SAM image encoder](https://raw.githubusercontent.com/ymy-k/Hi-SAM/main/hi_sam/modeling/image_encoder.py).** Encoder adapters, attention, window operations and checkpoint wrappers.

**[N06] [Hi-SAM modal aligner](https://raw.githubusercontent.com/ymy-k/Hi-SAM/main/hi_sam/modeling/modal_aligner.py).** Learned visual prompts and attention; no OCR input required.

**[N07] [Hi-SAM mask decoder](https://raw.githubusercontent.com/ymy-k/Hi-SAM/main/hi_sam/modeling/mask_decoder.py).** Separate high-resolution text-mask head.

**[N08] [Meta SAM ONNX exporter](https://raw.githubusercontent.com/facebookresearch/segment-anything/main/scripts/export_onnx_model.py).** Stock export is prompt encoder plus decoder, not this complete custom model.

**[N09] [PyTorch ONNX export documentation](https://docs.pytorch.org/docs/2.14/onnx.html).** Export reports, verification, custom translations and external weights.

**[N10] [ONNX Runtime execution providers](https://onnxruntime.ai/docs/execution-providers/).** Execution is provider-dependent; model validation is still required.

**[N11] [ONNX Runtime compatibility](https://onnxruntime.ai/docs/reference/compatibility.html).** Runtime, ONNX IR and opset compatibility are separate checks.

**[N12] [ONNX Runtime float16 and mixed precision](https://onnxruntime.ai/docs/performance/model-optimizations/float16.html).** Reduced-precision deployment is a separate optimization experiment.


## Primary sources and provenance

**[N13] [Ogkalu RT-DETR-v2 published artifacts](https://huggingface.co/ogkalu/comic-text-and-bubble-detector/tree/main).** Existing ONNX detector artifacts.

**[N14] [Manga Cleaner bubble detector adapter](https://raw.githubusercontent.com/k-omiq/manga-cleaner/main/crates/cleaner-core/src/balloon.rs).** Distinguishes CTD from ogkalu, its labels, ONNX adapter and export conventions.

**[N15] [Manga Cleaner ONNX Runtime dependency](https://raw.githubusercontent.com/k-omiq/manga-cleaner/main/crates/cleaner-core/Cargo.toml).** Existing Rust ort integration and configured providers.

**[N16] [Koharu COO detector and checkpoint selection](https://raw.githubusercontent.com/koharu-rs/koharu/main/crates/koharu-ml/src/comic_onomatopoeia/detector/mod.rs).** Pins MTSv3 weights from the mayocream conversion repository.

**[N17] [Koharu detection-only MTSv3 neural network](https://raw.githubusercontent.com/koharu-rs/koharu/main/crates/koharu-ml/src/comic_onomatopoeia/detector/model.rs).** ResNet-50/FPN plus segmentation-proposal head; no recognition forward path.

**[N18] [Koharu COO preprocessing and polygon postprocessing](https://raw.githubusercontent.com/koharu-rs/koharu/main/crates/koharu-ml/src/comic_onomatopoeia/detector/processor.rs).** BGR mean subtraction, size mapping and native contour/polygon work.

**[N19] [COO SafeTensors conversion card and permissions notes](https://huggingface.co/mayocream/coo-comic-onomatopoeia-safetensors).** Converted tensors require matching architecture; no new rights are granted.

**[N20] [Upstream COO MTSv3 documentation](https://raw.githubusercontent.com/ku21fan/COO-Comic-Onomatopoeia/main/MTSv3/README.md).** Original research implementation and component-license reference.

**[N21] [Current Koharu pipeline configuration](https://raw.githubusercontent.com/koharu-rs/koharu/main/crates/koharu-pipeline/src/config.rs).** Current RF-DETR selection is not the RT-DETR/SAM-TS target discussed here.

**[N22] [Koharu SAM-TS-L file listing](https://huggingface.co/mayocream/koharu-text-sam-ts-l/tree/5dd97423e0fbf2404264979136d47e8101144046).** Inspected published release contains SafeTensors, not an ONNX export.

### Conversation evidence and artifact scope

The requirements and decision history come from this conversation and the earlier Documents 01-08, including the locally reviewed DOCX decision register. The user’s four-column SFX comparison motivated the model-priority change. It is illustrative evidence, not a reproducible benchmark or a pixel-accurate reference mask.

This package contains documentation only. No model weights, font files, generated ONNX graphs, verified export script or modified application source is included. The original eight documents remain unchanged; this addendum takes precedence only where it explicitly overrides them.
