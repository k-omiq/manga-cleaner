# Agent start here — Manga Cleaner / Addendum 09

Read `09_Post_PDF_Decisions_and_ONNX_Export_Plan.md` first, then the original Documents 01–08 for unchanged requirements. This is the current implementation target, not an assertion that exports or benchmarks are complete.

## Decision

Add a separate **text-shaped + padding** cleaning mode. Do not replace the legacy mode. Target **ONNX Runtime via the app’s Rust `ort` integration**, with build-time Python conversion rather than a mandatory Python inference service.

1. **Ordinary discovery:** `ogkalu/comic-text-and-bubble-detector` (RT-DETR-v2; not CTD).
2. **Additional SFX discovery:** COO **MTSv3 detection-only proposal network**, using the Koharu-compatible implementation as the initial reference. It is not a text recognizer. Keep it optional and independently validated.
3. **Removal mask:** `mayocream/koharu-text-sam-ts-l`, full merged weights, high-resolution text-mask head. This specific manga-fine-tuned model supersedes the earlier CTD-first mask plan.
4. **CTD:** preserve in legacy mode and as an evaluation baseline. Do not require CTD agreement or combine its broad masks blindly with the new mask.
5. **No OCR:** all-text cleaning must not depend on recognition, translation, Japanese identification or recognized content. Keep outside-bubble permission explicit.
6. **Exact footprint:** base lettering mask -> adjustable original-image-pixel padding -> prepared preview -> edits restricted to that same support. No hidden margins or fallback rectangles. Rebuild padding from the base, never the previous dilation.
7. **Coverage:** retain plausible unassociated mask components for review; do not automatically erase or discard every component outside detector boxes.
8. **Compatibility:** preserve old defaults/projects, source pixels, saved layers, undo/redo, reruns and bounded lossless export.

## First implementation work

Reproduce the checkpoint’s reference output. Then export SAM-TS as an adapted encoder graph plus a modal-aligner/high-resolution text-head graph, initially batch 1/static 1024/FP32. Retain the trained adapters. A generic SAM decoder export is not equivalent.

Match gray-128 padding and the original threshold/crop/nearest-resize pipeline before experimenting with alternate postprocessing. Compare FP32 PyTorch to FP32 ONNX; measure BF16/FP16 differences separately. Keep COO contour extraction in Rust and export only its neural proposal map. Confirm runtime/provider support with measurements.

## Overrides and boundaries

The earlier DBNet++-first and CTD-refinement-first recommendations are superseded. Current Koharu RF-DETR is not an approved silent substitute. No successful ONNX export, universal performance win, numeric padding default, custom-op dependency or permanent alternate runtime has been established. Exact primary sources, revision/checksum information, acceptance gates and open choices are in Document 09.
