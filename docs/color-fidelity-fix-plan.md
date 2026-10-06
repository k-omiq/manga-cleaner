> Branch correction: implementation is now in `codex/cloud-integration`.
> Evidence below predating the target-branch section was collected on main and is historical only.

# Plan to close the color-fidelity gaps

Prepared 2026-10-03 from the current source and
[color-fidelity audit](color-fidelity-audit.md). Batches A–I are implemented;
the evidence and release-gate results below record the supported behavior and
explicit limits. The previous navigation, reading-direction and native-JPEG
import changes remain the baseline.

## Outcome and preservation rules

The finished app must preserve both what the source contains and what its colors
mean. Tests must distinguish four promises:

1. **Original files:** native PNG, TIFF and JPEG imports remain byte-for-byte
   copies. Converted inputs also get a chapter-owned copy of their original file.
2. **Original samples:** a lossless edited export preserves every decoded channel
   value outside the applied masks, including alpha, palette indices and 16-bit
   low bytes. An orientation transform may move samples, but never change them.
3. **Color description:** compatible profiles and color metadata survive
   re-encoding. A necessary change is planned and reported; a profile must never
   describe a different sample space from the actual pixels.
4. **Appearance:** supported source profiles drive preview and paint conversion.
   Preview buffers are display derivatives; exports use the native source and
   patches. Appearance comparisons use a reference transform and a stated
   tolerance, not byte equality between different color spaces.

Keep JPEG imports as JPEG. Keep untouched `SameAsSource` JPEG exports as the
original bytes. Edited RGB/grayscale JPEG exports remain lossless PNG; edited
CMYK sources use a lossless format that actually supports CMYK. Do not introduce
JPEG recompression in this work.

An untagged source has no recoverable, definitive original profile. Define and
report a display assumption without adding a guessed profile to its stored
original. Invalid profiles and unsupported color encodings must have specific
results, rather than silently entering an uncalibrated conversion.

## Implementation sequence

Each row is a separately reviewable change. “Depends on” identifies shared
contracts that must land first; this is not permission to ship a temporary guard
as the completed color-management implementation.

| Batch | Priority | Deliverable | Depends on |
| --- | --- | --- | --- |
| A | P0 | Reference fixtures, fidelity assertions, unsafe-profile guard | — |
| B | P0 | Shared color metadata and PNG writer policy | A |
| C | P0 | Export preflight and visible actual-format/metadata results | B |
| D | P1 | Correct native CMYK/YCCK JPEG decoding | A, B, C |
| E | P1 | Shared managed preview pipeline and transparency fixes | B, D |
| F | P1 | Managed brush/fill/model boundaries and matching live preview | C, E |
| G | P1 | Original archives and explicit conversion provenance | B, C |
| H | P1 | Orientation across display, edits, strips and export | B, G |
| I | Release gate | Reopen/export matrix and installed-app verification | A–H |

Deliver A–C first because they stop misleading output and metadata loss. D–F
then make decoded colors, displayed colors and newly painted colors agree.
G–H close the remaining structural and orientation losses. I verifies the whole
path rather than treating successful unit round trips as sufficient.

## A. Establish trustworthy evidence and block profile/sample mismatches

**Files:** `image/fixtures.rs`, `image/foreign.rs`, `image/mod.rs`,
`image/tiff_io.rs`, existing export tests, and a new reference-fixture directory.

- Replace the CMYK fixture's RGB sRGB profile with a genuine, redistributable CMYK
  profile. Preserve the existing malformed combination as a negative fixture.
- Add small independently generated sources: tagged grayscale; sRGB and
  non-sRGB RGB; 16-bit low-byte gradients; indexed transparency; grayscale/RGB
  color-key transparency; CMYK and YCCK JPEG with and without Adobe markers;
  orientation 1–8; animated GIF/WebP; gamma/chromaticity-only PNG; modern PNG
  color chunks; conflicting and malformed profiles.
- Record fixture licenses, hashes, generator versions and expected sample/color
  results. Generate JPEG references with an independent decoder, not only the
  same `image` decoder used by the application. Check in compact expected data
  so normal tests need no extra system tools.
- Parse ICC validity and input color-space signatures separately from carrying
  opaque bytes. Match Gray profiles to grayscale samples, RGB profiles to RGB
  samples/palette colors, and CMYK profiles to CMYK samples. Alpha is independent.
- Immediately reject the unsafe processing path that produces RGB pixels with
  a CMYK profile. Keep native import and untouched byte passthrough available;
  show an actionable processing reason until batch D supplies the correct path.
- Add reusable assertions for source hashes, mode/depth, exact native samples,
  palette/transparency, metadata, profile validity and changed-pixel containment.
  Compare logical packed samples as well as decoded buffers so row padding does
  not become a misleading color test.

**Acceptance:** negative fixtures cannot reach an encoder with a mismatched
profile; reference CMYK fixtures are genuinely CMYK; tests distinguish file,
sample, metadata and appearance preservation.

## B. Preserve color metadata through every lossless writer

**Files:** `image/mod.rs`, `image/png_io.rs`, `image/tiff_io.rs`,
`export/rows.rs`, `export/stitched.rs`, `export/psd.rs`, `project/buffers.rs`,
and raster constructors/crop helpers.

- Introduce a shared color-description object used by `Raster`, `Header` and
  streaming `Canvas`. Keep palette/transparency as sample interpretation data.
  Carry actual `gAMA`, `cHRM`, `sRGB`, ICC, `sBIT`, `cICP`, `mDCV` and `cLLI`
  declarations, retaining integer precision and whether each chunk was present.
  Do not reconstruct “original” chunks from decoder-derived defaults.
- Add one resolver for source interpretation and one writer policy for metadata
  that remains valid after crop, compositing, sample-depth change or color-space
  conversion. Propagate the description through all constructors, row buffers,
  split exports and reopen paths. Source metadata remains authoritative when
  loading raw patch buffers; do not give patches a competing page profile.
- Share PNG metadata emission between whole-image `png_io` and streaming `rows`.
  Test actual encoded chunks, not only the application's own decoder output.
- Handle conflicting `iCCP`/`sRGB` input deliberately. Original byte passthrough
  remains exact. For re-encoded SDR output, use a valid compatible ICC profile
  as the authoritative description and report removal of a conflicting sRGB
  declaration. Preserve compatible modern declarations; do not strip a valid
  `cICP` merely because ICC is present. Update the current tests that equate
  emitting both legacy profile declarations with correctness.
- Preserve valid significant-bit information only while it still describes the
  output. After an edit introduces extra precision, update or remove stale
  `sBIT` with a declaration. Likewise, do not blindly retain image-dependent
  luminance summaries or other metadata invalidated by edits.
- For TIFF/PSD targets that cannot directly carry a source PNG description,
  generate an equivalent supported ICC description where possible, preserving
  samples; otherwise expose a specific incompatibility. Do not silently tag
  gamma-only or wide-gamut data as sRGB.
- Strengthen stitched-export checks. They currently compare mode, depth and
  ICC, then take the first page's sRGB intent. Compare the complete interpretation
  of all pages, including gamma/chromaticities and transparency. Refuse an
  incompatible stitch with a page-specific reason and offer per-page export;
  a single strip cannot accurately label unlike page spaces with one profile.
- Classify ancillary metadata by preservation rules. Retain compatible known
  fields and safe-to-copy unknown ancillary chunks; do not copy unknown critical
  chunks or stale unsafe-to-copy chunks after changing image data.

The metadata precedence, profile compatibility, transparency and chunk-copy
rules should follow the [W3C PNG specification](https://www.w3.org/TR/png-3/).
In particular, color-key transparency must be evaluated at full source precision
before reducing samples to preview depth.

**Acceptance:** buffered and streaming exports preserve the same valid metadata;
gamma-only PNGs keep their interpretation; no invalid profile/sample pairing is
emitted; mixed-description stitches are detected before writing output.

## C. Make format and color changes visible and predictable

**Files:** `export/mod.rs`, `export/stitched.rs`, `src-tauri/src/exporting.rs`,
`src/lib/api/{backend,tauri,mock}.js`, `ExportDialog.svelte`, i18n and seam tests.

- Expand typed export declarations to include metadata normalization, assumed
  interpretation, actual format changes and unsupported transforms. Include page
  identity and relevant requested/used values; localize messages at the seam.
- Add a read-only export planning command. Use the same decision object for
  planned filenames, output extensions and execution; eliminate separate copies
  of fallback rules. Tie the plan to the chapter/source/patch revision and
  invalidate it if edits occur before execution.
- Show material changes in the export dialog before export: for example,
  “2 edited JPEG pages will be PNG” or “CMYK pages will be TIFF.” Refusals name
  the affected pages and supported output choice. Routine matching exports need
  no extra confirmation step.
- Preserve `ExportSummary.declared` through per-page, CBZ, stitched and PSD
  paths. Extend `ExportResult` with actual output formats and aggregated
  declarations. Success notices must describe the files written, rather than
  repeat the requested format when a fallback happened.
- Stage outputs using the existing atomic-write approach. A validation failure
  must not publish a chapter that appears complete; runtime write failures must
  report the actual completed files and failed page.

**Acceptance:** planning and actual formats/extensions agree; every fallback
reaches the frontend; mocked and Tauri backends share the same result shape;
mixed JPEG/PNG CBZ exports report their real contents.

## D. Decode CMYK/YCCK JPEG without losing its source space

**Files:** `image/foreign.rs`, `image/mod.rs`, core codec dependencies, ingest,
tile and export tests.

- Begin with a bounded codec experiment: determine whether the installed
  `zune-jpeg` path can return original CMYK and YCCK components. The `image`
  wrapper's advertised RGB result is not evidence of a calibrated conversion.
- Implement CMYK native decoding with explicit Adobe inversion handling.
  For YCCK, implement and verify the component conversion needed to obtain CMYK;
  do not assume requesting CMYK output from the codec supports YCCK correctly.
  If the experiment fails independent references, select a codec that exposes
  those components behind the same core interface before proceeding.
- Make JPEG header and pixel decoding agree on resulting mode, dimensions,
  depth and profile. Keep ICC bytes with CMYK samples; normalize decoder-specific
  conventions at this boundary so later code sees one documented CMYK layout.
- Use native CMYK TIFF for edited exports that cannot be represented as PNG.
  A requested RGB conversion must be an explicit managed conversion with an RGB
  output profile, never a relabeling of CMYK bytes.
- Handle untagged CMYK by preserving native numbers and offering a documented
  preview assumption. Do not claim that a guessed profile reconstructs the
  scanner's original appearance.

**Acceptance:** Adobe CMYK, non-Adobe CMYK and YCCK fixtures agree with independent
component references under documented JPEG-decoder tolerances; untouched JPEG
bytes remain exact; edited lossless output equals this decoder's native samples
outside masks; no CMYK ICC reaches an RGB output.

## E. Use one managed preview pipeline and fix transparency

**Files:** new `image/color.rs` (or equivalent shared module), `image/proxy.rs`,
`image/convert.rs`, `src-tauri/src/tile.rs`, tile caches and tests.

- Use the already available `lcms2` dependency for supported Gray/RGB/CMYK ICC
  transforms. Resolve SDR gamma/chromaticity and supported `cICP` descriptions
  through the same interface. Preserve unsupported HDR declarations in native
  output; require a tested HDR-to-SDR policy before offering a managed HDR preview
  or paint operation. Do not treat HDR samples as ordinary sRGB.
- Define defaults: untagged RGB displays under an sRGB assumption; untagged
  grayscale uses a documented grayscale transfer assumption. Untagged CMYK
  requires a named preview assumption. Keep these display choices separate from
  original file metadata. Use a fixed default rendering intent, with supported
  embedded intent respected where applicable.
- Convert display derivatives to explicitly tagged sRGB. Both original and
  cleaned views, whole-page tiles and reduced proxies must use this policy; the
  source-PNG shortcut must not bypass required interpretation. Do not attach the
  source profile again after converting to display RGB.
- Apply grayscale/RGB `tRNS` comparisons to native-depth samples, then generate
  preview alpha. Include 16-bit values that have the same high byte but differ
  in low bytes, and palette entries with partial transparency.
- Downsample in a defined linear-light, premultiplied-alpha working space. Avoid
  colored transparent fringes and averaging nonlinear encoded channels. Quantize
  only when creating the final 8-bit preview.
- Cache transforms by source description/profile hash, mode/depth, destination,
  rendering intent and pipeline version. Invalidate tile caches for source,
  patch, orientation or color-policy changes. Bound caches and temporary buffers;
  process tall pages by windows instead of loading whole chapters.

**Acceptance:** known non-sRGB and CMYK patches match reference sRGB transforms
within fixed fixture tolerances; all transparency modes render correctly;
changing zoom/tile path does not change the interpretation; previews never
replace native source or export buffers.

## F. Make selected colors, committed edits and live previews agree

**Files:** `paint/mod.rs`, `engines/fill.rs`, `engines/model.rs`, related engine
adapters, `src-tauri/src/region.rs`, `PaintLayer.svelte`, paint API and tests.

- Treat UI hex colors as sRGB. Transform selected brush/solid-fill colors through
  the page's resolved description. Blend coverage in a defined linear-light
  working space, then transform edited pixels back to the native page space.
  Grayscale uses its own profile rather than encoded RGB luminance coefficients.
- Start with the currently supported 8-bit Gray/GrayAlpha/RGB/RGBA paint modes.
  Keep existing unsupported-mode refusals accurate; widening paint support to
  indexed, bitonal, 16-bit or CMYK is a separate feature, not a hidden conversion
  to RGB8. A supported color description is also required for managed painting.
- Copy untouched pixels exactly. For fractional coverage, transform only the
  pixels the operation actually modifies. Preserve the existing alpha contract;
  do not accidentally paint into transparent pixels or turn alpha into opacity.
- Clone native source values when no blend is needed; manage fractional blends
  and healing in the defined working space. Preserve native values outside the
  operation's exact coverage, including pixels inside a rectangular patch bound
  but outside its mask.
- Audit solid fill and model inputs/outputs alongside brush paint. Models need
  an explicit working RGB interpretation and a conversion back for the patch;
  source-derived fills should retain native values when possible. Do not route
  export through model/display RGB buffers. Characterize detector/measurement
  behavior before changing those inputs, since color conversion can alter
  thresholds and masks.
- Fix selected-color fill for its existing wider set of modes: perform 16-bit
  transforms before quantization; convert selected colors to CMYK using the
  actual source profile; choose indexed palette entries by managed appearance
  rather than raw sRGB channel distance. Report palette/bitonal approximation
  where the chosen color is unrepresentable. Preserve the existing refusal of
  model engines for indexed and CMYK sources.
- Replace color-critical Canvas2D/proxy sampling with a bounded backend preview
  of the stroke's affected window, using the same renderer as commit. Coalesce
  updates, cancel superseded work and key results by stroke/page revision. Keep
  the cursor immediate; show pixels when the authoritative preview is ready.
  Clone/heal preview must use native data, not just displaced downsampled tiles.
- Hold the final authoritative preview until its matching committed tile arrives,
  and reject late frames from earlier strokes or pages. Do not use a timed stale
  overlay as evidence that preview and commit match.

**Acceptance:** known sRGB brush colors have the expected managed appearance on
non-sRGB/gray sources; opaque native clone samples remain exact; edge blending
matches references; masks/alpha/outside pixels stay correct; mounted tests cover
cancellation, page changes and stale preview responses.

## G. Retain originals and explain conversions

**Files:** `ingest.rs`, `image/foreign.rs`, `project/mod.rs`, library/chapter
creation APIs and UI.

- Keep native files in `pages/` unchanged. For WebP/GIF/BMP conversion, store the
  working PNG in `pages/` and original bytes in the same chapter's `originals/`
  directory. Record both relative paths and hashes, detected format, chosen
  frame, frame count where available, conversion version and retained/lost fields.
- Validate the input and conversion before publishing manifest references. Use
  atomic writes for both assets; clean unreferenced staged files on failure.
  Collision naming and duplicate handling must be deterministic for the pair.
- Detect animation explicitly. Keep the first-frame page policy for this image
  editor, but report it during import and retain all original animation bytes.
  Test disposal/composition semantics so “first frame” means the correct visible
  canvas, not an incorrectly positioned partial image.
- Where a static GIF can be represented directly as indexed PNG, retain its
  palette, indices and transparency rather than expand unnecessarily. Otherwise
  preserve decoded samples/appearance and record the structural conversion.
- Carry valid EXIF/ancillary metadata where the destination can represent it;
  reconcile orientation, dimensions and thumbnails after edits rather than
  blindly copying stale tags. Original archives preserve the source information
  that the working PNG cannot represent.
- Add provenance fields with compatible defaults. Older converted chapters remain
  readable and are marked as lacking an archived original. Recover an original
  only if the user still has it and a verifiable match exists; never fabricate
  the original JPEG from an old converted PNG.

**Acceptance:** deleting/moving the scan folder leaves working pages, original
archives and provenance usable; conversion failures do not leave referenced
half-files; animated input is reported; original archive hashes match the input.

## H. Apply orientation without moving edits to the wrong pixels

**Files:** header/foreign/TIFF metadata readers, shared geometry helpers,
`strip`, compositor/patch mapping, project/history persistence, Tauri page/region
mapping, tile rendering and editor gesture mapping.

- Introduce one tested orientation transform for all eight EXIF values, with
  native-to-display and inverse mappings for points, rectangles, masks and raster
  rows. Define whether API dimensions are native or displayed at every boundary.
- Keep imported source bytes untouched. Use an oriented working view for display,
  detection and user gestures; persisted native sample coordinates remain the
  authority. Map edits back before storage/compositing. For lossless PNG export,
  orient the composited samples once and normalize any retained orientation tag;
  untouched JPEG passthrough keeps its original orientation tag and bytes.
- Split legacy global-strip patches into their per-page native intersections
  before applying page orientation. Build visible strip geometry from oriented
  page dimensions. Map new seam-spanning operations back to each contributing
  native page, preserving coverage, order and undo grouping.
- Prove that the native-coordinate adapter can represent existing masks, clone
  anchors, reorder operations and journal/history entries. If new per-page patch
  grouping requires a schema change, introduce a new readable version with an
  explicit legacy adapter; never silently reinterpret existing raw-strip records
  as oriented-strip coordinates. Retain old source and patch buffers unchanged.
- Make the compatibility checkpoint precede UI enablement. Test reopen, undo,
  redo, crash recovery, source/cleaned comparison and edited export on asymmetric
  fixtures, including a 90-degree page beside an unrotated page in a long strip.

**Acceptance:** the displayed page, pointer location, selected mask, clone source
and exported result identify the same source pixels for orientations 1–8; legacy
edits remain aligned; inverse-oriented exports satisfy native sample assertions;
no JPEG-to-PNG import rewrite is used to solve orientation.

## I. Verify the complete workflow and update claims

Run a matrix across PNG, TIFF, RGB/gray/CMYK/YCCK JPEG and converted WebP/GIF/BMP;
tagged/untagged inputs; supported packed/8/16-bit modes; palette, alpha and color
keys; orientation; compatible and incompatible stitched pages.

For each meaningful combination:

1. Import and verify owned original/working hashes and chapter-relative paths.
2. Delete the external scan folder, reopen, render original and cleaned views.
3. Apply an edit; save, reopen, undo/redo and recover the journal.
4. Export using applicable per-page, CBZ, stitched and PSD paths; verify actual
   filenames, format declarations, native samples, masks and color description.
5. Compare independently decoded files and managed preview patches to references.
   Require zero native-sample differences outside masks; define separate numerical
   tolerances for JPEG decoding, ICC transforms and 8-bit preview quantization.
   Freeze those thresholds in batch A: for controlled in-gamut SDR fixtures,
   target at most one 8-bit code value per display channel against the reference
   pipeline. Use separate documented component tolerances for independent JPEG
   decoders; quantization bounds must never excuse a swapped channel, inversion,
   wrong profile or missing transparency. Test gamut clipping separately.
6. Exercise large/tall pages to check bounded memory, cache invalidation and
   cancellation. Do not introduce full-chapter decoding into streaming paths.

Run relevant core/Tauri suites, `npm test`, `npm run build`, and diff checks after
each batch; run the complete applicable suites at the release gate. Report skipped
model/platform checks explicitly. Verify the installed webview on the affected
chapter: repeated/held arrows past page 2 and back, both RTL/LTR and both page
modes, plus actual color/paint previews. DOM tests alone do not establish that
the originally reported installed-app navigation failure is resolved.

Update `docs/features.md`, the audit and user-facing import/export text to match
the final behavior. The feature document currently still claims JPEG imports
become PNG and describes a broader metadata guarantee than the audit supports.

## Definition of done

- All six audit gaps have an implementation, regression evidence and a documented
  policy for sources that lack a valid interpretation.
- Native originals are exact; supported converted inputs also retain exact
  originals inside the chapter. Existing jobs open without losing patches.
- Supported lossless exports retain native samples outside edits and valid color
  descriptions. Unsupported combinations produce a specific preflight result.
- Managed preview, brush/fill and model boundaries share the same interpretation;
  profiles never label the wrong channel space. Live paint previews use the same
  authoritative rendering path as saved strokes.
- Conversion/frame selection, metadata normalization and output-format changes
  reach the user; no successful export silently drops its declarations.
- Independent references, persistence/strip tests and installed-app checks pass.
  Passing ICC-byte round trips alone is not completion evidence.

## Implementation evidence — A profile guards / B metadata (2026-10-03)

- Added `image::metadata::ColorDescription` to raster/header/streaming canvas and
  propagated it through crops and row buffers. Raw patch files retain their old
  format; the source page remains the color authority when patches reopen.
- ICC header/length, parser validity and Gray/RGB/CMYK signatures are checked
  before buffered PNG/TIFF, streaming PNG/TIFF and PSD encode. Negative cases
  fail before writing sink bytes; native import and untouched passthrough remain
  available. The old ICC+sRGB test now requires authoritative ICC without sRGB.
- Both PNG writers use one chunk policy: exact integer gAMA/cHRM/sBIT/cICP/mDCV/
  cLLI, correct pre-PLTE ordering, and unknown safe ancillary placement preserved
  on its original side of IDAT. Edited composites drop stale sBIT/cLLI. Unsafe
  unknown ancillary data and unnormalized EXIF are not copied to edited files.
- TIFF/PSD use equivalent ICC profiles for declared SDR gamma/chromaticities and
  supported cICP. Unsupported transfer descriptions and PNG color-key transparency
  refuse these targets explicitly. Untagged sources are left untagged.
- Stitch headers now reject unlike gamma/chromaticities/cICP/mastering metadata,
  sRGB intent and transparency before decoding pages or writing output.
- Passed `cargo test -p cleaner-core image::metadata`: 4 tests. Raw encoded chunk
  assertions cover both writers, indexed ordering and post-IDAT safe chunks;
  malformed/wrong-space profiles leave sinks empty. Linear gamma → TIFF profile
  appearance is checked against the independent IEC sRGB formula within one
  8-bit value, alongside exact unchanged sample bytes.

## Implementation evidence — F authoritative preview / model boundaries (2026-10-03)

- LaMa and FLUX now receive managed encoded-sRGB model inputs; model replies
  blend in linear light and return to the native page interpretation at its own
  precision. Native crop metadata survives; alpha, transparent samples and
  uncovered samples are copied. Existing CMYK/indexed model refusals remain.
- Independent `gamma-chrm.png` samples verify model input conversion against the
  IEC sRGB equation within one code. Model-output tests check inverse conversion,
  linear-light half coverage (188/255), 16-bit output (14027/65535 within three
  native codes), alpha and exact uncovered samples.
- Live brush/clone/heal uses the same native `paint_patch` renderer as commit,
  followed by the shared managed display conversion. Preview work is bounded to
  8192 points, 16384 estimated dabs and a 4-megapixel native working window,
  including the clone source/target union; oversized previews leave the cursor.
  This bounds preview working/output allocations, not source-file decode size.
- Frontend coalesces pending frames, rejects canceled/stale stroke, chapter,
  page and revision results, flushes the final payload before commit, and holds
  final pixels until affected cleaned tiles actually load the new revision.
  There is no timeout handoff. Escape/page changes cannot commit a different
  draft while waiting. Failed/unavailable previews or commits clear the overlay.
- RGBA variants, overlapping proxy tiles and preview crops each composite onto
  the page's paper backdrop once, avoiding doubled alpha from layered images.
- Passed `cargo test -p cleaner-core engines::model --lib` (4),
  `engines::flux` (14, including independent reference and mock sidecar), and
  `engines::lama` (17, including available real-model inference, containment,
  alpha, 16-bit and determinism). Passed Tauri `paint_preview` budget test (1).
- Passed focused frontend queue/mounted-preview/DrawLayer/tile/Tauri API tests
  (51). Mounted preview tests cover first-frame rendering, canceled late frames,
  page changes, stale load events, actual committed tile handoff and failed
  commit cleanup. Installed-webview checks remain in release-gate evidence.

## Implementation evidence — E / F core and release matrix (2026-10-03)

- `ManagedColor` resolves native Gray/RGB/CMYK ICC and SDR PNG descriptions;
  the PNG 3 precedence is cICP, ICC, sRGB, then gamma/chromaticities. Unsupported
  HDR transforms explicitly refuse display/edit operations while native PNG
  declarations remain preservable. Untagged RGB/gray use the sRGB transfer;
  untagged CMYK uses the named multiplicative-ink preview assumption only.
- Both whole-page and tiled display use this resolver, emit explicit sRGB, and
  evaluate tRNS at native precision. Downsampling averages linear-light colors
  with premultiplied alpha. A 16-entry per-thread transform cache is keyed by
  interpretation, source profile, mode/depth, intent and policy version. Tile
  cache tokens include the color pipeline version.
- Brush and fractional clone/heal coverage operate in linear light, convert
  back only where covered, keep native alpha, and skip fully transparent pixels.
  Opaque clone copies native channels exactly. Heal's existing mode-specific
  mixing/3×3-neighborhood policy is evaluated in the managed working space.
- Selected fill transforms before 16-bit quantization, resolves palette choices
  by managed appearance, and requires an actual invertible CMYK profile. Palette
  and packed-gray approximation is identified for a localized notice. Native
  measured fills and detection/measurement thresholds stay in source space.
- `image::color` tests compare independent CMYK reference transforms and analytic
  linear-RGB/gray transfers within one 8-bit value; test cICP precedence, HDR
  refusal, full-depth transparency and cache bounds. Paint tests cover analytic
  linear blending (50% black/white → 188), selected color on linear RGB, and exact
  opaque native clone. Fill tests include independent Pillow/LCMS reverse CMYK
  values and 16-bit low-byte precision. Core suite excluding LaMa: 539 passed,
  one external PSD oracle ignored; the dedicated LaMa suite subsequently passed
  all 17 tests with the installed runtime/model.
- New `color_workflow` integration test imports the fixture matrix into owned
  chapter directories, verifies hashes/archives, removes external scans, reopens
  native patches, exports buffered and streaming lossless files, inverse-orients
  the result and requires exact logical samples outside masks. It passed across
  packed/8/16-bit, palette/alpha/color-key, CMYK/YCCK, native TIFF/PNG/JPEG and
  converted animation cases. Whole originals are checked independently of
  decoded samples. Final combined release checks are recorded below when done.

## Implementation evidence — C export planning / G owned conversions (2026-10-03)

- Core `PagePlan` is the format/extension decision used by prediction and writing.
  Tauri preflight runs the exact page/PSD/stitched encoder against a seekable
  discard sink, returns actual filenames/formats and localized declarations, and
  publishes nothing. PSD and stitched metadata declarations now reach the seam;
  multi-page stitching drops invalid first-page sBIT/cLLI summaries even without
  patches. PNG-only ancillary fields omitted by TIFF/PSD are explicitly declared.
- The reviewed revision covers export choices, serialized and on-disk manifest,
  source bytes and native patch/mask/ink buffers, including oriented native parts.
  A changed patch refuses execution before creating the destination. Outputs are
  staged and cleaned on failure; publication failures return the failed path and
  completed files rather than success. Duplicate output filenames (including
  mask sidecars) refuse during preflight before one page can overwrite another.
- Mixed CBZ tests assert a byte-identical untouched JPEG alongside an edited PNG,
  agreement of planned/written extensions, actual format aggregation and the
  edited page's fallback declaration. The mounted export dialog displays those
  decisions before export, sends the reviewed revision, rejects stale responses
  and displays validation failures.
- Converted GIF/WebP/BMP assets keep exact original bytes under the chapter's
  `originals/`, working PNGs under `pages/`, relative provenance paths, both
  SHA-256 hashes, format/frame/version and retained/lost fields. A failed second
  asset write removes the unreferenced archive. Animation notices state the first
  visible frame/count; old converted chapters report missing archives.
- Direct static GIF retains palette/indices/transparency. A positioned first-frame
  test verifies correct logical canvas/transparent surroundings before disposal;
  two-frame independent GIF/WebP references report two frames. WebP EXIF reaches
  the initial working PNG without altering native samples, and is recorded in
  retained fields. Native JPEG decoding deliberately omits EXIF from derivative
  rasters so oriented export cannot accidentally retain a stale rotation tag.
- Malformed-profile native JPEG import remains an exact owned copy and untouched
  passthrough remains possible; managed processing/re-encoding refuses it. Other
  unsupported decoder sample layouts now refuse instead of silently quantizing.
- Passed `cargo test -p manga-cleaner exporting::tests --lib`: 30 tests at the C
  checkpoint, plus the later oriented-export regression (PNG/CBZ/stitched and mask
  alignment, with raw PNG chunk assertions excluding stale EXIF).
- Passed `cargo test -p cleaner-core ingest::tests`: 24 tests at the G checkpoint;
  the added malformed-profile native JPEG test also passed independently.
- Passed `cargo test -p cleaner-core import_info`: 3 tests; and
  `npm test -- src/lib/dialogs/ExportDialog.dom.test.js`: 3 mounted tests.
- The stricter metadata scanner now catches truncated chunk framing at header
  validation. The Tauri refusal-seam fixture was corrected to corrupt IDAT data
  while retaining complete framing, independently testing pixel-decode refusal;
  its focused test passed.
- Final focused rerun: Tauri export suite 31/31 passed, followed by the new
  filename-collision refusal test (1/1). Core export suite 36 passed/1 ignored
  (manual external PSD oracle writer), followed by the new raw-chunk stitched
  sBIT/cLLI regression (1/1). `git diff --check` passed at this checkpoint.
- Follow-up review closed cross-join preview drift: preview now reads the same
  oriented native strip window as commit, shares paint coordinate/clone-offset
  remapping, and waits for neighboring affected tiles as well as the anchor.
  The anchor sheet rises while previewing so adjacent stacking contexts do not
  hide the preview. Preview queue/mounted tests now pass 12; Tauri preview bounds
  and cross-join remapping tests pass 2. Color-key model output avoids accidental
  transparency by the shared one-native-code collision policy.

## Implementation evidence — H orientation and coordinate compatibility (2026-10-03)

- EXIF permutations 1–8 are shared exact sample/mask transforms, including packed
  samples and 16-bit low bytes. Native source dimensions and bytes stay stored;
  Tauri page geometry, source/cleaned tiles, survey/detection windows, brush/fill
  and clone inputs use oriented dimensions and native samples in display order.
- Manifest v4 reads older versions. Missing orientation is read from the original
  EXIF without changing that file. Legacy seam masks intersect their frozen
  native strip layout before orientation; storing the legacy layout prevents a
  later reorder from attaching the second half to a different source. Old mask,
  ink and pixel buffers remain unchanged.
- New edits on oriented chapters persist one undo identity with per-source native
  intersections. Native part files use content-addressed names and atomic writes
  before the manifest switch, so interrupted replacement leaves the previous
  manifest's buffers readable. Reorder and visibility changes act on the group.
- Region bboxes and rerun geometry use the display adapter. The persisted undo
  journal carries an explicit coordinate version; legacy percentage boxes are
  adapted on read while IDs, presence, cursor and provenance remain intact.
- Per-page/CBZ native compositing, oriented mask exports, and stitched page/header
  geometry are integrated through the same adapter (export batch evidence records
  format coverage). Untouched JPEG passthrough retains original bytes and EXIF.
- Passed `cargo test -p cleaner-core orientation --lib`: 9 tests, including eight
  independently generated EXIF JPEG fixtures, explicit asymmetric permutation
  expectations, every fixture mode/depth, legacy reopen and seam reorder, grouped
  undo and simulated interruption before manifest publication.
- Passed the two Tauri orientation regressions: every orientation's page size,
  source/cleaned managed tiles, clone anchor, exact native pixels outside masks,
  reopen/undo/redo and legacy journal adaptation; plus an oriented 90-degree page
  beside an unrotated page with a seam-spanning ellipse committed into both native
  sources.
- Reconstruction for rerunning one edit is bounded to 16,777,216 pixels. An edit
  scattered farther apart by reordering is refused with a specific explanation;
  its individual native parts remain usable for display, export and undo.
- Final transparency review: source-derived planar/median fill and denoise now
  retain transparent samples and guard opaque color-key collisions. Indexed
  medians with a different alpha retain the original index; no palette promotion
  or opacity change is introduced. Fill tests pass 15; denoise tests pass 10;
  model boundary tests pass 5. Preview also caps estimated stamp work at
  67,108,864 pixel visits before allocating/rendering.
- Final full frontend gate: 933 tests across 52 files pass; production build
  passes (333 modules). Logs: `/tmp/manga-frontend-final.log` and
  `/tmp/manga-build-final.log`. Heal neighborhood averages now weight linear
  color by source alpha; independent partial-alpha arithmetic and hidden-RGB
  invariance checks pass for Soft/Object heal. Complete paint suite: 38 pass.
- TIFF association follow-up: managed preview unassociates native premultiplied
  channels before ICC interpretation without changing native samples. Explicit
  associated-alpha edit refusal is shared via `color::validate_editing`; native
  TIFF export and original passthrough remain available. The native fitting
  pipeline does not yet support associated-alpha editing. Unsupported TIFF
  transfer/color declarations fail before cached display transforms can be used.
- Alpha-aware bilateral denoise excludes fully transparent hidden colors and
  proportionally weights partial alpha; focused kernel regression passes.
  Updated focused gates: color 9, paint 38, denoise 11; diff check clean.

## Follow-up evidence — TIFF association and legacy declarations (2026-10-03)

- Native TIFF decoding now records associated alpha without unpremultiplying or
  quantizing stored samples. The managed preview resolver unassociates only its
  display derivative. Both TIFF writers explicitly emit `ExtraSamples=1` for
  associated alpha and `2` for straight alpha; GrayAlpha 8/16-bit TIFF is supported
  via explicit extra samples, including the decoder's Multiband compatibility
  adapter. Buffered/streaming output retains exact native bytes and association.
- Associated native values cannot be losslessly relabeled as PNG/PSD straight
  alpha: PNG export plans transparently choose TIFF; direct PNG/PSD encoders
  refuse before emitting bytes. Stitched targets follow the same fallback and
  reject pages with different alpha association. Editing associated-alpha TIFF
  currently refuses through the shared editing/automatic-run policy; preview,
  original ownership and unchanged TIFF passthrough remain available.
- Non-ICC TIFF GrayResponseCurve/TransferFunction/WhitePoint/PrimaryChromaticities
  declarations are detected. Until a tested equivalent interpretation is
  implemented, managed preview/editing and re-encoding explicitly refuse these
  cases rather than interpreting them as untagged sRGB. Native imports and exact
  same-format passthrough remain available. Embedded compatible ICC is supported.
- Added independently generated tifffile 8/16-bit RGBA/GrayAlpha fixtures and
  native references; Pillow independently renders associated RGBA8. Generator,
  versions, CC0 provenance and hashes are checked in. Preview agrees within one
  8-bit code; native samples and actual TIFF `ExtraSamples` tags are exact.
- Passed `cargo test -p cleaner-core image::tiff_io --lib`: 3 tests covering six
  native alpha fixtures, both TIFF writers, PNG fallback/PSD refusal, independent
  rendered appearance and three unsupported legacy-color declarations. Passed
  `cargo test -p cleaner-core export:: --lib`: 37 passed, 1 external oracle ignored.


## Follow-up evidence — D JPEG references, safe EXIF and selected fills

- Adobe CMYK, conventional markerless CMYK and Adobe YCCK native components
  match independent libjpeg-turbo references within two 8-bit codes. Added
  subsampled YCCK with a separate five-code bound: IDCT/chroma upsampling
  rounding is amplified by the YCCK matrix. Native export comparisons remain
  exact; decoder tolerances never relax outside-mask sample equality.
- A deliberately markerless YCCK fixture pins the ambiguity policy. Without
  APP14 its intended encoding cannot reliably be distinguished from CMYK.
  Both libjpeg and production use conventional non-inverted CMYK; processing
  declarations disclose that assumption. This is not a claim that missing
  YCCK signaling can be recovered. Fixtures, generators, licenses and hashes
  are retained in `tests/fixtures/color-reference`.
- PNG/JPEG/TIFF descriptive EXIF ASCII fields (description, camera make/model,
  software, date, artist, copyright) are rebuilt with bounded valid offsets
  for PNG output. Re-encoding excludes stale orientation, image dimensions,
  thumbnails, nested opaque fields and color-space claims; export preflight
  reports normalization/omission. Originals retain complete metadata.
- Selected-color fills preserve metadata, native transparency and hidden
  samples. Indexed selection is cached by existing opacity and stays within
  that opacity's palette entries; an exact color at a different opacity no
  longer suppresses the approximation notice. Opaque color-key collisions
  choose an adjacent native sample without changing tRNS. Fill suite: 16 pass.
- Independent JPEG comparison and EXIF sanitization regressions pass in the
  final core suite. No skills were invoked and no plugins were installed.

## I. Release-gate evidence (2026-10-03)

- Final core run: `cargo test -p cleaner-core -- --skip engines::lama::tests
  --test-threads=1` passed **564 unit tests, 15 integration tests and 2 doctests**.
  One manual PSD generator and one example doctest were ignored by that run.
  All **17 LaMa tests** passed separately earlier; they were excluded from the
  final run to avoid concurrent model initialization with Tauri. The later
  installed-manifest compatibility change passed **27 project-store tests**
  plus the sanitized v3 preservation/rerun regression.
- PSD manual fixture generation was then run explicitly. Independent
  `psd-tools` verified **22/22** flat/layered PSD files, native planes, masks and
  structure through `scripts/check-psd.py`. This is a successful external reader
  check, not merely an ICC byte round trip.
- Full serial Tauri: **313 passed, 1 failed** (314 tests). The failing
  `the_shipped_picks_keep_bubble_text_off_the_inpainter` routes fixture region
  `c1-p001-r8` to LaMa because the model classifies it outside the balloon.
  An isolated clean checkout of HEAD `03fcaba708d80835f0f5591b6d0cffe9154ad021`
  reproduces the same failure with the same models, serially. No expectation
  was weakened. After the installed compatibility fix, all **242 non-run
  Tauri tests** passed again, including library/history/region/export/tile.
- Frontend: **934 tests in 53 files passed**. Production Vite build passed.
  macOS release bundle built with `npx tauri build --bundles app --no-sign`.
  Local verification uses an ad-hoc signature; release/notarization/updater
  signing was intentionally not performed. Existing provisioner/uv sidecars
  were retained when updating the installed app. The prior app bundle is
  recoverable at `/tmp/Manga Cleaner before color fidelity.app`.
- Installed app verification created isolated four-page single/longstrip
  projects: linear RGB gAMA/cHRM PNG, orientation-6 JPEG with artist metadata,
  straight-alpha TIFF and two-frame WebP. Import retained native file hashes,
  chapter-owned WebP archive hash, selected frame and frame count. Actual PNG
  outputs independently matched Pillow samples, including oriented geometry,
  alpha and first animation frame. The JPEG's descriptive artist EXIF survived
  and exported orientation normalized to 1.
- Native webview repeated-arrow navigation reached position 4 and returned to
  1 in both single/longstrip modes and both RTL/LTR settings. RTL was restored
  after the check. A keyboard-created region was committed, undone, redone and
  exported; **all samples outside its 6,095-pixel mask matched exactly**, and
  gAMA remained 1.0. The actual-format/metadata preflight and final filenames
  were visible in the installed app.
- Installed verification exposed older real v3 manifests containing nullable
  cloud cost, absent tier, nested revision ink paths and extension fields.
  These now load; unknown project/source/patch/cloud fields and legacy patch
  revisions survive save and rerun. New manifests use **v4**, reading v1–v4.
  Read-only probes loaded all patches in real chapters with 34, 49 and 32 pages.
  The pre-existing 2,523 files in the navigation-test project remained byte
  identical during verification.
- Native pointer automation succeeded when binding the app by name. A #808080
  brush on linear-RGB PNG produced native [55,55,55] across its 3,897-pixel mask;
  clone produced a 4,612-pixel mask. Independent reconstruction of all three
  patches matched the export exactly, including every sample outside the
  14,604-pixel mask union. Original source hashes stayed unchanged. Physical
  held-key behavior and an intermediate drag-preview capture remain unverified.
  Windows/Linux webviews, physical HDR/wide-gamut displays and Photoshop were
  unavailable; independent PSD and numeric SDR reference checks passed.
- Branch correction: this evidence was collected in `main` at `03fcaba`, the
  wrong checkout. It does not validate `codex/cloud-integration`. See
  [wrong-branch handoff](color-fidelity-wrong-branch-handoff.md). Target adaptation and validation are recorded in the final section below.

### Explicit supported limits

Associated-alpha TIFF preview and native TIFF export are supported; editing
currently refuses to avoid fitting premultiplied samples as straight colors.
Non-ICC legacy TIFF transfer/white-point/primary declarations and unsupported
HDR cICP transformations refuse processing while keeping native originals and
unchanged passthrough. Markerless four-channel JPEG uses the disclosed CMYK
assumption. Mixed stitched interpretations require compatible pages. Existing
paint-mode restrictions and the bounded reordered-edit reconstruction limit
remain explicit. None of these cases silently promotes native samples to RGB8.

Validation logs are under `/tmp/manga-{core-release,tauri-final,tauri-post-compat,
frontend-final,build-final,package-final,psd-oracle-release}.log`; the baseline
model reproduction is `/tmp/manga-tauri-baseline-routing.log`.
- Installed-chapter compatibility follow-up: an existing manifest contained
  repeated untouched detections with identical geometry, producing duplicate
  frontend keyed-region IDs and partial navigation renders. API hydration now
  exposes one region per stable geometry identity with the latest reason, while
  preserving all original manifest records/history byte-for-byte. Tauri
  regression verifies resident/header/review counts and unchanged manifest.
  Full EditorScreen navigation regression includes legacy cloud provenance with
  null cost/tier and verifies layers, artwork and page readout together.
  Final frontend gate now passes 934 tests / 53 files; production build passes.

## Target branch adaptation and validation (2026-10-03)

All evidence above is historical **main** evidence. The implementation has been adapted in `/Users/caved/.codex/worktrees/cloud-integration/manga-cleaner`,
branch `codex/cloud-integration`, starting at `c5a61f4`. Its pre-existing dirty
cloud configuration, detection, denoise, settings and navigation work is
preserved. Three-way snapshots are in `/tmp/color-fidelity-branch-handoff`.
Main was restored by the coordinating navigation chat to its original
mouse-pressure change and `promo/`; its backup/cleanup record is under
`/Users/caved/.codex/backups/manga-cleaner/branch-correction-20261003-d339a4e2`.
The wrong-branch implementation and handoff remain recoverable there and in the
snapshot; this target checkout is the authoritative implementation going forward.

Target-specific integration decisions:

- Keep fast PNG compression and bounded tile caching while applying identical
  managed transforms to whole-page and tiled derivatives. Palette/tRNS/gamma
  metadata is included in the patch-layer header cache.
- Page artwork uses authoritative cleaned tiles with a retained previous image
  until replacement load. Independent browser layer blending cannot reproduce
  native compositing followed by linear-light reduction; a regression explicitly
  demonstrates this discrepancy instead of accepting color errors.
- Preserve the target's richer cloud render, text-shape, layer/revision and
  write-lock contracts. Native parts/orientation require schema v5; existing v3/v4
  manifests retain their established schema where these features are absent.
- Stored detection masks, padding bases and held lettering remain native. Reads
  adapt to display orientation and writes inverse-map; legacy records are not
  rewritten merely by reading. A regression covers orientations 2–8 and decline.
- Extend managed conversion to the target-only shared cloud render and local/
  cloud denoise paths. Denoise receives a native-depth composite, never a display
  PNG; only model transport is sRGB8, then output returns to native depth and
  retains alpha/hidden samples and compatible metadata.

Batch-specific target checks (full release results follow):

| Batch | Target evidence |
| --- | --- |
| A/B/D/E | Independent reference fixtures and metadata/codec guards pass in the final core gate; browser edge-blending assumptions were replaced by authoritative-tile coverage. |
| C | Core export 43 passed, one manual generator ignored; export dialog 3 passed; PSD resource-size validation retains target limits. |
| F | Managed engines 84 passed; paint 38 passed; local denoise 10 passed, two model/runtime/golden tests ignored. |
| G | Ingest 25 passed; chapter-owned reopen/export workflow passed; native JPEG passthrough/edited lossless regression passed. |
| H | Project 81 passed including target lock/revision/text-shape behavior, legacy exact-byte retention, oriented layers and native detection buffers. Tauri orientation 2 passed (all eight orientations plus seam edit/history). |
| I | Explicit manual PSD fixture generation passed; independent psd-tools 22/22 passed. Full frontend/build and installed-app checks are recorded below. |

Focused counts above are supplemented by the final target release results below. No cloud GPU requests were sent during
these tests; gateway behavior uses local test doubles. No skills/plugins used.


### Final target release evidence

The following results supersede the intermediate table above. All commands ran
in the `codex/cloud-integration` worktree, with the existing cloud, detection,
navigation, reading-direction and native-JPEG changes preserved.

- A/B/D/E: full core gate passes **827 unit tests, 16 integration tests and two
  doc tests**. Independent libjpeg component, Pillow/LCMS appearance, TIFF alpha,
  PNG metadata/transparency and all-eight-orientation references are exercised;
  native preservation assertions remain exact, separately from decoder/preview
  tolerances. Log: `/tmp/cloud-color-core-release.log`.
- C/G: core export **43 passed**, ingest **25 passed**, owned-chapter workflow
  passed, and independent PSD decoding **22/22 passed**. The manual PSD fixture
  generator was explicitly run despite being ignored in the default core suite.
- F: managed engine **84**, paint **38**, local denoise **10**, target model
  workflow **18**, and diagnostic **one** focused tests passed. Cloud denoise
  **15** tests passed, including independent linear 16-bit inverse conversion,
  exact alpha/hidden samples and oriented upload planning. No live GPU requests.
- H: native/display API separation preserves the target revision/write-lock
  behavior; existing detection masks and source/patch history are not rewritten
  on reads. Denoise replacement now inverse-orients its samples into native
  storage, retains original files and detection bytes, and writes EXIF orientation
  into its self-describing PNG. All eight orientations pass replacement plus
  forced-TIFF export tests; long descriptive EXIF values survive. Large chapter
  listing uses metadata bounds and passes the 100-page/1,000-layer latency test.
- E/F/I: all **2,193 frontend tests across 120 files pass**, including native
  preview cancellation, source identity, navigation, and cross-page tile handoff.
  Final overlays wait for every affected tile to advance revision and finish
  loading. Production Vite build passes (455 modules); its bundle-size warning
  remains. Logs: `/tmp/cloud-color-frontend-release.log`,
  `/tmp/cloud-color-package-release.log`.

Default core ignores: 28 unit tests and one doc test. These include optional
model/runtime/golden or performance checks and the separately executed PSD
fixture generator. Optional model tests are not claimed as validated here.

- Final full serial Tauri gate: **934 passed, zero failed, seven ignored** in
  308 seconds (`cargo test -p manga-cleaner --lib -- --test-threads=1`);
  `/tmp/cloud-color-tauri-release.log`. Ignored checks need live Modal credentials,
  a keychain probe, real legacy/release evidence, local SAM/WebGPU or underlay
  fixtures. The former scratch-directory collision, native-JPEG estimate and
  large-chapter listing failures are fixed and pass in this complete run.
- Final macOS bundle build passed using the existing verified provisioner/uv
  sidecars. The temporary release `externalBin` entries were restored to the
  original development configuration after packaging. No plugin installation,
  release signing or notarization was performed. Existing Rust unused/dead-code
  warnings and the Vite chunk-size warning remain.
- The correct target bundle replaced `/Applications/Manga Cleaner.app`, locally
  ad-hoc signed. `codesign --verify --deep --strict` passes. Both installed and
  target executable SHA-256 are
  `7185f89061dfa257b5b9e7deaf8a9c0e3bf5f2eff599d3c0a7caf634f3a6c4e0`.
- Installed cloud build: existing Ch. 2 renders its artwork and saved layers;
  repeated RTL arrows navigate past page two and return. Isolated four-page
  projects pass repeated RTL/LTR navigation in **single-page and longstrip**
  modes; the test change to reading direction was restored to RTL. Theme/cloud
  settings were not edited through the test workflow or restored from old snapshots.
- Native PNG/JPEG/TIFF imports and archived animated WebP all match input bytes.
  The conversion notice and two-frame/first-frame/archive notice are visible.
  After moving the input folder, a keyboard-created edit, undo, redo and four-page
  export succeed. Independent Pillow verification finds **301,105 outside-mask
  pixels exact**, 6,095 edit-mask pixels, gAMA 1.0 retained, all TIFF RGBA samples
  exact, rotated JPEG output at 640×480 with zero channel difference to Pillow,
  and first-frame WebP pixels exact. Log: `/tmp/cloud-color-installed-checks.log`.
- All **3,938 existing files** checked in projects p53 and p56 retain their hashes.
  Fresh snapshots are in `/tmp/cloud-color-installed-release-before`.
- Installed limitations: pointer drag repeatedly returns the automation error
  `noWindowsAvailable`, including after raising/rebinding the app, so physical
  brush/clone and intermediate live-drag appearance are **not reverified on this
  target build**. Their numeric/backend and mounted frontend regressions pass;
  earlier main-build pointer evidence is historical only. Held-key repeats,
  Windows/Linux webviews, physical HDR/wide-gamut displays, live cloud GPU models
  and Photoshop remain unavailable/unrun. Independent PSD reference checks pass.

- Restart/reopen succeeds with the external scan folder absent: saved Fill layer,
  native display and undo action remain available. Test projects p65/p67 were
  removed from the active library and retained with their original index entries
  at `/tmp/cloud-color-installed-verification-projects`; no existing chapter was
  deleted or restored from a stale snapshot. Final existing-file hash check is
  unchanged at 3,938/3,938. `git diff --check` passes. No commits were made.
