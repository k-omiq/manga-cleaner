//! The `.mtclean` project file, which is also the job manifest.
//!
//! Three sentences shape everything here:
//!
//! > **This file is the job manifest** flushed after every completed region -
//! > one artefact, not two. It is rewritten atomically via temp-and-rename.
//!
//! > On launch, any incomplete job offers a resume, and resume re-verifies
//! > source hashes before continuing.
//!
//! > **Output never overwrites input.**
//!
//! The first is why [`Job`] exists rather than a pair of free functions: a
//! caller that has to remember to flush is a caller that will not, and the
//! failure mode is a crash losing completed regions - the exact thing §6 is
//! written to prevent. So the only way to record a region is a method that
//! writes the sidecar buffers, appends the record and rewrites the manifest,
//! in that order.
//!
//! What is **not** here, and belongs to Phase 6: the LRU
//! patch cache, the free-disk preflight, the orphaned-session sweep, and the
//! single-instance job lock.

use std::path::{Path, PathBuf};

use crate::image::{BitDepth, ColorMode};
use crate::ingest::{sha256_hex, IngestReport, SourceRef, Warning};
use crate::mask::Rect;
use crate::patch::{Engine, Patch, Provenance};
use crate::text_shape::{
    GeometryPolicy, MaskPlan, MaskQualityState, MaskRaster, PlanIdentity, PreparedMaskPlan,
};
use sha2::{Digest, Sha256};

pub mod buffers;

/// The manifest format this build writes. A file from a later version is
/// refused rather than half-read: a manifest is the only record that a job's
/// completed regions exist.
///
/// **What this number can and cannot protect.** [`Job::open`] reads it *before*
/// deserialising the rest, so a version it does not recognise is refused as
/// [`StoreError::Version`] whatever shape the body has. That ordering is the
/// whole value of the field: with the version read out of the fully
/// deserialised [`Project`], a future version whose shape had also changed
/// would fail as [`StoreError::Json`] and the version gate would never be
/// reached - the check would fire only for the files that did not need it.
///
/// It still says nothing about a shape that changes *without* the number
/// changing. That is a decision to be justified in each case, and
/// [`Strip::splits`] carries the one instance of it in this build.
pub const FORMAT_VERSION: u32 = 4;

/// The oldest manifest this build reads.
///
/// Version 4 adds prepared text-shape mask plans and a per-patch geometry
/// policy. Missing plan rows and missing patch policy fields deserialize as
/// legacy, so v1-v3 projects open without reinterpreting their saved masks.
/// Opening upgrades the in-memory version; the disk file remains untouched
/// until the next [`Job::flush`].
pub const OLDEST_READABLE_VERSION: u32 = 1;

/// The extension §1 names, and the sidecar directory beside it.
pub const EXTENSION: &str = "mtclean";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("the manifest is not readable as JSON: {0}")]
    Json(String),
    #[error("{0}")]
    Malformed(String),
    #[error("prepared text-shaped mask plan is stale or does not match the approved preview")]
    StalePlan,
    #[error("this build reads manifest version {expected}; the file is version {found}")]
    Version { expected: u32, found: u32 },
}

impl StoreError {
    fn io(path: &Path, source: std::io::Error) -> StoreError {
        StoreError::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

/// Single page or longstrip. §1's `strip.mode`, chosen once on the home
/// screen and fixed for the project's life, because it determines stitching and
/// split points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StripMode {
    Single,
    Longstrip,
}

/// §1's `sources` row. The header fields are here rather than only in
/// [`SourceRef`] because §6 re-verifies "sha256, mtime and dimensions" and a
/// resume must be able to do that without decoding anything.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProjectSource {
    /// The page's own file, relative to the manifest's own directory where a
    /// relative path can be formed and absolute where it cannot - a path on
    /// another volume has no relative form. Both resolve through the same
    /// `join`, because joining an absolute path replaces the base.
    ///
    /// For a chapter created by this build this is always a file inside the
    /// job's own sidecar, put there at ingest; the user's file is
    /// `converted_from`. A manifest written before that names the user's file
    /// here and reads unchanged.
    pub rel_path: PathBuf,
    pub sha256: String,
    /// Seconds since the Unix epoch, where the filesystem gave one.
    pub mtime: Option<u64>,
    pub w: u32,
    pub h: u32,
    pub mode: ColorMode,
    pub bit_depth: BitDepth,
    /// The user's own file, which ingest copied or converted to produce
    /// `rel_path` - see [`crate::ingest::SourceRef::converted_from`]. Relative
    /// to the manifest's own directory on the same terms as `rel_path`.
    ///
    /// **This is the origin, and it is not required to still exist.** Nothing
    /// reads it: it is what [`Job::origin_path`] answers "where did this
    /// chapter come from" with, and what [`Job::output_refusal`] refuses to
    /// write over. A user who deletes their scan folder after creating the
    /// chapter loses neither the pages nor either of those answers - an export
    /// whose default directory is gone is a directory the exporter creates.
    ///
    /// Optional and defaulted, so a manifest written before ingest imported
    /// anything reads unchanged and no [`FORMAT_VERSION`] bump is owed: the
    /// field only ever *adds* a second path to a row that already resolved on
    /// its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub converted_from: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Strip {
    pub mode: StripMode,
    /// Indices into `sources`, in reading order. Held separately from the
    /// sources list because ingest order is natural-sort order and a user may
    /// reorder pages without the sources moving.
    pub order: Vec<usize>,
    /// Split rows in **strip** coordinates, from rule 2. Always empty
    /// for `Single`, where the page boundaries are the splits.
    ///
    /// A row rather than a bare number, because rule 2 requires a split that
    /// found no acceptable row to be "recorded as `split_fallback: true` in
    /// `strip.splits` and surfaced in review" - so the record has to carry more
    /// than the row. [`crate::strip::SplitKind`] carries that bit and two more.
    ///
    /// **The element type changed from `u32` to [`crate::strip::Split`] without
    /// a [`FORMAT_VERSION`] bump, and that is a decision rather than an
    /// oversight.** `#[serde(default)]` covers an *absent* field, not a
    /// populated one: a manifest holding `"splits": [2000]` fails [`Job::open`]
    /// with a decode error.
    ///
    /// No such manifest can exist. [`Project::new`] is the only constructor of
    /// this struct and it writes `Vec::new()`; nothing in this workspace has
    /// ever assigned a non-empty `splits`, because rule 2's planner
    /// ([`crate::strip::plan_splits`]) has no caller - so every `.mtclean` file
    /// any build has written carries `[]`, which decodes identically under both
    /// element types. The change is invisible to every file it could meet.
    ///
    /// Bumping instead would cost more than it bought, in both directions.
    /// Every existing manifest would be refused by [`Job::open`], including the
    /// ones whose `splits` is empty and therefore whose meaning is unchanged;
    /// there is no migration path in this build to read them with, so the
    /// refusal would be permanent rather than a step. And it would not even
    /// improve the error for the file it is nominally about: a v1 file holding
    /// `"splits": [2000]` read by a v2 build is refused for its *version*, but
    /// only because [`Job::open`] now probes the version first - as a shape
    /// mismatch it was always going to be refused either way.
    ///
    /// What this reasoning depends on is that the field has no writer. The
    /// moment rule 2's planner is wired to a manifest, a populated `splits` can
    /// reach disk, and the *next* change to this type is a bump.
    #[serde(default)]
    pub splits: Vec<crate::strip::Split>,
}

/// §1's `patches` row: one applied edit, and where its pixels live.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PatchRecord {
    pub id: String,
    pub source_idx: usize,
    pub bbox: Rect,
    /// File names within `<job>.mtclean.d/`.
    pub mask_ref: String,
    pub buffer_ref: String,
    pub engine: Engine,
    pub order: u32,
    pub visible: bool,
    /// The `review.reason.*` key this region was flagged under, or `None` if it
    /// needs no review.
    ///
    /// Stored rather than recomputed because the interface derives it from a
    /// live `Region` - `src/lib/model/review.js` reads `unusuallyLarge`,
    /// `cloudOutcome` and the gate cause - and a resumed job has no live
    /// region until the page is re-detected. §1 persists `order` and `visible`
    /// for the same reason: state the composite depends on that re-running
    /// would not reproduce.
    pub review_state: Option<String>,
    /// Geometry used to derive this patch's applied mask. Missing in v1-v3
    /// manifests and every existing patch means legacy.
    #[serde(default, skip_serializing_if = "is_legacy_geometry_policy")]
    pub geometry_policy: GeometryPolicy,
    /// Exact prepared-plan identity for a text-shaped patch. Kept separate
    /// from the stable region ID so an older undo state can still identify its
    /// own padding/correction revision after a newer preview is prepared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_shape_plan_identity: Option<String>,
    pub provenance: Provenance,
}

fn is_legacy_geometry_policy(policy: &GeometryPolicy) -> bool {
    *policy == GeometryPolicy::Legacy
}

impl PatchRecord {
    pub fn legacy_revision_id(&self) -> Option<String> {
        (self.geometry_policy == GeometryPolicy::Legacy)
            .then(|| revision_from_buffer_ref(&self.buffer_ref))
            .flatten()
    }

    /// Opaque content revision exposed to undo snapshots. Old M3 records can
    /// still be addressed by their plan identity if no content ref exists.
    pub fn text_shape_revision_id(&self) -> Option<String> {
        (self.geometry_policy == GeometryPolicy::TextShape).then(|| {
            revision_from_buffer_ref(&self.buffer_ref)
                .or_else(|| {
                    self.text_shape_plan_identity
                        .as_ref()
                        .map(|identity| format!("legacy:{identity}"))
                })
                .unwrap_or_default()
        })
    }

    /// The row a patch becomes.
    ///
    /// One constructor rather than two, because the row is built in two places -
    /// [`Job::complete_region`], which persists it, and the run, which
    /// reports the same region on the event channel - and a second spelling is
    /// a `mask_ref` that agrees today and diverges the moment either moves.
    pub fn of(source_idx: usize, patch: &Patch, review_state: Option<String>) -> PatchRecord {
        PatchRecord {
            id: patch.id.clone(),
            source_idx,
            bbox: patch.mask.bounds,
            mask_ref: format!("{}.mask", patch.id),
            buffer_ref: format!("{}.buf", patch.id),
            engine: patch.provenance.engine,
            order: patch.order,
            visible: patch.visible,
            review_state,
            geometry_policy: GeometryPolicy::Legacy,
            text_shape_plan_identity: None,
            provenance: patch.provenance.clone(),
        }
    }

    /// The sidecar file holding [`Patch::ink`]: `<id>.ink`, beside the
    /// `<id>.mask` that `mask_ref` names.
    ///
    /// Derived rather than stored, because it is the one sidecar entry that
    /// arrived after the manifest's row was specified, and a manifest without
    /// it still has to read: [`Job::load_patch`] answers a missing file with
    /// the applied mask.
    pub fn ink_ref(&self) -> String {
        if revision_from_buffer_ref(&self.buffer_ref).is_some() {
            self.mask_ref
                .strip_suffix(".mask")
                .map(|stem| format!("{stem}.ink"))
                .unwrap_or_else(|| format!("{}.ink", self.id))
        } else {
            format!("{}.ink", self.id)
        }
    }
}

fn revision_from_buffer_ref(reference: &str) -> Option<String> {
    reference
        .rsplit('/')
        .next()
        .and_then(|name| name.strip_suffix(".buf"))
        .filter(|stem| stem.len() == 64 && stem.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_owned)
}

/// Manifest metadata for one prepared text-shaped mask revision. Pixel data
/// lives in bounded versioned sidecar artifacts; it is never embedded in the
/// JSON manifest. Rows are append-only by `(region_id, identity)` so undo and
/// redo can keep referring to the exact plan that produced a patch.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TextShapePlanRecord {
    pub region_id: String,
    pub source_idx: usize,
    pub geometry_policy: GeometryPolicy,
    pub plan_version: u32,
    pub source_sha256: String,
    pub lower_composite_sha256: String,
    pub algorithm_id: String,
    pub model_id: Option<String>,
    pub candidate_bounds: Rect,
    pub refinement_crop: Rect,
    pub base_revision: u64,
    pub correction_revision: u64,
    pub plan_revision: u64,
    pub padding_px: u32,
    pub model_hole_margin_px: u32,
    pub reading_context: Rect,
    pub quality: MaskQualityState,
    pub identity: PlanIdentity,
    /// References are relative to the job sidecar directory.
    pub base_mask_ref: String,
    pub additions_ref: String,
    pub removals_ref: String,
    pub write_support_ref: String,
    pub model_hole_ref: String,
    pub blend_alpha_ref: Option<String>,
}

/// Latest failed text-shaped preparation for a region. Failed previews have
/// no approved W and therefore no prepared sidecar, but their correction
/// state must survive save/reopen until a newer plan is accepted.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TextShapeCorrectionRecord {
    pub region_id: String,
    pub source_idx: usize,
    pub plan_version: u32,
    pub plan_revision: u64,
    pub candidate_bounds: Rect,
    pub reason: String,
}

impl TextShapePlanRecord {
    fn same_identity(&self, region_id: &str, identity_sha256: &str) -> bool {
        self.region_id == region_id && self.identity.identity_sha256 == identity_sha256
    }
}

/// §1's `regions_untouched`: declined and gate-dropped regions.
///
/// They are persisted because they are the review list's other half. A region
/// the gate held back has no patch, so without this row a resumed job shows a
/// clean page and no record that anything was deliberately left alone.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RegionUntouched {
    pub source_idx: usize,
    pub bbox: Rect,
    /// An i18n key - `review.reason.*` or `decline.reason.*`. No English
    /// crosses the seam, and none is written to disk either.
    pub reason: String,
}

/// One file that ingest refused, and why.
///
/// The file name rather than the path: a skipped file has no source row to
/// hang a `rel_path` on, and what the report says is "`wm014.jpg` was skipped".
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SkippedInput {
    pub file: String,
    /// An `input.skipReason.*` key - [`crate::ingest::SkipReason::reason_key`].
    pub reason: String,
}

/// §2's input report, kept for the same reason §1 keeps `regions_untouched`.
///
/// §2 requires every refused file to be "skipped and **listed**", and the seam
/// carries that list as `ApiChapter.inputReports`. Neither survives the session
/// that produced it unless it is written down: the accepted files become
/// `sources` and the refused ones become nothing at all, so reopening a chapter
/// would show twenty-two pages where twenty-four files sit and no record that
/// two were refused. Re-deriving it means re-hashing and re-decoding every file
/// in the folder on every open, which is the cost §1 persists `counters` to
/// avoid.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct InputReport {
    pub skipped: Vec<SkippedInput>,
    /// Dotfiles, `__MACOSX/`, `Thumbs.db`, `desktop.ini`. A count rather than a
    /// list, because the notice that reports it is a count.
    pub junk_skipped: u32,
    /// §2's "warn rather than guess": the stems that appeared under two
    /// extensions.
    pub duplicate_basenames: Vec<String>,
    /// The files ingest converted on the way in. Persisted for the same reason
    /// as `skipped`: it is a thing that happened to the user's folder, and
    /// re-deriving it on open would mean re-reading every file to find out
    /// what format it used to be.
    pub converted: Vec<ConvertedInput>,
}

/// One row of [`InputReport::converted`].
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ConvertedInput {
    /// The original file's name, which is the one the user recognises.
    pub file: String,
    /// `JPEG`, `WebP`, `GIF`, `BMP` - not a translated string, interpolated
    /// into one that is.
    pub from: String,
}

impl InputReport {
    /// The report an ingest produced, in the form the manifest keeps.
    pub fn of(report: &IngestReport) -> InputReport {
        InputReport {
            skipped: report
                .skipped
                .iter()
                .filter(|entry| entry.reason != crate::ingest::SkipReason::Junk)
                .map(|entry| SkippedInput {
                    file: file_name(&entry.path),
                    reason: entry.reason.reason_key().to_owned(),
                })
                .collect(),
            junk_skipped: report.junk_skipped() as u32,
            duplicate_basenames: report
                .warnings
                .iter()
                .map(|warning| match warning {
                    Warning::DuplicateBasename { stem, .. } => stem.clone(),
                })
                .collect(),
            converted: report
                .converted
                .iter()
                .map(|entry| ConvertedInput {
                    file: file_name(&entry.original),
                    from: entry.from.to_owned(),
                })
                .collect(),
        }
    }
}

/// §1's `counters`, verbatim.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Counters {
    pub refused: u32,
    pub errored: u32,
    pub param_reject: u32,
    pub residual_reject: u32,
    pub structural_reject: u32,
    pub declined: u32,
    pub gate_dropped: u32,
}

/// The whole `.mtclean` file.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Project {
    pub version: u32,
    /// Seconds since the Unix epoch.
    pub created: u64,
    pub app_version: String,
    pub sources: Vec<ProjectSource>,
    pub strip: Strip,
    pub patches: Vec<PatchRecord>,
    /// Prepared text-shaped geometry, including plans that have a preview but
    /// have not produced an applied patch yet. Absent from old projects, where
    /// the policy remains legacy.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub text_shape_plans: Vec<TextShapePlanRecord>,
    /// Earlier committed patch records retain their own immutable sidecars so
    /// a correction or rerun can be undone after a later revision is saved.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub text_shape_patch_revisions: Vec<PatchRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub legacy_patch_revisions: Vec<PatchRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub text_shape_corrections: Vec<TextShapeCorrectionRecord>,
    pub regions_untouched: Vec<RegionUntouched>,
    pub counters: Counters,
    /// What §2 refused on the way in, and what it warned about.
    ///
    /// Not in §1's field list; added because §2's listing and the seam's
    /// `ApiChapter.inputReports` both outlive the ingest that produced them.
    /// Defaulted on read, so a manifest written before it existed still opens.
    #[serde(default)]
    pub input_report: InputReport,
    /// Which sources the detector has actually looked at, as indices into
    /// `sources`.
    ///
    /// Not in §1's field list, and the one bit no other field can carry. Every
    /// other record here describes work that *produced* something: a patch, an
    /// untouched region, a counter above zero. A page the detector examined and
    /// found no text on produces none of them, so without this field it is
    /// indistinguishable from a page nothing has ever looked at - and the two
    /// want opposite behaviour. The first is finished and must not be queued
    /// again; the second is the entire remaining job.
    ///
    /// It is also the only thing that can answer the seam's
    /// `ApiChapter.noTextDetected`, which is a claim about what was *found*
    /// and therefore presupposes that something looked. Defaulted on read, so a
    /// manifest written before it existed still opens - as an unexamined
    /// chapter, which is the safe reading of a job whose run predates the
    /// record.
    #[serde(default)]
    pub examined: Vec<usize>,
    /// Which sources a run failed on, as indices into `sources`.
    ///
    /// The same shape as `examined` and there for the same kind of reason:
    /// `counters.errored` is a number, and a number cannot be re-derived. A
    /// page that could not be read or could not be cleaned produces no patch
    /// and no untouched row, is deliberately **not** marked examined so a later
    /// run tries it again, and so was counted afresh on every attempt - the
    /// counter climbed for ever over one corrupt file. This is the record the
    /// counter is kept in step with: a page enters the list when it fails and
    /// leaves it when it is re-attempted, and `counters.errored` is the length
    /// of that state rather than a tally of attempts.
    ///
    /// Cleared for a source whose hash changed, alongside `examined`, and
    /// defaulted on read like the three fields above it.
    #[serde(default)]
    pub errored: Vec<usize>,
    /// Where a cancelled or crashed run stopped, as a position in
    /// `strip.order`, or `None` for a job that is not interrupted.
    ///
    /// §6 says "on launch, any incomplete job offers a resume" and §1 gives the
    /// manifest no way to say that a job **is** incomplete: every field it
    /// holds describes work that finished. This is the one bit that does not,
    /// and it is what `resumeJob` and `ApiProject.interruptedJob` are read
    /// from. Defaulted on read for the same reason as `input_report`.
    #[serde(default)]
    pub interrupted_at: Option<u32>,
    /// A snapshot of the settings the job ran under.
    ///
    /// Opaque here on purpose: the settings vocabulary belongs to the interface
    /// (`readSettings`/`writeSettings`), and duplicating it in the core
    /// would give two definitions to disagree with each other. What the core
    /// owes is that the snapshot survives a round trip unchanged.
    pub settings: serde_json::Value,
}

impl Project {
    fn requires_v4(&self) -> bool {
        !self.text_shape_plans.is_empty()
            || !self.text_shape_corrections.is_empty()
            || self.patches.iter().any(|row| row.geometry_policy == GeometryPolicy::TextShape)
            || !self.text_shape_patch_revisions.is_empty()
    }

    /// A new project over a set of ingested sources.
    ///
    /// Takes [`SourceRef`]s because that is what [`crate::ingest`] produces and
    /// what §2's policy has already filtered - a project is never built from
    /// paths that have not been through it.
    pub fn new(
        manifest_dir: &Path,
        app_version: &str,
        mode: StripMode,
        sources: &[SourceRef],
    ) -> Project {
        let rows: Vec<ProjectSource> = sources
            .iter()
            .map(|source| ProjectSource {
                rel_path: relative_to(manifest_dir, &source.path),
                sha256: source.sha256.clone(),
                mtime: mtime_of(&source.path),
                w: source.width,
                h: source.height,
                mode: source.mode,
                bit_depth: source.bit_depth,
                converted_from: source
                    .converted_from
                    .as_ref()
                    .map(|original| relative_to(manifest_dir, original)),
            })
            .collect();

        Project {
            version: 3,
            created: now(),
            app_version: app_version.to_owned(),
            strip: Strip {
                mode,
                order: (0..rows.len()).collect(),
                splits: Vec::new(),
            },
            sources: rows,
            patches: Vec::new(),
            text_shape_plans: Vec::new(),
            text_shape_patch_revisions: Vec::new(),
            legacy_patch_revisions: Vec::new(),
            text_shape_corrections: Vec::new(),
            regions_untouched: Vec::new(),
            counters: Counters::default(),
            input_report: InputReport::default(),
            examined: Vec::new(),
            errored: Vec::new(),
            interrupted_at: None,
            settings: serde_json::Value::Object(serde_json::Map::new()),
        }
    }

    /// A new project over a whole ingest, refusals included.
    ///
    /// The constructor to reach for when the caller has an
    /// [`IngestReport`] rather than a bare source list: it is the only way the
    /// refused files reach the manifest, and a project built with [`new`] from
    /// `report.sources` alone drops them silently.
    ///
    /// [`new`]: Project::new
    pub fn from_ingest(
        manifest_dir: &Path,
        app_version: &str,
        mode: StripMode,
        report: &IngestReport,
    ) -> Project {
        let mut project = Project::new(manifest_dir, app_version, mode, &report.sources);
        project.input_report = InputReport::of(report);
        project
    }

    /// Which source has these bytes, if any. The byte-level half of "output
    /// never overwrites input": a destination path can be anything, but a file
    /// whose content is one of ours is one of ours.
    pub fn source_with_hash(&self, sha256: &str) -> Option<usize> {
        self.sources
            .iter()
            .position(|source| source.sha256 == sha256)
    }
}

/// Why a write was refused. §1: "Refuse to write to any path whose sha256 is in
/// `sources`; default the output directory to a sibling `<input>_cleaned/`."
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputRefusal {
    /// The destination directory is where sources live. Refused as a directory
    /// rather than per file, because an export writes a set and "some of them
    /// would have been overwritten" is not a state to leave a chapter in.
    SourceDirectory { source_idx: usize },
    /// A specific destination file resolves to a source.
    WouldOverwriteSource { source_idx: usize },
}

impl OutputRefusal {
    /// The key the seam reports this under.
    /// `exportChapter` returns
    /// `status: 'refused'` with a `reasonKey`, and the interface already has
    /// this one.
    pub fn reason_key(&self) -> &'static str {
        "notice.export.refusedOverwrite"
    }
}

/// What re-verification found for one source. §6.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceState {
    Unchanged,
    Missing,
    /// The file is there and is not what it was.
    Changed {
        sha256: String,
    },
}

impl SourceState {
    pub fn is_stale(&self) -> bool {
        !matches!(self, SourceState::Unchanged)
    }
}

/// What a resume dropped, so the user can be told rather than left to notice.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StaleReport {
    pub sources: Vec<usize>,
    pub dropped_patches: Vec<String>,
}

impl StaleReport {
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }
}

/// An open job: the manifest, its sidecar directory, and the flush.
pub struct Job {
    path: PathBuf,
    dir: PathBuf,
    pub project: Project,
}

impl Job {
    /// Create a job and write its first manifest.
    ///
    /// The manifest exists on disk before any region runs, so a crash during
    /// the first region still leaves something to resume from - a job that only
    /// appears once it has produced output is a job that cannot report having
    /// produced none.
    pub fn create(path: &Path, project: Project) -> Result<Job, StoreError> {
        let job = Job {
            path: path.to_path_buf(),
            dir: sidecar_dir(path),
            project,
        };
        std::fs::create_dir_all(&job.dir).map_err(|e| StoreError::io(&job.dir, e))?;
        job.flush()?;
        Ok(job)
    }

    /// Open a job, refusing a manifest this build cannot read.
    ///
    /// The version is read **first**, from a probe that knows about one field,
    /// and the rest is deserialised only once the number matches. Reading it
    /// off a fully decoded [`Project`] instead would mean a manifest from a
    /// later version could only be recognised as one where its *shape* had not
    /// changed - every version that moved a field would arrive as
    /// [`StoreError::Json`], which says "not readable as JSON" about a file
    /// that is perfectly good JSON from a build that is not this one.
    pub fn open(path: &Path) -> Result<Job, StoreError> {
        let bytes = std::fs::read(path).map_err(|e| StoreError::io(path, e))?;

        #[derive(serde::Deserialize)]
        struct VersionOnly {
            version: u32,
        }
        let probe: VersionOnly =
            serde_json::from_slice(&bytes).map_err(|e| StoreError::Json(e.to_string()))?;
        if !(OLDEST_READABLE_VERSION..=FORMAT_VERSION).contains(&probe.version) {
            return Err(StoreError::Version {
                expected: FORMAT_VERSION,
                found: probe.version,
            });
        }

        let mut project: Project =
            serde_json::from_slice(&bytes).map_err(|e| StoreError::Json(e.to_string()))?;
        // The v3 reader ignores added legacy metadata. Only text-shaped rows
        // and plans require v4; older versions still advance to the v3 floor.
        project.version = if project.requires_v4() { FORMAT_VERSION } else { 3 };
        Ok(Job {
            path: path.to_path_buf(),
            dir: sidecar_dir(path),
            project,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Where patch buffers and masks live: a sibling `<job>.mtclean.d/`.
    pub fn sidecar(&self) -> &Path {
        &self.dir
    }

    /// The directory `rel_path` is resolved against.
    pub fn root(&self) -> &Path {
        self.path.parent().unwrap_or(Path::new("."))
    }

    pub fn source_path(&self, index: usize) -> Option<PathBuf> {
        self.project
            .sources
            .get(index)
            .map(|source| self.root().join(&source.rel_path))
    }

    /// Where the page **came from**, which since ingest began importing pages
    /// is never where its file is.
    ///
    /// [`source_path`] names a file inside the job's own sidecar, and every
    /// question about the *user's* folder - where the export's
    /// `<input>_cleaned/` sibling goes, which directory the chapter says it was
    /// made from, which files an export must not land on - wants the scan
    /// instead. The path is returned whether or not it still exists, because
    /// every one of those questions is about a *directory the user chose* and
    /// none of them opens the file.
    ///
    /// A manifest from a build before the import falls back to `rel_path`,
    /// where the two genuinely were the same file.
    ///
    /// [`source_path`]: Job::source_path
    pub fn origin_path(&self, index: usize) -> Option<PathBuf> {
        let source = self.project.sources.get(index)?;
        Some(
            self.root()
                .join(source.converted_from.as_ref().unwrap_or(&source.rel_path)),
        )
    }

    /// Rewrite the manifest atomically.
    pub fn flush(&self) -> Result<(), StoreError> {
        let mut project = self.project.clone();
        project.version = if project.requires_v4() { FORMAT_VERSION } else { 3 };
        let bytes = serde_json::to_vec_pretty(&project)
            .map_err(|e| StoreError::Json(e.to_string()))?;
        buffers::write_atomic(&self.path, &bytes)
    }

    pub fn text_shape_correction(&self, region_id: &str) -> Option<&TextShapeCorrectionRecord> {
        self.project
            .text_shape_corrections
            .iter()
            .find(|record| record.region_id == region_id)
    }

    /// Save the latest review state without fabricating an approved support.
    /// The mask plan itself remains in the caller until a newer valid revision
    /// can be stored with `store_text_shape_plan`.
    pub fn record_text_shape_correction(
        &mut self,
        source_idx: usize,
        region_id: &str,
        plan_revision: u64,
        candidate_bounds: Rect,
        reason: &str,
    ) -> Result<(), StoreError> {
        if self.project.sources.get(source_idx).is_none()
            || region_id.is_empty()
            || reason.is_empty()
        {
            return Err(StoreError::Malformed(
                "invalid text-shape correction state".into(),
            ));
        }
        if self.text_shape_correction(region_id).is_some_and(|record| {
            record.source_idx != source_idx || plan_revision < record.plan_revision
        }) {
            return Err(StoreError::StalePlan);
        }
        self.project
            .text_shape_corrections
            .retain(|record| record.region_id != region_id);
        let source = &self.project.sources[source_idx];
        let safe_locator = candidate_bounds.w > 0
            && candidate_bounds.h > 0
            && candidate_bounds.x >= 0
            && candidate_bounds.y >= 0
            && candidate_bounds
                .x
                .checked_add(candidate_bounds.w as i64)
                .is_some_and(|right| right <= source.w as i64)
            && candidate_bounds
                .y
                .checked_add(candidate_bounds.h as i64)
                .is_some_and(|bottom| bottom <= source.h as i64)
            && u64::from(candidate_bounds.w) * u64::from(candidate_bounds.h)
                <= crate::text_shape::MAX_PLAN_PIXELS as u64;
        let candidate_bounds = if safe_locator {
            candidate_bounds
        } else {
            Rect::new(
                (source.w / 2) as i64,
                (source.h / 2) as i64,
                source.w.min(1),
                source.h.min(1),
            )
        };
        self.project
            .text_shape_corrections
            .push(TextShapeCorrectionRecord {
                region_id: region_id.to_owned(),
                source_idx,
                plan_version: crate::text_shape::MASK_PLAN_VERSION,
                plan_revision,
                candidate_bounds,
                reason: reason.to_owned(),
            });
        self.project.version = FORMAT_VERSION;
        self.flush()
    }

    /// Persist a prepared text-shaped plan before any engine runs. Raster
    /// artifacts are written before the manifest names them, matching the
    /// patch-buffer ordering used by [`complete_region`]. Revisions remain in
    /// the manifest so an undo snapshot can keep naming the plan that produced
    /// its patch while a newer preview is prepared for the same region.
    pub fn store_text_shape_plan(
        &mut self,
        source_idx: usize,
        plan: &MaskPlan,
        prepared: &PreparedMaskPlan,
    ) -> Result<(), StoreError> {
        let source = self.project.sources.get(source_idx).ok_or_else(|| {
            StoreError::Malformed(format!(
                "text-shape plan source index {source_idx} is out of range"
            ))
        })?;
        if source.sha256 != plan.source_sha256 || prepared.identity.source_sha256 != source.sha256 {
            return Err(StoreError::StalePlan);
        }
        let canonical = plan
            .prepare(prepared.identity.page_w, prepared.identity.page_h)
            .map_err(|error| StoreError::Malformed(error.to_string()))?;
        if canonical != *prepared
            || plan.region_id != prepared.identity.region_id
            || plan.lower_composite_sha256 != prepared.identity.lower_composite_sha256
            || crate::text_shape::support_sha256(&prepared.write_support)
                != prepared.identity.support_sha256
        {
            return Err(StoreError::StalePlan);
        }
        if prepared.identity.page_w != source.w || prepared.identity.page_h != source.h {
            return Err(StoreError::StalePlan);
        }
        if self
            .text_shape_correction(&plan.region_id)
            .is_some_and(|failed| {
                failed.source_idx != source_idx || failed.plan_revision >= plan.plan_revision
            })
        {
            return Err(StoreError::StalePlan);
        }

        let directory_key = sha256_hex(plan.region_id.as_bytes());
        let id_key = &prepared.identity.identity_sha256;
        if !id_key.bytes().all(|byte| byte.is_ascii_hexdigit()) || id_key.len() != 64 {
            return Err(StoreError::Malformed(
                "invalid text-shape plan identity digest".into(),
            ));
        }
        let prefix = format!(
            "mask-plans/{directory_key}/r{}-{}/",
            plan.plan_revision,
            &id_key[..16]
        );
        let record = TextShapePlanRecord {
            region_id: plan.region_id.clone(),
            source_idx,
            geometry_policy: GeometryPolicy::TextShape,
            plan_version: plan.version,
            source_sha256: plan.source_sha256.clone(),
            lower_composite_sha256: plan.lower_composite_sha256.clone(),
            algorithm_id: plan.algorithm_id.clone(),
            model_id: plan.model_id.clone(),
            candidate_bounds: plan.candidate_bounds,
            refinement_crop: plan.refinement_crop,
            base_revision: plan.base_revision,
            correction_revision: plan.correction_revision,
            plan_revision: plan.plan_revision,
            padding_px: plan.padding_px,
            model_hole_margin_px: plan.model_hole_margin_px,
            reading_context: plan.reading_context,
            quality: plan.quality.clone(),
            identity: prepared.identity.clone(),
            base_mask_ref: format!("{prefix}base.mask"),
            additions_ref: format!("{prefix}additions.mask"),
            removals_ref: format!("{prefix}removals.mask"),
            write_support_ref: format!("{prefix}support.mask"),
            model_hole_ref: format!("{prefix}model-hole.mask"),
            blend_alpha_ref: plan
                .blend_alpha
                .as_ref()
                .map(|_| format!("{prefix}blend-alpha.mask")),
        };

        let existing = self.project.text_shape_plans.iter().find(|existing| {
            existing.same_identity(&plan.region_id, &prepared.identity.identity_sha256)
        });
        if let Some(existing) = existing {
            // Check before writing: a corrupted caller reusing an identity must
            // not overwrite a sidecar that an older patch still references.
            if existing != &record {
                return Err(StoreError::Malformed(
                    "a prepared plan identity was reused with different metadata".into(),
                ));
            }
            let (saved_plan, saved_prepared) = self.load_text_shape_plan_record(existing)?;
            if saved_plan != *plan || saved_prepared != *prepared {
                return Err(StoreError::Malformed(
                    "a prepared plan identity was reused with different raster inputs".into(),
                ));
            }
        } else if self.project.text_shape_plans.iter().any(|existing| {
            existing.region_id == plan.region_id && existing.plan_revision == plan.plan_revision
        }) {
            return Err(StoreError::Malformed(
                "a text-shape plan revision was reused".into(),
            ));
        }

        for (reference, raster, kind) in [
            (
                &record.base_mask_ref,
                &plan.base_mask,
                buffers::TextShapeMaskKind::Base,
            ),
            (
                &record.additions_ref,
                &plan.additions,
                buffers::TextShapeMaskKind::Additions,
            ),
            (
                &record.removals_ref,
                &plan.removals,
                buffers::TextShapeMaskKind::Removals,
            ),
            (
                &record.write_support_ref,
                &prepared.write_support,
                buffers::TextShapeMaskKind::Support,
            ),
            (
                &record.model_hole_ref,
                &prepared.model_hole,
                buffers::TextShapeMaskKind::ModelHole,
            ),
        ] {
            self.write_plan_mask(reference, raster, kind)?;
        }
        if let (Some(reference), Some(alpha)) = (&record.blend_alpha_ref, &plan.blend_alpha) {
            self.write_plan_mask(reference, alpha, buffers::TextShapeMaskKind::BlendAlpha)?;
        }

        if existing.is_none() {
            // Keep the row in append order and never discard the exact identity
            // referenced by an already committed patch or undo record.
            self.project.text_shape_plans.push(record.clone());
        }
        self.project
            .text_shape_corrections
            .retain(|failed| failed.region_id != plan.region_id);
        self.project.version = FORMAT_VERSION;
        // The same row may already exist; the just-encoded refs are stable and
        // identical, and flush is still useful when called as a retry after an
        // interrupted manifest write.
        if self.project.text_shape_plans.iter().any(|existing| {
            existing.same_identity(&plan.region_id, &prepared.identity.identity_sha256)
        }) {
            self.flush()?;
        }
        Ok(())
    }

    /// Load the most recent prepared plan for a stable region ID. The returned
    /// raster is verified against the persisted support artifact before it is
    /// trusted as preview authority.
    pub fn load_text_shape_plan(
        &self,
        region_id: &str,
    ) -> Result<Option<(MaskPlan, PreparedMaskPlan)>, StoreError> {
        let Some(record) = self
            .project
            .text_shape_plans
            .iter()
            .filter(|record| record.region_id == region_id)
            .max_by_key(|record| record.plan_revision)
        else {
            return Ok(None);
        };
        self.load_text_shape_plan_record(record).map(Some)
    }

    /// Load an exact plan revision by its prepared identity. Patches and undo
    /// snapshots use this instead of resolving the latest preview.
    pub fn load_text_shape_plan_identity(
        &self,
        region_id: &str,
        identity_sha256: &str,
    ) -> Result<Option<(MaskPlan, PreparedMaskPlan)>, StoreError> {
        let Some(record) = self
            .project
            .text_shape_plans
            .iter()
            .find(|record| record.same_identity(region_id, identity_sha256))
        else {
            return Ok(None);
        };
        self.load_text_shape_plan_record(record).map(Some)
    }

    /// Recheck the values captured by the preview/apply protocol immediately
    /// before rendering or attaching output.
    pub fn validate_text_shape_plan(
        &self,
        region_id: &str,
        expected_source_sha256: &str,
        expected_lower_composite_sha256: &str,
        expected_identity_sha256: &str,
        expected_support_sha256: &str,
    ) -> Result<PreparedMaskPlan, StoreError> {
        let (_, prepared) = self
            .load_text_shape_plan_identity(region_id, expected_identity_sha256)?
            .ok_or(StoreError::StalePlan)?;
        if prepared.identity.source_sha256 != expected_source_sha256
            || prepared.identity.lower_composite_sha256 != expected_lower_composite_sha256
            || prepared.identity.identity_sha256 != expected_identity_sha256
            || prepared.identity.support_sha256 != expected_support_sha256
        {
            return Err(StoreError::StalePlan);
        }
        Ok(prepared)
    }

    /// Commit a patch against one exact text-shape preview. This method is the
    /// only persistence path that marks a patch as `text_shape`; the legacy
    /// [`complete_region`] path keeps its prior behavior even if an unused
    /// prepared preview exists for a region with the same ID.
    pub fn complete_text_shape_region(
        &mut self,
        source_idx: usize,
        patch: &Patch,
        plan_identity_sha256: &str,
        review_state: Option<String>,
    ) -> Result<(), StoreError> {
        let (plan, prepared) = self
            .load_text_shape_plan_identity(&patch.id, plan_identity_sha256)?
            .ok_or(StoreError::StalePlan)?;
        let source = self
            .project
            .sources
            .get(source_idx)
            .ok_or(StoreError::StalePlan)?;
        if plan.region_id != patch.id
            || source_idx
                != self
                    .project
                    .text_shape_plans
                    .iter()
                    .find(|record| record.same_identity(&patch.id, plan_identity_sha256))
                    .map(|record| record.source_idx)
                    .unwrap_or(usize::MAX)
            || source.sha256 != plan.source_sha256
            || patch.mask.bounds != prepared.write_support.bounds
            || patch.mask.bits != prepared.write_support.bits
        {
            return Err(StoreError::StalePlan);
        }
        let mut patch = patch.clone();
        if !patch.provenance.params_snapshot.is_object() {
            patch.provenance.params_snapshot = serde_json::json!({});
        }
        patch.provenance.params_snapshot["geometry_policy"] = serde_json::json!("text_shape");
        patch.provenance.params_snapshot["write_support_sha256"] =
            serde_json::json!(prepared.identity.support_sha256);
        self.complete_region_with_policy(
            source_idx,
            &patch,
            review_state,
            GeometryPolicy::TextShape,
            Some(plan_identity_sha256.to_owned()),
        )
    }

    /// Select a previously committed text-shaped patch, preserving every
    /// revision for subsequent redo. The caller supplies the opaque content
    /// revision from a saved region snapshot (or an old plan identity).
    pub fn restore_text_shape_revision(
        &mut self,
        region_id: &str,
        revision_or_plan_identity: &str,
    ) -> Result<bool, StoreError> {
        let mut revisions = self
            .project
            .patches
            .iter()
            .chain(self.project.text_shape_patch_revisions.iter());
        let exact = revisions.clone().find(|record| {
            record.id == region_id
                && record.text_shape_revision_id().as_deref() == Some(revision_or_plan_identity)
        });
        let Some(selected) = exact
            .or_else(|| {
                revisions.find(|record| {
                    record.id == region_id
                        && record.geometry_policy == GeometryPolicy::TextShape
                        && record.text_shape_plan_identity.as_deref()
                            == Some(revision_or_plan_identity)
                })
            })
            .cloned()
        else {
            return Ok(false);
        };
        let identity = selected
            .text_shape_plan_identity
            .as_deref()
            .ok_or(StoreError::StalePlan)?;
        let (_, prepared) = self
            .load_text_shape_plan_identity(region_id, identity)?
            .ok_or(StoreError::StalePlan)?;
        let selected_patch = self.load_patch(&selected)?;
        if selected_patch.mask.bounds != prepared.write_support.bounds
            || selected_patch.mask.bits != prepared.write_support.bits
        {
            return Err(StoreError::StalePlan);
        }
        let Some(current) = self
            .project
            .patches
            .iter_mut()
            .find(|record| record.id == region_id)
        else {
            return Ok(false);
        };
        if *current != selected {
            *current = selected;
            self.flush()?;
        }
        Ok(true)
    }

    pub fn restore_legacy_revision(
        &mut self,
        region_id: &str,
        revision: &str,
    ) -> Result<bool, StoreError> {
        let selected = self.project.patches.iter()
            .chain(self.project.legacy_patch_revisions.iter())
            .find(|row| row.id == region_id && row.legacy_revision_id().as_deref() == Some(revision))
            .cloned();
        let Some(selected) = selected else { return Ok(false) };
        let patch = self.load_patch(&selected)?;
        buffers::write_atomic(
            &self.dir.join(format!("{region_id}.ink")),
            &buffers::encode_mask(&patch.ink),
        )?;
        let Some(current) = self.project.patches.iter_mut().find(|row| row.id == region_id) else {
            return Ok(false);
        };
        if *current != selected {
            *current = selected;
            self.flush()?;
        }
        Ok(true)
    }

    fn write_plan_mask(
        &self,
        reference: &str,
        raster: &MaskRaster,
        kind: buffers::TextShapeMaskKind,
    ) -> Result<(), StoreError> {
        let path = self.plan_artifact_path(reference)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| StoreError::io(parent, error))?;
        }
        let bytes = buffers::encode_text_shape_mask_parts(raster.bounds, &raster.bits, kind)?;
        buffers::write_atomic(&path, &bytes)
    }

    fn read_plan_mask(
        &self,
        reference: &str,
        kind: buffers::TextShapeMaskKind,
    ) -> Result<MaskRaster, StoreError> {
        let path = self.plan_artifact_path(reference)?;
        let bytes = std::fs::read(&path).map_err(|error| StoreError::io(&path, error))?;
        Ok(buffers::decode_text_shape_mask(&bytes, kind)?.into())
    }

    fn plan_artifact_path(&self, reference: &str) -> Result<PathBuf, StoreError> {
        let relative = Path::new(reference);
        if relative.is_absolute()
            || relative.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            })
        {
            return Err(StoreError::Malformed(
                "unsafe text-shape artifact reference".into(),
            ));
        }
        Ok(self.dir.join(relative))
    }

    fn load_text_shape_plan_record(
        &self,
        record: &TextShapePlanRecord,
    ) -> Result<(MaskPlan, PreparedMaskPlan), StoreError> {
        let source = self
            .project
            .sources
            .get(record.source_idx)
            .ok_or(StoreError::StalePlan)?;
        if record.geometry_policy != GeometryPolicy::TextShape
            || record.region_id != record.identity.region_id
            || source.sha256 != record.source_sha256
            || record.source_sha256 != record.identity.source_sha256
            || record.lower_composite_sha256 != record.identity.lower_composite_sha256
        {
            return Err(StoreError::StalePlan);
        }
        let plan = MaskPlan {
            version: record.plan_version,
            region_id: record.region_id.clone(),
            source_sha256: record.source_sha256.clone(),
            lower_composite_sha256: record.lower_composite_sha256.clone(),
            algorithm_id: record.algorithm_id.clone(),
            model_id: record.model_id.clone(),
            candidate_bounds: record.candidate_bounds,
            refinement_crop: record.refinement_crop,
            base_revision: record.base_revision,
            correction_revision: record.correction_revision,
            plan_revision: record.plan_revision,
            base_mask: self
                .read_plan_mask(&record.base_mask_ref, buffers::TextShapeMaskKind::Base)?,
            additions: self
                .read_plan_mask(&record.additions_ref, buffers::TextShapeMaskKind::Additions)?,
            removals: self
                .read_plan_mask(&record.removals_ref, buffers::TextShapeMaskKind::Removals)?,
            padding_px: record.padding_px,
            model_hole_margin_px: record.model_hole_margin_px,
            reading_context: record.reading_context,
            blend_alpha: record
                .blend_alpha_ref
                .as_deref()
                .map(|reference| {
                    self.read_plan_mask(reference, buffers::TextShapeMaskKind::BlendAlpha)
                })
                .transpose()?,
            quality: record.quality.clone(),
        };
        let prepared = plan
            .prepare(source.w, source.h)
            .map_err(|error| StoreError::Malformed(error.to_string()))?;
        let persisted_support = self.read_plan_mask(
            &record.write_support_ref,
            buffers::TextShapeMaskKind::Support,
        )?;
        let persisted_hole = self.read_plan_mask(
            &record.model_hole_ref,
            buffers::TextShapeMaskKind::ModelHole,
        )?;
        if prepared.identity != record.identity
            || prepared.write_support != persisted_support
            || prepared.model_hole != persisted_hole
            || crate::text_shape::support_sha256(&persisted_support)
                != record.identity.support_sha256
        {
            return Err(StoreError::StalePlan);
        }
        prepared
            .verify_against(&plan, source.w, source.h)
            .map_err(|_| StoreError::StalePlan)?;
        Ok((plan, prepared))
    }

    #[cfg(test)]
    fn plan_fixture(&self, region_id: &str, plan_revision: u64, padding_px: u32) -> MaskPlan {
        let source = &self.project.sources[0];
        let base_bounds = Rect::new(10, 10, 3, 2);
        let mut base = crate::mask::Mask::empty(base_bounds);
        base.set(10, 10, true);
        base.set(12, 11, true);
        let empty = crate::mask::Mask::empty(Rect::new(0, 0, 0, 0));
        MaskPlan {
            version: crate::text_shape::MASK_PLAN_VERSION,
            region_id: region_id.to_owned(),
            source_sha256: source.sha256.clone(),
            lower_composite_sha256: format!("underlay-{plan_revision}"),
            algorithm_id: "synthetic-test".into(),
            model_id: None,
            candidate_bounds: Rect::new(
                6,
                6,
                source.w.saturating_sub(6).min(12),
                source.h.saturating_sub(6).min(12),
            ),
            refinement_crop: Rect::new(
                5,
                5,
                source.w.saturating_sub(5).min(14),
                source.h.saturating_sub(5).min(14),
            ),
            base_revision: 1,
            correction_revision: 1,
            plan_revision,
            base_mask: base.into(),
            additions: empty.clone().into(),
            removals: empty.into(),
            padding_px,
            model_hole_margin_px: 2,
            reading_context: Rect::new(0, 0, source.w, source.h),
            blend_alpha: Some(MaskRaster {
                bounds: base_bounds,
                bits: vec![64, 0, 0, 0, 0, 192],
            }),
            quality: MaskQualityState::Ready,
        }
    }

    #[cfg(test)]
    fn patch_with_support(id: &str, support: &MaskRaster) -> Patch {
        let mut pixels = crate::image::fixtures::by_name("l8").raster;
        pixels.width = support.bounds.w;
        pixels.height = support.bounds.h;
        pixels.data = vec![127; support.bounds.w as usize * support.bounds.h as usize];
        Patch {
            id: id.to_owned(),
            mask: support.to_mask(),
            ink: support.to_mask(),
            pixels,
            order: 3,
            visible: true,
            provenance: Provenance {
                engine: Engine::Fill,
                engine_version: "test".into(),
                model_sha256: None,
                execution_provider: "cpu".into(),
                params_snapshot: serde_json::Value::Null,
                mask_sha256: "0".repeat(64),
                source_sha256: "1".repeat(64),
                cloud: None,
                created: 0,
            },
        }
    }

    /// Record one completed region: its pixels, its mask, its provenance - and
    /// flush.
    ///
    /// §6's "flushed after every completed region" is this method's whole
    /// reason for existing, and the order matters: the buffers are on disk
    /// before the manifest names them, so a crash between the two leaves an
    /// orphaned buffer (swept at launch) rather than a manifest pointing at
    /// nothing.
    pub fn complete_region(
        &mut self,
        source_idx: usize,
        patch: &Patch,
        review_state: Option<String>,
    ) -> Result<(), StoreError> {
        self.complete_region_with_policy(
            source_idx,
            patch,
            review_state,
            GeometryPolicy::Legacy,
            None,
        )
    }

    fn complete_region_with_policy(
        &mut self,
        source_idx: usize,
        patch: &Patch,
        review_state: Option<String>,
        geometry_policy: GeometryPolicy,
        text_shape_plan_identity: Option<String>,
    ) -> Result<(), StoreError> {
        let record = PatchRecord::of(source_idx, patch, review_state);
        let mut record = PatchRecord {
            geometry_policy,
            text_shape_plan_identity,
            ..record
        };
        let mask_bytes = buffers::encode_mask(&patch.mask);
        let ink_bytes = buffers::encode_mask(&patch.ink);
        let pixel_bytes = buffers::encode_patch(&patch.pixels);
        if geometry_policy == GeometryPolicy::TextShape || geometry_policy == GeometryPolicy::Legacy {
            let mut hash = Sha256::new();
            hash.update(b"manga-cleaner/patch-revision/v1\0");
            for bytes in [&mask_bytes, &ink_bytes, &pixel_bytes] {
                hash.update((bytes.len() as u64).to_le_bytes());
                hash.update(bytes);
            }
            hash.update(
                serde_json::to_vec(&record).map_err(|error| StoreError::Json(error.to_string()))?,
            );
            let revision = format!("{:x}", hash.finalize());
            let region_key = sha256_hex(patch.id.as_bytes());
            let stem = format!("patch-revisions/{region_key}/{revision}");
            record.mask_ref = format!("{stem}.mask");
            record.buffer_ref = format!("{stem}.buf");
        }
        for reference in [&record.mask_ref, &record.buffer_ref] {
            let path = self.dir.join(reference);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|error| StoreError::io(parent, error))?;
            }
        }
        buffers::write_atomic(&self.dir.join(&record.mask_ref), &mask_bytes)?;
        buffers::write_atomic(&self.dir.join(record.ink_ref()), &ink_bytes)?;
        buffers::write_atomic(&self.dir.join(&record.buffer_ref), &pixel_bytes)?;

        // A re-run replaces the record rather than appending a second one with
        // the same id - §3 forbids overwriting a prior *provenance* entry, and
        // that history lives in the mask revisions, not in duplicate rows here.
        if let Some(previous) = self
            .project
            .patches
            .iter()
            .find(|existing| existing.id == record.id)
            .cloned()
        {
            if previous.geometry_policy == GeometryPolicy::TextShape
                && previous != record
                && !self.project.text_shape_patch_revisions.contains(&previous)
            {
                self.project
                    .text_shape_patch_revisions
                    .push(previous.clone());
            }
            if previous.geometry_policy == GeometryPolicy::Legacy && previous != record {
                let archived = if previous.legacy_revision_id().is_some() {
                    previous
                } else {
                    // A v3 row used mutable sidecar names. Preserve its bytes
                    // before the next revision can replace those names.
                    let old = self.load_patch(&previous)?;
                    let old_mask = buffers::encode_mask(&old.mask);
                    let old_ink = buffers::encode_mask(&old.ink);
                    let old_pixels = buffers::encode_patch(&old.pixels);
                    let mut hash = Sha256::new();
                    hash.update(b"manga-cleaner/patch-revision/v1\0");
                    for bytes in [&old_mask, &old_ink, &old_pixels] {
                        hash.update((bytes.len() as u64).to_le_bytes());
                        hash.update(bytes);
                    }
                    hash.update(serde_json::to_vec(&previous).map_err(|error| StoreError::Json(error.to_string()))?);
                    let stem = format!("patch-revisions/{}/{:x}", sha256_hex(previous.id.as_bytes()), hash.finalize());
                    let mut archived = previous;
                    archived.mask_ref = format!("{stem}.mask");
                    archived.buffer_ref = format!("{stem}.buf");
                    let parent = self.dir.join("patch-revisions").join(sha256_hex(archived.id.as_bytes()));
                    std::fs::create_dir_all(&parent).map_err(|error| StoreError::io(&parent, error))?;
                    buffers::write_atomic(&self.dir.join(&archived.mask_ref), &old_mask)?;
                    buffers::write_atomic(&self.dir.join(archived.ink_ref()), &old_ink)?;
                    buffers::write_atomic(&self.dir.join(&archived.buffer_ref), &old_pixels)?;
                    archived
                };
                if !self.project.legacy_patch_revisions.contains(&archived) {
                    self.project.legacy_patch_revisions.push(archived);
                }
            }
        }
        if geometry_policy == GeometryPolicy::TextShape
            && !self.project.text_shape_patch_revisions.contains(&record)
        {
            self.project.text_shape_patch_revisions.push(record.clone());
        }
        if geometry_policy == GeometryPolicy::Legacy
            && !self.project.legacy_patch_revisions.contains(&record)
        {
            self.project.legacy_patch_revisions.push(record.clone());
        }
        if geometry_policy == GeometryPolicy::Legacy {
            // The v3 reader derives <id>.ink even for an unfamiliar buffer
            // reference. Keep that compatibility sidecar at the active ink.
            buffers::write_atomic(&self.dir.join(format!("{}.ink", record.id)), &ink_bytes)?;
        }
        match self
            .project
            .patches
            .iter_mut()
            .find(|existing| existing.id == record.id)
        {
            Some(existing) => *existing = record,
            None => self.project.patches.push(record),
        }
        if geometry_policy == GeometryPolicy::TextShape {
            self.project.version = FORMAT_VERSION;
        }
        self.flush()
    }

    /// Record a region that was deliberately left alone, and flush.
    pub fn leave_untouched(
        &mut self,
        source_idx: usize,
        bbox: Rect,
        reason: &str,
    ) -> Result<(), StoreError> {
        self.project.regions_untouched.push(RegionUntouched {
            source_idx,
            bbox,
            reason: reason.to_owned(),
        });
        self.flush()
    }

    /// Record that the detector has looked at this source, and flush.
    ///
    /// Idempotent, because a page may be run more than once - a re-run over a
    /// chapter re-examines a page whose regions were all held back - and a
    /// list that grew on every pass would make `examined.len()` meaningless.
    ///
    /// Called *after* the page's regions are recorded, for the reason §6 gives
    /// for buffers before manifest: a crash between the two re-examines one
    /// page, which is work repeated, rather than skipping it, which is work
    /// lost.
    pub fn mark_examined(&mut self, source_idx: usize) -> Result<(), StoreError> {
        if self.project.examined.contains(&source_idx) {
            return Ok(());
        }
        self.project.examined.push(source_idx);
        self.flush()
    }

    /// Record that a run failed on this source, and flush.
    ///
    /// Idempotent for the reason [`mark_examined`] is, and with a sharper edge:
    /// the page is deliberately left unexamined so a later run tries it again,
    /// so without this the counter would be incremented once per attempt rather
    /// than once per failing page. `counters.errored` moves with the list and
    /// only with it.
    ///
    /// [`mark_examined`]: Job::mark_examined
    pub fn mark_errored(&mut self, source_idx: usize) -> Result<(), StoreError> {
        if self.project.errored.contains(&source_idx) {
            return Ok(());
        }
        self.project.errored.push(source_idx);
        self.project.counters.errored += 1;
        self.flush()
    }

    /// Take back a page's errored state, for a run that is about to re-attempt
    /// it. The counter follows the list down as well as up.
    ///
    /// Does not flush: the caller is `reset_page`, which drops a page's other
    /// records in the same breath and flushes once for all of them.
    pub fn clear_errored(&mut self, source_idx: usize) -> bool {
        if !self.project.errored.contains(&source_idx) {
            return false;
        }
        self.project.errored.retain(|index| *index != source_idx);
        self.project.counters.errored = self.project.counters.errored.saturating_sub(1);
        true
    }

    /// Record that this job stopped part-way, or that it did not - and flush.
    ///
    /// `page_index` is a position in `strip.order`, which is what
    /// `nextPageIndex` carries and
    /// what a resume picks up from. Passing `None` is how a completed run
    /// clears the offer, and it has to be the same call: a job that records
    /// being interrupted and has no way to say it no longer is would offer a
    /// resume for ever.
    pub fn mark_interrupted(&mut self, page_index: Option<u32>) -> Result<(), StoreError> {
        self.project.interrupted_at = page_index;
        self.flush()
    }

    /// Read one recorded patch back.
    ///
    /// A patch written before the ink file existed has none, and its absence
    /// is answered with the applied mask rather than an error: that is what
    /// such a patch's mask file used to say, and a re-run writes the narrower
    /// one. Any other failure to read it is the error it is.
    pub fn load_patch(&self, record: &PatchRecord) -> Result<Patch, StoreError> {
        let mask_path = self.dir.join(&record.mask_ref);
        let ink_path = self.dir.join(record.ink_ref());
        let buffer_path = self.dir.join(&record.buffer_ref);
        let mask = buffers::decode_mask(
            &std::fs::read(&mask_path).map_err(|e| StoreError::io(&mask_path, e))?,
        )?;
        if record.geometry_policy == GeometryPolicy::TextShape {
            let identity = record.text_shape_plan_identity.as_deref().ok_or(StoreError::StalePlan)?;
            let (_, prepared) = self
                .load_text_shape_plan_identity(&record.id, identity)?
                .ok_or(StoreError::StalePlan)?;
            if mask.bounds != prepared.write_support.bounds || mask.bits != prepared.write_support.bits {
                return Err(StoreError::StalePlan);
            }
        }
        let ink = match std::fs::read(&ink_path) {
            Ok(bytes) => buffers::decode_mask(&bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => mask.clone(),
            Err(e) => return Err(StoreError::io(&ink_path, e)),
        };
        let pixels = buffers::decode_patch(
            &std::fs::read(&buffer_path).map_err(|e| StoreError::io(&buffer_path, e))?,
        )?;
        Ok(Patch {
            id: record.id.clone(),
            mask,
            ink,
            pixels,
            order: record.order,
            visible: record.visible,
            provenance: record.provenance.clone(),
        })
    }

    /// Re-verify every source. §6, and the reason it is a hash and not an
    /// mtime: an mtime moves when nothing changed - a copy, a sync, a restore
    /// from backup - and stays put when a file is rewritten in place with the
    /// timestamp preserved. It is recorded because it is cheap and sometimes
    /// explanatory; it never decides.
    ///
    /// **What it can catch changed when ingest began importing pages.** It
    /// hashes the job's own copy, so it no longer notices a user re-cropping
    /// the scan they ingested from - that scan is not this chapter's page any
    /// more, and a re-crop is a new chapter. What it still catches is the
    /// failure §6 wrote it for: a
    /// page that is truncated, corrupted or gone underneath a job that is about
    /// to resume onto it, which is now the library's own file and so squarely
    /// this application's problem to report.
    pub fn verify_sources(&self) -> Vec<SourceState> {
        self.project
            .sources
            .iter()
            .enumerate()
            .map(|(index, source)| {
                let Some(path) = self.source_path(index) else {
                    return SourceState::Missing;
                };
                match std::fs::read(&path) {
                    Err(_) => SourceState::Missing,
                    Ok(bytes) => {
                        let sha256 = sha256_hex(&bytes);
                        if sha256 == source.sha256 {
                            SourceState::Unchanged
                        } else {
                            SourceState::Changed { sha256 }
                        }
                    }
                }
            })
            .collect()
    }

    /// Drop the patches of every stale source, and flush.
    ///
    /// §6: "On mismatch the page is marked stale, its patches are dropped, and
    /// the user is told." The last clause is why this returns a report instead
    /// of a count - a page that silently lost its edits is the failure the rule
    /// exists to prevent, and a caller cannot name the pages it was not given.
    pub fn drop_stale(&mut self, states: &[SourceState]) -> Result<StaleReport, StoreError> {
        let mut report = StaleReport::default();
        for (index, state) in states.iter().enumerate() {
            if state.is_stale() {
                report.sources.push(index);
            }
        }
        if report.is_empty() {
            return Ok(report);
        }

        let stale = &report.sources;
        for record in &self.project.patches {
            if stale.contains(&record.source_idx) {
                report.dropped_patches.push(record.id.clone());
            }
        }
        self.project
            .patches
            .retain(|record| !stale.contains(&record.source_idx));
        self.project
            .text_shape_plans
            .retain(|record| !stale.contains(&record.source_idx));
        self.project
            .text_shape_patch_revisions
            .retain(|record| !stale.contains(&record.source_idx));
        self.project
            .legacy_patch_revisions
            .retain(|record| !stale.contains(&record.source_idx));
        self.project
            .text_shape_corrections
            .retain(|record| !stale.contains(&record.source_idx));
        self.project
            .regions_untouched
            .retain(|region| !stale.contains(&region.source_idx));
        // A source that changed has not been examined in its present form. The
        // patches go, and the record of having looked has to go with them, or a
        // re-crop would leave the page permanently out of the queue with
        // nothing on it.
        self.project.examined.retain(|index| !stale.contains(index));
        // And the record of having failed on it, with the counter it keeps in
        // step: a re-cropped page has not failed in its present form either.
        let before = self.project.errored.len();
        self.project.errored.retain(|index| !stale.contains(index));
        let dropped = (before - self.project.errored.len()) as u32;
        self.project.counters.errored = self.project.counters.errored.saturating_sub(dropped);
        self.flush()?;
        Ok(report)
    }

    /// Whether writing the chapter's output into `destination` would touch a
    /// source. §1's rule, checked before anything is written.
    pub fn output_refusal(&self, destination: &Path) -> Option<OutputRefusal> {
        let root = self.root();
        for (index, source) in self.project.sources.iter().enumerate() {
            // Both files, where a page has two. A converted page's input is
            // the JPEG the user still has, and an export that landed on top of
            // it would be output overwriting input however well the PNG beside
            // it was protected.
            let paths = [Some(&source.rel_path), source.converted_from.as_ref()];
            for path in paths.into_iter().flatten().map(|rel| root.join(rel)) {
                let Some(parent) = path.parent() else {
                    continue;
                };
                if same_directory(parent, destination) {
                    return Some(OutputRefusal::SourceDirectory { source_idx: index });
                }
                if let Some(name) = path.file_name() {
                    if same_directory(&destination.join(name), &path) {
                        return Some(OutputRefusal::WouldOverwriteSource { source_idx: index });
                    }
                }
            }
        }
        None
    }
}

/// §1's default: a sibling `<input>_cleaned/`.
pub fn default_output_dir(input_dir: &Path) -> PathBuf {
    let name = input_dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned());
    match (input_dir.parent(), name) {
        (Some(parent), Some(name)) if !name.is_empty() => parent.join(format!("{name}_cleaned")),
        // A path with no name to extend - a bare root, or the empty path. Put
        // the output under it rather than beside it, which is still not the
        // source directory.
        _ => input_dir.join("cleaned"),
    }
}

/// `<job>.mtclean` → `<job>.mtclean.d`.
///
/// Public because the sidecar is where everything a job owns that is not the
/// manifest lives - masks, patch pixels, and the undo journal - and a
/// caller that only wants the directory should not have to parse the
/// manifest to learn where it is. [`Job::sidecar`] is the same answer for a
/// job already open.
pub fn sidecar_dir(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".d");
    PathBuf::from(name)
}

/// Compare two paths without requiring either to exist.
///
/// `canonicalize` would be stronger - it resolves symlinks and `..` - but it
/// fails on a destination directory the user has just named and not yet
/// created, which is the common case for an export. So: canonical form where
/// both sides exist, lexical form where they do not.
fn same_directory(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => normalise(a) == normalise(b),
    }
}

fn normalise(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `target` expressed relative to `base`, or `target` itself when no relative
/// form exists - a different volume, or a base that is not a prefix and cannot
/// be reached by `..` from a relative path.
fn relative_to(base: &Path, target: &Path) -> PathBuf {
    let (base, target) = (normalise(base), normalise(target));
    if base.is_absolute() != target.is_absolute() {
        return target;
    }

    let mut base_parts = base.components().peekable();
    let mut target_parts = target.components().peekable();
    while base_parts.peek().is_some() && base_parts.peek() == target_parts.peek() {
        base_parts.next();
        target_parts.next();
    }

    let mut out = PathBuf::new();
    for _ in base_parts {
        out.push("..");
    }
    for part in target_parts {
        out.push(part.as_os_str());
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

/// A path's last component, or the whole path where it has none.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn mtime_of(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_secs())
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod v3_manifest;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::{decode, encode, fixtures, Format};
    use crate::mask::Mask;
    use crate::patch::Engine;

    /// A scratch directory that removes itself. `std::env::temp_dir` plus a
    /// counter rather than a crate: the tests need a unique path, not a
    /// dependency.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            use std::sync::atomic::{AtomicU32, Ordering};
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir()
                .join("cleaner-core-project")
                .join(format!("{name}-{}", NEXT.fetch_add(1, Ordering::Relaxed)));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch directory");
            Scratch(dir)
        }

        fn join(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write_source(dir: &Path, name: &str, fixture: &str) -> SourceRef {
        std::fs::create_dir_all(dir).unwrap();
        let bytes = encode(&fixtures::by_name(fixture).raster, Format::Png).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, &bytes).unwrap();
        crate::ingest::source_ref(&path, &bytes).unwrap()
    }

    fn provenance() -> Provenance {
        Provenance {
            engine: Engine::Fill,
            engine_version: "test".into(),
            model_sha256: None,
            execution_provider: "cpu".into(),
            params_snapshot: serde_json::json!({ "thickness": 9 }),
            mask_sha256: "0".repeat(64),
            source_sha256: "1".repeat(64),
            cloud: None,
            created: 1_760_000_000,
        }
    }

    fn a_patch(id: &str) -> Patch {
        let bounds = Rect::new(8, 8, 12, 10);
        let page = fixtures::by_name("l8").raster;
        let mut pixels = page.clone();
        pixels.width = bounds.w;
        pixels.height = bounds.h;
        pixels.data = vec![200; (bounds.w * bounds.h) as usize];
        Patch {
            id: id.into(),
            mask: Mask::filled(bounds),
            ink: Mask::filled(bounds),
            pixels,
            order: 3,
            visible: true,
            provenance: provenance(),
        }
    }

    fn a_job(scratch: &Scratch) -> Job {
        let source = write_source(&scratch.join("raws"), "001.png", "l8");
        let manifest = scratch.join("out/chapter.mtclean");
        std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        let project = Project::new(
            manifest.parent().unwrap(),
            "0.1.0",
            StripMode::Single,
            &[source],
        );
        Job::create(&manifest, project).unwrap()
    }

    #[test]
    fn flushing_legacy_v3_does_not_add_empty_text_shape_plans() {
        let scratch = Scratch::new("v3-no-empty-plans");
        let job = a_job(&scratch);
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(job.path()).unwrap()).unwrap();
        manifest.as_object_mut().unwrap().remove("text_shape_plans");
        std::fs::write(job.path(), serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();

        let reopened = Job::open(job.path()).unwrap();
        reopened.flush().unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(job.path()).unwrap()).unwrap();
        assert_eq!(saved["version"], 3);
        assert!(saved.get("text_shape_plans").is_none());
    }

    #[test]
    fn flushing_legacy_v3_with_patches_preserves_manifest_bytes() {
        let fixture = include_bytes!("fixtures/legacy_v3_patches.json");
        let old: v3_manifest::Project = serde_json::from_slice(fixture).unwrap();
        assert_eq!(old.patches.len(), 4);
        assert_eq!(serde_json::to_vec_pretty(&old).unwrap(), fixture);

        let scratch = Scratch::new("v3-patches-byte-for-byte");
        let path = scratch.join("chapter.mtclean");
        std::fs::write(&path, fixture).unwrap();
        let job = Job::open(&path).unwrap();
        assert_eq!(job.project.version, 3);
        job.flush().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), fixture);
    }

    #[test]
    fn legacy_edits_keep_v3_and_text_shape_plan_upgrades_to_v4() {
        let scratch = Scratch::new("v3-legacy-roundtrip");
        let mut job = a_job(&scratch);
        let path = job.path().to_path_buf();
        let mut raw: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        raw["version"] = serde_json::json!(3);
        std::fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
        job = Job::open(&path).unwrap();
        job.complete_region(0, &a_patch("legacy"), None).unwrap();
        let old_revision = job.project.patches[0].legacy_revision_id().unwrap();
        let mut retry = a_patch("legacy");
        retry.pixels.data.fill(91);
        job.complete_region(0, &retry, None).unwrap();
        assert!(job.restore_legacy_revision("legacy", &old_revision).unwrap());
        let record = &mut job.project.patches[0];
        record.review_state = Some("review.reason.inputChanged".into());
        record.provenance.params_snapshot["input_provenance"] = serde_json::json!({
            "read_footprint": Rect::new(8, 8, 12, 10),
            "input_sha256": "earlier-composite"
        });
        record.provenance.params_snapshot["review_before_input_change"] = serde_json::Value::Null;
        job.flush().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let old: v3_manifest::Project = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(old.version, 3);
        assert_eq!(old.sources.len(), 1);
        assert_eq!(old.strip.order, vec![0]);
        assert_eq!(old.patches.len(), 1);
        let row = &old.patches[0];
        assert_eq!(row.id, "legacy");
        assert!(!row.mask_ref.is_empty() && !row.buffer_ref.is_empty());
        assert_eq!(row.engine, Engine::Fill);
        assert_eq!(row.order, 3);
        assert!(row.visible);
        assert_eq!(row.provenance.engine, Engine::Fill);
        assert_eq!(row.review_state.as_deref(), Some("review.reason.inputChanged"));
        assert_eq!(row.provenance.params_snapshot["input_provenance"]["input_sha256"], "earlier-composite");
        let saved: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(saved["legacy_patch_revisions"].as_array().unwrap().len(), 2);
        assert!(saved["patches"][0].get("geometry_policy").is_none());

        let plan = job.plan_fixture("shape", 1, 0);
        let prepared = plan.prepare(job.project.sources[0].w, job.project.sources[0].h).unwrap();
        job.store_text_shape_plan(0, &plan, &prepared).unwrap();
        let upgraded: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(upgraded["version"], 4);
        job.project.text_shape_plans.clear();
        job.flush().unwrap();
        let rolled_back: serde_json::Value = serde_json::from_slice(&std::fs::read(job.path()).unwrap()).unwrap();
        assert_eq!(rolled_back["version"], 3);
    }

    #[test]
    fn legacy_retry_restores_exact_pixels_after_reopen() {
        let scratch = Scratch::new("legacy-retry-revisions");
        let mut job = a_job(&scratch);
        let mut patch = a_patch("legacy");
        patch.pixels.data.fill(23);
        job.complete_region(0, &patch, None).unwrap();
        let first = job.project.patches[0].legacy_revision_id().unwrap();
        patch.pixels.data.fill(91);
        job.complete_region(0, &patch, None).unwrap();
        let second = job.project.patches[0].legacy_revision_id().unwrap();
        assert_ne!(first, second);
        let mut job = Job::open(job.path()).unwrap();
        assert!(job.restore_legacy_revision("legacy", &first).unwrap());
        assert!(job.load_patch(&job.project.patches[0]).unwrap().pixels.data.iter().all(|&p| p == 23));
        job = Job::open(job.path()).unwrap();
        assert!(job.restore_legacy_revision("legacy", &second).unwrap());
        assert!(job.load_patch(&job.project.patches[0]).unwrap().pixels.data.iter().all(|&p| p == 91));
    }

    #[test]
    fn old_legacy_sidecars_are_archived_before_retry() {
        let scratch = Scratch::new("legacy-pre-archive-retry");
        let mut job = a_job(&scratch);
        let mut patch = a_patch("legacy");
        patch.pixels.data.fill(7);
        let old = PatchRecord::of(0, &patch, None);
        buffers::write_atomic(&job.sidecar().join(&old.mask_ref), &buffers::encode_mask(&patch.mask)).unwrap();
        buffers::write_atomic(&job.sidecar().join(old.ink_ref()), &buffers::encode_mask(&patch.ink)).unwrap();
        buffers::write_atomic(&job.sidecar().join(&old.buffer_ref), &buffers::encode_patch(&patch.pixels)).unwrap();
        job.project.patches.push(old);
        job.flush().unwrap();

        patch.pixels.data.fill(99);
        job.complete_region(0, &patch, None).unwrap();
        let old_revision = job.project.legacy_patch_revisions.iter()
            .find(|row| row.buffer_ref != job.project.patches[0].buffer_ref)
            .unwrap().legacy_revision_id().unwrap();
        let new_revision = job.project.patches[0].legacy_revision_id().unwrap();
        let mut reopened = Job::open(job.path()).unwrap();
        assert!(reopened.restore_legacy_revision("legacy", &old_revision).unwrap());
        assert!(reopened.load_patch(&reopened.project.patches[0]).unwrap().pixels.data.iter().all(|&p| p == 7));
        assert!(reopened.restore_legacy_revision("legacy", &new_revision).unwrap());
        assert!(reopened.load_patch(&reopened.project.patches[0]).unwrap().pixels.data.iter().all(|&p| p == 99));
    }

    #[test]
    fn tampered_text_shape_mask_is_rejected_on_patch_load() {
        let scratch = Scratch::new("text-shape-tampered-mask");
        let mut job = a_job(&scratch);
        let plan = job.plan_fixture("shape", 1, 0);
        let prepared = plan.prepare(job.project.sources[0].w, job.project.sources[0].h).unwrap();
        job.store_text_shape_plan(0, &plan, &prepared).unwrap();
        let patch = Job::patch_with_support("shape", &prepared.write_support);
        job.complete_text_shape_region(0, &patch, &prepared.identity.identity_sha256, None).unwrap();
        let row = &job.project.patches[0];
        let mut mask = job.load_patch(row).unwrap().mask;
        let bounds = mask.bounds;
        let point = (bounds.y..bounds.bottom())
            .flat_map(|y| (bounds.x..bounds.right()).map(move |x| (x, y)))
            .find(|&(x, y)| !mask.contains(x, y))
            .unwrap();
        mask.set(point.0, point.1, true);
        std::fs::write(job.sidecar().join(&row.mask_ref), buffers::encode_mask(&mask)).unwrap();
        assert!(matches!(job.load_patch(row), Err(StoreError::StalePlan)));
    }

    #[test]
    fn a_manifest_round_trips_through_the_file_on_disk() {
        let scratch = Scratch::new("round-trip");
        let mut job = a_job(&scratch);
        job.project.settings = serde_json::json!({ "engineCeiling": "lama", "cloud": false });
        job.project.counters.declined = 2;
        job.complete_region(0, &a_patch("r0"), Some("review.reason.declined".into()))
            .unwrap();
        job.leave_untouched(
            0,
            Rect::new(4, 4, 6, 6),
            "review.reason.gateSkippedLowConfidence",
        )
        .unwrap();

        let reopened = Job::open(job.path()).unwrap();
        assert_eq!(reopened.project, job.project);
        assert_eq!(reopened.project.patches[0].engine, Engine::Fill);
        assert_eq!(
            reopened.project.patches[0].provenance.params_snapshot,
            serde_json::json!({ "thickness": 9 }),
            "params_snapshot came back double-encoded"
        );
    }

    #[test]
    fn text_shape_plan_and_exact_support_survive_reopen_and_patch_commit() {
        let scratch = Scratch::new("text-shape-plan-roundtrip");
        let mut job = a_job(&scratch);
        let plan = job.plan_fixture("region-text", 1, 2);
        let prepared = plan
            .prepare(job.project.sources[0].w, job.project.sources[0].h)
            .unwrap();
        let support_hash = prepared.identity.support_sha256.clone();
        let identity_hash = prepared.identity.identity_sha256.clone();
        job.store_text_shape_plan(0, &plan, &prepared).unwrap();

        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(job.path()).unwrap()).unwrap();
        assert_eq!(manifest["version"], FORMAT_VERSION);
        assert_eq!(manifest["text_shape_plans"].as_array().unwrap().len(), 1);
        assert!(
            manifest.to_string().len() < 30_000,
            "mask pixels must be stored in sidecar artifacts"
        );

        let mut reopened = Job::open(job.path()).unwrap();
        let (loaded_plan, loaded_prepared) = reopened
            .load_text_shape_plan("region-text")
            .unwrap()
            .expect("preview plan was persisted before apply");
        assert_eq!(loaded_plan, plan);
        assert_eq!(loaded_prepared, prepared);
        let validated = reopened
            .validate_text_shape_plan(
                "region-text",
                &plan.source_sha256,
                &plan.lower_composite_sha256,
                &identity_hash,
                &support_hash,
            )
            .unwrap();
        assert_eq!(validated.write_support, prepared.write_support);

        let patch = Job::patch_with_support("region-text", &prepared.write_support);
        reopened
            .complete_text_shape_region(0, &patch, &identity_hash, None)
            .unwrap();
        let row = &reopened.project.patches[0];
        assert_eq!(row.geometry_policy, GeometryPolicy::TextShape);
        assert_eq!(
            row.provenance.params_snapshot["geometry_policy"],
            "text_shape"
        );
        assert_eq!(
            row.provenance.params_snapshot["write_support_sha256"],
            support_hash
        );
        assert_eq!(
            row.text_shape_plan_identity.as_deref(),
            Some(identity_hash.as_str())
        );
        let saved_patch = reopened.load_patch(row).unwrap();
        assert_eq!(saved_patch.mask, prepared.write_support.to_mask());
    }

    #[test]
    fn text_shape_revisions_are_retained_and_stale_preview_values_are_rejected() {
        let scratch = Scratch::new("text-shape-plan-revisions");
        let mut job = a_job(&scratch);
        let first = job.plan_fixture("stable-region", 1, 2);
        let first_prepared = first
            .prepare(job.project.sources[0].w, job.project.sources[0].h)
            .unwrap();
        job.store_text_shape_plan(0, &first, &first_prepared)
            .unwrap();
        let first_patch = Job::patch_with_support("stable-region", &first_prepared.write_support);
        job.complete_text_shape_region(
            0,
            &first_patch,
            &first_prepared.identity.identity_sha256,
            None,
        )
        .unwrap();
        let second = job.plan_fixture("stable-region", 2, 5);
        let second_prepared = second
            .prepare(job.project.sources[0].w, job.project.sources[0].h)
            .unwrap();
        job.store_text_shape_plan(0, &second, &second_prepared)
            .unwrap();

        let reopened = Job::open(job.path()).unwrap();
        let latest = reopened
            .load_text_shape_plan("stable-region")
            .unwrap()
            .unwrap();
        assert_eq!(latest.0.plan_revision, 2);
        let old = reopened
            .load_text_shape_plan_identity(
                "stable-region",
                &first_prepared.identity.identity_sha256,
            )
            .unwrap()
            .unwrap();
        assert_eq!(old.1.write_support, first_prepared.write_support);
        assert_eq!(
            reopened.project.patches[0]
                .text_shape_plan_identity
                .as_deref(),
            Some(first_prepared.identity.identity_sha256.as_str()),
            "an applied patch keeps its plan identity across a newer preview"
        );
        assert!(matches!(
            reopened.validate_text_shape_plan(
                "stable-region",
                &first.source_sha256,
                "changed-lower-layer",
                &first_prepared.identity.identity_sha256,
                &first_prepared.identity.support_sha256,
            ),
            Err(StoreError::StalePlan)
        ));
        assert!(matches!(
            reopened.validate_text_shape_plan(
                "stable-region",
                &first.source_sha256,
                &first.lower_composite_sha256,
                &first_prepared.identity.identity_sha256,
                "bad-support-hash",
            ),
            Err(StoreError::StalePlan)
        ));
    }

    #[test]
    fn text_shape_commit_refuses_a_patch_mask_different_from_preview_support() {
        let scratch = Scratch::new("text-shape-plan-mismatch");
        let mut job = a_job(&scratch);
        let plan = job.plan_fixture("region-text", 1, 0);
        let prepared = plan
            .prepare(job.project.sources[0].w, job.project.sources[0].h)
            .unwrap();
        job.store_text_shape_plan(0, &plan, &prepared).unwrap();
        let mut patch = Job::patch_with_support("region-text", &prepared.write_support);
        patch.mask.set(
            patch.mask.bounds.x,
            patch.mask.bounds.y,
            !patch
                .mask
                .contains(patch.mask.bounds.x, patch.mask.bounds.y),
        );
        assert!(matches!(
            job.complete_text_shape_region(0, &patch, &prepared.identity.identity_sha256, None),
            Err(StoreError::StalePlan)
        ));
        assert!(job.project.patches.is_empty());
    }

    #[test]
    fn reusing_plan_identity_cannot_replace_immutable_base_or_corrections() {
        let scratch = Scratch::new("text-shape-plan-identity-collision");
        let mut job = a_job(&scratch);
        let plan = job.plan_fixture("region-text", 1, 2);
        let prepared = plan
            .prepare(job.project.sources[0].w, job.project.sources[0].h)
            .unwrap();
        job.store_text_shape_plan(0, &plan, &prepared).unwrap();

        // Add a base pixel that the unchanged correction removes. The final W
        // and identity are the same, but the immutable inputs are not; a
        // revision-reuse bug must not overwrite the first plan's sidecars.
        let mut alias = plan.clone();
        alias.base_mask.bits[1] = 255;
        alias.removals = MaskRaster {
            bounds: alias.base_mask.bounds,
            bits: vec![0, 255, 0, 0, 0, 0],
        };
        let alias_prepared = alias
            .prepare(job.project.sources[0].w, job.project.sources[0].h)
            .unwrap();
        assert_eq!(alias_prepared.identity, prepared.identity);
        assert!(matches!(
            job.store_text_shape_plan(0, &alias, &alias_prepared),
            Err(StoreError::Malformed(_))
        ));
        let (saved, _) = job
            .load_text_shape_plan_identity("region-text", &prepared.identity.identity_sha256)
            .unwrap()
            .unwrap();
        assert_eq!(saved, plan);
    }

    #[test]
    fn mixed_legacy_and_text_shape_patches_reopen_and_export_the_same_composite() {
        let scratch = Scratch::new("mixed-geometry-export");
        let mut job = a_job(&scratch);
        job.complete_region(0, &a_patch("legacy"), None).unwrap();

        let plan = job.plan_fixture("text-shape", 1, 2);
        let prepared = plan
            .prepare(job.project.sources[0].w, job.project.sources[0].h)
            .unwrap();
        job.store_text_shape_plan(0, &plan, &prepared).unwrap();
        let mut text_patch = Job::patch_with_support("text-shape", &prepared.write_support);
        text_patch.order = 9;
        job.complete_text_shape_region(0, &text_patch, &prepared.identity.identity_sha256, None)
            .unwrap();

        let reopened = Job::open(job.path()).unwrap();
        assert_eq!(reopened.project.patches.len(), 2);
        assert_eq!(
            reopened.project.patches[0].geometry_policy,
            GeometryPolicy::Legacy
        );
        assert_eq!(
            reopened.project.patches[1].geometry_policy,
            GeometryPolicy::TextShape
        );
        let patches: Vec<Patch> = reopened
            .project
            .patches
            .iter()
            .map(|record| reopened.load_patch(record).unwrap())
            .collect();
        let source_bytes = std::fs::read(reopened.source_path(0).unwrap()).unwrap();
        let source_page = decode(&source_bytes).unwrap();
        let preview = crate::composite::composite(&source_page, &patches).unwrap();
        let exported = crate::export::export_page(
            &source_bytes,
            &patches,
            crate::export::Target::SameAsSource,
        )
        .unwrap();
        let flattened = decode(&exported.bytes).unwrap();
        assert_eq!(flattened.data, preview.data);
        assert_eq!(flattened.mode, preview.mode);
        assert_eq!(flattened.depth, preview.depth);
    }

    #[test]
    fn text_shape_saved_patches_preserve_source_samples_mode_depth_and_icc_on_lossless_export() {
        for fixture in fixtures::all() {
            let scratch = Scratch::new(fixture.name);
            let format = if fixture.name == "cmyk8" {
                Format::Tiff
            } else {
                Format::Png
            };
            let source_bytes = encode(&fixture.raster, format).unwrap();
            let source_path = scratch.join(if format == Format::Tiff {
                "source.tiff"
            } else {
                "source.png"
            });
            std::fs::write(&source_path, &source_bytes).unwrap();
            let source = crate::ingest::source_ref(&source_path, &source_bytes).unwrap();
            let manifest = scratch.join("chapter.mtclean");
            let mut job = Job::create(
                &manifest,
                Project::new(
                    manifest.parent().unwrap(),
                    "test",
                    StripMode::Single,
                    &[source],
                ),
            )
            .unwrap();
            let plan = job.plan_fixture("shape", 1, 2);
            let prepared = plan
                .prepare(fixture.raster.width, fixture.raster.height)
                .unwrap();
            job.store_text_shape_plan(0, &plan, &prepared).unwrap();
            let mut patch = Job::patch_with_support("shape", &prepared.write_support);
            let decoded_source = decode(&source_bytes).unwrap();
            patch.pixels = crate::engines::model::page_crop(&decoded_source, patch.mask.bounds);
            let (x, y) = (plan.base_mask.bounds.x, plan.base_mask.bounds.y);
            let (local_x, local_y) = (
                (x - patch.mask.bounds.x) as u32,
                (y - patch.mask.bounds.y) as u32,
            );
            let old = patch.pixels.sample(local_x, local_y, 0);
            patch.pixels.set_sample(local_x, local_y, 0, old ^ 1);
            assert!(patch.mask.contains(x, y));
            job.complete_text_shape_region(0, &patch, &prepared.identity.identity_sha256, None)
                .unwrap();

            let reopened = Job::open(&manifest).unwrap();
            let loaded = reopened.load_patch(&reopened.project.patches[0]).unwrap();
            let preview = crate::composite::composite(&decoded_source, std::slice::from_ref(&loaded)).unwrap();
            let output = crate::export::export_page(
                &source_bytes,
                &[loaded],
                crate::export::Target::SameAsSource,
            )
            .unwrap();
            let after = decode(&output.bytes).unwrap();
            assert_eq!(
                (after.mode, after.depth, &after.icc),
                (
                    decoded_source.mode,
                    decoded_source.depth,
                    &decoded_source.icc
                ),
                "{}",
                fixture.name
            );
            assert_eq!(after.data, preview.data, "{}", fixture.name);
            for y in 0..after.height {
                for x in 0..after.width {
                    if prepared.write_support.contains(x as i64, y as i64) {
                        continue;
                    }
                    for channel in 0..after.mode.samples() {
                        assert_eq!(
                            after.sample(x, y, channel),
                            decoded_source.sample(x, y, channel),
                            "{} ({x}, {y}) channel {channel}",
                            fixture.name
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn text_shape_visibility_undo_redo_keeps_the_prepared_support_identity() {
        let scratch = Scratch::new("text-shape-visibility-undo-redo");
        let mut job = a_job(&scratch);
        let plan = job.plan_fixture("text-shape", 1, 2);
        let prepared = plan
            .prepare(job.project.sources[0].w, job.project.sources[0].h)
            .unwrap();
        job.store_text_shape_plan(0, &plan, &prepared).unwrap();
        let patch = Job::patch_with_support("text-shape", &prepared.write_support);
        job.complete_text_shape_region(0, &patch, &prepared.identity.identity_sha256, None)
            .unwrap();
        let identity = prepared.identity.identity_sha256.clone();

        for visible in [false, true, false, true] {
            job.project.patches[0].visible = visible;
            job.flush().unwrap();
            job = Job::open(job.path()).unwrap();
            let row = &job.project.patches[0];
            assert_eq!(row.visible, visible);
            assert_eq!(row.geometry_policy, GeometryPolicy::TextShape);
            assert_eq!(
                row.text_shape_plan_identity.as_deref(),
                Some(identity.as_str())
            );
            assert_eq!(
                job.load_patch(row).unwrap().mask.bits,
                prepared.write_support.bits
            );
            assert!(job
                .load_text_shape_plan_identity("text-shape", &identity)
                .unwrap()
                .is_some());
        }
    }

    #[test]
    fn successive_text_shape_revisions_restore_exact_patch_bytes_in_both_directions() {
        let scratch = Scratch::new("text-shape-successive-revision-undo");
        let mut job = a_job(&scratch);
        let first = job.plan_fixture("shape", 1, 2);
        let first_plan = first
            .prepare(job.project.sources[0].w, job.project.sources[0].h)
            .unwrap();
        job.store_text_shape_plan(0, &first, &first_plan).unwrap();
        let mut first_patch = Job::patch_with_support("shape", &first_plan.write_support);
        first_patch.pixels.data.fill(21);
        job.complete_text_shape_region(0, &first_patch, &first_plan.identity.identity_sha256, None)
            .unwrap();
        let first_record = job.project.patches[0].clone();
        let first_revision = first_record.text_shape_revision_id().unwrap();

        let second = job.plan_fixture("shape", 2, 5);
        let second_plan = second
            .prepare(job.project.sources[0].w, job.project.sources[0].h)
            .unwrap();
        job.store_text_shape_plan(0, &second, &second_plan).unwrap();
        let mut second_patch = Job::patch_with_support("shape", &second_plan.write_support);
        second_patch.pixels.data.fill(89);
        job.complete_text_shape_region(
            0,
            &second_patch,
            &second_plan.identity.identity_sha256,
            None,
        )
        .unwrap();
        let second_record = job.project.patches[0].clone();
        let second_revision = second_record.text_shape_revision_id().unwrap();
        assert_ne!(first_record.buffer_ref, second_record.buffer_ref);

        // A retry can keep the same plan identity while producing different
        // pixels. Its undo token must still name a distinct patch revision.
        let mut retry_patch = second_patch.clone();
        retry_patch.pixels.data.fill(117);
        job.complete_text_shape_region(
            0,
            &retry_patch,
            &second_plan.identity.identity_sha256,
            None,
        )
        .unwrap();
        let retry_record = job.project.patches[0].clone();
        let retry_revision = retry_record.text_shape_revision_id().unwrap();
        assert_ne!(second_revision, retry_revision);

        let mut reopened = Job::open(job.path()).unwrap();
        assert!(reopened
            .restore_text_shape_revision("shape", &first_revision)
            .unwrap());
        assert_eq!(reopened.project.patches[0], first_record);
        assert_eq!(
            reopened
                .load_patch(&reopened.project.patches[0])
                .unwrap()
                .pixels
                .data,
            first_patch.pixels.data
        );
        reopened = Job::open(job.path()).unwrap();
        assert!(reopened
            .restore_text_shape_revision("shape", &second_revision)
            .unwrap());
        assert_eq!(reopened.project.patches[0], second_record);
        assert_eq!(
            reopened
                .load_patch(&reopened.project.patches[0])
                .unwrap()
                .pixels
                .data,
            second_patch.pixels.data
        );
        reopened = Job::open(job.path()).unwrap();
        assert!(reopened
            .restore_text_shape_revision("shape", &retry_revision)
            .unwrap());
        assert_eq!(reopened.project.patches[0], retry_record);
        assert_eq!(
            reopened
                .load_patch(&reopened.project.patches[0])
                .unwrap()
                .pixels
                .data,
            retry_patch.pixels.data
        );
    }

    #[test]
    fn pre_archive_text_shape_patch_can_be_restored_after_a_new_retry() {
        let scratch = Scratch::new("text-shape-pre-archive-patch");
        let mut job = a_job(&scratch);
        let plan = job.plan_fixture("shape", 1, 2);
        let prepared = plan
            .prepare(job.project.sources[0].w, job.project.sources[0].h)
            .unwrap();
        job.store_text_shape_plan(0, &plan, &prepared).unwrap();
        let mut patch = Job::patch_with_support("shape", &prepared.write_support);
        patch.pixels.data.fill(31);
        job.complete_text_shape_region(0, &patch, &prepared.identity.identity_sha256, None)
            .unwrap();
        // Simulate the uncommitted M3 sidecar naming, which used stable IDs.
        let row = &mut job.project.patches[0];
        for (from, to) in [
            (row.mask_ref.clone(), "shape.mask"),
            (row.ink_ref(), "shape.ink"),
            (row.buffer_ref.clone(), "shape.buf"),
        ] {
            std::fs::copy(job.dir.join(from), job.dir.join(to)).unwrap();
        }
        row.mask_ref = "shape.mask".into();
        row.buffer_ref = "shape.buf".into();
        let legacy_revision = row.text_shape_revision_id().unwrap();
        job.flush().unwrap();

        let mut retry = patch.clone();
        retry.pixels.data.fill(92);
        job.complete_text_shape_region(0, &retry, &prepared.identity.identity_sha256, None)
            .unwrap();
        let mut reopened = Job::open(job.path()).unwrap();
        assert!(reopened
            .restore_text_shape_revision("shape", &legacy_revision)
            .unwrap());
        assert_eq!(
            reopened
                .load_patch(&reopened.project.patches[0])
                .unwrap()
                .pixels
                .data,
            patch.pixels.data
        );
    }

    #[test]
    fn correction_state_survives_reopen_blocks_old_plan_and_clears_after_new_approval() {
        let scratch = Scratch::new("text-shape-correction-state");
        let mut job = a_job(&scratch);
        let first = job.plan_fixture("shape", 1, 2);
        let first_prepared = first
            .prepare(job.project.sources[0].w, job.project.sources[0].h)
            .unwrap();
        job.store_text_shape_plan(0, &first, &first_prepared)
            .unwrap();
        job.record_text_shape_correction(
            0,
            "shape",
            2,
            Rect::new(8, 8, 10, 10),
            "lettering mask is empty",
        )
        .unwrap();
        let mut reopened = Job::open(job.path()).unwrap();
        assert_eq!(
            reopened.text_shape_correction("shape").unwrap().reason,
            "lettering mask is empty"
        );
        let second = reopened.plan_fixture("shape", 2, 5);
        let second_prepared = second
            .prepare(reopened.project.sources[0].w, reopened.project.sources[0].h)
            .unwrap();
        assert!(matches!(
            reopened.store_text_shape_plan(0, &second, &second_prepared),
            Err(StoreError::StalePlan)
        ));
        let third = reopened.plan_fixture("shape", 3, 5);
        let third_prepared = third
            .prepare(reopened.project.sources[0].w, reopened.project.sources[0].h)
            .unwrap();
        reopened
            .store_text_shape_plan(0, &third, &third_prepared)
            .unwrap();
        let reopened = Job::open(job.path()).unwrap();
        assert!(reopened.text_shape_correction("shape").is_none());
        assert_eq!(
            reopened
                .load_text_shape_plan("shape")
                .unwrap()
                .unwrap()
                .0
                .plan_revision,
            3
        );
    }

    /// The manifest is a human-readable record of a job, and §1 fixes the field
    /// names. A rename here is a break for anything reading it.
    #[test]
    fn the_manifest_has_the_field_names_the_document_specifies() {
        let scratch = Scratch::new("field-names");
        let mut job = a_job(&scratch);
        job.complete_region(0, &a_patch("r0"), None).unwrap();
        job.leave_untouched(0, Rect::new(4, 4, 6, 6), "decline.reason.qualityMetric")
            .unwrap();

        let text = std::fs::read_to_string(job.path()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        for key in [
            "version",
            "created",
            "app_version",
            "sources",
            "strip",
            "patches",
            "regions_untouched",
            "counters",
            "settings",
        ] {
            assert!(value.get(key).is_some(), "the manifest has no {key}");
        }
        for key in ["rel_path", "sha256", "mtime", "w", "h", "mode", "bit_depth"] {
            assert!(
                value["sources"][0].get(key).is_some(),
                "a source row has no {key}"
            );
        }
        for key in [
            "id",
            "source_idx",
            "bbox",
            "mask_ref",
            "buffer_ref",
            "engine",
            "order",
            "visible",
            "review_state",
            "provenance",
        ] {
            assert!(
                value["patches"][0].get(key).is_some(),
                "a patch row has no {key}"
            );
        }
        for key in [
            "refused",
            "errored",
            "param_reject",
            "residual_reject",
            "structural_reject",
            "declined",
            "gate_dropped",
        ] {
            assert!(
                value["counters"].get(key).is_some(),
                "the counters have no {key}"
            );
        }
        // The vocabulary a reader expects: mode names and a bit depth that
        // is a number.
        assert_eq!(value["sources"][0]["mode"], "L");
        assert_eq!(value["sources"][0]["bit_depth"], 8);
        assert_eq!(value["patches"][0]["engine"], "fill");
    }

    #[test]
    fn a_completed_region_is_on_disk_before_the_call_returns() {
        let scratch = Scratch::new("flush");
        let mut job = a_job(&scratch);
        let patch = a_patch("r0");
        job.complete_region(0, &patch, None).unwrap();

        // Nothing else is called - no explicit save, no drop. §6's flush is
        // what makes a crash here survivable.
        let reopened = Job::open(job.path()).unwrap();
        assert_eq!(reopened.project.patches.len(), 1);
        let loaded = reopened.load_patch(&reopened.project.patches[0]).unwrap();
        assert_eq!(loaded.mask, patch.mask);
        assert_eq!(loaded.ink, patch.ink);
        assert_eq!(loaded.pixels.data, patch.pixels.data);
    }

    /// A job written before the ink file existed has a `.mask` and a `.buf`
    /// per patch and nothing else. It still opens, and the ink it answers is
    /// the applied mask - what its mask file used to be made of.
    #[test]
    fn a_patch_without_an_ink_file_answers_its_applied_mask_as_ink() {
        let scratch = Scratch::new("no-ink-file");
        let mut job = a_job(&scratch);
        let mut patch = a_patch("r0");
        patch.ink = Mask::filled(Rect::new(10, 10, 4, 3));
        job.complete_region(0, &patch, None).unwrap();
        let record = job.project.patches[0].clone();
        assert!(
            job.sidecar().join(record.ink_ref()).exists(),
            "the ink is a sidecar file"
        );

        std::fs::remove_file(job.sidecar().join(record.ink_ref())).unwrap();
        let loaded = job.load_patch(&record).unwrap();
        assert_eq!(
            loaded.ink, patch.mask,
            "no ink file: the applied mask stands in"
        );
    }

    #[test]
    fn re_running_a_region_replaces_its_record_rather_than_doubling_it() {
        let scratch = Scratch::new("rerun");
        let mut job = a_job(&scratch);
        job.complete_region(0, &a_patch("r0"), None).unwrap();
        let mut second = a_patch("r0");
        second.provenance.engine = Engine::Denoise;
        second.order = 7;
        job.complete_region(0, &second, None).unwrap();

        assert_eq!(job.project.patches.len(), 1);
        assert_eq!(job.project.patches[0].engine, Engine::Denoise);
        assert_eq!(job.project.patches[0].order, 7);
    }

    #[test]
    fn a_manifest_is_never_seen_half_written() {
        // The property temp-and-rename buys: at no point does the manifest path
        // hold anything but a complete file. Approximated here by checking that
        // the write goes through a temp path and that the destination parses
        // after every flush.
        let scratch = Scratch::new("atomic");
        let mut job = a_job(&scratch);
        for index in 0..8 {
            job.complete_region(0, &a_patch(&format!("r{index}")), None)
                .unwrap();
            let bytes = std::fs::read(job.path()).unwrap();
            serde_json::from_slice::<Project>(&bytes).expect("the manifest parsed mid-job");
        }
        let leftovers: Vec<_> = std::fs::read_dir(job.path().parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "a temp file was left behind");
    }

    #[test]
    fn a_resume_hashes_the_sources_and_drops_what_changed() {
        let scratch = Scratch::new("resume");
        let mut job = a_job(&scratch);
        job.complete_region(0, &a_patch("r0"), None).unwrap();
        assert_eq!(job.verify_sources(), vec![SourceState::Unchanged]);

        // Re-crop the page in another application, which is §6's worked
        // example: without this check it composites at stale coordinates and
        // the fidelity test still passes, because it compares against the new
        // source.
        let source = job.source_path(0).unwrap();
        let other = encode(&fixtures::by_name("rgb8").raster, Format::Png).unwrap();
        std::fs::write(&source, other).unwrap();

        let states = job.verify_sources();
        assert!(matches!(states[0], SourceState::Changed { .. }));
        let report = job.drop_stale(&states).unwrap();
        assert_eq!(report.sources, vec![0]);
        assert_eq!(report.dropped_patches, vec!["r0".to_string()]);
        assert!(job.project.patches.is_empty());
        assert!(
            Job::open(job.path()).unwrap().project.patches.is_empty(),
            "the drop was not flushed"
        );
    }

    #[test]
    fn a_deleted_source_is_missing_rather_than_changed() {
        let scratch = Scratch::new("missing");
        let mut job = a_job(&scratch);
        job.complete_region(0, &a_patch("r0"), None).unwrap();
        std::fs::remove_file(job.source_path(0).unwrap()).unwrap();

        let states = job.verify_sources();
        assert_eq!(states, vec![SourceState::Missing]);
        assert_eq!(
            job.drop_stale(&states).unwrap().dropped_patches,
            vec!["r0".to_string()]
        );
    }

    /// An untouched mtime is not evidence of an untouched file, and a moved one
    /// is not evidence of a changed one. The hash decides.
    #[test]
    fn an_mtime_alone_does_not_decide_staleness() {
        let scratch = Scratch::new("mtime");
        let job = a_job(&scratch);
        let source = job.source_path(0).unwrap();
        let bytes = std::fs::read(&source).unwrap();
        std::fs::remove_file(&source).unwrap();
        std::fs::write(&source, &bytes).unwrap();
        assert_eq!(
            job.verify_sources(),
            vec![SourceState::Unchanged],
            "a new mtime read as a change"
        );
    }

    #[test]
    fn the_source_folder_is_refused_as_a_destination() {
        let scratch = Scratch::new("refuse");
        let job = a_job(&scratch);
        let raws = scratch.join("raws");
        assert!(matches!(
            job.output_refusal(&raws),
            Some(OutputRefusal::SourceDirectory { .. })
        ));
        assert_eq!(job.output_refusal(&scratch.join("raws_cleaned")), None);
        // Reached the long way round, which the lexical comparison alone would
        // miss.
        assert!(job.output_refusal(&scratch.join("out/../raws")).is_some());
    }

    #[test]
    fn a_file_whose_bytes_are_a_source_is_recognised_wherever_it_sits() {
        let scratch = Scratch::new("hash-refuse");
        let job = a_job(&scratch);
        let bytes = std::fs::read(job.source_path(0).unwrap()).unwrap();
        assert_eq!(job.project.source_with_hash(&sha256_hex(&bytes)), Some(0));
        assert_eq!(job.project.source_with_hash(&"0".repeat(64)), None);
    }

    #[test]
    fn the_default_output_directory_is_a_sibling_of_the_input() {
        assert_eq!(
            default_output_dir(Path::new("/scans/ch01")),
            PathBuf::from("/scans/ch01_cleaned")
        );
    }

    #[test]
    fn sources_are_stored_relative_so_a_job_survives_being_moved() {
        let scratch = Scratch::new("relative");
        let job = a_job(&scratch);
        assert_eq!(
            job.project.sources[0].rel_path,
            PathBuf::from("../raws/001.png")
        );

        // Move the pair, and the source still resolves.
        let moved = scratch.join("moved");
        std::fs::create_dir_all(&moved).unwrap();
        std::fs::rename(scratch.join("raws"), moved.join("raws")).unwrap();
        std::fs::rename(scratch.join("out"), moved.join("out")).unwrap();
        let reopened = Job::open(&moved.join("out/chapter.mtclean")).unwrap();
        assert_eq!(reopened.verify_sources(), vec![SourceState::Unchanged]);
    }

    /// §2's listing is the product - "a job that silently drops files is worse
    /// than one that cleans nothing" - and the listing is worthless if it dies
    /// with the session that made it.
    #[test]
    fn the_files_ingest_refused_survive_in_the_manifest() {
        let scratch = Scratch::new("input-report");
        let dir = scratch.join("raws");
        std::fs::create_dir_all(&dir).unwrap();
        let png = encode(&fixtures::by_name("l8").raster, Format::Png).unwrap();
        let other = encode(&fixtures::by_name("rgb8").raster, Format::Png).unwrap();
        let files: Vec<(PathBuf, Vec<u8>)> = vec![
            (dir.join("001.png"), png.clone()),
            (dir.join("001.tif"), other.clone()),
            (dir.join("002.png"), b"not an image at all".to_vec()),
            (dir.join(".DS_Store"), b"junk".to_vec()),
        ];
        let report = crate::ingest::ingest(
            files
                .iter()
                .map(|(path, bytes)| (path.as_path(), bytes.as_slice())),
        );

        let manifest = scratch.join("out/chapter.mtclean");
        std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        let project = Project::from_ingest(
            manifest.parent().unwrap(),
            "0.1.0",
            StripMode::Single,
            &report,
        );
        let job = Job::create(&manifest, project).unwrap();

        let reopened = Job::open(job.path()).unwrap().project;
        assert_eq!(reopened.input_report.junk_skipped, 1);
        assert_eq!(
            reopened.input_report.duplicate_basenames,
            vec!["001".to_string()]
        );
        assert_eq!(
            reopened.input_report.skipped,
            vec![SkippedInput {
                file: "002.png".into(),
                reason: "input.skipReason.notAnImage".into()
            }],
            "a refused file left no trace in the manifest"
        );
        // The junk entry is counted, never listed: it is not a page anybody
        // expected to see, and naming `.DS_Store` in a skip list is noise.
        assert!(!reopened
            .input_report
            .skipped
            .iter()
            .any(|s| s.file == ".DS_Store"));
    }

    /// §6's "any incomplete job offers a resume" needs a manifest that can say
    /// a job is incomplete, and the offer has to be withdrawable.
    #[test]
    fn an_interrupted_job_says_so_and_can_stop_saying_so() {
        let scratch = Scratch::new("interrupted");
        let mut job = a_job(&scratch);
        assert_eq!(job.project.interrupted_at, None);

        job.mark_interrupted(Some(7)).unwrap();
        assert_eq!(
            Job::open(job.path()).unwrap().project.interrupted_at,
            Some(7)
        );

        job.mark_interrupted(None).unwrap();
        assert_eq!(Job::open(job.path()).unwrap().project.interrupted_at, None);
    }

    /// The distinction the field exists for. A page the detector looked at and
    /// found nothing on records nothing else at all - no patch, no untouched
    /// region, no counter above zero - so this is the only thing that tells it
    /// apart from a page nothing has ever opened.
    #[test]
    fn a_page_examined_and_found_empty_is_not_a_page_nobody_looked_at() {
        let scratch = Scratch::new("examined");
        let mut job = a_job(&scratch);
        assert!(job.project.examined.is_empty());

        job.mark_examined(0).unwrap();
        job.mark_examined(0).unwrap();

        let reopened = Job::open(job.path()).unwrap().project;
        assert_eq!(
            reopened.examined,
            vec![0],
            "a second pass doubled the record"
        );
        assert!(reopened.patches.is_empty());
        assert!(reopened.regions_untouched.is_empty());
        assert_eq!(reopened.counters, Counters::default());
    }

    #[test]
    fn a_source_that_changed_counts_as_unexamined_again() {
        let scratch = Scratch::new("examined-stale");
        let mut job = a_job(&scratch);
        job.mark_examined(0).unwrap();

        let source = job.source_path(0).unwrap();
        let other = encode(&fixtures::by_name("rgb8").raster, Format::Png).unwrap();
        std::fs::write(&source, other).unwrap();
        let states = job.verify_sources();
        job.drop_stale(&states).unwrap();

        assert!(
            Job::open(job.path()).unwrap().project.examined.is_empty(),
            "a re-cropped page stayed out of the queue with nothing on it"
        );
    }

    /// **A version-1 manifest opens, and that is what the bump to 2 was for.**
    ///
    /// The number went up so a v1 *build* refuses a manifest holding
    /// `"engine": "paint"` for its version rather than as an `unknown variant`
    /// deep in the body. Nothing about that requires this build to refuse the
    /// files it wrote last week: the v2 types only widened an enum, so a v1
    /// file decodes into them unchanged. Written against a file that really
    /// says `"version": 1` on disk rather than against the constant, because
    /// the constant is the thing that moved.
    #[test]
    fn a_version_one_manifest_still_opens_and_is_rewritten_at_the_legacy_floor() {
        let scratch = Scratch::new("version-one");
        let job = a_job(&scratch);
        let mut old: serde_json::Value =
            serde_json::from_slice(&std::fs::read(job.path()).unwrap()).unwrap();
        old["version"] = serde_json::json!(1);
        std::fs::write(job.path(), serde_json::to_vec_pretty(&old).unwrap()).unwrap();
        assert!(
            std::fs::read_to_string(job.path())
                .unwrap()
                .contains("\"version\": 1"),
            "the fixture has to actually be a v1 file"
        );

        // Opening raises the number in memory; the file on disk is untouched
        // until something writes it.
        let reopened = Job::open(job.path()).expect("a v1 manifest is still readable");
        assert_eq!(reopened.project.version, 3);

        // And the next flush is what upgrades the file - no migration pass, no
        // conversion step, and nothing the caller has to remember to do. This is
        // the half that was missing: `flush` writes `project.version` verbatim,
        // so a v1 chapter that gained a painted patch was being written back as
        // a v1 file carrying `"engine": "paint"`.
        reopened.flush().unwrap();
        let text = std::fs::read_to_string(job.path()).unwrap();
        assert!(
            text.contains("\"version\": 3"),
            "the flush did not rewrite the version"
        );
        assert_eq!(
            Job::open(job.path()).unwrap().project.version,
            3
        );
    }

    #[test]
    fn legacy_cloud_json_migrates_without_inventing_history_or_writing_on_read() {
        for version in [1, 2, 3] {
            let scratch = Scratch::new(&format!("legacy-cloud-v{version}"));
            let mut job = a_job(&scratch);
            job.complete_region(0, &a_patch("legacy"), None).unwrap();
            if version >= 2 {
                for (id, engine) in [("paint", Engine::Paint), ("clone", Engine::Clone)] {
                    let mut patch = a_patch(id);
                    patch.provenance.engine = engine;
                    job.complete_region(0, &patch, None).unwrap();
                }
            }
            let mut fixture: serde_json::Value =
                serde_json::from_slice(&std::fs::read(job.path()).unwrap()).unwrap();
            fixture["version"] = serde_json::json!(version);
            // These fields did not exist before v4 and must not turn a legacy
            // record into an implicit text-shaped plan during migration.
            fixture.as_object_mut().unwrap().remove("text_shape_plans");
            for record in fixture["patches"].as_array_mut().unwrap() {
                record.as_object_mut().unwrap().remove("geometry_policy");
                record
                    .as_object_mut()
                    .unwrap()
                    .remove("text_shape_plan_identity");
            }
            fixture["patches"][0]["engine"] = serde_json::json!("cloud");
            fixture["patches"][0]["provenance"]["engine"] = serde_json::json!("cloud");
            // This is the exact old JSON shape, not serialization of the new
            // CloudRecord with invented defaults for its additional fields.
            fixture["patches"][0]["provenance"]["cloud"] = serde_json::json!({
                "provider": "legacy-provider", "model": "legacy-model",
                "request_id": "legacy-request", "tier": "standard", "cost": 0.014
            });
            let before = serde_json::to_vec_pretty(&fixture).unwrap();
            std::fs::write(job.path(), &before).unwrap();
            let reopened = Job::open(job.path()).unwrap();
            assert_eq!(std::fs::read(job.path()).unwrap(), before);
            assert_eq!(reopened.project.version, 3);
            let record = &reopened.project.patches[0];
            assert!(reopened.project.text_shape_plans.is_empty());
            assert!(reopened
                .project
                .patches
                .iter()
                .all(|patch| patch.geometry_policy == GeometryPolicy::Legacy));
            assert_eq!(record.engine, Engine::Cloud);
            assert_eq!(record.provenance.engine, Engine::Cloud);
            let cloud = record.provenance.cloud.as_ref().unwrap();
            assert_eq!(cloud.provider, "legacy-provider");
            assert_eq!(cloud.model, "legacy-model");
            assert_eq!(cloud.cost, Some(0.014));
            assert_eq!(cloud.tier.as_deref(), Some("standard"));
            assert_eq!(cloud.profile_id, None);
            assert_eq!(cloud.job_id, None);
            assert_eq!(cloud.attempt_id, None);
            assert_eq!(cloud.recipe_id, None);
            assert_eq!(cloud.model_revision, None);
            assert_eq!(cloud.duration_ms, None);
            reopened.flush().unwrap();
            let upgraded = Job::open(job.path()).unwrap();
            assert_eq!(upgraded.project.patches, reopened.project.patches);
            let saved: serde_json::Value =
                serde_json::from_slice(&std::fs::read(job.path()).unwrap()).unwrap();
            assert_eq!(saved["version"], 3);
            assert_eq!(
                saved["patches"][0]["provenance"]["cloud"],
                fixture["patches"][0]["provenance"]["cloud"]
            );
            if version >= 2 {
                assert_eq!(upgraded.project.patches[1].engine, Engine::Paint);
                assert_eq!(upgraded.project.patches[2].engine, Engine::Clone);
            }
        }
    }

    #[test]
    fn v3_cloud_patch_roundtrips_explicit_null_cost_and_provenance() {
        let scratch = Scratch::new("v3-cloud-null-cost");
        let mut job = a_job(&scratch);
        let mut patch = a_patch("remote");
        patch.provenance.engine = Engine::Flux;
        patch.provenance.cloud = Some(
            serde_json::from_value(serde_json::json!({
                "provider": "modal", "profile_id": "test-profile", "job_id": "test-job",
                "request_id": "test-request", "attempt_id": "test-attempt",
                "recipe_id": "test-recipe", "model": "test-model",
                "model_revision": "test-immutable-revision", "cost": null, "duration_ms": 120
            }))
            .unwrap(),
        );
        job.complete_region(0, &patch, None).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(job.path()).unwrap()).unwrap();
        assert_eq!(saved["version"], 3);
        let cloud_json = &saved["patches"][0]["provenance"]["cloud"];
        assert!(cloud_json.as_object().unwrap().contains_key("cost"));
        assert!(cloud_json["cost"].is_null());
        let reopened = Job::open(job.path()).unwrap();
        assert_eq!(reopened.project.patches[0].provenance, patch.provenance);
        assert_eq!(reopened.project.patches[0].engine, Engine::Flux);
        assert_eq!(
            reopened
                .load_patch(&reopened.project.patches[0])
                .unwrap()
                .pixels
                .data,
            patch.pixels.data
        );
    }

    /// A patch made by either hand tool survives the manifest, under the rung
    /// id its `rung_key` names. This is the value that could not exist before
    /// the bump and is the reason for it.
    #[test]
    fn a_painted_patch_round_trips_through_the_manifest() {
        let scratch = Scratch::new("paint-record");
        let mut job = a_job(&scratch);
        for (index, engine) in [Engine::Paint, Engine::Clone].into_iter().enumerate() {
            let mut patch = a_patch(&format!("r{index}"));
            patch.provenance.engine = engine;
            job.complete_region(0, &patch, None).unwrap();
        }

        let text = std::fs::read_to_string(job.path()).unwrap();
        assert!(text.contains("\"engine\": \"paint\""), "{text}");
        assert!(text.contains("\"engine\": \"clone\""), "{text}");

        let reopened = Job::open(job.path()).unwrap();
        let engines: Vec<Engine> = reopened
            .project
            .patches
            .iter()
            .map(|record| record.provenance.engine)
            .collect();
        assert_eq!(engines, vec![Engine::Paint, Engine::Clone]);
    }

    #[test]
    fn a_manifest_from_a_later_version_is_refused_rather_than_half_read() {
        let scratch = Scratch::new("version");
        let job = a_job(&scratch);
        let mut future: serde_json::Value =
            serde_json::from_slice(&std::fs::read(job.path()).unwrap()).unwrap();
        future["version"] = serde_json::json!(FORMAT_VERSION + 1);
        std::fs::write(job.path(), serde_json::to_vec(&future).unwrap()).unwrap();
        assert!(matches!(
            Job::open(job.path()),
            Err(StoreError::Version { .. })
        ));
    }

    #[test]
    fn an_unknown_geometry_policy_is_not_migrated_to_legacy() {
        let scratch = Scratch::new("unknown-geometry-policy");
        let mut job = a_job(&scratch);
        job.complete_region(0, &a_patch("r0"), None).unwrap();
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(job.path()).unwrap()).unwrap();
        manifest["patches"][0]["geometry_policy"] = serde_json::json!("future_mode");
        std::fs::write(job.path(), serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(matches!(Job::open(job.path()), Err(StoreError::Json(_))));
    }

    /// The version has to be read **before** the shape, or it is only ever
    /// consulted for the files that did not need it. A later version whose
    /// layout also moved is the ordinary case - that is what a version is for -
    /// and read in the other order it arrives as "not readable as JSON" about a
    /// file that is perfectly good JSON.
    #[test]
    fn a_later_version_is_recognised_even_when_its_shape_is_unreadable() {
        let scratch = Scratch::new("version-shape");
        let job = a_job(&scratch);
        std::fs::write(
            job.path(),
            serde_json::json!({ "version": FORMAT_VERSION + 1, "whatever": [1, 2, 3] }).to_string(),
        )
        .unwrap();

        match Job::open(job.path()).map(|_| ()) {
            Err(StoreError::Version { expected, found }) => {
                assert_eq!((expected, found), (FORMAT_VERSION, FORMAT_VERSION + 1));
            }
            other => panic!("a later version was refused as a malformed file: {other:?}"),
        }
    }

    /// `strip.splits` changed element type - `u32` to
    /// [`crate::strip::Split`] - inside version 1, and the field's own
    /// documentation is the justification. This is the fact that justification
    /// rests on: no build has ever written a populated `splits`, so the only
    /// value the change can meet on disk is `[]`, which decodes the same way
    /// under both types. A populated one from a hypothetical older writer is
    /// refused loudly rather than silently misread.
    #[test]
    fn nothing_writes_a_populated_splits_and_an_old_shaped_one_is_refused_loudly() {
        let scratch = Scratch::new("splits-shape");
        let job = a_job(&scratch);
        assert!(
            job.project.strip.splits.is_empty(),
            "a fresh project writes no splits"
        );

        let written = std::fs::read_to_string(job.path()).unwrap();
        assert!(written.contains(r#""splits": []"#), "{written}");
        assert!(Job::open(job.path()).is_ok());

        // The one manifest the change could have been wrong about, if a writer
        // for it had ever existed.
        std::fs::write(
            job.path(),
            written.replace(r#""splits": []"#, r#""splits": [2000]"#),
        )
        .unwrap();
        assert!(
            matches!(Job::open(job.path()), Err(StoreError::Json(_))),
            "a row of the old shape was accepted, or dropped, instead of refused"
        );
    }
}
