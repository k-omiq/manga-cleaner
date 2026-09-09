//! `exportChapter` - [`export_page`](cleaner_core::export::export_page),
//! [`export_stitched`](cleaner_core::export::export_stitched), the
//! [`Cbz`](cleaner_core::export::Cbz) container and
//! [`Job::output_refusal`](cleaner_core::project::Job::output_refusal), wired
//! to a chapter id.
//!
//! The seam fixes the return
//! shape as `{status, fileCount, path, reasonKey, gutterPixels}`, and
//! [`ExportResult`] serialises to exactly that JSON (`#[serde(rename_all =
//! "camelCase")]`), so `src/lib/api/tauri.js` needs no mapping layer - the
//! command's answer *is* the seam's answer.
//!
//! ## Every "no" is a refusal with a reason
//!
//! There are eleven things this command will not do, and all eleven come back
//! the same way: `status: 'refused'` and a `reasonKey` the catalogue has a
//! sentence for. **One of them was always here** - writing over the source
//! folder, which is [`Job::output_refusal`]'s decision and not this module's
//! (output never overwrites input), reported under
//! `notice.export.refusedOverwrite`. The last is a family of five keys rather
//! than one, because five different disagreements between pages can stop a
//! strip being written as one file:
//!
//! | Asked for | Refused with |
//! |---|---|
//! | JPEG, or any other lossy encoder | `notice.export.refusedLossyFormat` |
//! | PSB | `notice.export.refusedLayeredFormat` |
//! | a format string this build does not know | `notice.export.refusedUnknownFormat` |
//! | `masks: 'separate-layer'` into a CBZ | `notice.export.refusedMaskLayers` |
//! | a destination that is neither sentinel nor an absolute path | `notice.export.refusedDestination` |
//! | a stitched export of a paginated project | `notice.export.refusedStitchPaginated` |
//! | a stitched export into a CBZ | `notice.export.refusedStitchedArchive` |
//! | a stitched export as PSD | `notice.export.refusedStitchedLayered` |
//! | a PSD of an indexed or sub-8-bit page | `notice.export.refusedLayeredMode` |
//! | a PSD of a page over 30 000 px a side | `notice.export.refusedLayeredSize` |
//! | a strip whose pages disagree | [`StitchRefusal::reason_key`](cleaner_core::export::StitchRefusal::reason_key) |
//!
//! **This is what replaced the silent downgrade**, and the half that was true
//! about it is worth keeping straight. Until now `target_for` mapped anything
//! it did not recognise to [`Target::SameAsSource`], so a user who chose JPEG
//! got PNG and was told the export succeeded. Half of the old argument holds:
//! the fallback never produced *lossy* output, so that contract was never
//! violated by it. The other half does not: it produced
//! a **different container than the one that was asked for**, which is
//! "the UI never silently downgrades" failing on the word *silently* rather
//! than on the word *downgrade*. A format this build cannot write is now named
//! and refused before anything is opened.
//!
//! ## What it does write
//!
//! **PNG and TIFF**, per page, which is the default and what this command
//! always did. TIFF is now offered rather than merely accepted: it is the
//! only format here that carries CMYK, named for stitched longstrip.
//!
//! **CBZ**, which is not a picture format at all: it is those same per-page
//! files, unaltered, inside one zip. The pages keep their own lossless format
//! rather than being converted to the container's - a CBZ names an archive and
//! not a codec, and keeping the page's format is what keeps the byte-for-byte
//! passthrough available *inside* the archive. `cleaner_core::export::cbz` is
//! the writer and carries the memory argument for the one buffer it needs.
//!
//! **Stitched**, one file for the whole strip, through
//! [`export_stitched`](cleaner_core::export::export_stitched) - which was built
//! and tested and had no caller at all. Reaching it took the seam
//! parameter needed: `layout`, `'per-page' | 'stitched'`.
//! Offered for a longstrip project and refused for a paginated one, because a
//! paginated chapter has no strip - its pages are separate images that happen
//! to be listed in an order, and stacking them into one tall file would invent
//! a document nobody has.
//!
//! An interface offering stitched output **states the gutter
//! count before it runs**. `ExportDialog.svelte` computes it from the page
//! dimensions it already holds and says so under the control; this command
//! reports what was actually written back on `gutterPixels`, and the finished
//! notice says that too. The two are the same arithmetic from the two ends.
//!
//! **PSD**, per page, through
//! [`export_page_psd_to`](cleaner_core::export::export_page_psd_to). This is
//! where `masks` finally branches, and it means two different things by
//! format:
//!
//! - **PSD**: `'separate-layer'` is the layered shape - the untouched source as
//!   the `Background`, one layer per region with a layer mask, inside a
//!   `Cleaned` group. `'flattened'` is one `Background` layer holding the
//!   composite.
//! - **PNG and TIFF**, per page or stitched: `'separate-layer'` writes a
//!   **mask file beside each image** - `001.png` and `001_mask.png`, an 8-bit
//!   grayscale PNG that is white where the lettering the page's edits removed
//!   was (the union of their [`Patch::ink`](cleaner_core::patch::Patch::ink)
//!   masks, not of the areas they painted) - through
//!   [`export_mask_to`](cleaner_core::export::export_mask_to).
//!   `'flattened'` is the image alone, which is what every export was.
//! - **CBZ**: refused. A mask file inside the archive would be read as a page.
//!
//! Which pages a PSD can be is decided from the **manifest**, before a
//! directory is created: `psd::refusal` over every source's mode, depth and
//! dimensions, so a chapter with one indexed page refuses whole rather than
//! writing nine files and failing on the tenth. A stitched PSD is refused too  - 
//! a layered file holds its whole page as the background, and a strip may never
//! be resident whole.
//!
//! ## What it still refuses, and why
//!
//! JPEG is a lossy encoder, and lossy formats sit "behind the
//! acknowledgement" - the acknowledgement being a written statement of what
//! will change, shown and agreed to before the write, which for a lossy format
//! is a statement that the fidelity contract does not hold at all. That flow
//! does not exist, and an encoder in front of a missing acknowledgement would
//! be the silent downgrade again with more steps. PSB is not written. Both
//! refuse by name.
//!
//! ## Rule 8, per page
//!
//! Each source is exported at its own dimensions, one page resident at a time,
//! encoded straight into the file rather than into a buffer the command then
//! writes ([`export_page_to`]).
//!
//! For a longstrip job the patch set is a **global** one in strip coordinates,
//! and which patches a page gets is decided by **intersection**:
//!
//! > In longstrip, "zero applied patches" is decided by per-page
//! > **intersection** with the global patch set, not by patch ownership, since
//! > a patch may span a join.
//!
//! [`patches_on_page`] is that intersection, and it is reached through
//! [`intersecting_records`], which does the selection from the manifest's
//! `bbox` fields alone. That ordering is the memory rule rather than an
//! optimisation: loading the whole patch set to ask which parts of it touch
//! page 7 would put every patch in the chapter in RAM at once, which is what
//! keeping them on disk avoids. The stitched path cannot do that -
//! `export_stitched` takes the global
//! set as a slice, so [`global_patches`] builds all of it - and that is the one
//! place in this file where that discipline is not honoured.
//!
//! ## Merge point
//!
//! `resolve_chapter` and `LibraryError` live in `src-tauri/src/library.rs`.
//! This module is coded against
//!
//! ```ignore
//! pub fn resolve_chapter(app: &tauri::AppHandle, chapter_id: &str) -> Result<std::path::PathBuf, library::LibraryError>;
//! ```
//!
//! and against `LibraryError: std::fmt::Display` (every other error type in
//! this crate gets to a `String` the same way).

use std::path::{Path, PathBuf};

use cleaner_core::export::{
    Cbz, PageSource, StripPatch, Target, export_mask_to, export_page, export_page_psd_to,
    export_page_to, export_stitched, patches_on_page, planned_stitched_format, psd,
};
use cleaner_core::image::{Format, Header, Raster, decode, png_header, tiff_header};
use cleaner_core::mask::{Mask, Rect};
use cleaner_core::patch::Patch;
use cleaner_core::project::{Job, PatchRecord, ProjectSource, StripMode, default_output_dir};
use cleaner_core::strip::Strip;

use crate::library;

/// The seam's own shape for `exportChapter`'s resolved value, verbatim.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_key: Option<&'static str>,
    /// How many pixels of the written file no source page had,
    /// and `Some` on a stitched export alone. A per-page export has no gutter
    /// and nothing to say, which is not the same fact as a gutter of zero.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gutter_pixels: Option<u64>,
}

impl ExportResult {
    fn refused(reason_key: &'static str, path: Option<String>) -> ExportResult {
        ExportResult {
            status: "refused",
            file_count: None,
            path,
            reason_key: Some(reason_key),
            gutter_pixels: None,
        }
    }
}

/// `async`, and the body on the blocking executor - see
/// [`library::blocking`](crate::library::blocking) for why. This command
/// composites and writes every page of a chapter, and it is the one the
/// interface draws a busy spinner over: run inline on the invoke handler's
/// thread, it would hold the webview for the whole export and the spinner would
/// never paint.
#[tauri::command]
pub async fn export_chapter(
    app: tauri::AppHandle,
    chapter_id: String,
    format: Option<String>,
    destination: Option<String>,
    masks: Option<String>,
    layout: Option<String>,
) -> Result<ExportResult, String> {
    let declared = format.clone().unwrap_or_else(|| "PNG".into());
    let result = library::blocking(move || {
        let job_path = library::resolve_chapter(&app, &chapter_id).map_err(|e| e.to_string())?;
        let _lock = crate::run::lock_job(&job_path);
        let job = Job::open(&job_path).map_err(|e| e.to_string())?;
        export_open_job(
            &job,
            format.as_deref().unwrap_or("PNG"),
            destination.as_deref().unwrap_or("new-folder"),
            masks.as_deref().unwrap_or("flattened"),
            layout.as_deref().unwrap_or("per-page"),
        )
    })
    .await?;

    // The seam carries the outcome as a return value *and* the interface
    // expects the notice stack to say so - `library.svelte.js` states that the
    // backend announces and the screen does not repeat. Until the channel
    // existed, inside a Tauri window it announced nothing.
    if let Some((key, params, tone)) = announcement(&result, &declared) {
        crate::events::notice(key, params, tone);
    }
    Ok(result)
}

/// What an export result says on the notice stack, if anything.
///
/// **Both statuses are matched by name.** `status` is one of two words today
/// and the arm for the second used to be `_`, which is correct exactly while
/// there are two: a third - a partial write, a cancelled export - would have
/// announced *"Exported N files"* over it, in the confident voice of a thing
/// that finished. An unrecognised status announces nothing instead, and the
/// return value still carries it to the caller.
///
/// **A refusal announces its own reason**, which it did not use to: the arm was
/// written when the overwrite was the only refusal there was and named
/// `notice.export.refusedOverwrite` outright. Seven more refusals later that
/// would have told a user who asked for JPEG that the export would have
/// replaced their source files. A refusal with no key announces nothing rather
/// than picking one.
///
/// A stitched export gets a sentence of its own, because the gutter count
/// is in it and *"Exported 1 page"* is not a true summary of a file that
/// holds the chapter.
fn announcement(
    result: &ExportResult,
    declared: &str,
) -> Option<(&'static str, serde_json::Value, &'static str)> {
    let path = result.path.clone().unwrap_or_default();
    match result.status {
        "refused" => result
            .reason_key
            .map(|key| (key, serde_json::json!({ "path": path, "format": declared }), "warn")),
        "exported" => Some(match result.gutter_pixels {
            Some(gutter) => (
                "notice.export.stitched",
                serde_json::json!({ "count": gutter, "format": declared, "path": path }),
                "info",
            ),
            None => (
                "notice.export.finished",
                serde_json::json!({
                    "count": result.file_count.unwrap_or(0),
                    "format": declared,
                    "path": path,
                }),
                "info",
            ),
        }),
        _ => None,
    }
}

/// What the caller's three strings resolve to, once they have been read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Container {
    /// One file per page, in this target's format, with or without a mask
    /// file beside each.
    Pages { target: Target, masks: MaskFiles },
    /// One `.cbz` holding one file per page.
    Archive,
    /// One file for the whole strip, with or without its mask file beside it.
    Stitched { target: Target, masks: MaskFiles },
    /// One `.psd` per page: layered, or one flattened layer.
    Layered { layered: bool },
}

/// Whether a raster export writes `001_mask.png` beside `001.png`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MaskFiles {
    None,
    Beside,
}

/// The command's body, split out from the Tauri boundary so it can be tested
/// against a real [`Job`] on disk without an `AppHandle`.
///
/// **Every refusal is decided before anything is created.** An export writes a
/// set, and "some files were written and then we changed our minds" is not a
/// state to leave a chapter in - which is the argument `Job::output_refusal`
/// was already made under, extended to the other seven.
fn export_open_job(
    job: &Job,
    format: &str,
    destination: &str,
    masks: &str,
    layout: &str,
) -> Result<ExportResult, String> {
    let requested = match requested_format(format) {
        Ok(requested) => requested,
        Err(reason) => return Ok(ExportResult::refused(reason, None)),
    };
    let longstrip = job.project.strip.mode == StripMode::Longstrip;
    let container = match container_for(requested, layout, mask_files(masks), longstrip) {
        Ok(container) => container,
        Err(reason) => return Ok(ExportResult::refused(reason, None)),
    };
    if let Container::Layered { .. } = container {
        // Every page, from the manifest: a chapter with one page PSD cannot
        // carry refuses whole rather than writing nine files and failing on
        // the tenth.
        for &source_idx in &job.project.strip.order {
            let Some(source) = job.project.sources.get(source_idx) else { continue };
            if let Some(refusal) = psd::refusal(source.w, source.h, source.mode, source.bit_depth) {
                return Ok(ExportResult::refused(refusal.reason_key(), None));
            }
        }
    }

    let source_dir = source_directory(job);
    let destination_dir = match destination_dir(destination, &source_dir) {
        Ok(dir) => dir,
        Err(reason) => return Ok(ExportResult::refused(reason, None)),
    };

    // The refusal is Job::output_refusal's decision, made once, before
    // anything is written.
    if let Some(refusal) = job.output_refusal(&destination_dir) {
        return Ok(ExportResult::refused(
            refusal.reason_key(),
            // The directory that was refused, because the notice names it:
            // *"Export refused - it would replace the files in {path}"*. A
            // refusal with no path in it produces that sentence with a hole
            // where the only actionable word was.
            Some(destination_dir.display().to_string()),
        ));
    }

    std::fs::create_dir_all(&destination_dir)
        .map_err(|e| format!("{}: {e}", destination_dir.display()))?;
    // `default_output_dir` builds a path lexically (a `parent()`/`file_name()`
    // pair), so a job whose manifest sits under a different tree than its
    // sources can hand back something like `.../out/../raws_cleaned`. Now
    // that the directory exists, canonicalising it is what makes the `path`
    // this command returns the one a user would recognise.
    let destination_dir = destination_dir.canonicalize().unwrap_or(destination_dir);

    match container {
        Container::Pages { target, masks } => {
            let file_count = write_pages(job, target, masks, &destination_dir)?;
            Ok(ExportResult {
                status: "exported",
                file_count: Some(file_count),
                path: Some(destination_dir.display().to_string()),
                reason_key: None,
                gutter_pixels: None,
            })
        }
        Container::Layered { layered } => {
            let file_count = write_layered(job, layered, &destination_dir)?;
            Ok(ExportResult {
                status: "exported",
                file_count: Some(file_count),
                path: Some(destination_dir.display().to_string()),
                reason_key: None,
                gutter_pixels: None,
            })
        }
        Container::Archive => {
            let path = destination_dir.join(format!("{}.cbz", chapter_stem(job)));
            let file_count = write_archive(job, &path)?;
            Ok(ExportResult {
                status: "exported",
                file_count: Some(file_count),
                path: Some(path.display().to_string()),
                reason_key: None,
                gutter_pixels: None,
            })
        }
        Container::Stitched { target, masks } => write_stitched(job, target, masks, &destination_dir),
    }
}

/// A format this build can produce, from the string the seam carried.
///
/// The four it knows are the four [`Container`] has a writer for. Everything
/// else is named rather than approximated - see the module doc's table.
fn requested_format(format: &str) -> Result<Requested, &'static str> {
    match format.to_ascii_uppercase().as_str() {
        "PNG" => Ok(Requested::Png),
        "TIFF" | "TIF" => Ok(Requested::Tiff),
        "CBZ" => Ok(Requested::Cbz),
        "PSD" => Ok(Requested::Psd),
        // Lossy, and lossy formats sit behind the acknowledgement. WebP is
        // here rather than under "unknown" because lossless WebP is the same
        // line and refusing it as unrecognised would be a less true sentence.
        "JPEG" | "JPG" | "WEBP" => Err("notice.export.refusedLossyFormat"),
        // The large-document variant. Not written, and named apart from
        // "unknown" because the sentence can say which format is.
        "PSB" => Err("notice.export.refusedLayeredFormat"),
        _ => Err("notice.export.refusedUnknownFormat"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Requested {
    Png,
    Tiff,
    Cbz,
    Psd,
}

/// The format, the layout and the mask choice together.
///
/// **An unrecognised layout is per-page**, which is rule 8's own default and
/// the only one of the two that is always available. That is the opposite
/// treatment from an unrecognised *format*, and deliberately: a format this
/// build cannot write has no safe substitute, while "one file per page" is what
/// every export did before the parameter existed. [`mask_files`] treats an
/// unrecognised `masks` the same way, for the same reason.
fn container_for(
    requested: Requested,
    layout: &str,
    masks: MaskFiles,
    longstrip: bool,
) -> Result<Container, &'static str> {
    let stitched = layout == "stitched";
    if requested == Requested::Psd {
        if stitched {
            // A layered file holds its whole page as the background, and the
            // strip is the one page that never exists.
            return Err("notice.export.refusedStitchedLayered");
        }
        return Ok(Container::Layered { layered: masks == MaskFiles::Beside });
    }
    let target = match requested {
        Requested::Png => Target::Explicit(Format::Png),
        Requested::Tiff => Target::Explicit(Format::Tiff),
        // A CBZ names an archive, not a codec: the pages inside keep their own
        // lossless format, which is what leaves byte-for-byte passthrough
        // reachable through the container.
        Requested::Cbz | Requested::Psd => Target::SameAsSource,
    };
    if !stitched {
        return Ok(match requested {
            Requested::Cbz if masks == MaskFiles::Beside => {
                // A reader app shows every file in the archive as a page, so
                // a mask file inside it would be read as one.
                return Err("notice.export.refusedMaskLayers");
            }
            Requested::Cbz => Container::Archive,
            _ => Container::Pages { target, masks },
        });
    }
    if requested == Requested::Cbz {
        // A stitched file inside an archive of one entry is a shape nobody
        // asked for, and packing it would mean holding the whole strip's
        // encoded file to compute the entry's CRC - the same prohibition,
        // through the back door.
        return Err("notice.export.refusedStitchedArchive");
    }
    if !longstrip {
        return Err("notice.export.refusedStitchPaginated");
    }
    Ok(Container::Stitched { target, masks })
}

/// The seam's `masks`: `'separate-layer'` asks for the masks as their own
/// thing - a file beside each raster image, a layer per region in a PSD - and
/// `'flattened'` is the image alone. Anything else is the image alone, which
/// is what every export did before the parameter was branched on.
fn mask_files(masks: &str) -> MaskFiles {
    match masks {
        "separate-layer" => MaskFiles::Beside,
        _ => MaskFiles::None,
    }
}

/// Where the export writes.
///
/// Two sentinels and an **absolute path**: the seam takes a path when it is
/// absolute and nothing else. `'new-folder'` resolves to
/// [`default_output_dir`], the sibling `<input>_cleaned/`.
///
/// **A relative path is refused rather than resolved.** There is no directory
/// for it to be relative *to* that both ends of the seam would agree on - the
/// backend's working directory is wherever the bundle was launched from, which
/// is not a place any user chose - so resolving one would write the chapter
/// somewhere neither side named. This used to fall back to `'new-folder'`,
/// which was safe and silent; silent is the thing this file no longer does.
///
/// Whatever this returns is still put to [`Job::output_refusal`], so an
/// absolute path that *is* a source directory is refused like any other.
fn destination_dir(destination: &str, source_dir: &Path) -> Result<PathBuf, &'static str> {
    match destination {
        "new-folder" => Ok(default_output_dir(source_dir)),
        "source-folder" => Ok(source_dir.to_path_buf()),
        other if Path::new(other).is_absolute() => Ok(PathBuf::from(other)),
        _ => Err("notice.export.refusedDestination"),
    }
}

/// One file per page, streamed, and a mask file beside each when asked for.
/// Returns how many pages were written.
fn write_pages(
    job: &Job,
    target: Target,
    masks: MaskFiles,
    destination_dir: &Path,
) -> Result<u32, String> {
    let strip = strip_of(job);
    let longstrip = job.project.strip.mode == StripMode::Longstrip;

    let mut file_count = 0u32;
    for (position, &source_idx) in job.project.strip.order.iter().enumerate() {
        let Some(source) = job.project.sources.get(source_idx) else { continue };
        let Some(source_path) = job.source_path(source_idx) else { continue };
        let bytes =
            std::fs::read(&source_path).map_err(|e| format!("{}: {e}", source_path.display()))?;
        let patches = page_patches_for(job, &strip, longstrip, position, source_idx)?;

        // Streaming means the file is opened before the export decides
        // anything, so the name has to be settled first. `planned_format` is
        // the export's own decision reached from a header read rather than a
        // second copy of the rule - the one thing that could change it is a
        // mode the target format cannot carry, which a header knows.
        let planned = cleaner_core::export::planned_format(&bytes, target)
            .map_err(|e| format!("{}: {e}", source_path.display()))?;
        let out_path = destination_dir.join(output_filename(source, extension(planned)));
        let summary = write_file(&out_path, |file| {
            export_page_to(&bytes, &patches, target, file)
                .map_err(|e| format!("{}: {e}", source_path.display()))
        })?;
        debug_assert_eq!(summary.format, planned, "the file was named for another format");

        if masks == MaskFiles::Beside {
            write_mask_file(&destination_dir.join(mask_filename(source)), &patches, source.w, source.h)?;
        }
        file_count += 1;
    }
    Ok(file_count)
}

/// One `.psd` per page. Returns how many were written.
///
/// No passthrough and no format to predict: a PSD is a PSD, and a page nothing
/// touched still becomes one because that is what was asked for.
fn write_layered(job: &Job, layered: bool, destination_dir: &Path) -> Result<u32, String> {
    let strip = strip_of(job);
    let longstrip = job.project.strip.mode == StripMode::Longstrip;

    let mut file_count = 0u32;
    for (position, &source_idx) in job.project.strip.order.iter().enumerate() {
        let Some(source) = job.project.sources.get(source_idx) else { continue };
        let Some(source_path) = job.source_path(source_idx) else { continue };
        let bytes =
            std::fs::read(&source_path).map_err(|e| format!("{}: {e}", source_path.display()))?;
        let patches = page_patches_for(job, &strip, longstrip, position, source_idx)?;
        let out_path = destination_dir.join(output_filename(source, "psd"));
        write_file(&out_path, |file| {
            export_page_psd_to(&bytes, &patches, layered, file)
                .map_err(|e| format!("{}: {e}", source_path.display()))
        })?;
        file_count += 1;
    }
    Ok(file_count)
}

/// `<stem>_mask.png`: the union of the page's ink masks - the lettering the
/// visible edits removed, not the area they painted - white inside.
fn write_mask_file(path: &Path, patches: &[Patch], width: u32, height: u32) -> Result<(), String> {
    let masks: Vec<&Mask> = patches.iter().filter(|p| p.visible).map(|p| &p.ink).collect();
    write_file(path, |file| {
        export_mask_to(&masks, width, height, file).map_err(|e| format!("{}: {e}", path.display()))
    })
    .map(|_| ())
}

/// Create `path`, run `write` into it, flush - and remove the file if either
/// fails.
///
/// A failure has already created the file. An export that stops partway
/// leaves the pages it finished, which it always did; what it must not leave
/// is a *truncated* page, which looks like a cleaned one until it is opened.
fn write_file<T>(
    path: &Path,
    write: impl FnOnce(&mut std::io::BufWriter<std::fs::File>) -> Result<T, String>,
) -> Result<T, String> {
    let mut file = std::io::BufWriter::new(
        std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?,
    );
    let written = write(&mut file).and_then(|value| {
        std::io::Write::flush(&mut file)
            .map(|()| value)
            .map_err(|e| format!("{}: {e}", path.display()))
    });
    if written.is_err() {
        drop(file);
        let _ = std::fs::remove_file(path);
    }
    written
}

/// The same per-page files, inside one zip. Returns how many pages went in.
///
/// This is the one export that buffers a page's encoded file, because a stored
/// zip entry carries its own CRC-32 and length ahead of its data and both are
/// facts about the finished file. One page at a time - the entry is added and
/// the buffer dropped before the next source is read - so the resident cost is
/// a page, which is the unit the memory budget is stated in.
fn write_archive(job: &Job, path: &Path) -> Result<u32, String> {
    let strip = strip_of(job);
    let longstrip = job.project.strip.mode == StripMode::Longstrip;

    let file = std::io::BufWriter::new(
        std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?,
    );
    let mut archive = Cbz::new(file);
    let mut file_count = 0u32;

    let packed = (|| -> Result<u32, String> {
        for (position, &source_idx) in job.project.strip.order.iter().enumerate() {
            let Some(source) = job.project.sources.get(source_idx) else { continue };
            let Some(source_path) = job.source_path(source_idx) else { continue };
            let bytes = std::fs::read(&source_path)
                .map_err(|e| format!("{}: {e}", source_path.display()))?;
            let patches = page_patches_for(job, &strip, longstrip, position, source_idx)?;
            let page = export_page(&bytes, &patches, Target::SameAsSource)
                .map_err(|e| format!("{}: {e}", source_path.display()))?;
            archive
                .add(&output_filename(source, extension(page.format)), &page.bytes)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            file_count += 1;
        }
        Ok(file_count)
    })();

    match packed {
        // A zip is not a zip until its central directory is written, so a
        // half-packed archive is not a file with some pages missing - it is a
        // file nothing can open. Removing it is the honest outcome.
        Err(error) => {
            let _ = std::fs::remove_file(path);
            Err(error)
        }
        Ok(count) => {
            archive.finish().map_err(|e| format!("{}: {e}", path.display()))?;
            Ok(count)
        }
    }
}

/// One file for the whole strip, rows at a time, one page resident - and its
/// mask file beside it when asked for, from the same global patch set in the
/// same strip coordinates.
fn write_stitched(
    job: &Job,
    target: Target,
    masks: MaskFiles,
    destination_dir: &Path,
) -> Result<ExportResult, String> {
    let strip = strip_of(job);
    if strip.pages().is_empty() {
        return Ok(ExportResult::refused(
            cleaner_core::export::StitchRefusal::NoPages.reason_key(),
            None,
        ));
    }

    let mut pages = JobPages { job };
    // Page 0's header names the file, and it is also the first thing
    // `export_stitched` reads: every other page has to agree with it or the
    // strip is refused. Read here rather than predicted, for the same reason
    // the per-page path calls `planned_format`.
    let first = match pages.header(0) {
        Ok(header) => header,
        Err(error) => return Err(error.to_string()),
    };
    let planned = planned_stitched_format(&first, target);
    let out_path = destination_dir.join(format!("{}.{}", chapter_stem(job), extension(planned)));

    let global = global_patches(job, &strip)?;
    let mut file = std::io::BufWriter::new(
        std::fs::File::create(&out_path).map_err(|e| format!("{}: {e}", out_path.display()))?,
    );
    let written = export_stitched(&strip, &global, &mut pages, target, &mut file)
        .and_then(|stitched| {
            std::io::Write::flush(&mut file)
                .map(|()| stitched)
                .map_err(cleaner_core::export::ExportError::from)
        });

    match written {
        Ok(stitched) => {
            debug_assert_eq!(stitched.format, planned, "the file was named for another format");
            if masks == MaskFiles::Beside {
                let strip_masks: Vec<&Mask> = global
                    .iter()
                    .map(StripPatch::in_strip_coordinates)
                    .filter(|p| p.visible)
                    .map(|p| &p.ink)
                    .collect();
                let mask_path = destination_dir.join(format!("{}_mask.png", chapter_stem(job)));
                write_file(&mask_path, |file| {
                    export_mask_to(&strip_masks, strip.width(), strip.height(), file)
                        .map_err(|e| format!("{}: {e}", mask_path.display()))
                })?;
            }
            Ok(ExportResult {
                status: "exported",
                file_count: Some(1),
                path: Some(out_path.display().to_string()),
                reason_key: None,
                gutter_pixels: Some(stitched.gutter_pixels),
            })
        }
        Err(error) => {
            // `export_stitched` decides every refusal from headers before it
            // writes a byte, so the file this removes is the empty one
            // `File::create` just made.
            let _ = std::fs::remove_file(&out_path);
            match error {
                cleaner_core::export::ExportError::NotStitchable(refusal) => {
                    Ok(ExportResult::refused(refusal.reason_key(), None))
                }
                other => Err(other.to_string()),
            }
        }
    }
}

/// The job's pages, read off disk one at a time.
///
/// The in-memory implementation of the same trait is in `export::stitched`'s
/// own tests; this is the one that goes to files, and it holds nothing between
/// calls, which is the whole point of the trait having two methods.
struct JobPages<'a> {
    job: &'a Job,
}

impl JobPages<'_> {
    fn bytes(&self, position: usize) -> Result<Vec<u8>, cleaner_core::export::ExportError> {
        let sink = |message: String| cleaner_core::export::ExportError::Sink(message);
        let source_idx = *self
            .job
            .project
            .strip
            .order
            .get(position)
            .ok_or_else(|| sink(format!("position {position} is not in the strip order")))?;
        let path = self
            .job
            .source_path(source_idx)
            .ok_or_else(|| sink(format!("source {source_idx} has no path")))?;
        std::fs::read(&path).map_err(|e| sink(format!("{}: {e}", path.display())))
    }
}

impl PageSource for JobPages<'_> {
    fn header(&mut self, position: usize) -> Result<Header, cleaner_core::export::ExportError> {
        let bytes = self.bytes(position)?;
        let format = Format::sniff(&bytes)
            .ok_or(cleaner_core::image::ImageError::UnknownFormat)?;
        Ok(match format {
            Format::Png => png_header(&bytes)?,
            Format::Tiff => tiff_header(&bytes)?,
        })
    }

    fn page(&mut self, position: usize) -> Result<Raster, cleaner_core::export::ExportError> {
        Ok(decode(&self.bytes(position)?)?)
    }
}

/// The whole patch set, lifted into strip coordinates.
///
/// **This is the one place the on-disk discipline gives way**, and it is
/// worth being plain about rather than leaving as an accident of the signature.
/// `export_stitched` takes the global set as a slice, so every patch buffer in
/// the chapter is resident while the strip is written - where the per-page path
/// selects from `bbox` fields and loads only what it needs. The pixels the rule
/// is really about are still one page at a time; what is held is the patch set,
/// which for a heavily cleaned 200-page chapter is not nothing.
fn global_patches(job: &Job, strip: &Strip) -> Result<Vec<StripPatch>, String> {
    let mut global = Vec::new();
    for record in &job.project.patches {
        let Some(anchor) = job.project.strip.order.iter().position(|i| *i == record.source_idx)
        else {
            continue;
        };
        let patch = load(job, record)?;
        let Some(lifted) = StripPatch::lift(strip, anchor, &patch) else { continue };
        global.push(lifted);
    }
    Ok(global)
}

/// Which patches one page's file gets: the intersection for a longstrip job,
/// ownership for a paginated one. Shared by the per-page and the archive
/// writers, which differ in where the encoded file goes and in nothing else.
fn page_patches_for(
    job: &Job,
    strip: &Strip,
    longstrip: bool,
    position: usize,
    source_idx: usize,
) -> Result<Vec<Patch>, String> {
    if longstrip { page_patches(job, strip, position) } else { owned_patches(job, source_idx) }
}

/// The strip's coordinate system, from the manifest's `sources` and
/// `strip.order`.
///
/// Rebuilt here rather than persisted, which is what
/// [`cleaner_core::strip::Strip`]'s own doc says to do: the manifest holds the
/// order and the page dimensions, and the placements follow from them. The
/// placement at position `k` is the page `strip.order[k]` names - the two
/// indices differ whenever a user has reordered pages, and everything below
/// takes the position, never the source index.
fn strip_of(job: &Job) -> Strip {
    let sizes: Vec<(u32, u32)> = job
        .project
        .strip
        .order
        .iter()
        .filter_map(|i| job.project.sources.get(*i).map(|s| (s.w, s.h)))
        .collect();
    Strip::of_sizes(&sizes)
}

/// Every applied patch recorded against one source, reconstructed from the
/// manifest and its sidecar buffers.
///
/// **Ownership**, and correct for exactly the case it is used in: a `Single`
/// project, where a page is a page and there is no strip to span.
fn owned_patches(job: &Job, source_idx: usize) -> Result<Vec<Patch>, String> {
    job.project
        .patches
        .iter()
        .filter(|record| record.source_idx == source_idx)
        .map(|record| load(job, record))
        .collect()
}

/// **The intersection**, for a longstrip job: every part of every patch in the
/// global set that lands on the page at `position`, in that page's own
/// coordinates.
///
/// The two steps are ordered by the memory rule, not by taste. Selection is
/// from `bbox` alone, which is in the manifest; only the records that survive
/// it are loaded off disk. Building the whole global set first and then asking
/// which parts of it touch this page would hold every patch in the chapter at
/// once.
fn page_patches(job: &Job, strip: &Strip, position: usize) -> Result<Vec<Patch>, String> {
    let mut patches = Vec::new();
    for (anchor, record) in intersecting_records(job, strip, position) {
        let patch = load(job, record)?;
        let Some(lifted) = StripPatch::lift(strip, anchor, &patch) else { continue };
        patches.extend(patches_on_page(strip, position, std::slice::from_ref(&lifted)));
    }
    Ok(patches)
}

/// Which patch records can reach the page at `position`, decided from
/// rectangles alone.
///
/// A record's `bbox` is in the coordinate system of the page its `source_idx`
/// names, and a patch that spans a join is one whose bbox runs past that page's
/// own rectangle. That is the whole of the representation: there is no separate
/// "join-spanning" flag, and there does not need to be one, because the strip
/// geometry answers the question from the rectangle.
fn intersecting_records<'a>(
    job: &'a Job,
    strip: &Strip,
    position: usize,
) -> Vec<(usize, &'a PatchRecord)> {
    let Some(page) = strip.pages().get(position).map(|p| p.rect()) else { return Vec::new() };
    job.project
        .patches
        .iter()
        .filter_map(|record| {
            let anchor = job.project.strip.order.iter().position(|i| *i == record.source_idx)?;
            let (x, y) = strip.to_strip(anchor, record.bbox.x, record.bbox.y)?;
            let bbox = Rect::new(x, y, record.bbox.w, record.bbox.h);
            (bbox.x < page.right()
                && bbox.right() > page.x
                && bbox.y < page.bottom()
                && bbox.bottom() > page.y)
                .then_some((anchor, record))
        })
        .collect()
}

fn load(job: &Job, record: &PatchRecord) -> Result<Patch, String> {
    job.load_patch(record).map_err(|e| format!("{}: {e}", record.id))
}

/// The directory the chapter's sources live in, for `'new-folder'`'s sibling
/// and for recognising `'source-folder'`. Taken from the first source: a
/// chapter's pages are ingested from one folder in practice, and
/// `Job::output_refusal` itself checks every source's own directory, so a
/// destination that matches only one of several source directories is still
/// caught correctly.
fn source_directory(job: &Job) -> PathBuf {
    // `origin_path` and not `source_path`: a page converted at ingest has its
    // file in the job's sidecar and its *origin* in the user's scan folder, and
    // both `'new-folder'` and `'source-folder'` are about the scan folder. A
    // sibling of the sidecar would put the export inside the application's own
    // library, where the user would never find it.
    job.origin_path(0)
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| job.root().to_path_buf())
}

/// What a one-file export is called, before its extension: the manifest's own
/// stem, which is the chapter's name on disk.
fn chapter_stem(job: &Job) -> String {
    job.path().file_stem().and_then(|s| s.to_str()).unwrap_or("chapter").to_owned()
}

fn output_filename(source: &ProjectSource, extension: &str) -> String {
    format!("{}.{extension}", source_stem(source))
}

/// `001_mask.png` beside `001.png`: the mask file is always a PNG, whatever
/// the image is, because it is an 8-bit grayscale mask and not a page.
fn mask_filename(source: &ProjectSource) -> String {
    format!("{}_mask.png", source_stem(source))
}

fn source_stem(source: &ProjectSource) -> &str {
    source.rel_path.file_stem().and_then(|s| s.to_str()).unwrap_or("page")
}

fn extension(format: Format) -> &'static str {
    match format {
        Format::Png => "png",
        Format::Tiff => "tiff",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cleaner_core::composite::{changed_pixels, permitted_region};
    use cleaner_core::image::{Format, encode, fixtures};
    use cleaner_core::ingest::SourceRef;
    use cleaner_core::mask::Mask;
    use cleaner_core::patch::{Engine, Provenance};
    use cleaner_core::project::Project;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            use std::sync::atomic::{AtomicU32, Ordering};
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir()
                .join("cleaner-tauri-export")
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
        cleaner_core::ingest::source_ref(&path, &bytes).unwrap()
    }

    /// A grayscale page narrower than the rest of the strip, which is the only
    /// thing that produces a gutter (the pages are centred).
    fn write_narrow_source(dir: &Path, name: &str, width: u32) -> SourceRef {
        std::fs::create_dir_all(dir).unwrap();
        let mut page = fixtures::by_name("l8").raster;
        page.data = (0..page.height)
            .flat_map(|y| (0..width).map(move |x| ((x + y) % 251) as u8))
            .collect();
        page.width = width;
        let bytes = encode(&page, Format::Png).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, &bytes).unwrap();
        cleaner_core::ingest::source_ref(&path, &bytes).unwrap()
    }

    fn a_job(scratch: &Scratch) -> Job {
        let source = write_source(&scratch.join("raws"), "001.png", "l8");
        let manifest = scratch.join("out/chapter.mtclean");
        std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        let project = Project::new(manifest.parent().unwrap(), "0.1.0", StripMode::Single, &[source]);
        Job::create(&manifest, project).unwrap()
    }

    /// Three 64×48 grayscale pages, stacked. The joins are at strip rows 48 and
    /// 96.
    fn a_strip_job(scratch: &Scratch, mode: StripMode) -> Job {
        let sources: Vec<SourceRef> = (0..3)
            .map(|n| write_source(&scratch.join("raws"), &format!("{n:03}.png"), "l8"))
            .collect();
        let manifest = scratch.join("out/chapter.mtclean");
        std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        let project = Project::new(manifest.parent().unwrap(), "0.1.0", mode, &sources);
        Job::create(&manifest, project).unwrap()
    }

    /// A rung-0 planar fill anchored on `source_idx`, in that page's own
    /// coordinates - and deliberately taller than the page, which is what a
    /// patch that spans a join looks like in a manifest. `PatchRecord.bbox` has
    /// no separate flag for it and needs none: the strip geometry answers from
    /// the rectangle.
    fn overhanging_fill(job: &mut Job, source_idx: usize, bounds: Rect, tone: u8) {
        let page = fixtures::by_name("l8").raster;
        let mut pixels = page.clone();
        pixels.width = bounds.w;
        pixels.height = bounds.h;
        pixels.icc = None;
        pixels.data = vec![tone; (bounds.w * bounds.h) as usize];
        let patch = Patch {
            id: "across-the-join".into(),
            mask: Mask::filled(bounds),
            ink: Mask::filled(bounds),
            pixels,
            order: 0,
            visible: true,
            provenance: provenance(),
        };
        job.complete_region(source_idx, &patch, None).unwrap();
    }

    fn provenance() -> Provenance {
        Provenance {
            engine: Engine::Fill,
            engine_version: "test".into(),
            model_sha256: None,
            execution_provider: "cpu".into(),
            params_snapshot: serde_json::json!({}),
            mask_sha256: "0".repeat(64),
            source_sha256: "0".repeat(64),
            cloud: None,
            created: 0,
        }
    }

    /// `export_open_job` with the two parameters most tests do not vary.
    fn export(job: &Job, format: &str, destination: &str) -> ExportResult {
        export_open_job(job, format, destination, "flattened", "per-page").unwrap()
    }

    /// A page with no applied patches is copied byte-for-byte, and lands in
    /// the sibling `_cleaned` directory `default_output_dir` names.
    #[test]
    fn a_page_with_no_patches_is_exported_byte_for_byte() {
        let scratch = Scratch::new("passthrough");
        let job = a_job(&scratch);
        let source_bytes = std::fs::read(job.source_path(0).unwrap()).unwrap();

        let result = export(&job, "PNG", "new-folder");
        assert_eq!(result.status, "exported");
        assert_eq!(result.file_count, Some(1));

        let out_dir = PathBuf::from(result.path.unwrap());
        assert_eq!(out_dir, scratch.join("raws_cleaned").canonicalize().unwrap());
        assert_eq!(std::fs::read(out_dir.join("001.png")).unwrap(), source_bytes);
    }

    /// The refusal this module must not re-decide: `Job::output_refusal`
    /// already says no, and this pins that the command reports it exactly:
    /// `status: 'refused'`, the one reasonKey, no file count,
    /// and the directory that was refused.
    #[test]
    fn exporting_to_the_source_folder_is_refused_rather_than_written() {
        let scratch = Scratch::new("refuse");
        let job = a_job(&scratch);

        let result = export(&job, "PNG", "source-folder");
        assert_eq!(result.status, "refused");
        assert_eq!(result.reason_key, Some("notice.export.refusedOverwrite"));
        assert_eq!(result.file_count, None);
        assert!(
            result.path.is_some_and(|path| path.ends_with("raws")),
            "the refusal did not name the directory it refused"
        );
        assert!(std::fs::read_dir(scratch.join("raws_cleaned")).is_err(), "wrote anyway");
    }

    /// An applied patch reconstructed from the manifest and its sidecar
    /// buffers reaches `export_page`, and the composited page - not the raw
    /// source - is what lands on disk.
    #[test]
    fn a_completed_region_is_composited_into_the_export() {
        let scratch = Scratch::new("patch");
        let mut job = a_job(&scratch);
        let source_bytes = std::fs::read(job.source_path(0).unwrap()).unwrap();
        fill_page(&mut job, 0, Rect::new(4, 4, 8, 8), 10);

        let result = export(&job, "PNG", "new-folder");
        assert_eq!(result.status, "exported");
        assert_eq!(result.file_count, Some(1));

        let out_dir = PathBuf::from(result.path.unwrap());
        let written = std::fs::read(out_dir.join("001.png")).unwrap();
        assert_ne!(written, source_bytes, "the patch changed nothing");
    }

    /// A patch wholly inside one page, for the tests that only need the export
    /// to have something to composite.
    fn fill_page(job: &mut Job, source_idx: usize, bounds: Rect, tone: u8) {
        let mut pixels = fixtures::by_name("l8").raster;
        pixels.width = bounds.w;
        pixels.height = bounds.h;
        pixels.icc = None;
        pixels.data = vec![tone; (bounds.w * bounds.h) as usize];
        let patch = Patch {
            id: format!("p{source_idx}"),
            mask: Mask::filled(bounds),
            ink: Mask::filled(bounds),
            pixels,
            order: 0,
            visible: true,
            provenance: provenance(),
        };
        job.complete_region(source_idx, &patch, None).unwrap();
    }

    /* -------------------------------------------------------------- */
    /* The refusals                                                    */
    /* -------------------------------------------------------------- */

    /// **The defect this phase came for.** `target_for` used to map every
    /// format it did not recognise to `Target::SameAsSource`, so a user who
    /// chose JPEG got PNG and was told the export succeeded. Half of the old
    /// argument survives - the fallback never produced *lossy* output, so
    /// that contract was never violated by it - and the other half does not:
    /// it produced a container nobody asked for, without saying so.
    #[test]
    fn a_format_this_build_cannot_write_is_refused_by_name() {
        assert_eq!(requested_format("PNG"), Ok(Requested::Png));
        assert_eq!(requested_format("png"), Ok(Requested::Png));
        assert_eq!(requested_format("TIFF"), Ok(Requested::Tiff));
        assert_eq!(requested_format("tif"), Ok(Requested::Tiff));
        assert_eq!(requested_format("CBZ"), Ok(Requested::Cbz));

        assert_eq!(requested_format("JPEG"), Err("notice.export.refusedLossyFormat"));
        assert_eq!(requested_format("jpg"), Err("notice.export.refusedLossyFormat"));
        assert_eq!(requested_format("WebP"), Err("notice.export.refusedLossyFormat"));
        assert_eq!(requested_format("PSD"), Ok(Requested::Psd));
        assert_eq!(requested_format("psb"), Err("notice.export.refusedLayeredFormat"));
        assert_eq!(requested_format("AVIF"), Err("notice.export.refusedUnknownFormat"));
        assert_eq!(requested_format(""), Err("notice.export.refusedUnknownFormat"));
    }

    /// And the refusal reaches disk as a refusal: nothing is created, and the
    /// result carries the reason rather than a file count.
    #[test]
    fn a_refused_format_writes_nothing_at_all() {
        let scratch = Scratch::new("jpeg");
        let job = a_job(&scratch);

        let result = export(&job, "JPEG", "new-folder");
        assert_eq!(result.status, "refused");
        assert_eq!(result.reason_key, Some("notice.export.refusedLossyFormat"));
        assert_eq!(result.file_count, None);
        assert!(
            std::fs::read_dir(scratch.join("raws_cleaned")).is_err(),
            "a refused format created the output directory"
        );
    }

    /// `masks` branches. For a raster format, `'separate-layer'` is a mask file
    /// beside each page - `001.png` and `001_mask.png` - white where the
    /// lettering was and black everywhere else, the page's own size. The
    /// fixture's ink is its whole mask, so the two readings agree here; the
    /// test after this one is where they differ.
    #[test]
    fn separate_layer_writes_a_mask_file_beside_each_page() {
        let scratch = Scratch::new("mask-files");
        let mut job = a_job(&scratch);
        fill_page(&mut job, 0, Rect::new(4, 4, 8, 8), 10);

        let result =
            export_open_job(&job, "PNG", "new-folder", "separate-layer", "per-page").unwrap();
        assert_eq!(result.status, "exported");
        assert_eq!(result.file_count, Some(1), "the count is pages, not files");
        let out_dir = PathBuf::from(result.path.unwrap());
        assert!(out_dir.join("001.png").exists());

        let mask = cleaner_core::image::decode(&std::fs::read(out_dir.join("001_mask.png")).unwrap())
            .unwrap();
        assert_eq!((mask.width, mask.height), (64, 48));
        assert_eq!(mask.mode, cleaner_core::image::ColorMode::Gray);
        assert_eq!(mask.depth, cleaner_core::image::BitDepth::Eight);
        for y in 0..48 {
            for x in 0..64 {
                let inside = (4..12).contains(&x) && (4..12).contains(&y);
                assert_eq!(mask.sample(x, y, 0), if inside { 255 } else { 0 }, "({x}, {y})");
            }
        }

        // TIFF pages get the same PNG mask beside them: a mask is not a page.
        let tiff = export_open_job(
            &job,
            "TIFF",
            scratch.join("tiff").to_str().unwrap(),
            "separate-layer",
            "per-page",
        )
        .unwrap();
        let tiff_dir = PathBuf::from(tiff.path.unwrap());
        assert!(tiff_dir.join("001.tiff").exists());
        assert!(tiff_dir.join("001_mask.png").exists());

        // A flattened export writes no mask file, which is what it always did.
        let flat = export(&job, "PNG", scratch.join("flat").to_str().unwrap());
        assert!(!PathBuf::from(flat.path.unwrap()).join("001_mask.png").exists());

        assert_eq!(mask_files("flattened"), MaskFiles::None);
        assert_eq!(mask_files("separate-layer"), MaskFiles::Beside);
    }

    /// **The mask file is the lettering, not the fill.** A balloon rung 0
    /// cleaned has an applied mask the size of the balloon, and a mask file
    /// made of applied masks was a page of white blobs where the balloons were.
    /// The file is made of each patch's ink instead: white where the text was,
    /// black over the rest of the fill.
    #[test]
    fn the_mask_file_is_the_ink_and_not_the_applied_mask() {
        let scratch = Scratch::new("mask-files-ink");
        let mut job = a_job(&scratch);
        let balloon = Rect::new(4, 4, 24, 20);
        let lettering = Rect::new(10, 8, 6, 10);
        let mut pixels = fixtures::by_name("l8").raster;
        pixels.width = balloon.w;
        pixels.height = balloon.h;
        pixels.icc = None;
        pixels.data = vec![10; (balloon.w * balloon.h) as usize];
        let patch = Patch {
            id: "balloon".into(),
            mask: Mask::filled(balloon),
            ink: Mask::filled(lettering),
            pixels,
            order: 0,
            visible: true,
            provenance: provenance(),
        };
        job.complete_region(0, &patch, None).unwrap();

        let result =
            export_open_job(&job, "PNG", "new-folder", "separate-layer", "per-page").unwrap();
        let out_dir = PathBuf::from(result.path.unwrap());
        let mask = cleaner_core::image::decode(&std::fs::read(out_dir.join("001_mask.png")).unwrap())
            .unwrap();
        for y in 0..48 {
            for x in 0..64 {
                let text = lettering.contains(i64::from(x), i64::from(y));
                assert_eq!(mask.sample(x, y, 0), if text { 255 } else { 0 }, "({x}, {y})");
            }
        }

        // The page itself is still the whole fill: the ink changes what the
        // mask file says, not what was painted.
        let page = cleaner_core::image::decode(&std::fs::read(out_dir.join("001.png")).unwrap())
            .unwrap();
        assert_eq!(page.sample(5, 5, 0), 10, "the fill reaches the balloon's edge");
        assert_eq!(mask_files("sideways"), MaskFiles::None, "an unknown choice is the default");
    }

    /// A mask file inside a CBZ would be read as a page, so the pair is
    /// refused rather than packed.
    #[test]
    fn a_mask_file_cannot_go_inside_an_archive() {
        let scratch = Scratch::new("mask-cbz");
        let job = a_job(&scratch);

        let result =
            export_open_job(&job, "CBZ", "new-folder", "separate-layer", "per-page").unwrap();
        assert_eq!(result.status, "refused");
        assert_eq!(result.reason_key, Some("notice.export.refusedMaskLayers"));
        assert!(std::fs::read_dir(scratch.join("raws_cleaned")).is_err(), "wrote anyway");
    }

    /* -------------------------------------------------------------- */
    /* PSD                                                             */
    /* -------------------------------------------------------------- */

    /// The header fields of a PSD, read the way a reader would: enough to
    /// know the file is the page's size, mode and depth. The layer structure
    /// is pinned in `cleaner_core::export::psd`'s own tests against a
    /// specification reader; here the claim is that the command reaches it.
    fn psd_header(bytes: &[u8]) -> (u16, u32, u32, u16, u16) {
        assert_eq!(&bytes[..4], b"8BPS");
        let be16 = |at: usize| u16::from_be_bytes([bytes[at], bytes[at + 1]]);
        let be32 = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
        (be16(12), be32(14), be32(18), be16(22), be16(24))
    }

    /// The UTF-16BE `luni` block Photoshop reads a layer's name from.
    fn has_layer_named(bytes: &[u8], name: &str) -> bool {
        let mut needle = b"8BIMluni".to_vec();
        let units: Vec<u16> = name.encode_utf16().collect();
        let mut data = (units.len() as u32).to_be_bytes().to_vec();
        for unit in units {
            data.extend_from_slice(&unit.to_be_bytes());
        }
        if data.len() % 2 == 1 {
            data.push(0);
        }
        needle.extend_from_slice(&(data.len() as u32).to_be_bytes());
        needle.extend_from_slice(&data);
        bytes.windows(needle.len()).any(|w| w == needle)
    }

    /// The layered shape: one `.psd` per page, the untouched source as
    /// `Background`, each region a masked layer in a `Cleaned` group - and,
    /// flattened, one `Background` layer of the composite with no group.
    #[test]
    fn a_psd_is_written_layered_or_flattened_as_asked() {
        let scratch = Scratch::new("psd");
        let mut job = a_strip_job(&scratch, StripMode::Longstrip);
        overhanging_fill(&mut job, 0, Rect::new(10, 40, 20, 16), 200);

        let layered = export_open_job(
            &job,
            "PSD",
            scratch.join("layered").to_str().unwrap(),
            "separate-layer",
            "per-page",
        )
        .unwrap();
        assert_eq!(layered.status, "exported");
        assert_eq!(layered.file_count, Some(3));
        let layered_dir = PathBuf::from(layered.path.unwrap());
        for n in 0..3 {
            let bytes = std::fs::read(layered_dir.join(format!("{n:03}.psd"))).unwrap();
            assert_eq!(psd_header(&bytes), (1, 48, 64, 8, 1), "page {n}: channels, h, w, depth, mode");
            assert!(has_layer_named(&bytes, "Background"), "page {n}");
        }
        // The patch spans the join, so pages 0 and 1 have a region and page 2
        // has none - and no group.
        let page0 = std::fs::read(layered_dir.join("000.psd")).unwrap();
        let page1 = std::fs::read(layered_dir.join("001.psd")).unwrap();
        let page2 = std::fs::read(layered_dir.join("002.psd")).unwrap();
        assert!(has_layer_named(&page0, "Cleaned") && has_layer_named(&page0, "region-0001"));
        assert!(has_layer_named(&page1, "Cleaned") && has_layer_named(&page1, "region-0001"));
        assert!(!has_layer_named(&page2, "Cleaned"));

        let flat = export_open_job(
            &job,
            "PSD",
            scratch.join("flat").to_str().unwrap(),
            "flattened",
            "per-page",
        )
        .unwrap();
        assert_eq!(flat.file_count, Some(3));
        let flat_page0 = std::fs::read(PathBuf::from(flat.path.unwrap()).join("000.psd")).unwrap();
        assert!(has_layer_named(&flat_page0, "Background"));
        assert!(!has_layer_named(&flat_page0, "Cleaned"), "a flattened PSD has no group");
        assert!(flat_page0.len() < page0.len(), "one layer is smaller than three");
    }

    /// A page PSD cannot carry refuses the chapter from the manifest, before
    /// a directory exists; and a stitched PSD is refused by name.
    #[test]
    fn a_psd_of_what_psd_cannot_carry_is_refused_before_anything_is_written() {
        let scratch = Scratch::new("psd-refused");
        let raws = scratch.join("raws");
        let sources = vec![
            write_source(&raws, "000.png", "l8"),
            write_source(&raws, "001.png", "indexed-p"),
        ];
        let manifest = scratch.join("out/chapter.mtclean");
        std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        let project =
            Project::new(manifest.parent().unwrap(), "0.1.0", StripMode::Single, &sources);
        let job = Job::create(&manifest, project).unwrap();

        let result = export_open_job(&job, "PSD", "new-folder", "flattened", "per-page").unwrap();
        assert_eq!(result.status, "refused");
        assert_eq!(result.reason_key, Some("notice.export.refusedLayeredMode"));
        assert!(std::fs::read_dir(scratch.join("raws_cleaned")).is_err(), "wrote anyway");

        let strip_job = a_strip_job(&scratch, StripMode::Longstrip);
        let stitched =
            export_open_job(&strip_job, "PSD", "new-folder", "flattened", "stitched").unwrap();
        assert_eq!(stitched.status, "refused");
        assert_eq!(stitched.reason_key, Some("notice.export.refusedStitchedLayered"));
    }

    /// The seam takes an absolute path, and refuses a
    /// relative one rather than resolving it against a working directory
    /// neither end of the seam chose.
    #[test]
    fn an_absolute_destination_is_written_to_and_a_relative_one_is_refused() {
        let scratch = Scratch::new("destination");
        let job = a_job(&scratch);
        let elsewhere = scratch.join("somewhere/else");

        let result = export(&job, "PNG", elsewhere.to_str().unwrap());
        assert_eq!(result.status, "exported");
        assert_eq!(result.file_count, Some(1));
        assert_eq!(PathBuf::from(result.path.unwrap()), elsewhere.canonicalize().unwrap());
        assert!(elsewhere.join("001.png").exists());

        let refused = export(&job, "PNG", "../up/one");
        assert_eq!(refused.status, "refused");
        assert_eq!(refused.reason_key, Some("notice.export.refusedDestination"));
    }

    /// An absolute path is still put to `Job::output_refusal`: naming the
    /// source directory the long way round is the same request.
    #[test]
    fn an_absolute_path_at_the_source_folder_is_refused_like_the_sentinel() {
        let scratch = Scratch::new("absolute-source");
        let job = a_job(&scratch);
        let raws = scratch.join("raws");

        let result = export(&job, "PNG", raws.to_str().unwrap());
        assert_eq!(result.status, "refused");
        assert_eq!(result.reason_key, Some("notice.export.refusedOverwrite"));
    }

    /* -------------------------------------------------------------- */
    /* CBZ                                                             */
    /* -------------------------------------------------------------- */

    /// A CBZ is the per-page files inside a zip, and the pages in it are the
    /// *same bytes* the per-page export writes - which is the whole claim, and
    /// the reason the container is not an encoder. Asserted against a second
    /// export rather than against a re-encode.
    #[test]
    fn a_cbz_holds_exactly_what_the_per_page_export_writes() {
        let scratch = Scratch::new("cbz");
        let mut job = a_strip_job(&scratch, StripMode::Longstrip);
        overhanging_fill(&mut job, 0, Rect::new(10, 40, 20, 16), 200);

        let pages = export(&job, "PNG", scratch.join("pages").to_str().unwrap());
        assert_eq!(pages.file_count, Some(3));
        let pages_dir = PathBuf::from(pages.path.unwrap());

        let archive = export(&job, "CBZ", scratch.join("archive").to_str().unwrap());
        assert_eq!(archive.status, "exported");
        assert_eq!(archive.file_count, Some(3));
        let archive_path = PathBuf::from(archive.path.unwrap());
        assert_eq!(archive_path.extension().unwrap(), "cbz");
        assert_eq!(archive_path.file_name().unwrap(), "chapter.cbz");

        let entries = unzip(&std::fs::read(&archive_path).unwrap());
        assert_eq!(
            entries.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(),
            ["000.png", "001.png", "002.png"],
            "the archive is not the chapter in strip order"
        );
        for (name, bytes) in &entries {
            assert_eq!(
                bytes,
                &std::fs::read(pages_dir.join(name)).unwrap(),
                "{name} in the archive is not the file the per-page export wrote"
            );
        }
    }

    /// The archive's entries decode, and the page that nothing touched is still
    /// the source file byte-for-byte *inside the zip* - the strongest
    /// form of passthrough, reached through the container.
    #[test]
    fn an_untouched_page_survives_the_container_byte_for_byte() {
        let scratch = Scratch::new("cbz-passthrough");
        let mut job = a_strip_job(&scratch, StripMode::Longstrip);
        overhanging_fill(&mut job, 0, Rect::new(10, 40, 20, 16), 200);

        let result = export(&job, "CBZ", "new-folder");
        let entries = unzip(&std::fs::read(result.path.unwrap()).unwrap());
        let by_name = |name: &str| {
            entries.iter().find(|(entry, _)| entry == name).map(|(_, bytes)| bytes.clone()).unwrap()
        };

        let source = |n: usize| std::fs::read(job.source_path(n).unwrap()).unwrap();
        assert_eq!(by_name("002.png"), source(2), "a page nothing reaches was re-encoded");
        assert_ne!(by_name("000.png"), source(0), "the page the patch anchors on");
        assert_ne!(by_name("001.png"), source(1), "the page below the join");

        let below = cleaner_core::image::decode(&by_name("001.png")).unwrap();
        for x in 10..30 {
            assert_eq!(below.sample(x, 0, 0), 200, "column {x} of the page below the join");
        }
    }

    /// Reads an archive the way an unpacker does: end record, central
    /// directory, local headers. The writer's own bookkeeping is not consulted.
    fn unzip(archive: &[u8]) -> Vec<(String, Vec<u8>)> {
        let end = archive.len() - 22;
        assert_eq!(&archive[end..end + 4], &[0x50, 0x4b, 0x05, 0x06], "no end-of-directory record");
        let count = u16::from_le_bytes(archive[end + 10..end + 12].try_into().unwrap()) as usize;
        let mut at = u32::from_le_bytes(archive[end + 16..end + 20].try_into().unwrap()) as usize;

        let mut out = Vec::new();
        for _ in 0..count {
            let size = u32::from_le_bytes(archive[at + 24..at + 28].try_into().unwrap()) as usize;
            let name_len = u16::from_le_bytes(archive[at + 28..at + 30].try_into().unwrap()) as usize;
            let local = u32::from_le_bytes(archive[at + 42..at + 46].try_into().unwrap()) as usize;
            let name = String::from_utf8(archive[at + 46..at + 46 + name_len].to_vec()).unwrap();
            at += 46 + name_len;
            let local_name =
                u16::from_le_bytes(archive[local + 26..local + 28].try_into().unwrap()) as usize;
            let extra =
                u16::from_le_bytes(archive[local + 28..local + 30].try_into().unwrap()) as usize;
            let data = local + 30 + local_name + extra;
            out.push((name, archive[data..data + size].to_vec()));
        }
        out
    }

    /* -------------------------------------------------------------- */
    /* Stitched                                                        */
    /* -------------------------------------------------------------- */

    /// The writer was built, tested and unreachable. This is the
    /// seam parameter reaching it - one file, the strip's dimensions, and the
    /// patch that spans a join landing across the join inside it.
    #[test]
    fn a_stitched_export_writes_one_file_the_height_of_the_strip() {
        let scratch = Scratch::new("stitched");
        let mut job = a_strip_job(&scratch, StripMode::Longstrip);
        overhanging_fill(&mut job, 0, Rect::new(10, 40, 20, 16), 200);

        let result =
            export_open_job(&job, "PNG", "new-folder", "flattened", "stitched").unwrap();
        assert_eq!(result.status, "exported");
        assert_eq!(result.file_count, Some(1), "a stitched export is one file");
        assert_eq!(result.gutter_pixels, Some(0), "three pages of one width have no gutter");

        let path = PathBuf::from(result.path.unwrap());
        assert_eq!(path.file_name().unwrap(), "chapter.png");
        let strip = strip_of(&job);
        let written = cleaner_core::image::decode(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!((written.width, written.height), (strip.width(), strip.height()));

        // The join at strip row 48, from inside one file: the fill is
        // continuous across it.
        for x in 10..30 {
            assert_eq!(written.sample(x, 47, 0), 200, "column {x} above the join");
            assert_eq!(written.sample(x, 48, 0), 200, "column {x} below the join");
        }
    }

    /// A paginated project has no strip. Stacking its pages into one tall file
    /// would invent a document nobody has, so it is refused rather than
    /// offered - and the interface does not offer it either.
    #[test]
    fn a_paginated_project_cannot_be_stitched() {
        let scratch = Scratch::new("stitch-single");
        let job = a_strip_job(&scratch, StripMode::Single);

        let result = export_open_job(&job, "PNG", "new-folder", "flattened", "stitched").unwrap();
        assert_eq!(result.status, "refused");
        assert_eq!(result.reason_key, Some("notice.export.refusedStitchPaginated"));
        assert!(std::fs::read_dir(scratch.join("raws_cleaned")).is_err(), "wrote anyway");
    }

    /// The strip's own refusals reach the seam as refusals with the reason on
    /// them, rather than as an error string the interface would have to
    /// translate. Page 1 is RGB where the strip is grayscale.
    #[test]
    fn a_strip_whose_pages_disagree_refuses_with_the_reason_on_it() {
        let scratch = Scratch::new("stitch-mixed");
        let raws = scratch.join("raws");
        let sources = vec![
            write_source(&raws, "000.png", "l8"),
            write_source(&raws, "001.png", "rgb8"),
            write_source(&raws, "002.png", "l8"),
        ];
        let manifest = scratch.join("out/chapter.mtclean");
        std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        let project =
            Project::new(manifest.parent().unwrap(), "0.1.0", StripMode::Longstrip, &sources);
        let job = Job::create(&manifest, project).unwrap();

        let result = export_open_job(&job, "PNG", "new-folder", "flattened", "stitched").unwrap();
        assert_eq!(result.status, "refused");
        assert_eq!(result.reason_key, Some("notice.export.stitchRefused.mixedMode"));
        assert!(
            !scratch.join("raws_cleaned/chapter.png").exists(),
            "a refused stitch left the file it opened"
        );
    }

    /// The gutter is counted, and the count is on the
    /// seam, which is where the interface reads it back from. Page 1 is
    /// narrower than the strip, so it is the one with white beside it.
    #[test]
    fn a_ragged_strip_reports_the_pixels_no_source_page_had() {
        let scratch = Scratch::new("stitch-gutter");
        let raws = scratch.join("raws");
        let sources = vec![
            write_source(&raws, "000.png", "l8"),
            write_narrow_source(&raws, "001.png", 40),
            write_source(&raws, "002.png", "l8"),
        ];
        let manifest = scratch.join("out/chapter.mtclean");
        std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        let project =
            Project::new(manifest.parent().unwrap(), "0.1.0", StripMode::Longstrip, &sources);
        let job = Job::create(&manifest, project).unwrap();

        let strip = strip_of(&job);
        let expected: u64 = strip
            .pages()
            .iter()
            .map(|page| u64::from(strip.width() - page.width) * u64::from(page.height))
            .sum();
        assert!(expected > 0, "the fixture is not ragged");

        let result = export_open_job(&job, "PNG", "new-folder", "flattened", "stitched").unwrap();
        assert_eq!(result.gutter_pixels, Some(expected));
    }

    /// A CBZ of one stitched file is a shape nobody asked for, and packing it
    /// would mean holding the strip's encoded file whole.
    #[test]
    fn a_stitched_archive_is_refused() {
        let none = MaskFiles::None;
        let beside = MaskFiles::Beside;
        assert_eq!(
            container_for(Requested::Cbz, "stitched", none, true),
            Err("notice.export.refusedStitchedArchive")
        );
        assert_eq!(container_for(Requested::Cbz, "per-page", none, true), Ok(Container::Archive));
        assert_eq!(
            container_for(Requested::Cbz, "per-page", beside, true),
            Err("notice.export.refusedMaskLayers")
        );
        // An unrecognised layout is rule 8's default rather than a refusal:
        // "one file per page" is what every export did before the parameter.
        assert_eq!(
            container_for(Requested::Png, "sideways", none, true),
            Ok(Container::Pages { target: Target::Explicit(Format::Png), masks: none })
        );
        assert_eq!(
            container_for(Requested::Tiff, "stitched", beside, true),
            Ok(Container::Stitched { target: Target::Explicit(Format::Tiff), masks: beside })
        );
        assert_eq!(
            container_for(Requested::Psd, "per-page", beside, false),
            Ok(Container::Layered { layered: true })
        );
        assert_eq!(
            container_for(Requested::Psd, "per-page", none, false),
            Ok(Container::Layered { layered: false })
        );
        assert_eq!(
            container_for(Requested::Psd, "stitched", none, true),
            Err("notice.export.refusedStitchedLayered")
        );
    }

    /// The strip's mask file: one PNG the strip's size, from the same global
    /// patch set in the same coordinates, so the fill that spans the join is
    /// white on both sides of it.
    #[test]
    fn a_stitched_export_writes_the_strips_mask_beside_it() {
        let scratch = Scratch::new("stitched-mask");
        let mut job = a_strip_job(&scratch, StripMode::Longstrip);
        overhanging_fill(&mut job, 0, Rect::new(10, 40, 20, 16), 200);

        let result =
            export_open_job(&job, "PNG", "new-folder", "separate-layer", "stitched").unwrap();
        assert_eq!(result.status, "exported");
        let path = PathBuf::from(result.path.unwrap());
        let mask_path = path.with_file_name("chapter_mask.png");
        let mask = cleaner_core::image::decode(&std::fs::read(&mask_path).unwrap()).unwrap();
        assert_eq!((mask.width, mask.height), (64, 144));
        for y in 0..144u32 {
            for x in 0..64u32 {
                let inside = (10..30).contains(&x) && (40..56).contains(&y);
                assert_eq!(mask.sample(x, y, 0), if inside { 255 } else { 0 }, "({x}, {y})");
            }
        }
    }

    /* -------------------------------------------------------------- */
    /* A real chapter, in every shape this build offers                */
    /* -------------------------------------------------------------- */

    /// **The four writers, over `fixtures/pages/strip-01…03.png`.**
    ///
    /// Every other test here runs on the 64×48 synthetic fixtures, which is
    /// right for asserting a rule and wrong for the claim "this build can
    /// export a chapter": three 800×4000 pages is a chapter, a 12 000-row
    /// stitched file is a strip, and neither shape is exercised by a page the
    /// size of a postage stamp. What is checked of each written file is what a
    /// user would check - that it opens, that it is the size it should be, and
    /// that its mode, depth and profile are the source's - plus
    /// the subset assertion, which is the one that fails on any conversion.
    #[test]
    fn a_real_chapter_exports_to_png_tiff_cbz_and_one_stitched_file() {
        let scratch = Scratch::new("real-chapter");
        let raws = scratch.join("raws");
        std::fs::create_dir_all(&raws).unwrap();
        let pages_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/pages");

        let sources: Vec<SourceRef> = (1..=3)
            .map(|n| {
                let bytes = std::fs::read(pages_dir.join(format!("strip-{n:02}.png"))).unwrap();
                let path = raws.join(format!("{n:03}.png"));
                std::fs::write(&path, &bytes).unwrap();
                cleaner_core::ingest::source_ref(&path, &bytes).unwrap()
            })
            .collect();
        assert_eq!(
            (sources[0].width, sources[0].height),
            (800, 4000),
            "the fixture is not the strip"
        );

        let manifest = scratch.join("out/chapter.mtclean");
        std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        let project =
            Project::new(manifest.parent().unwrap(), "0.1.0", StripMode::Longstrip, &sources);
        let mut job = Job::create(&manifest, project).unwrap();
        // A fill that runs 40 rows past page 0's bottom edge, so the join at
        // strip row 4000 is crossed and pages 0 and 1 both have work in them.
        overhanging_fill(&mut job, 0, Rect::new(120, 3960, 200, 80), 40);

        let source = |n: usize| {
            cleaner_core::image::decode(&std::fs::read(job.source_path(n).unwrap()).unwrap())
                .unwrap()
        };
        let strip = strip_of(&job);

        /* PNG, per page ------------------------------------------------ */
        let png = export(&job, "PNG", scratch.join("png").to_str().unwrap());
        assert_eq!(png.status, "exported");
        assert_eq!(png.file_count, Some(3));
        let png_dir = PathBuf::from(png.path.unwrap());
        for position in 0..3 {
            let before = source(position);
            let path = png_dir.join(format!("{:03}.png", position + 1));
            let after =
                cleaner_core::image::decode(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!((after.width, after.height), (800, 4000), "page {position}: dimensions");
            assert_eq!(after.mode, before.mode, "page {position}: mode");
            assert_eq!(after.depth, before.depth, "page {position}: bit depth");
            assert_eq!(after.icc, before.icc, "page {position}: icc profile");

            // The subset assertion, against the file on disk.
            let patches = page_patches(&job, &strip, position).unwrap();
            let permitted = permitted_region(&patches, before.width, before.height);
            for (x, y) in changed_pixels(&before, &after) {
                assert!(
                    permitted.contains(x as i64, y as i64),
                    "page {position}: ({x}, {y}) changed outside the permitted region"
                );
            }
        }
        assert_ne!(
            std::fs::read(png_dir.join("002.png")).unwrap(),
            std::fs::read(job.source_path(1).unwrap()).unwrap(),
            "the page below the join kept nothing of the patch that spans it"
        );

        /* TIFF, per page ----------------------------------------------- */
        let tiff = export(&job, "TIFF", scratch.join("tiff").to_str().unwrap());
        assert_eq!(tiff.file_count, Some(3));
        let tiff_dir = PathBuf::from(tiff.path.unwrap());
        for position in 0..3 {
            let before = source(position);
            let path = tiff_dir.join(format!("{:03}.tiff", position + 1));
            let after =
                cleaner_core::image::decode(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!((after.width, after.height), (800, 4000), "page {position}: dimensions");
            assert_eq!(after.mode, before.mode, "page {position}: mode");
            assert_eq!(after.depth, before.depth, "page {position}: bit depth");

            let patches = page_patches(&job, &strip, position).unwrap();
            let permitted = permitted_region(&patches, before.width, before.height);
            for (x, y) in changed_pixels(&before, &after) {
                assert!(
                    permitted.contains(x as i64, y as i64),
                    "page {position} as TIFF: ({x}, {y}) changed outside the permitted region"
                );
            }
        }

        /* CBZ ----------------------------------------------------------- */
        let cbz = export(&job, "CBZ", scratch.join("cbz").to_str().unwrap());
        assert_eq!(cbz.file_count, Some(3));
        let archive_path = PathBuf::from(cbz.path.unwrap());
        let entries = unzip(&std::fs::read(&archive_path).unwrap());
        assert_eq!(
            entries.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(),
            ["001.png", "002.png", "003.png"]
        );
        for (name, bytes) in &entries {
            assert_eq!(
                bytes,
                &std::fs::read(png_dir.join(name)).unwrap(),
                "{name} inside the archive is not the file the per-page export wrote"
            );
        }

        /* One file for the chapter --------------------------------------- */
        let stitched = export_open_job(
            &job,
            "PNG",
            scratch.join("stitched").to_str().unwrap(),
            "flattened",
            "stitched",
        )
        .unwrap();
        assert_eq!(stitched.status, "exported");
        assert_eq!(stitched.file_count, Some(1));
        assert_eq!(stitched.gutter_pixels, Some(0), "three 800-wide pages have no gutter");
        let one_file =
            cleaner_core::image::decode(&std::fs::read(stitched.path.unwrap()).unwrap()).unwrap();
        assert_eq!((one_file.width, one_file.height), (800, 12_000));
        assert_eq!(one_file.mode, source(0).mode);
        assert_eq!(one_file.depth, source(0).depth);

        // The join at strip row 4000, from inside the one file: the fill that
        // spans it is one tone on both sides of it.
        for x in 120..320 {
            assert_eq!(one_file.sample(x, 3999, 0), 40, "column {x} above the join");
            assert_eq!(one_file.sample(x, 4000, 0), 40, "column {x} below the join");
        }
        // And a row nothing touched is the source's own row.
        let last = source(2);
        for x in (0..800).step_by(37) {
            assert_eq!(
                one_file.sample(x, 8000 + 2500, 0),
                last.sample(x, 2500, 0),
                "column {x} of the last page was not carried through"
            );
        }
    }


    /* -------------------------------------------------------------- */
    /* The notice stack                                                */
    /* -------------------------------------------------------------- */

    /// Both statuses are announced by name, and a status this build does not
    /// know is announced not at all. The arm that used to be `_` would have
    /// said *"Exported N files"* over a third outcome - a partial write, a
    /// cancelled export - in the voice of a thing that finished.
    ///
    /// And a refusal announces **its own** reason. The arm named
    /// `notice.export.refusedOverwrite` outright while the overwrite was the
    /// only refusal there was; seven refusals later that would tell a user who
    /// asked for JPEG that their source files were nearly replaced.
    #[test]
    fn every_outcome_announces_itself_and_no_outcome_announces_another() {
        let exported = ExportResult {
            status: "exported",
            file_count: Some(3),
            path: Some("/out".into()),
            reason_key: None,
            gutter_pixels: None,
        };
        let (key, params, tone) = announcement(&exported, "PNG").expect("no announcement");
        assert_eq!(key, "notice.export.finished");
        assert_eq!(params["count"], 3);
        assert_eq!(params["format"], "PNG");
        assert_eq!(tone, "info");

        let refused =
            ExportResult::refused("notice.export.refusedOverwrite", Some("/out".into()));
        let (key, params, tone) = announcement(&refused, "PNG").expect("no announcement");
        assert_eq!(key, "notice.export.refusedOverwrite");
        assert_eq!(params["path"], "/out");
        assert_eq!(tone, "warn");

        let lossy = ExportResult::refused("notice.export.refusedLossyFormat", None);
        let (key, params, _) = announcement(&lossy, "JPEG").expect("no announcement");
        assert_eq!(key, "notice.export.refusedLossyFormat", "a refusal announced another's reason");
        assert_eq!(params["format"], "JPEG");

        let stitched = ExportResult {
            status: "exported",
            file_count: Some(1),
            path: Some("/out/chapter.png".into()),
            reason_key: None,
            gutter_pixels: Some(1536),
        };
        let (key, params, _) = announcement(&stitched, "PNG").expect("no announcement");
        assert_eq!(key, "notice.export.stitched");
        assert_eq!(params["count"], 1536, "the gutter count is not in the sentence that states it");

        let unknown = ExportResult { status: "partial", ..exported };
        assert!(
            announcement(&unknown, "PNG").is_none(),
            "a status this build does not know announced that the export finished"
        );
    }

    /* -------------------------------------------------------------- */
    /* The geometry, unchanged                                         */
    /* -------------------------------------------------------------- */

    /// **The intersection, through a real manifest.**
    ///
    /// The patch is anchored on page 0 and runs 8 rows past its bottom edge.
    /// Page 1 owns no patch - `PatchRecord.source_idx` says 0, and it is the
    /// only page-ownership fact a manifest holds - so an exporter that selects
    /// by ownership writes page 1 byte-for-byte and loses the bottom of the
    /// bubble. The intersection writes both.
    #[test]
    fn a_longstrip_patch_that_spans_a_join_reaches_both_pages_files() {
        let scratch = Scratch::new("join");
        let mut job = a_strip_job(&scratch, StripMode::Longstrip);
        overhanging_fill(&mut job, 0, Rect::new(10, 40, 20, 16), 200);

        let result = export(&job, "PNG", "new-folder");
        assert_eq!(result.file_count, Some(3));
        let out = PathBuf::from(result.path.unwrap());

        let source = |n: usize| std::fs::read(job.source_path(n).unwrap()).unwrap();
        let written = |n: usize| std::fs::read(out.join(format!("{n:03}.png"))).unwrap();

        assert_ne!(written(0), source(0), "page 0 lost the part it anchors");
        assert_ne!(written(1), source(1), "page 1 was declared untouched and is not");
        assert_eq!(written(2), source(2), "a page nothing reaches was re-encoded");

        // The seam, from the two files: page 0's last row and page 1's first
        // row are adjacent rows of one fill, and they carry one tone.
        let above = cleaner_core::image::decode(&written(0)).unwrap();
        let below = cleaner_core::image::decode(&written(1)).unwrap();
        for x in 10..30 {
            assert_eq!(above.sample(x, 47, 0), 200, "page 0 column {x}");
            assert_eq!(below.sample(x, 0, 0), 200, "page 1 column {x}");
        }
        assert_eq!(below.sample(9, 0, 0), source_raster(&job, 1).sample(9, 0, 0));

        // The subset assertion against the written files, page by page, over
        // the parts the intersection actually gave each page.
        let strip = strip_of(&job);
        for position in 0..3 {
            let before = source_raster(&job, position);
            let after = cleaner_core::image::decode(&written(position)).unwrap();
            assert_eq!(after.mode, before.mode, "page {position}: mode");
            assert_eq!(after.depth, before.depth, "page {position}: bit depth");
            assert_eq!(after.icc, before.icc, "page {position}: icc profile");

            let patches = page_patches(&job, &strip, position).unwrap();
            let permitted = permitted_region(&patches, before.width, before.height);
            for (x, y) in changed_pixels(&before, &after) {
                assert!(
                    permitted.contains(x as i64, y as i64),
                    "page {position}: ({x}, {y}) changed outside the permitted region"
                );
            }
        }
    }

    /// The same manifest in `Single` mode. There is no strip, so there is no
    /// join for a patch to span: a bbox running past the page is a malformed
    /// patch, clipped at that page's edge, and it must not appear in the next
    /// file. Splitting unconditionally would spill one page's edit into another
    /// image that has nothing to do with it.
    #[test]
    fn a_paginated_job_never_spills_a_patch_into_the_next_file() {
        let scratch = Scratch::new("single");
        let mut job = a_strip_job(&scratch, StripMode::Single);
        overhanging_fill(&mut job, 0, Rect::new(10, 40, 20, 16), 200);

        let result = export(&job, "PNG", "new-folder");
        let out = PathBuf::from(result.path.unwrap());
        let source = |n: usize| std::fs::read(job.source_path(n).unwrap()).unwrap();
        let written = |n: usize| std::fs::read(out.join(format!("{n:03}.png"))).unwrap();

        assert_ne!(written(0), source(0), "the page the patch is on");
        assert_eq!(written(1), source(1), "an edit crossed into the next file");
        assert_eq!(written(2), source(2));
    }

    /// Selection is from the manifest's rectangles, and only the records that
    /// survive it are read off disk. Loading the global set first would put
    /// every patch in the chapter in RAM to answer a question about one page -
    /// which is what keeping them on disk avoids.
    #[test]
    fn a_page_is_selected_from_bboxes_before_any_buffer_is_loaded() {
        let scratch = Scratch::new("select");
        let mut job = a_strip_job(&scratch, StripMode::Longstrip);
        overhanging_fill(&mut job, 0, Rect::new(10, 40, 20, 16), 200);
        // A second patch, wholly inside page 2 and nowhere near the first.
        let mut second = fixtures::by_name("l8").raster;
        second.width = 4;
        second.height = 4;
        second.icc = None;
        second.data = vec![9; 16];
        job.complete_region(
            2,
            &Patch {
                id: "on-page-2".into(),
                mask: Mask::filled(Rect::new(2, 2, 4, 4)),
                ink: Mask::filled(Rect::new(2, 2, 4, 4)),
                pixels: second,
                order: 0,
                visible: true,
                provenance: provenance(),
            },
            None,
        )
        .unwrap();

        let strip = strip_of(&job);
        let ids = |position: usize| {
            intersecting_records(&job, &strip, position)
                .into_iter()
                .map(|(_, record)| record.id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(0), ["across-the-join"]);
        assert_eq!(ids(1), ["across-the-join"], "the page it does not belong to");
        assert_eq!(ids(2), ["on-page-2"], "a patch two pages away was loaded for this one");
    }

    fn source_raster(job: &Job, index: usize) -> cleaner_core::image::Raster {
        cleaner_core::image::decode(&std::fs::read(job.source_path(index).unwrap()).unwrap())
            .unwrap()
    }
}
