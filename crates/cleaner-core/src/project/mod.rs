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
use crate::ingest::{IngestReport, SourceRef, Warning, sha256_hex};
use crate::mask::Rect;
use crate::patch::{Engine, Patch, Provenance};

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
pub const FORMAT_VERSION: u32 = 2;

/// The oldest manifest this build reads.
///
/// **Version 2 is the paint bump, and it is a widening rather than a change of
/// shape.** [`crate::patch::Engine`] gained `Paint` and `Clone`, so the first
/// time anyone paints, `"engine": "paint"` reaches disk and a version-1 build
/// meeting it fails as `unknown variant`, having passed the version probe.
/// That is what the bump exists to make legible from the *other* side; it is
/// not a reason for this build to refuse the files it already wrote.
///
/// So the gate is a range and not an equality. A v1 manifest decodes
/// identically under the v2 types - the enum only widened, and nothing else
/// moved - so there is no migration to run and reading one costs a comparison.
/// Bumping without this would refuse every project that already exists, and
/// there is no path in this build to read them with, so the refusal would be
/// permanent rather than a step.
///
/// A file is rewritten at [`FORMAT_VERSION`] on the next flush, which is how a
/// v1 project becomes a v2 one: by being saved, not by being converted.
pub const OLDEST_READABLE_VERSION: u32 = 1;

/// The extension §1 names, and the sidecar directory beside it.
pub const EXTENSION: &str = "mtclean";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("the manifest is not readable as JSON: {0}")]
    Json(String),
    #[error("{0}")]
    Malformed(String),
    #[error("this build reads manifest version {expected}; the file is version {found}")]
    Version { expected: u32, found: u32 },
}

impl StoreError {
    fn io(path: &Path, source: std::io::Error) -> StoreError {
        StoreError::Io { path: path.to_path_buf(), source }
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
    pub provenance: Provenance,
}

impl PatchRecord {
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
        format!("{}.ink", self.id)
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
            version: FORMAT_VERSION,
            created: now(),
            app_version: app_version.to_owned(),
            strip: Strip { mode, order: (0..rows.len()).collect(), splits: Vec::new() },
            sources: rows,
            patches: Vec::new(),
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
        self.sources.iter().position(|source| source.sha256 == sha256)
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
    Changed { sha256: String },
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
        let job = Job { path: path.to_path_buf(), dir: sidecar_dir(path), project };
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
            return Err(StoreError::Version { expected: FORMAT_VERSION, found: probe.version });
        }

        let mut project: Project =
            serde_json::from_slice(&bytes).map_err(|e| StoreError::Json(e.to_string()))?;
        // **Opening upgrades the number, so the next flush upgrades the file.**
        // [`OLDEST_READABLE_VERSION`] promises exactly this, and without the
        // line the promise is empty: [`Job::flush`] writes `project.version`
        // verbatim, so a v1 chapter that gains a painted patch would be written
        // back as `{"version": 1, ..., "engine": "paint"}`. A v1 build meeting
        // *that* file passes the version probe and dies inside the body as
        // `unknown variant \`paint\``, which is the failure the bump to 2
        // exists to convert into a clean refusal. The upgrade is free because
        // the v2 types only widened an enum - the value now in memory decodes
        // from the v1 bytes unchanged.
        project.version = FORMAT_VERSION;
        Ok(Job { path: path.to_path_buf(), dir: sidecar_dir(path), project })
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
        self.project.sources.get(index).map(|source| self.root().join(&source.rel_path))
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
        Some(self.root().join(source.converted_from.as_ref().unwrap_or(&source.rel_path)))
    }

    /// Rewrite the manifest atomically.
    pub fn flush(&self) -> Result<(), StoreError> {
        let bytes = serde_json::to_vec_pretty(&self.project)
            .map_err(|e| StoreError::Json(e.to_string()))?;
        buffers::write_atomic(&self.path, &bytes)
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
        let record = PatchRecord::of(source_idx, patch, review_state);
        buffers::write_atomic(
            &self.dir.join(&record.mask_ref),
            &buffers::encode_mask(&patch.mask),
        )?;
        buffers::write_atomic(
            &self.dir.join(record.ink_ref()),
            &buffers::encode_mask(&patch.ink),
        )?;
        buffers::write_atomic(
            &self.dir.join(&record.buffer_ref),
            &buffers::encode_patch(&patch.pixels),
        )?;

        // A re-run replaces the record rather than appending a second one with
        // the same id - §3 forbids overwriting a prior *provenance* entry, and
        // that history lives in the mask revisions, not in duplicate rows here.
        match self.project.patches.iter_mut().find(|existing| existing.id == record.id) {
            Some(existing) => *existing = record,
            None => self.project.patches.push(record),
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
        self.project.patches.retain(|record| !stale.contains(&record.source_idx));
        self.project.regions_untouched.retain(|region| !stale.contains(&region.source_idx));
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
                let Some(parent) = path.parent() else { continue };
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
    let name = input_dir.file_name().map(|n| n.to_string_lossy().into_owned());
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
    if out.as_os_str().is_empty() { PathBuf::from(".") } else { out }
}

/// A path's last component, or the whole path where it has none.
fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| {
        path.to_string_lossy().into_owned()
    })
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
mod tests {
    use super::*;
    use crate::image::{Format, encode, fixtures};
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
        let project =
            Project::new(manifest.parent().unwrap(), "0.1.0", StripMode::Single, &[source]);
        Job::create(&manifest, project).unwrap()
    }

    #[test]
    fn a_manifest_round_trips_through_the_file_on_disk() {
        let scratch = Scratch::new("round-trip");
        let mut job = a_job(&scratch);
        job.project.settings = serde_json::json!({ "engineCeiling": "lama", "cloud": false });
        job.project.counters.declined = 2;
        job.complete_region(0, &a_patch("r0"), Some("review.reason.declined".into())).unwrap();
        job.leave_untouched(0, Rect::new(4, 4, 6, 6), "review.reason.gateSkippedLowConfidence")
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

    /// The manifest is a human-readable record of a job, and §1 fixes the field
    /// names. A rename here is a break for anything reading it.
    #[test]
    fn the_manifest_has_the_field_names_the_document_specifies() {
        let scratch = Scratch::new("field-names");
        let mut job = a_job(&scratch);
        job.complete_region(0, &a_patch("r0"), None).unwrap();
        job.leave_untouched(0, Rect::new(4, 4, 6, 6), "decline.reason.qualityMetric").unwrap();

        let text = std::fs::read_to_string(job.path()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        for key in [
            "version", "created", "app_version", "sources", "strip", "patches",
            "regions_untouched", "counters", "settings",
        ] {
            assert!(value.get(key).is_some(), "the manifest has no {key}");
        }
        for key in ["rel_path", "sha256", "mtime", "w", "h", "mode", "bit_depth"] {
            assert!(value["sources"][0].get(key).is_some(), "a source row has no {key}");
        }
        for key in [
            "id", "source_idx", "bbox", "mask_ref", "buffer_ref", "engine", "order", "visible",
            "review_state", "provenance",
        ] {
            assert!(value["patches"][0].get(key).is_some(), "a patch row has no {key}");
        }
        for key in [
            "refused", "errored", "param_reject", "residual_reject", "structural_reject",
            "declined", "gate_dropped",
        ] {
            assert!(value["counters"].get(key).is_some(), "the counters have no {key}");
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
        assert!(job.sidecar().join(record.ink_ref()).exists(), "the ink is a sidecar file");

        std::fs::remove_file(job.sidecar().join(record.ink_ref())).unwrap();
        let loaded = job.load_patch(&record).unwrap();
        assert_eq!(loaded.ink, patch.mask, "no ink file: the applied mask stands in");
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
            job.complete_region(0, &a_patch(&format!("r{index}")), None).unwrap();
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
        assert!(Job::open(job.path()).unwrap().project.patches.is_empty(), "the drop was not flushed");
    }

    #[test]
    fn a_deleted_source_is_missing_rather_than_changed() {
        let scratch = Scratch::new("missing");
        let mut job = a_job(&scratch);
        job.complete_region(0, &a_patch("r0"), None).unwrap();
        std::fs::remove_file(job.source_path(0).unwrap()).unwrap();

        let states = job.verify_sources();
        assert_eq!(states, vec![SourceState::Missing]);
        assert_eq!(job.drop_stale(&states).unwrap().dropped_patches, vec!["r0".to_string()]);
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
        assert_eq!(job.verify_sources(), vec![SourceState::Unchanged], "a new mtime read as a change");
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
        assert_eq!(job.project.sources[0].rel_path, PathBuf::from("../raws/001.png"));

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
            files.iter().map(|(path, bytes)| (path.as_path(), bytes.as_slice())),
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
        assert_eq!(reopened.input_report.duplicate_basenames, vec!["001".to_string()]);
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
        assert!(!reopened.input_report.skipped.iter().any(|s| s.file == ".DS_Store"));
    }

    /// §6's "any incomplete job offers a resume" needs a manifest that can say
    /// a job is incomplete, and the offer has to be withdrawable.
    #[test]
    fn an_interrupted_job_says_so_and_can_stop_saying_so() {
        let scratch = Scratch::new("interrupted");
        let mut job = a_job(&scratch);
        assert_eq!(job.project.interrupted_at, None);

        job.mark_interrupted(Some(7)).unwrap();
        assert_eq!(Job::open(job.path()).unwrap().project.interrupted_at, Some(7));

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
        assert_eq!(reopened.examined, vec![0], "a second pass doubled the record");
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
    fn a_version_one_manifest_still_opens_and_is_rewritten_at_the_current_version() {
        let scratch = Scratch::new("version-one");
        let mut job = a_job(&scratch);
        job.project.version = 1;
        job.flush().unwrap();
        assert!(
            std::fs::read_to_string(job.path()).unwrap().contains("\"version\": 1"),
            "the fixture has to actually be a v1 file"
        );

        // Opening raises the number in memory; the file on disk is untouched
        // until something writes it.
        let reopened = Job::open(job.path()).expect("a v1 manifest is readable by a v2 build");
        assert_eq!(reopened.project.version, FORMAT_VERSION);

        // And the next flush is what upgrades the file - no migration pass, no
        // conversion step, and nothing the caller has to remember to do. This is
        // the half that was missing: `flush` writes `project.version` verbatim,
        // so a v1 chapter that gained a painted patch was being written back as
        // a v1 file carrying `"engine": "paint"`.
        reopened.flush().unwrap();
        let text = std::fs::read_to_string(job.path()).unwrap();
        assert!(text.contains("\"version\": 2"), "the flush did not rewrite the version");
        assert_eq!(Job::open(job.path()).unwrap().project.version, FORMAT_VERSION);
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
        let engines: Vec<Engine> =
            reopened.project.patches.iter().map(|record| record.provenance.engine).collect();
        assert_eq!(engines, vec![Engine::Paint, Engine::Clone]);
    }

    #[test]
    fn a_manifest_from_a_later_version_is_refused_rather_than_half_read() {
        let scratch = Scratch::new("version");
        let mut job = a_job(&scratch);
        job.project.version = FORMAT_VERSION + 1;
        job.flush().unwrap();
        assert!(matches!(Job::open(job.path()), Err(StoreError::Version { .. })));
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
        assert!(job.project.strip.splits.is_empty(), "a fresh project writes no splits");

        let written = std::fs::read_to_string(job.path()).unwrap();
        assert!(written.contains(r#""splits": []"#), "{written}");
        assert!(Job::open(job.path()).is_ok());

        // The one manifest the change could have been wrong about, if a writer
        // for it had ever existed.
        std::fs::write(job.path(), written.replace(r#""splits": []"#, r#""splits": [2000]"#))
            .unwrap();
        assert!(
            matches!(Job::open(job.path()), Err(StoreError::Json(_))),
            "a row of the old shape was accepted, or dropped, instead of refused"
        );
    }
}
