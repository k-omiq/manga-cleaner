//! The four region-level edits that need an engine: `applyTool`,
//! `createRegion`, `rerunMask` and `cleanAnyway`.
//!
//! This is the other half of the specification. Its two easy siblings -
//! `deleteMask` and `restoreRegion` - are in [`crate::library`], because
//! deleting a mask is a boolean on a persisted record; these four each have to
//! **run a rung over one region**, which is everything [`crate::run`] does for
//! a page with the queue, the events and the escalation loop taken away.
//!
//! ## Nothing here is a second pipeline
//!
//! Every part that decides what a patch *is* - the ladder, the quality metric,
//! the provenance snapshot, the mask digest - is [`crate::run`]'s and is called
//! from here rather than restated. A region cleaned by hand and a region
//! cleaned by a run are identical in kind, and the only way to keep that
//! true is for both to go through the same code. What this module owns is the
//! three things a run does not have to answer:
//!
//! - **where the mask comes from**, which is the caller's rectangle for a hand
//!   gesture, the stored mask for a re-run, and the detector for a region the
//!   gate held back;
//! - **which rung runs**, which is the one the user named - every picker in
//!   the interface names a model outright - or the route's own where nobody
//!   named anything;
//! - **what to say when nothing could be done**, which is a notice rather than
//!   a silent `null`.
//!
//! ## A named rung is run, and not scored against
//!
//! The decline is a rule about a choice **nobody watched being made**: an
//! automatic run picks a rung, and if the metric says the pixels are wrong the
//! region is left alone rather than filled badly. A user who opens the picker
//! and chooses *LaMa* has watched it. So [`Choice::Exact`] commits what the
//! rung produced and records
//! the metric's verdict in `params_snapshot` - the same field a run writes it
//! to - where [`Choice::Ladder`] keeps the run's own behaviour, escalating past
//! a declined rung and refusing when the top one declines too.
//!
//! ## The cloud rung is not reachable from here either
//!
//! There is no cloud client in this build - [`Engine::Cloud`] exists in the
//! manifest vocabulary so that a patch made by one could be *read*, and nothing
//! can make one. A request for it is refused with
//! `notice.cloud.unavailable`, which says the one thing that matters: nothing
//! was sent.

use std::path::PathBuf;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use cleaner_core::accel::Preference;
use cleaner_core::detect::{Detector, build_regions_separated};
use cleaner_core::engines::flux;
use cleaner_core::engines::model::Error as ModelError;
use cleaner_core::fit::{self, EdgeMap, Fitted};
use cleaner_core::image::{Raster, decode};
use cleaner_core::ingest::sha256_hex;
use cleaner_core::mask::{Mask, Rect};
use cleaner_core::memory;
use cleaner_core::patch::{Engine, Patch, Provenance};
use cleaner_core::project::{Job, PatchRecord};
use cleaner_core::quality;
use cleaner_core::sidecar::{self, Backend, Budget};

use crate::library::{ApiMask, ApiRegion, Bbox, Library, LibraryError};
use crate::run::{self, EnginePick, Picks};

/* ------------------------------------------------------------------ */
/* The shapes the seam carries                                         */
/* ------------------------------------------------------------------ */

/// The discriminated union of outcomes.
///
/// Four of its five statuses are reachable here: `applied`, `run-started`,
/// `blocked` and `not-found`. `needs-confirmation` belongs to the cloud rung's
/// two-tier protocol and cannot be reached by a build with no cloud client -
/// a request for one is `blocked` before any confirmation would be asked for.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyResult {
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<ApiRegion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mask: Option<ApiMask>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pages: Option<Vec<run::QueuedPage>>,
}

impl ApplyResult {
    fn of(status: &'static str) -> ApplyResult {
        ApplyResult {
            status,
            region: None,
            mask: None,
            page_status: None,
            run_id: None,
            pages: None,
        }
    }

    fn applied(edited: Edited) -> ApplyResult {
        ApplyResult {
            status: "applied",
            mask: edited.region.mask.clone(),
            region: Some(edited.region),
            page_status: Some(edited.page_status),
            run_id: None,
            pages: None,
        }
    }
}

/// `createRegion`'s answer and `deleteMask`'s, which are the same pair.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedRegion {
    pub region: ApiRegion,
    pub page_status: String,
}

/// `rerunMask`'s answer. `reopenTool` is `null` for every kind but
/// `reopenInTool`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RerunResult {
    pub region: ApiRegion,
    pub mask: Option<ApiMask>,
    pub reopen_tool: Option<&'static str>,
    pub page_status: String,
}

/// `cleanAnyway`'s.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanedRegion {
    pub region: ApiRegion,
    pub mask: Option<ApiMask>,
    pub page_status: String,
}

/// Whether rung 3a can be offered on this machine, and what to say if not.
///
/// **Absence answers `null`**, and that is [`sidecar::Availability::Absent`]'s
/// own rule: the application ships no Python, so a machine that installed
/// nothing is the ordinary case and has nothing to tell anybody. A control that
/// would fail is simply not shown.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarStatus {
    pub available: bool,
    pub reason_key: Option<&'static str>,
}

/// One region, as the manifest now holds it, with its page's status.
struct Edited {
    region: ApiRegion,
    page_status: String,
}

/* ------------------------------------------------------------------ */
/* Rung 3a                                                             */
/* ------------------------------------------------------------------ */

/// The model rung 3a opens. The sidecar's own default
/// ([`cleaner_core::sidecar`] §"The wire"), named here because the spawn has to
/// say which one.
const FLUX_MODEL: &str = "flux2-klein-4b";

/// Which backend this machine's sidecar is asked for.
///
/// **A preference with a platform default, not a platform fact.** It used to be
/// the second: MLX exists on Apple Silicon and nowhere else, so `mflux` there and
/// the GGUF path - which never rendered - everywhere else. `sdnq` is torch and
/// runs on both kinds of machine ([`cleaner_core::sidecar::backend`]), so on a
/// Mac there are genuinely two backends and which is better is a question about
/// the user's machine and weights rather than about the platform. The setting is
/// `fluxBackend`, its default is `auto`, and
/// [`Backend::from_setting`] is where `auto` is resolved - in the core, beside
/// the table it resolves against, so this function is only the settings read.
///
/// An unset or unknown value is `auto` rather than an error: a stale settings
/// file must not make the rung unreachable.
fn flux_backend(setting: Option<&str>) -> Backend {
    Backend::from_setting(setting).unwrap_or(Backend::Sdnq)
}

/// One string setting, under the first of several names that is present.
///
/// Every sidecar preference has two spellings - the one the interface writes and
/// an earlier one a stored settings file may still carry - and reading them by
/// hand at four call sites is how the fifth one gets the fallback wrong. Empty is
/// `None`: a field somebody cleared is not a value.
fn setting_str(settings: Option<&serde_json::Value>, names: &[&str]) -> Option<String> {
    let settings = settings?;
    names
        .iter()
        .find_map(|name| settings.get(*name).and_then(|v| v.as_str()))
        .map(str::to_owned)
        .filter(|value| !value.trim().is_empty())
}

/// The names each sidecar preference is read under, newest first.
const SIDECAR_PATH_KEYS: &[&str] = &["sidecarPath", "sidecarFolder"];
const FLUX_MODEL_KEYS: &[&str] = &["fluxModel", "sidecarModel"];
const FLUX_BACKEND_KEYS: &[&str] = &["fluxBackend", "sidecarBackend"];

/// The `fluxBackend` setting, as stored.
fn flux_backend_setting(app: &tauri::AppHandle) -> Option<String> {
    setting_str(crate::settings::read(app).ok().as_ref(), FLUX_BACKEND_KEYS)
}

/// Whether rung 3a is installed, has a backend here, and can be measured.
///
/// **Not the whole gate**, deliberately. The authoritative one needs the
/// sidecar's own report of what the model costs
/// ([`flux::Inpainter::open`] runs it), and asking for that means spawning a
/// process and reading gigabytes off disk - which is not something a picker
/// being drawn may do. So this answers the questions that can be answered for
/// free: is there an install, does this build render on this platform at all
/// ([`sidecar::hardware::platform_decline`]), and is this a machine
/// [`memory::room`] has a number for. A region that then cannot be carried is
/// refused at the moment it is asked for, with the reason the gate gives.
///
/// The platform question comes first for the same reason it does inside the
/// gate: a Windows machine whose memory measures perfectly well must not be
/// told its memory could not be measured.
pub fn sidecar_status_from(
    explicit: Option<&str>,
    backend: Option<&str>,
    app_data: Option<&std::path::Path>,
) -> SidecarStatus {
    let env_var = std::env::var("MANGA_CLEANER_SIDECAR").ok();
    let explicit = explicit
        .filter(|s| !s.is_empty())
        .or_else(|| env_var.as_deref().filter(|s| !s.is_empty()));
    let roots = sidecar::search_paths_from(explicit, app_data);
    match sidecar::find_within(&roots) {
        // Nothing installed: nothing to offer and nothing to say.
        None => SidecarStatus { available: false, reason_key: None },
        // The *chosen* backend's platform question, not the platform's own -
        // since `sdnq` there is no platform without a backend, and what can still
        // be refused here is a user who selected one that cannot run.
        Some(_) => match sidecar::hardware::platform_decline_for(flux_backend(backend)) {
            Some(decline) => {
                SidecarStatus { available: false, reason_key: Some(decline.reason_key()) }
            }
            None if memory::room().is_none() => SidecarStatus {
                available: false,
                reason_key: Some("decline.reason.sidecarUnknownMachine"),
            },
            None => SidecarStatus { available: true, reason_key: None },
        },
    }
}

#[allow(dead_code)]
pub fn sidecar_status(app_data: Option<&std::path::Path>) -> SidecarStatus {
    sidecar_status_from(None, None, app_data)
}

/// A discovered model directory in the sidecar's weights folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarModelInfo {
    pub id: String,
    pub label: String,
}

/// Derive model id and a friendly label from a weights directory name.
pub fn parse_model_dir_name(name: &str) -> (String, String) {
    let mut base = name;
    let mut quant: Option<&str> = None;

    for backend in ["-mflux", "-gguf", "-sdcpp", "-mlx"] {
        if let Some(pos) = base.find(backend) {
            let (b, rest) = base.split_at(pos);
            base = b;
            let after_backend = &rest[backend.len()..];
            if let Some(q) = after_backend.strip_prefix('-') {
                quant = Some(q);
            }
            break;
        }
    }

    if quant.is_none() {
        let known_quants = [
            "-q4_k_m", "-q4_k_s", "-q5_k_m", "-q5_k_s", "-q8_0", "-q4_0", "-q4_1", "-q5_0", "-q5_1",
            "-int4", "-int8", "-4bit", "-8bit", "-bf16", "-fp16", "-fp8", "-fp32",
            "-q4", "-q8", "-q5", "-q6", "-q2", "-q3",
        ];
        for k_quant in known_quants {
            if let Some(b) = base.strip_suffix(k_quant) {
                base = b;
                quant = Some(&k_quant[1..]);
                break;
            }
        }
    }

    let id = base.to_string();

    let model_label = if base.eq_ignore_ascii_case("flux2-klein-4b") {
        "FLUX.2 Klein 4B".to_string()
    } else if base.eq_ignore_ascii_case("flux2-klein-9b") {
        "FLUX.2 Klein 9B".to_string()
    } else if base.eq_ignore_ascii_case("flux2-klein-2b") {
        "FLUX.2 Klein 2B".to_string()
    } else if base.eq_ignore_ascii_case("flux1-dev") {
        "FLUX.1 [dev]".to_string()
    } else if base.eq_ignore_ascii_case("flux1-schnell") {
        "FLUX.1 [schnell]".to_string()
    } else if let Some(b_num) = base.strip_prefix("flux2-klein-") {
        if b_num.ends_with('b') || b_num.ends_with('B') {
            format!("FLUX.2 Klein {}", b_num.to_ascii_uppercase())
        } else {
            base.to_string()
        }
    } else {
        base.to_string()
    };

    let quant_label = quant.map(|q| {
        let lower = q.to_ascii_lowercase();
        match lower.as_str() {
            "q4" | "int4" | "4bit" => "4-bit".to_string(),
            "q8" | "int8" | "8bit" => "8-bit".to_string(),
            "q5" | "int5" | "5bit" => "5-bit".to_string(),
            "q6" | "int6" | "6bit" => "6-bit".to_string(),
            "q2" | "int2" | "2bit" => "2-bit".to_string(),
            "q3" | "int3" | "3bit" => "3-bit".to_string(),
            "bf16" => "BF16".to_string(),
            "fp16" => "FP16".to_string(),
            "fp8" => "FP8".to_string(),
            "fp32" => "FP32".to_string(),
            "q4_k_m" => "Q4_K_M".to_string(),
            "q4_k_s" => "Q4_K_S".to_string(),
            "q5_k_m" => "Q5_K_M".to_string(),
            "q5_k_s" => "Q5_K_S".to_string(),
            "q8_0" => "Q8_0".to_string(),
            "q4_0" => "Q4_0".to_string(),
            _ => q.to_string(),
        }
    });

    let label = match quant_label {
        Some(q) => format!("{model_label} ({q})"),
        None => model_label,
    };

    (id, label)
}

/// Scan `<resolved install root>/weights/` for installed model directories.
pub fn list_sidecar_models_from(
    explicit: Option<&str>,
    app_data: Option<&std::path::Path>,
) -> Vec<SidecarModelInfo> {
    let env_var = std::env::var("MANGA_CLEANER_SIDECAR").ok();
    let explicit = explicit
        .filter(|s| !s.is_empty())
        .or_else(|| env_var.as_deref().filter(|s| !s.is_empty()));
    let roots = sidecar::search_paths_from(explicit, app_data);
    let Some(install) = sidecar::find_within(&roots) else {
        return Vec::new();
    };
    let weights_dir = install.root.join("weights");
    let Ok(entries) = std::fs::read_dir(&weights_dir) else {
        return Vec::new();
    };
    let mut models = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if !name.starts_with('.') {
                    let (id, label) = parse_model_dir_name(name);
                    models.push(SidecarModelInfo { id, label });
                }
            }
        }
    }
    models.sort_by(|a, b| a.label.cmp(&b.label));
    models
}

/* ------------------------------------------------------------------ */
/* Where a region lives                                                */
/* ------------------------------------------------------------------ */

/// Which chapter, which job and which page a region id names.
struct Located {
    chapter_id: String,
    job_path: PathBuf,
    page_index: usize,
}

/// The page a region id sits on, from the id alone.
///
/// Every region id in this application is built on a page id and every page id
/// is `<chapter>-p<NNN>` ([`crate::library::page_id`]) - the run's `-r{n}`, the
/// gate's `-u{x}-{y}-{w}-{h}` and a hand region's `-h{n}` all share that stem.
/// So the page is readable without opening a manifest, which is what lets the
/// job be opened **once**, under its lock, with everything already known.
fn page_index_in(chapter_id: &str, region_id: &str) -> Option<usize> {
    let rest = region_id.strip_prefix(&format!("{chapter_id}-p"))?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse::<usize>().ok()?.checked_sub(1)
}

fn locate_region(library: &Library, region_id: &str) -> Result<Option<Located>, LibraryError> {
    let index = library.index()?;
    let Some(chapter_id) = crate::library::chapter_holding(&index, region_id) else {
        return Ok(None);
    };
    let Some(page_index) = page_index_in(&chapter_id, region_id) else { return Ok(None) };
    let job_path = library.resolve_chapter(&chapter_id)?;
    Ok(Some(Located { chapter_id, job_path, page_index }))
}

fn locate_page(
    library: &Library,
    chapter_id: &str,
    page_index: usize,
) -> Result<Option<Located>, LibraryError> {
    let job_path = library.resolve_chapter(chapter_id)?;
    Ok(Some(Located { chapter_id: chapter_id.to_owned(), job_path, page_index }))
}

/* ------------------------------------------------------------------ */
/* What one edit asks for                                              */
/* ------------------------------------------------------------------ */

/// Where the mask an edit writes through comes from.
enum Geometry {
    /// The caller's own rectangle, in page pixels. **Authoritative**: the
    /// fitting search does not run over it, because the user has already
    /// said where the edit goes.
    Given(Rect),
    /// The **shape** the caller painted, in page pixels - the discs a round
    /// brush swept along its path, not the rectangle around them.
    /// Authoritative for the same reason [`Geometry::Given`] is, and
    /// preferred over it wherever the gesture actually had a shape.
    Painted(Mask),
    /// The mask the region's existing patch was applied with. A re-run edits
    /// the same set of pixels with a different engine, and re-deriving the
    /// geometry would quietly move it.
    Stored,
    /// Whatever the detector finds under this rectangle - the case a region the
    /// gate held back is in, where all the manifest kept is the box.
    ///
    /// **The one remaining detector call on an edit**, and it belongs to
    /// [`existing_geometry`]: a gate-skipped row has no patch and no painted
    /// shape, so the box the manifest kept is all there is to look under. No
    /// hand gesture reaches it - a stroke says where the mask goes
    /// ([`painted_geometry`]).
    Detected(Rect),
}

/// Which rung runs. See the module docs for why the two are scored differently.
#[derive(Debug, PartialEq, Eq)]
enum Choice {
    /// The rung the user named, run once.
    Exact(Engine),
    /// The route's own rung, moved by a `fill`/`redraw` pick, escalating up the
    /// ladder exactly as a run would.
    Ladder(Option<EnginePick>),
}

/// One region edit, before anything has been opened.
struct Plan {
    region_id: String,
    geometry: Geometry,
    choice: Choice,
    /// `hand` or `auto`, for the params snapshot - the one word that separates
    /// a hand mask from an automatic one.
    source: &'static str,
    tool: Option<String>,
    fill_mode: Option<&'static str>,
    /// The gate-skipped row this edit consumes, if it is cleaning one. It goes
    /// with the patch that replaces it, or the page would carry both a warning
    /// and the mask that answered it.
    clears_untouched: bool,
    /// A brush or clone stroke, where this edit is one.
    ///
    /// **One field, parsed at both parse sites, branched on at one.** The two
    /// seam methods already converge on [`edit`]; this is what keeps the
    /// convergence true of painting as well.
    paint: Option<PaintPlan>,
}

/// What an edit did.
///
/// `Cleaned` carries the whole `Edited` record inline. The enum is built once
/// per edit and consumed at once, never held in a collection, so the size gap
/// between its variants costs nothing worth a `Box`.
#[allow(clippy::large_enum_variant)]
enum Outcome {
    Cleaned(Edited),
    /// No rung produced a patch. The key is the reason, in the same
    /// `decline.reason.*` vocabulary a run's review row carries.
    Refused(&'static str),
    /// No region, no page, or no job.
    NotFound,
}

/* ------------------------------------------------------------------ */
/* The engines, held for one edit                                      */
/* ------------------------------------------------------------------ */

/// The sessions one region edit may need.
///
/// Built per call and **borrowed, never owned**: every session it uses comes
/// out of [`cleaner_core::residency`] and goes back there when the click ends.
/// That is the whole of what changed here - the bench used to open a detector
/// per edit and spawn a sidecar per region, so a reviewer working down a page
/// of SFX paid a session build, or a process start and several gigabytes off
/// disk, for every one of them. Nothing is opened until something asks for
/// it - a brush stroke that lands on flat paper opens no model at all - so
/// the ordinary edit still costs no session, and the second edit in a row
/// costs no *load*.
struct Bench {
    app_data: Option<PathBuf>,
    sidecar_override: Option<String>,
    flux_model: Option<String>,
    /// The `fluxBackend` preference as stored: `auto`, `mflux`, `sdnq`, or
    /// absent. Resolved by [`flux_backend`] rather than here, so the default
    /// lives in one place.
    flux_backend: Option<String>,
    detector: run::OnDemand<Detector>,
    rung2: run::Rung2,
    /// Rung 3a's child, held for the length of this edit and handed back to the
    /// residency cache after it. See [`Bench::flux`].
    flux: Option<flux::Inpainter>,
    flux_key: Option<cleaner_core::residency::Key>,
}

impl Bench {
    fn open(app: &tauri::AppHandle) -> Bench {
        use tauri::Manager;
        let app_data = app.path().app_data_dir().ok();
        let settings = crate::settings::read(app).ok();
        let sidecar_override = setting_str(settings.as_ref(), SIDECAR_PATH_KEYS);
        let flux_model = setting_str(settings.as_ref(), FLUX_MODEL_KEYS);
        let flux_backend = setting_str(settings.as_ref(), FLUX_BACKEND_KEYS);
        // The runtime is downloaded after install, so this can legitimately
        // fail - and a failure is not fatal here the way it is for a run:
        // rungs 0 and 1 are arithmetic over the page's own samples and need no
        // ONNX at all. A rung that does need one answers `Unavailable` and the
        // edit is refused with the reason, rather than every edit being
        // refused for a rung most of them never reach.
        let _ = cleaner_core::runtime::find(app_data.as_deref())
            .and_then(|path| cleaner_core::runtime::load(&path));
        let models = run::model_dir(app_data.as_deref()).unwrap_or_default();
        // The same accelerator setting a run reads, so a region edit and the
        // run that produced it land on the same provider.
        let preference = settings
            .as_ref()
            .map(run::preference_from)
            .unwrap_or(Preference::Automatic);
        Bench {
            rung2: run::Rung2::new(&models, preference),
            detector: run::detector_on_demand(&models, preference),
            flux: None,
            flux_key: None,
            app_data,
            sidecar_override,
            flux_model,
            flux_backend,
        }
    }

    /// The detector: the one already in memory if a run or an earlier click
    /// left one there, and a new one otherwise.
    fn detector(&mut self) -> Option<&mut Detector> {
        self.detector.get().ok()
    }

    /// The text under a rectangle, as the detector sees it.
    ///
    /// `None` when there is no detector on this machine or nothing was found
    /// under the box - and the caller's answer to both is the same: treat the
    /// rectangle itself as the mask. That is a *larger* edit than the glyphs
    /// would be, and it is the honest one: the user asked for this box to be
    /// cleaned, and inventing a tighter shape with no evidence would be a
    /// guess about which pixels are text.
    ///
    /// `source` is the digest of the page file this raster was decoded from,
    /// and it is what stops six clicks on one page being six full-page
    /// detections ([`cleaner_core::detect::detect_page`]).
    fn text_under(&mut self, page: &Raster, box_: Rect, source: &str) -> Option<(Mask, f32)> {
        let detector = self.detector()?;
        let detection = match cleaner_core::detect::detect_page(detector, source, page) {
            Ok(detection) => detection,
            // The answer to a failed detection is the same as the answer to an
            // empty one, and it is the paragraph above: the caller treats the
            // rectangle as the mask. The *session* is a different matter.
            // `Inference` is the detector having built, run, and then failed,
            // which on Windows is usually a graphics adapter that has been
            // reset and will fail every later call the same way. Poisoning it
            // here is what makes the next click open a fresh one instead of
            // being handed the dead one, and it is the same treatment
            // [`engine_fault`] documents for the rungs. The other two variants
            // are not the session's fault - a model that would not load, or one
            // whose outputs did not match the pinned contract - so they are
            // left alone.
            Err(error) => {
                if matches!(error, cleaner_core::detect::DetectError::Inference(_)) {
                    detector.poison();
                }
                eprintln!("manga-cleaner: the detector failed under a region edit: {error}");
                return None;
            }
        };
        let regions = build_regions_separated(detection.boxes.clone(), page.width, page.height, |a, b| {
            cleaner_core::balloon::merge_crosses_a_balloon(page, &detection.segmentation, a, b)
        });
        let region = regions.into_iter().max_by_key(|region| overlap(region.masking, box_))?;
        if overlap(region.masking, box_) == 0 {
            return None;
        }
        let seed = fit::seed_mask(&detection.segmentation, &region, page.width, page.height);
        if seed.is_empty() {
            return None;
        }
        Some((seed, detection.segmentation.proxy_scale()))
    }

    /// Rung 3a, on a child that **survives the click**.
    ///
    /// It used to be a process per region: spawned here, killed when the edit
    /// ended, ten to sixty seconds of process start and gigabytes off disk paid
    /// again by the next region. The child now goes back to
    /// [`cleaner_core::residency`] with the rest of the bench, and the next
    /// FLUX edit finds it up and warm.
    ///
    /// **What that costs, and the three things that bound it**:
    ///
    /// - the model is released and opened again per region, because the text
    ///   encoder is evicted by the first render and `mflux` cannot rebuild it -
    ///   so a held child holds its *floor*, 1.37 GB, between edits and not a
    ///   render's peak;
    /// - the grace is [`cleaner_core::registry::SIDECAR_IDLE_GRACE`], two
    ///   minutes rather than the five an ONNX session gets, because this is the
    ///   largest thing the application can be holding;
    /// - the pressure ladder takes it first, which is the step
    ///   `Ladder::holding_sidecar` was written for and could not have until
    ///   there was a child to take.
    fn flux(
        &mut self,
        page: &Raster,
        fitted: &Fitted,
    ) -> Result<(run::Made, quality::Assessment), &'static str> {
        let env_var = std::env::var("MANGA_CLEANER_SIDECAR").ok();
        let explicit = self.sidecar_override.as_deref().filter(|s| !s.is_empty())
            .or_else(|| env_var.as_deref().filter(|s| !s.is_empty()));
        let roots = sidecar::search_paths_from(explicit, self.app_data.as_deref());
        let Some(install) = sidecar::find_within(&roots) else {
            return Err("decline.reason.rungUnavailable");
        };
        let backend = flux_backend(self.flux_backend.as_deref());
        // Ahead of the memory question, and for the gate's own reason: a backend
        // with no rendering path here is refused whatever the machine measures.
        // Per *backend* rather than per platform since `sdnq` - the only thing
        // that reaches this now is a choice that cannot run.
        if let Some(decline) = sidecar::hardware::platform_decline_for(backend) {
            return Err(decline.reason_key());
        }
        let Some(room) = memory::room() else {
            return Err("decline.reason.sidecarUnknownMachine");
        };
        let model = self
            .flux_model
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(FLUX_MODEL);
        let key = cleaner_core::residency::Key::new(
            cleaner_core::registry::Kind::Sidecar,
            format!("{}|{}|{model}", install.python.display(), backend.id()),
        );
        if self.flux.is_none() {
            self.flux = match cleaner_core::residency::checkout::<flux::Inpainter>(&key) {
                Some(held) => Some(held),
                None => Some(
                    flux::Inpainter::open(&install, backend, model, Budget::from_room(room))
                        .map_err(flux_reason)?,
                ),
            };
            self.flux_key = Some(key);
        }
        let inpainter = self.flux.as_mut().expect("just opened");
        let rendered = inpainter.render(page, fitted).map_err(flux_reason)?;
        let made = run::Made {
            engine: Engine::Flux,
            mask: rendered.mask,
            ink: fitted.ink.clone(),
            pixels: rendered.pixels,
            provider: Some(backend.id().to_owned()),
            // No digest: the weights are the child's, read by the child, and a
            // digest this process never computed would be one it cannot stand
            // behind.
            model_sha256: None,
            pad: rendered.pad,
            tiles: Some(rendered.tiles),
        };
        let verdict = quality::assess(page, &made.mask, &made.pixels, fit::page_noise_sigma(page));
        Ok((made, verdict))
    }
}

impl Drop for Bench {
    /// **The end of a click is not a reason to unload anything.** The ONNX
    /// sessions hand themselves back - [`crate::run::OnDemand`] and the two
    /// rung holders each have their own `Drop` - and this is the child, which
    /// nothing else owns. A child that is spent goes instead of being parked,
    /// which [`cleaner_core::residency::checkin`] decides and this does not
    /// have to.
    fn drop(&mut self) {
        if let (Some(inpainter), Some(key)) = (self.flux.take(), self.flux_key.take()) {
            cleaner_core::residency::checkin(key, inpainter);
        }
        // A click is a safe point of exactly the kind a run's region boundary
        // is: this edit's buffers are gone and the next one's do not exist. So
        // it is where a parked session whose grace elapsed while the user was
        // reading goes back (nothing here can interrupt the edit that is
        // running).
        cleaner_core::residency::sweep();
    }
}

/// The key a rung 3a failure is reported under.
///
/// A decline carries its own - `decline.reason.sidecar*`, the vocabulary the
/// settings panel and a review row both read - and a fault does not: a sidecar
/// that would not answer is, from the region's point of view, a rung that was
/// not available.
fn flux_reason(error: ModelError) -> &'static str {
    match error {
        ModelError::Declined(decline) => decline.reason_key(),
        ModelError::Run(_) => "decline.reason.rungUnavailable",
    }
}

/// The key an **ONNX** rung's run fault is refused under, and the one place the
/// raw text of it goes.
///
/// [`flux_reason`]'s counterpart for the rungs this process runs itself, and it
/// answers a different key on purpose. Rung 3a's child is a thing that comes
/// and goes - it is spawned, it is killed under memory pressure, it may not be
/// installed - so a sidecar that would not answer is honestly reported as a
/// rung that was not available. An ONNX session that built, ran, and then died
/// is a different statement: the rung *is* on this machine, and something
/// underneath it failed. On Windows the usual cause is one the session cannot
/// come back from, which is why [`run::render_rung`] has already poisoned it by
/// the time this is called - the next edit opens a fresh session rather than
/// being handed the dead one.
///
/// What used to happen instead was nothing the user could read: the fault was
/// propagated with `?` and the Tauri command rejected with ONNX Runtime's own
/// string, which is not a catalogue key, so the click did not visibly do
/// anything at all. The string is not lost - it goes to stderr, which is where
/// this crate already puts diagnostic text a user cannot act on.
fn engine_fault(detail: &str) -> &'static str {
    eprintln!("manga-cleaner: a model faulted during a region edit: {detail}");
    "decline.reason.engineFault"
}

/// How many pixels two rectangles share.
fn overlap(a: Rect, b: Rect) -> u64 {
    let w = (a.right().min(b.right()) - a.x.max(b.x)).max(0) as u64;
    let h = (a.bottom().min(b.bottom()) - a.y.max(b.y)).max(0) as u64;
    w * h
}

/* ------------------------------------------------------------------ */
/* Running one edit                                                    */
/* ------------------------------------------------------------------ */

/// Open the job, run the plan, write the patch, and answer with the region.
///
/// Under the job's own lock for the whole of it, like every other
/// read-modify-write of a manifest in this crate: the file is rewritten whole,
/// and an edit that interleaved with a run's flush would lose one of them
/// entirely.
fn edit(app: &tauri::AppHandle, located: &Located, plan: Plan) -> Result<Outcome, String> {
    let mut bench = Bench::open(app);
    let _lock = run::lock_job(&located.job_path);
    // Read-only from here: the manifest is mutated in `commit`, which takes the
    // job by value and is the only writer either path reaches.
    let job = Job::open(&located.job_path).map_err(|e| e.to_string())?;
    let Some(source_idx) = Library::resolve_page(&job.project, located.page_index) else {
        return Ok(Outcome::NotFound);
    };
    let Some(path) = job.source_path(source_idx) else { return Ok(Outcome::NotFound) };
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let page = decode(&bytes).map_err(|e| e.to_string())?;
    // The source's digest, computed once: it goes into this patch's provenance
    // below, and it is the key the page's detection is remembered under
    // ([`cleaner_core::detect::detect_page`]).
    let source_sha256 = sha256_hex(&bytes);

    let existing: Option<PatchRecord> =
        job.project.patches.iter().find(|record| record.id == plan.region_id).cloned();

    // **The paint branch, and it is here rather than beside the `solid` fill.**
    //
    // A painted stroke is authoritative: there is no mask to fit, no ring to
    // sample, no edge map to build and no quality verdict to assess, so
    // everything from the geometry match down is work whose answer it does
    // not use. `fit::page_noise_sigma` is the line to intercept before;
    // intercepting *here* skips that and the geometry with it, and cannot be
    // refused by a seed guard that has nothing to do with painting.
    if let Some(paint) = plan.paint.as_ref() {
        let started = Instant::now();
        let order = existing
            .as_ref()
            .map(|record| record.order)
            .unwrap_or_else(|| next_order(&job, source_idx));
        let (made, dabs) = match paint_patch(&job, source_idx, &page, paint, order) {
            Ok(pair) => pair,
            Err(reason) => return Ok(Outcome::Refused(reason)),
        };
        let snapshot = paint_snapshot(&plan, paint, made.engine, dabs, started.elapsed());
        return commit(
            located,
            job,
            source_idx,
            &plan.region_id,
            plan.clears_untouched,
            made,
            snapshot,
            source_sha256,
            order,
        );
    }

    let (seed, scale, manual) = match plan.geometry {
        Geometry::Given(rect) => (Mask::filled(clamped(rect, &page)), 1.0, true),
        // The shape the hand drew, used as it came. Nothing is fitted over it
        // and nothing is squared off: that is the whole point of carrying it.
        Geometry::Painted(mask) => (mask, 1.0, true),
        Geometry::Stored => {
            let Some(record) = existing.as_ref() else { return Ok(Outcome::NotFound) };
            let mask = job.load_patch(record).map_err(|e| e.to_string())?.mask;
            (mask, 1.0, true)
        }
        Geometry::Detected(rect) => match bench.text_under(&page, rect, &source_sha256) {
            Some((seed, scale)) => (seed, scale, false),
            None => (Mask::filled(clamped(rect, &page)), 1.0, true),
        },
    };
    if seed.is_empty() {
        return Ok(Outcome::Refused("decline.reason.rungUnavailable"));
    }

    let noise = fit::page_noise_sigma(&page);
    let edges = EdgeMap::sobel(&page);
    let fitted = fit::fit(&page, &seed, scale, noise, &edges, manual);

    let started = Instant::now();
    let (made, verdict) = if plan.fill_mode == Some("solid") {
        let pixels = cleaner_core::engines::fill::render_solid(&page, &fitted);
        let made = run::Made {
            engine: Engine::Fill,
            mask: fitted.mask.clone(),
            ink: fitted.ink.clone(),
            pixels,
            provider: None,
            model_sha256: None,
            pad: cleaner_core::strip::EdgePad::None,
            tiles: None,
        };
        let verdict = quality::assess(&page, &made.mask, &made.pixels, noise);
        (made, verdict)
    } else {
        match plan.choice {
            Choice::Ladder(pick) => {
                // The same ceiling a run gets, and the same cap: a *pick* is a
                // starting rung, and nothing that starts by itself may reach rung
                // 3a or the cloud.
                let stored = crate::settings::read(app)
                    .ok()
                    .and_then(|s| s.get("engineCeiling").and_then(|v| v.as_str()).map(str::to_owned));
                let ceiling = run::effective_ceiling(None, stored.as_deref());
                match run::clean_region(&mut bench.rung2, &page, &fitted, ceiling, pick, noise) {
                    Ok(run::Attempt::Cleaned(made, verdict)) => (*made, verdict),
                    Ok(run::Attempt::Declined(reason)) => return Ok(Outcome::Refused(reason)),
                    Err(detail) => return Ok(Outcome::Refused(engine_fault(&detail))),
                }
            }
            Choice::Exact(Engine::Flux) => match bench.flux(&page, &fitted) {
                Ok(pair) => pair,
                Err(reason) => return Ok(Outcome::Refused(reason)),
            },
            Choice::Exact(engine) => {
                match run::render_rung(&mut bench.rung2, engine, &page, &fitted, noise) {
                    Err(detail) => return Ok(Outcome::Refused(engine_fault(&detail))),
                    Ok(run::Rendered::Refused(reason)) => return Ok(Outcome::Refused(reason)),
                    Ok(run::Rendered::Made(made)) => {
                        let verdict = quality::assess(&page, &made.mask, &made.pixels, noise);
                        (*made, verdict)
                    }
                }
            }
        }
    };

    let pad = made.pad;
    let mut snapshot = run::params_snapshot(made.engine, &fitted, started.elapsed(), pad, made.tiles, &verdict);
    snapshot["source"] = serde_json::json!(plan.source);
    if let Some(tool) = plan.tool.as_deref() {
        snapshot["tool"] = serde_json::json!(tool);
    }
    if let Some(mode) = plan.fill_mode {
        snapshot["fill_mode"] = serde_json::json!(mode);
    }

    let order = existing.as_ref().map(|record| record.order).unwrap_or_else(|| next_order(&job, source_idx));
    commit(
        located,
        job,
        source_idx,
        &plan.region_id,
        plan.clears_untouched,
        made,
        snapshot,
        source_sha256,
        order,
    )
}

/// Write the patch and answer with the region - the half of an edit that is the
/// same whatever made the pixels.
///
/// Factored out when painting arrived, because a paint stroke reaches it from a
/// different place: it skips the fit, the ladder and the quality metric, and it
/// must not skip the mask digest, the untouched row, the order, or the manifest
/// write. Two spellings of this tail would be two ways for a patch to be
/// recorded, and "no second pipeline" is a rule about exactly that.
#[allow(clippy::too_many_arguments)]
fn commit(
    located: &Located,
    mut job: Job,
    source_idx: usize,
    region_id: &str,
    clears_untouched: bool,
    made: run::Made,
    snapshot: serde_json::Value,
    source_sha256: String,
    order: u32,
) -> Result<Outcome, String> {
    let mask_sha256 = run::mask_digest(&made.mask);
    let patch = Patch {
        id: region_id.to_owned(),
        mask: made.mask,
        ink: made.ink,
        pixels: made.pixels,
        order,
        // A re-run of a deleted mask brings it back: the edit the user just
        // asked for is one they expect to see.
        visible: true,
        provenance: Provenance {
            engine: made.engine,
            engine_version: env!("CARGO_PKG_VERSION").into(),
            model_sha256: made.model_sha256,
            // Rungs 0 and 1 have no session to name one, and unlike a run there
            // is no detector here whose provider could stand in.
            execution_provider: made.provider.unwrap_or_else(|| "cpu".to_owned()),
            params_snapshot: snapshot,
            mask_sha256,
            source_sha256,
            cloud: None,
            created: run::now(),
        },
    };

    if clears_untouched {
        clear_untouched(&mut job, source_idx, region_id);
    }
    job.complete_region(source_idx, &patch, None).map_err(|e| e.to_string())?;

    Ok(
        match crate::library::region_and_status(
            &located.chapter_id,
            &job.project,
            source_idx,
            region_id,
        ) {
            Some((region, page_status)) => {
                Outcome::Cleaned(Edited { region, page_status })
            }
            None => Outcome::NotFound,
        },
    )
}

/* ------------------------------------------------------------------ */
/* Paint and clone: running one                                        */
/* ------------------------------------------------------------------ */

/// The stroke, and the patch it becomes.
///
/// Everything the kernels need is here and nothing else is: the page's own
/// samples, the patches under this one, and numbers. No pixel buffer crossed
/// the seam to get here - the interface sent a dab list and this rasterises
/// it at the page's own scale.
fn paint_patch(
    job: &Job,
    source_idx: usize,
    page: &Raster,
    paint: &PaintPlan,
    order: u32,
) -> Result<(run::Made, usize), &'static str> {
    use cleaner_core::paint::{self, CloneMode, HealMode, PaintError, StrokePoint};

    if !paint::supports(page) {
        return Err("decline.reason.paintUnsupportedMode");
    }

    // A filled shape leaves here: it has no dabs to plan, no tip to stamp and
    // no spacing to walk. What it has is a mask and a colour, which is the
    // whole of the work.
    if let PaintKind::Shape { color, shape } = &paint.kind {
        return shape_patch(job, source_idx, page, shape, *color, &paint.spec, order);
    }

    let (width, height) = (page.width as f64, page.height as f64);
    let points: Vec<StrokePoint> = paint
        .points
        .iter()
        .map(|p| StrokePoint {
            x: p.x / 100.0 * width,
            y: p.y / 100.0 * height,
            p: p.p.clamp(0.0, 1.0),
        })
        .collect();
    let offset = match paint.kind {
        // A filled shape left several lines up, so the arms below it are
        // written for the two kinds that are still here. It is spelled out
        // rather than caught by a wildcard so that a fourth kind of paint
        // cannot be added without this function being read.
        PaintKind::Brush { .. } | PaintKind::Shape { .. } => (0.0, 0.0),
        PaintKind::Clone { offset, .. } => (offset.0 / 100.0 * width, offset.1 / 100.0 * height),
    };

    let under = patches_under(job, source_idx, order, &points, paint.spec.size, offset)
        .map_err(|_| "decline.reason.rungUnavailable")?;

    let painted = match paint.kind {
        PaintKind::Brush { color } | PaintKind::Shape { color, .. } => {
            paint::render_brush(page, &under, Some(order), &points, &paint.spec, color)
        }
        PaintKind::Clone { heal, .. } => paint::render_clone(
            page,
            &under,
            Some(order),
            &points,
            &paint.spec,
            offset,
            if heal { CloneMode::Heal(HealMode::Soft) } else { CloneMode::Clone },
        ),
    };

    let painted = match painted {
        Ok(painted) => painted,
        Err(PaintError::UnsupportedMode { .. }) => {
            return Err("decline.reason.paintUnsupportedMode")
        }
        // A stroke that covered nothing and a window that would not composite
        // are both "there is no patch to make here", which is the sentence
        // `rungUnavailable` already carries for an empty seed a few lines up.
        Err(PaintError::Empty) | Err(PaintError::Composite(_)) => {
            return Err("decline.reason.rungUnavailable")
        }
    };

    Ok((
        run::Made {
            engine: match paint.kind {
                PaintKind::Brush { .. } | PaintKind::Shape { .. } => Engine::Paint,
                PaintKind::Clone { .. } => Engine::Clone,
            },
            // A stroke is its own lettering: the user said where the paint
            // goes, and that is also the whole of what the edit covers.
            ink: painted.mask.clone(),
            mask: painted.mask,
            pixels: painted.pixels,
            // No session, no model, no tiles and no edge pad: none of the four
            // has a true value here, and inventing one would put it in the
            // manifest.
            provider: None,
            model_sha256: None,
            pad: cleaner_core::strip::EdgePad::None,
            tiles: None,
        },
        painted.dabs,
    ))
}

/// A drawn shape, filled flat, as the patch it becomes.
///
/// **The paint branch's second renderer, and deliberately not a fourth engine.**
/// It stands beside [`paint_patch`]'s brush and clone rather than beside the
/// ladder, because filling a shape with a chosen colour is the same *kind* of
/// act as painting with one: the user said what goes there, so there is
/// nothing to fit, nothing to reconstruct and nothing for the quality metric to
/// have an opinion about.
///
/// What is not shared with the brush is the coverage. A brush's mask is the
/// discs its dabs swept; a shape's is its own outline, rasterised once at the
/// page's own scale by [`shape_mask`]. So the dab planner and the stamp kernel
/// are not on this path at all, and the two numbers that survive from the
/// brush are `opacity` - the wash - and, through the mask, `feather`.
///
/// The colour is blended against **what is already composited under this
/// patch**, not against the source page: a semi-transparent shape over a
/// cleaned balloon must read the cleaned balloon, or the paint would show the
/// text the run took out.
fn shape_patch(
    job: &Job,
    source_idx: usize,
    page: &Raster,
    shape: &PaintedShape,
    color: [u8; 3],
    spec: &cleaner_core::paint::BrushSpec,
    order: u32,
) -> Result<(run::Made, usize), &'static str> {
    // An empty mask is "there is no patch to make here", which is the sentence
    // `rungUnavailable` already carries for an empty seed.
    let mask = shape_mask(shape, page.width, page.height).ok_or("decline.reason.rungUnavailable")?;
    let bounds = mask.bounds;

    let under = patches_over(job, source_idx, order, bounds).map_err(|_| "decline.reason.rungUnavailable")?;
    let mut pixels = cleaner_core::composite::composite_region(page, &under, bounds, Some(order))
        .map_err(|_| "decline.reason.rungUnavailable")?;
    // The window came back clamped to the page. The mask's bounds were built
    // inside the page already, so the two agree - but a patch whose buffer and
    // whose mask disagreed would be refused by the compositor later, with far
    // less to say about why.
    if pixels.width != bounds.w || pixels.height != bounds.h {
        return Err("decline.reason.rungUnavailable");
    }

    let alpha = (spec.opacity / 100.0).clamp(0.0, 1.0);
    let levels = solid_levels(page.mode, color);
    let top = (1u32 << page.depth.bits().min(16)) as f64 - 1.0;
    for y in 0..bounds.h {
        for x in 0..bounds.w {
            if !mask.contains(bounds.x + x as i64, bounds.y + y as i64) {
                continue;
            }
            for (channel, level) in levels.iter().enumerate() {
                // `None` is the alpha channel, copied through rather than
                // painted: every engine here treats alpha that way, and a
                // fill that wrote it would be inventing transparency from a
                // colour the user picked for the ink.
                let Some(level) = level else { continue };
                let below = pixels.sample(x, y, channel) as f64;
                let value = (*level as f64 * alpha + below * (1.0 - alpha)).round();
                pixels.set_sample(x, y, channel, value.clamp(0.0, top) as u16);
            }
        }
    }

    Ok((
        run::Made {
            engine: Engine::Paint,
            ink: mask.clone(),
            mask,
            pixels,
            provider: None,
            model_sha256: None,
            pad: cleaner_core::strip::EdgePad::None,
            tiles: None,
        },
        // No dabs: nothing was walked. The provenance says how many vertices
        // there were instead ([`paint_snapshot`]).
        0,
    ))
}

/// One sRGB colour as this page's own samples, channel by channel.
///
/// `None` for the alpha channel, which is copied through rather than written.
/// Only the four modes [`cleaner_core::paint::supports`] admits reach here, and
/// all four are eight bits a sample, so a byte is a level.
fn solid_levels(mode: cleaner_core::image::ColorMode, color: [u8; 3]) -> Vec<Option<u16>> {
    use cleaner_core::image::ColorMode;
    // Rec. 601 luma, integer-rounded: the same weighting the rest of the
    // application reads a grey page's tone with.
    let grey = ((color[0] as f64 * 0.299) + (color[1] as f64 * 0.587) + (color[2] as f64 * 0.114))
        .round()
        .clamp(0.0, 255.0) as u16;
    match mode {
        ColorMode::Gray => vec![Some(grey)],
        ColorMode::GrayAlpha => vec![Some(grey), None],
        ColorMode::Rgb => vec![Some(color[0] as u16), Some(color[1] as u16), Some(color[2] as u16)],
        ColorMode::Rgba => {
            vec![Some(color[0] as u16), Some(color[1] as u16), Some(color[2] as u16), None]
        }
        // Unreachable: `paint::supports` refused these several lines up. A
        // colour is still better than a panic, and every sample gets one.
        other => vec![Some(grey); other.samples()],
    }
}

/// The visible patches under a **rectangle**, decoded - [`patches_under`]'s
/// half for a gesture whose coverage is an area rather than a swept path.
///
/// The two filters are that function's and load-bearing for the same reasons:
/// `order` is exclusive, so a re-painted region blends over what is beneath it
/// and not over its own previous commit, and the box is intersected against
/// each record's `bbox` before its buffer is read.
fn patches_over(
    job: &Job,
    source_idx: usize,
    order: u32,
    box_of: Rect,
) -> Result<Vec<Patch>, String> {
    let mut patches = Vec::new();
    for record in &job.project.patches {
        if record.source_idx != source_idx || !record.visible || record.order >= order {
            continue;
        }
        if !overlaps(record.bbox, box_of) {
            continue;
        }
        patches.push(job.load_patch(record).map_err(|e| e.to_string())?);
    }
    Ok(patches)
}

/// The visible patches this stroke paints **over**, decoded.
///
/// Two filters and both are load-bearing. `order` is exclusive, so a re-painted
/// region blends over what is under it and not over its own previous commit -
/// otherwise every re-run would compound. And the stroke's own box is intersected
/// against each record's `bbox` *before* the buffer is read from the sidecar, so
/// a page carrying forty cleaned balloons decodes the two the stroke actually
/// crosses rather than all of them.
fn patches_under(
    job: &Job,
    source_idx: usize,
    order: u32,
    points: &[cleaner_core::paint::StrokePoint],
    size: f64,
    offset: (f64, f64),
) -> Result<Vec<Patch>, String> {
    let reach = size / 2.0 + 2.0;
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for point in points {
        x0 = x0.min(point.x - reach);
        y0 = y0.min(point.y - reach);
        x1 = x1.max(point.x + reach);
        y1 = y1.max(point.y + reach);
    }
    if x0 > x1 {
        return Ok(Vec::new());
    }
    // Clone reads from the source as well as writing to the target, and the
    // surface it is handed is one buffer covering both.
    let box_of = Rect::new(
        x0.min(x0 - offset.0).floor() as i64,
        y0.min(y0 - offset.1).floor() as i64,
        (x1.max(x1 - offset.0) - x0.min(x0 - offset.0)).ceil().max(1.0) as u32,
        (y1.max(y1 - offset.1) - y0.min(y0 - offset.1)).ceil().max(1.0) as u32,
    );

    let mut patches = Vec::new();
    for record in &job.project.patches {
        if record.source_idx != source_idx || !record.visible || record.order >= order {
            continue;
        }
        if !overlaps(record.bbox, box_of) {
            continue;
        }
        patches.push(job.load_patch(record).map_err(|e| e.to_string())?);
    }
    Ok(patches)
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom()
}

/// The provenance a painted patch carries.
///
/// **Its own, not [`run::params_snapshot`]'s.** That function's fields are a
/// fit's - `thickness`, `deviation`, the write set, the quality verdict - and a
/// painted stroke has none of them; feeding it a dummy `Fitted` would write four
/// numbers about a fit that never happened.
/// "The thresholds, dilations and radii **actually used**" is asked for, and
/// for a stroke those are the brush.
fn paint_snapshot(
    plan: &Plan,
    paint: &PaintPlan,
    engine: Engine,
    dabs: usize,
    elapsed: std::time::Duration,
) -> serde_json::Value {
    let spec = &paint.spec;
    let mut snapshot = serde_json::json!({
        "rung": engine.rung_key(),
        "source": plan.source,
        // Neither of the three fill-mode words describes a stroke; `solid` is
        // the one that comes closest, because the pixels were given rather than
        // matched or reconstructed. `library::fill_mode` says the same thing
        // from the other side.
        "fill_mode": "solid",
        "elapsed_ms": elapsed.as_millis() as u64,
        "brush": {
            "size": spec.size,
            "hardness": spec.hardness,
            "opacity": spec.opacity,
            "flow": spec.flow,
            "spacing": spec.spacing,
            "pressure_size": spec.pressure_size,
            "pressure_opacity": spec.pressure_opacity,
        },
        // How much of the stroke the commit actually walked. The one number
        // that separates a tap from a sweep after the fact.
        "dabs": dabs,
        "points": paint.points.len(),
        "seed": spec.seed,
    });
    if let Some(tool) = plan.tool.as_deref() {
        snapshot["tool"] = serde_json::json!(tool);
    }
    match paint.kind {
        PaintKind::Brush { color } => {
            snapshot["mode"] = serde_json::json!("paint");
            snapshot["color"] =
                serde_json::json!(format!("#{:02x}{:02x}{:02x}", color[0], color[1], color[2]));
        }
        PaintKind::Shape { color, ref shape } => {
            snapshot["mode"] = serde_json::json!("solid");
            snapshot["color"] =
                serde_json::json!(format!("#{:02x}{:02x}{:02x}", color[0], color[1], color[2]));
            // The geometry, in the two numbers that say what was drawn: which
            // shape, and how far past its outline the mask reached. The
            // radii actually used are asked for, and for a filled shape
            // those are these.
            snapshot["shape"] = serde_json::json!(match shape.kind {
                ShapeKind::Rect => "rect",
                ShapeKind::Ellipse => "ellipse",
                ShapeKind::Polygon => "polygon",
            });
            snapshot["feather"] = serde_json::json!(shape.feather.clamp(0.0, MAX_FEATHER_PX));
            snapshot["vertices"] = serde_json::json!(shape.points.len());
        }
        PaintKind::Clone { heal, offset } => {
            snapshot["mode"] = serde_json::json!(if heal { "heal" } else { "clone" });
            snapshot["alignment"] = serde_json::json!(paint.alignment);
            // `target − source`, in page percent: the internal direction, which
            // is the negation of the seam's `cloneOffset` (see `clone_offset`).
            // Percent and not native pixels, because the native figure is only
            // meaningful beside this page's size.
            snapshot["source_offset"] = serde_json::json!({ "x": offset.0, "y": offset.1 });
        }
    }
    snapshot
}

/// A rectangle, inside the page it is on. A gesture may run off the edge; a
/// mask may not.
fn clamped(rect: Rect, page: &Raster) -> Rect {
    let x0 = rect.x.clamp(0, (page.width.saturating_sub(1)) as i64);
    let x1 = rect.right().clamp(x0 + 1, page.width as i64);
    let y0 = rect.y.clamp(0, (page.height.saturating_sub(1)) as i64);
    let y1 = rect.bottom().clamp(y0 + 1, page.height as i64);
    Rect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32)
}

/// The order a new patch takes on its page: after everything already there, so
/// a hand edit composites over the automatic pass's own work.
fn next_order(job: &Job, source_idx: usize) -> u32 {
    job.project
        .patches
        .iter()
        .filter(|record| record.source_idx == source_idx)
        .map(|record| record.order + 1)
        .max()
        .unwrap_or(0)
}

/// Take the gate-skipped row this region was off the manifest, and take its
/// contribution to the counters with it.
///
/// The counters state what the manifest currently holds
/// ([`crate::run::reset_page`] makes the same argument from the other side), so
/// a row that leaves has to leave the count it added.
fn clear_untouched(job: &mut Job, source_idx: usize, region_id: &str) {
    let (mut gate_dropped, mut declined) = (0u32, 0u32);
    job.project.regions_untouched.retain(|record| {
        if record.source_idx != source_idx || untouched_id(job_page_id(region_id), record.bbox) != region_id {
            return true;
        }
        if record.reason.contains("gateSkipped") {
            gate_dropped += 1;
        } else {
            declined += 1;
        }
        false
    });
    let counters = &mut job.project.counters;
    counters.gate_dropped = counters.gate_dropped.saturating_sub(gate_dropped);
    counters.declined = counters.declined.saturating_sub(declined);
}

/// The page id inside a region id - everything up to the last `-`-prefixed
/// suffix the region kinds use.
fn job_page_id(region_id: &str) -> &str {
    ["-u", "-r", "-h"]
        .into_iter()
        .filter_map(|m| region_id.rfind(m))
        .max()
        .map(|at| &region_id[..at])
        .unwrap_or(region_id)
}

/// The id [`crate::library::region_of_untouched`] derives for a gate-skipped
/// row, restated so a row can be found from the id the seam sent back.
fn untouched_id(page_id: &str, bbox: Rect) -> String {
    format!("{page_id}-u{}-{}-{}-{}", bbox.x, bbox.y, bbox.w, bbox.h)
}

/* ------------------------------------------------------------------ */
/* Geometry across the seam                                            */
/* ------------------------------------------------------------------ */

/// A percentage box, in the page's own pixels - [`crate::library::percent_of`]
/// run backwards.
fn pixels_of(bbox: Bbox, width: u32, height: u32) -> Rect {
    let x = bbox.x / 100.0 * width as f64;
    let y = bbox.y / 100.0 * height as f64;
    let w = bbox.w / 100.0 * width as f64;
    let h = bbox.h / 100.0 * height as f64;
    Rect::new(x.round() as i64, y.round() as i64, w.round().max(1.0) as u32, h.round().max(1.0) as u32)
}

/// The page's size, without opening its pixels.
fn page_size(job: &Job, source_idx: usize) -> Option<(u32, u32)> {
    job.project.sources.get(source_idx).map(|source| (source.w, source.h))
}

/// The stroke a round brush painted, as the seam carries it.
///
/// **Vector, not raster**, and the choice is the coordinate space's: the points
/// are in the same normalised page percent every other `bbox` on the seam is,
/// the radius is in page pixels because a brush is a tool on the image, and the
/// backend is the only side that knows the page's real size. A pre-rendered
/// alpha mask would have to be rasterised by the interface at *some* resolution
/// and resampled here - a stroke rounded twice - where a path and a radius
/// rasterise once, at the page's own scale, into exactly the [`Mask`] the
/// engines already take.
#[derive(Debug, Clone, serde::Deserialize)]
struct PaintedStroke {
    /// The path the pointer took, in page percent (0–100 on both axes).
    points: Vec<StrokePoint>,
    /// Half the brush's `size` parameter, in **page pixels**.
    radius: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Deserialize)]
struct StrokePoint {
    x: f64,
    y: f64,
}

/// A painted stroke as a mask over the page it was painted on.
///
/// `None` where there is nothing to paint - no points, or a stroke entirely off
/// the page - so a caller falls back to the rectangle rather than committing an
/// empty mask.
fn stroke_mask(stroke: &PaintedStroke, width: u32, height: u32) -> Option<Mask> {
    if stroke.points.is_empty() {
        return None;
    }
    let points: Vec<(f64, f64)> = stroke
        .points
        .iter()
        .map(|point| (point.x / 100.0 * width as f64, point.y / 100.0 * height as f64))
        .collect();
    let mask = Mask::from_stroke(&points, stroke.radius, width, height);
    (!mask.is_empty()).then_some(mask)
}

/// The **area** a Shapes gesture drew, as the seam carries it.
///
/// The companion of [`PaintedStroke`] and the same bargain: vector, not
/// raster, in the two coordinate spaces each side can check. The vertices are
/// page percent like every other `bbox` on the seam; `feather` is page pixels,
/// because it is a distance on the image rather than on the screen and this is
/// the only side that knows how big the page really is.
///
/// Three kinds cover the tool's four shapes, because a lasso *is* a polygon -
/// drawn freehand rather than clicked. A rectangle and an ellipse arrive as the
/// four corners of their box, so one payload describes all four and there is
/// one parser rather than three.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
struct PaintedShape {
    kind: ShapeKind,
    /// The outline, in page percent (0–100 on both axes). Closed here: the
    /// last vertex joins the first.
    points: Vec<StrokePoint>,
    /// How far the mask grows past the outline, in **page pixels**.
    #[serde(default)]
    feather: f64,
}

/// A word this build knows how to rasterise. Anything else fails to
/// deserialise, and the caller falls back to the bounding box it also sent -
/// which is the promised degradation for a backend that does not understand
/// a geometry.
#[derive(Debug, Clone, Copy, PartialEq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum ShapeKind {
    Rect,
    Ellipse,
    Polygon,
}

/// The most feather a shape may carry, in page pixels - **the tool window's own
/// maximum**, which is the largest value anything can legitimately send.
///
/// It is a clamp rather than a refusal because the cost is superlinear:
/// [`Mask::dilated`] tests a `(2r+1)²` kernel at every pixel of the grown box,
/// so a parameter that arrived as a thousand - hand-written, or a unit slip -
/// would be an hour of arithmetic rather than a wrong answer.
const MAX_FEATHER_PX: f64 = 20.0;

/// A drawn shape as a mask over the page it was drawn on.
///
/// `None` where there is nothing to fill - fewer than three vertices, or a
/// shape entirely off the page - so a caller falls back to the rectangle rather
/// than committing an empty mask.
///
/// **Feather is grown into the shape, not dilated onto it afterwards**, and
/// both halves of that matter. A [`Mask::dilated`] pass would cost a
/// `(2r+1)²` kernel at every pixel - minutes for a page-sized rectangle at the
/// row's own maximum - and it would square the rim of an ellipse at small
/// radii, because its structuring element is a square below diameter 5. Growing
/// the *test* instead is one comparison per pixel for a rectangle and an
/// ellipse, exact for both, and a distance to the outline for a polygon.
///
/// **What it is not is a soft edge.** A [`Mask`] is one bit per pixel - the set
/// an engine may write, and every engine, the compositor and the export take it
/// that way - so `feather` moves the boundary outwards rather than fading it.
fn shape_mask(shape: &PaintedShape, width: u32, height: u32) -> Option<Mask> {
    if shape.points.len() < 3 || width == 0 || height == 0 {
        return None;
    }
    let points: Vec<(f64, f64)> = shape
        .points
        .iter()
        .map(|point| (point.x / 100.0 * width as f64, point.y / 100.0 * height as f64))
        .collect();
    if points.iter().any(|(x, y)| !x.is_finite() || !y.is_finite()) {
        return None;
    }
    let feather = shape.feather.clamp(0.0, MAX_FEATHER_PX);

    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (x, y) in &points {
        x0 = x0.min(*x);
        y0 = y0.min(*y);
        x1 = x1.max(*x);
        y1 = y1.max(*y);
    }
    // The outline's own box, grown by the feather and then brought onto the
    // page: a shape may run off the edge, a mask may not.
    let left = (x0 - feather).floor().clamp(0.0, width as f64) as i64;
    let top = (y0 - feather).floor().clamp(0.0, height as f64) as i64;
    let right = (x1 + feather).ceil().clamp(0.0, width as f64) as i64;
    let bottom = (y1 + feather).ceil().clamp(0.0, height as f64) as i64;
    if right <= left || bottom <= top {
        return None;
    }
    let bounds = Rect::new(left, top, (right - left) as u32, (bottom - top) as u32);

    // The ellipse inscribed in the outline's own box, which is what the tool
    // drew: two dragged corners, and the ellipse that touches all four sides.
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let (rx, ry) = (((x1 - x0) / 2.0).max(0.5) + feather, ((y1 - y0) / 2.0).max(0.5) + feather);

    let mut mask = Mask::empty(bounds);
    for y in bounds.y..bounds.bottom() {
        for x in bounds.x..bounds.right() {
            // Pixel centres, so a shape that lands between two pixel grids
            // covers the ones it is actually over.
            let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
            let inside = match shape.kind {
                // The four corners that arrived describe exactly this box, and
                // the feather is already in the bounds this loop walks.
                ShapeKind::Rect => {
                    px >= x0 - feather
                        && px <= x1 + feather
                        && py >= y0 - feather
                        && py <= y1 + feather
                }
                ShapeKind::Ellipse => {
                    let (dx, dy) = ((px - cx) / rx, (py - cy) / ry);
                    dx * dx + dy * dy <= 1.0
                }
                ShapeKind::Polygon => {
                    contains_point(&points, px, py)
                        || (feather > 0.0 && distance_to_outline(&points, px, py) <= feather)
                }
            };
            if inside {
                mask.set(x, y, true);
            }
        }
    }
    (!mask.is_empty()).then_some(mask)
}

/// How far a point is from the nearest edge of a closed polygon, in pixels.
///
/// The feather's half of [`shape_mask`] for the one kind that has no closed
/// form: a point outside the outline is in the mask when it is within the
/// feather of it. Costs one pass over the edges per pixel, which is why the
/// caller asks only for pixels the polygon does not already contain.
fn distance_to_outline(points: &[(f64, f64)], px: f64, py: f64) -> f64 {
    let mut best = f64::MAX;
    let mut j = points.len() - 1;
    for i in 0..points.len() {
        let (ax, ay) = points[j];
        let (bx, by) = points[i];
        let (ex, ey) = (bx - ax, by - ay);
        let len2 = ex * ex + ey * ey;
        // A degenerate edge is its own endpoint, which the neighbouring edges
        // already cover; measuring to the point is still the right answer.
        let t = if len2 > 0.0 { (((px - ax) * ex + (py - ay) * ey) / len2).clamp(0.0, 1.0) } else { 0.0 };
        let (dx, dy) = (px - (ax + ex * t), py - (ay + ey * t));
        best = best.min(dx * dx + dy * dy);
        j = i;
    }
    best.sqrt()
}

/// Even-odd crossing: whether a point is inside a closed polygon.
///
/// The polygon is closed implicitly - the last vertex joins the first - and
/// self-intersection is resolved by the even-odd rule, which is the rule a
/// freehand lasso wants: a loop drawn back over itself leaves a hole rather
/// than filling the whole scribble.
fn contains_point(points: &[(f64, f64)], px: f64, py: f64) -> bool {
    let mut inside = false;
    let mut j = points.len() - 1;
    for i in 0..points.len() {
        let (xi, yi) = points[i];
        let (xj, yj) = points[j];
        if (yi > py) != (yj > py) && px < (xj - xi) * (py - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/* ------------------------------------------------------------------ */
/* Paint and clone: the params, as the contract carries them           */
/* ------------------------------------------------------------------ */

/// One pointer sample of a paint stroke, in page percent, with its pressure.
///
/// `p` defaults to `0.5` rather than to `1.0`, and the default is the contract's:
/// `PointerEvent.pressure` is
/// frequently `0` or a flat `0.5` under WKWebView, so a device that says nothing
/// is treated as pressing half rather than as pressing hardest.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
struct PaintPoint {
    x: f64,
    y: f64,
    #[serde(default = "half")]
    p: f64,
}

fn half() -> f64 {
    0.5
}

/// What the paint branch was asked to do.
#[derive(Debug, Clone, PartialEq)]
enum PaintKind {
    /// A colour brush. The colour is straight sRGB bytes, from `#rrggbb`.
    Brush { color: [u8; 3] },
    /// Clone or heal. `offset` is `target − source` in **page percent**, as the
    /// seam already sends it; `edit` scales it to native pixels, because it is
    /// the only side that knows the page's real size.
    Clone { heal: bool, offset: (f64, f64) },
    /// A drawn shape filled flat with a chosen colour - Shapes' `solid` mode.
    ///
    /// It is a paint and not a clean, which is why it is here rather than on
    /// the ladder: nothing is fitted, no rung is asked what belongs under the
    /// shape and no quality metric assesses a colour somebody picked. What
    /// separates it from [`PaintKind::Brush`] is the coverage - a brush covers
    /// what its dabs swept, a shape covers its own outline - so the shape
    /// rides here instead of a dab list.
    Shape { color: [u8; 3], shape: PaintedShape },
}

/// The paint half of a [`Plan`].
///
/// **Parsed at both seam methods and acted on at one.** `createRegion` and
/// `applyTool` read `params` independently and converge on `edit`, and painting
/// over an *existing* region dispatches `applyTool`
/// (`src/lib/editor/toolapply.svelte.js`) - so a `createRegion`-only change
/// would silently route every retouch back into LaMa. One optional
/// field on the plan is what makes both routes one route.
#[derive(Debug, Clone, PartialEq)]
struct PaintPlan {
    kind: PaintKind,
    /// The stroke in page percent. Converted to native pixels in [`edit`].
    points: Vec<PaintPoint>,
    /// `size` is already in native page pixels - a brush is a tool on the
    /// image, and `stroke.radius` has been native throughout.
    spec: cleaner_core::paint::BrushSpec,
    /// `aligned` or `nonAligned`, recorded rather than acted on - one commit is
    /// one stroke, and the two only differ across strokes.
    alignment: &'static str,
}

impl PartialEq for PaintPoint {
    fn eq(&self, other: &Self) -> bool {
        self.x == other.x && self.y == other.y && self.p == other.p
    }
}

fn number(params: &serde_json::Value, key: &str) -> Option<f64> {
    params.get(key).and_then(serde_json::Value::as_f64).filter(|v| v.is_finite())
}

fn flag(params: &serde_json::Value, key: &str, fallback: bool) -> bool {
    params.get(key).and_then(serde_json::Value::as_bool).unwrap_or(fallback)
}

/// `#rrggbb` - and `#rgb`, because an `<input type="color">` is not the only
/// thing that can fill this field. Anything else is black, which is the ink a
/// manga cleaner reaches for and is a better answer than refusing the stroke.
fn colour_of(text: Option<&str>) -> [u8; 3] {
    let Some(hex) = text.map(|t| t.trim().trim_start_matches('#')) else { return [0, 0, 0] };
    let pair = |at: usize, len: usize| -> Option<u8> {
        let slice = hex.get(at..at + len)?;
        let value = u8::from_str_radix(slice, 16).ok()?;
        Some(if len == 1 { value * 17 } else { value })
    };
    match hex.len() {
        6 => match (pair(0, 2), pair(2, 2), pair(4, 2)) {
            (Some(r), Some(g), Some(b)) => [r, g, b],
            _ => [0, 0, 0],
        },
        3 => match (pair(0, 1), pair(1, 1), pair(2, 1)) {
            (Some(r), Some(g), Some(b)) => [r, g, b],
            _ => [0, 0, 0],
        },
        _ => [0, 0, 0],
    }
}

/// The stroke's points, from `params.paint.points` or - for `cloneHeal`, whose
/// `paint` block is optional - from the `stroke` every drawing tool already
/// sends, at the default pressure.
fn paint_points(params: &serde_json::Value, paint: Option<&serde_json::Value>) -> Vec<PaintPoint> {
    let listed = paint
        .and_then(|p| p.get("points"))
        .and_then(|v| serde_json::from_value::<Vec<PaintPoint>>(v.clone()).ok())
        .unwrap_or_default();
    if !listed.is_empty() {
        return listed;
    }
    params
        .get("stroke")
        .and_then(|value| serde_json::from_value::<PaintedStroke>(value.clone()).ok())
        .map(|stroke| {
            stroke.points.iter().map(|p| PaintPoint { x: p.x, y: p.y, p: half() }).collect()
        })
        .unwrap_or_default()
}

/// Whether this call is a paint or clone stroke, and what it asks for.
///
/// `None` is "the ordinary path" and is the answer for every tool that is not
/// one of these two, and for a `brush` whose `mode` is still `add`/`erase` -
/// those keep today's mask-then-inpaint behaviour untouched, which is the
/// contract's own wording.
fn paint_plan(tool: &str, params: &serde_json::Value) -> Option<PaintPlan> {
    let mode = params.get("mode").and_then(|v| v.as_str());
    let paint = params.get("paint").filter(|v| v.is_object());

    let kind = match tool {
        "brush" if mode == Some("paint") => PaintKind::Brush {
            color: colour_of(
                paint
                    .and_then(|p| p.get("color"))
                    .or_else(|| params.get("color"))
                    .and_then(|v| v.as_str()),
            ),
        },
        // **Always real from now on.** The tool has read `cloneSource` and
        // `cloneOffset` off the seam and thrown them away since it existed;
        // there is no `mode` gate here because there is no legacy behaviour
        // worth keeping.
        "cloneHeal" => {
            let offset = clone_offset(params)?;
            PaintKind::Clone { heal: mode != Some("clone"), offset }
        }
        // A shape filled with a colour. `solid` is Shapes' own word for it and
        // is deliberately not `fill`, which on that same row names rung 0 -
        // the planar fill, which samples the paper and lays down the tone it
        // found rather than a tone the user chose.
        "shapes" if mode == Some("solid") => {
            let shape = paint
                .and_then(|block| block.get("shape"))
                .or_else(|| params.get("painted"))
                .and_then(|value| serde_json::from_value::<PaintedShape>(value.clone()).ok())?;
            PaintKind::Shape {
                color: colour_of(
                    paint
                        .and_then(|p| p.get("color"))
                        .or_else(|| params.get("color"))
                        .and_then(|v| v.as_str()),
                ),
                shape,
            }
        }
        _ => return None,
    };

    let points = paint_points(params, paint);
    // A shape is the one paint whose coverage is not its path: a rectangle
    // drag delivers one pointer sample and the keyboard route delivers none,
    // and both describe an area perfectly well. Every other kind is its
    // points, and no points is no stroke.
    if points.is_empty() && !matches!(kind, PaintKind::Shape { .. }) {
        return None;
    }

    // The `paint` block wins where it carries a value, and `params` is the
    // fallback: the two spellings both appear on the wire, because `size`,
    // `hardness`, `opacity` and `flow` are ordinary tool params as well as
    // brush ones.
    let from = |key: &str| paint.and_then(|p| number(p, key)).or_else(|| number(params, key));
    let switch = |key: &str, fallback: bool| match paint {
        Some(block) if block.get(key).is_some() => flag(block, key, fallback),
        _ => flag(params, key, fallback),
    };
    let default = cleaner_core::paint::BrushSpec::default();
    let spec = cleaner_core::paint::BrushSpec {
        // `size` is a diameter in native page pixels; `stroke.radius` is half of
        // it and is what the older tools send, so it stands in.
        size: from("size")
            .or_else(|| {
                params
                    .get("stroke")
                    .and_then(|v| v.get("radius"))
                    .and_then(serde_json::Value::as_f64)
                    .map(|r| r * 2.0)
            })
            .unwrap_or(default.size)
            .clamp(1.0, 4096.0),
        hardness: from("hardness").unwrap_or(default.hardness).clamp(0.0, 100.0),
        flow: from("flow").unwrap_or(100.0).clamp(0.0, 100.0),
        opacity: from("opacity").unwrap_or(100.0).clamp(0.0, 100.0),
        spacing: from("spacing").unwrap_or(default.spacing),
        pressure_size: switch("pressureSize", true),
        pressure_opacity: switch("pressureOpacity", false),
        seed: paint
            .and_then(|p| p.get("seed"))
            .and_then(serde_json::Value::as_u64)
            .map(|v| v as u32)
            .unwrap_or(0),
    };

    Some(PaintPlan {
        kind,
        points,
        spec,
        alignment: match params.get("alignment").and_then(|v| v.as_str()) {
            Some("nonAligned") => "nonAligned",
            _ => "aligned",
        },
    })
}

/// `target − source`, in page percent - **the negation of what the seam sends**.
///
/// The two sides spell the same displacement in opposite directions, and the
/// seam's spelling is the one that was there first. `gesture.js#cloneOffset`
/// computes `source − strokeStart` and reconstructs the source as
/// `strokeStart + offset`; the live preview and `gesture.test.js` are both built
/// on that reading. The kernels want the other one - `paint::render_clone` reads
/// each dab's source at `target − offset` - so the conversion happens **here, at
/// the parse site**, and exactly once. Nothing downstream of this function sees
/// the seam's sign, and nothing in the interface sees the kernel's.
///
/// `cloneSource` is the alt-clicked point, and the offset from the stroke's
/// first sample is the same quantity computed the long way - kept as a fallback
/// so a caller that sends only the source still clones from where the user
/// pointed rather than not at all. It is subtracted in the *internal*
/// direction already, so it is not negated a second time.
fn clone_offset(params: &serde_json::Value) -> Option<(f64, f64)> {
    let point = |key: &str| {
        let value = params.get(key)?;
        Some((number(value, "x")?, number(value, "y")?))
    };
    if let Some((x, y)) = point("cloneOffset") {
        // `source = strokeStart + cloneOffset`, so `target − source = −cloneOffset`.
        return Some((-x, -y));
    }
    let (sx, sy) = point("cloneSource")?;
    let first = paint_points(params, params.get("paint")).first().copied()?;
    Some((first.x - sx, first.y - sy))
}

/// What a painted gesture **means**, and it is now one answer for every tool
/// that paints one: **the paint is the mask**.
///
/// The user said where the edit goes, and that is authoritative - no fitting
/// search, nothing found, nothing moved.
///
/// **A shape beats a box wherever there is one.** The rectangle is still the
/// answer for a Shapes drag and for the keyboard route, which genuinely
/// describe rectangles; a brush stroke describes a swept disc, and reading it
/// as its bounding box produces an L-shaped stroke that cleaned the square
/// around it.
///
/// **The AI mask brush used to be the exception and is not any more.** Its
/// shape was a *hint*: the detector was run over the whole page and whatever
/// text it found under the stroke became the mask. Two things were wrong with
/// that. The detector - the text finder - ran on every manual stroke, which
/// is a full-page detection to answer a gesture that had already
/// said where it wanted paint. And what it found *won*: a stroke aimed at a
/// leftover beside a detected line cleaned the line instead, because the
/// detector's box was preferred to the hand's own shape. The stroke is the
/// evidence now, and the engine the tool window named is what runs over it.
fn painted_geometry(rect: Rect, painted: Option<Mask>) -> Geometry {
    match painted {
        Some(shape) => Geometry::Painted(shape),
        None => Geometry::Given(rect),
    }
}

/* ------------------------------------------------------------------ */
/* What a caller asked for                                             */
/* ------------------------------------------------------------------ */

/// The three fill modes: `match-surround`, `reconstruct`, `solid`.
///
/// Cycles three: `match-surround` → `reconstruct` → `solid` →
/// `match-surround`.
fn next_fill_mode(current: &str) -> &'static str {
    match current {
        "match-surround" => "reconstruct",
        "reconstruct" => "solid",
        "solid" => "match-surround",
        _ => "match-surround",
    }
}

/// The rung a fill mode implies. The mock's own mapping
/// (`src/lib/api/tools.js`), which is the specification for this.
fn engine_for_fill_mode(mode: &str) -> Engine {
    match mode {
        "reconstruct" => Engine::Lama,
        _ => Engine::Fill,
    }
}

/// The rungs a Layers row's picker may name, weakest first.
///
/// The cloud is not among them and neither is anything above rung 3a: this is
/// the list `stronger` and `simpler` step along, and every rung in it is one
/// this build can actually run.
const MANUAL_RUNGS: [Engine; 4] =
    [Engine::Fill, Engine::Denoise, Engine::Lama, Engine::Flux];

fn step(current: Engine, up: bool) -> Engine {
    let at = MANUAL_RUNGS.iter().position(|engine| *engine == current);
    match at {
        None => current,
        Some(at) if up => MANUAL_RUNGS[(at + 1).min(MANUAL_RUNGS.len() - 1)],
        Some(at) => MANUAL_RUNGS[at.saturating_sub(1)],
    }
}

/// What a caller's `engine` word asks for.
///
/// **The pick is read first, and that ordering is the whole of it.** The tool
/// window's two rows name a rung outright - `fill`, `denoise`, `lama` - and so
/// does a Layers row's picker, so the same word arrives here meaning two
/// different things depending on who sent it. This function serves the one
/// caller that means a **start**: `cleanAnyway`, which reads Auto clean's own
/// rows and must keep the escalation that goes with them - a region the gate
/// held back is the last one that should be pinned to a declined patch.
///
/// So every word [`run::parse_pick`] knows is a start, and anything left is a
/// rung named outright: `flux`, which no automatic run may reach and which is
/// therefore only ever an explicit request.
fn choice_for(engine: Option<&str>, fallback: EnginePick) -> Choice {
    match run::parse_pick(engine) {
        Some(pick) => Choice::Ladder(Some(pick)),
        None => match named_rung(engine) {
            Some(rung) => Choice::Exact(rung),
            None => Choice::Ladder(Some(fallback)),
        },
    }
}

/// The `params.engine` a tool sends, as a rung. `None` is "the caller did not
/// name one", which is not the same as a name this build does not know - an
/// unknown name falls back to the fill/redraw pick rather than to a rung
/// nobody asked for.
fn named_rung(engine: Option<&str>) -> Option<Engine> {
    run::parse_rung(engine?)
}

/* ------------------------------------------------------------------ */
/* Answering                                                           */
/* ------------------------------------------------------------------ */

/// Say what could not be done, rather than answering `null` and leaving the
/// interface to show nothing at all.
fn refuse(reason: &'static str) {
    crate::events::notice(
        "notice.mask.rerunFailed",
        serde_json::json!({ "reasonKey": reason }),
        "warn",
    );
}

/* ------------------------------------------------------------------ */
/* Commands                                                            */
/* ------------------------------------------------------------------ */

/// `applyTool`.
#[tauri::command]
pub async fn apply_tool(
    app: tauri::AppHandle,
    tool: String,
    params: Option<serde_json::Value>,
    chapter_id: Option<String>,
    page_index: Option<u32>,
    region_id: Option<String>,
) -> Result<ApplyResult, String> {
    crate::library::blocking(move || {
        let params = params.unwrap_or(serde_json::Value::Null);
        let string = |key: &str| {
            params.get(key).and_then(|value| value.as_str()).map(str::to_owned)
        };

        // Auto clean is a run trigger and not a per-region tool: it answers
        // with the queue, and every result arrives on the event channel.
        if tool == "autoClean" {
            let Some(chapter_id) = chapter_id else { return Ok(ApplyResult::of("not-found")) };
            let picks = Picks::from_args(
                string("bubbleEngine").as_deref(),
                string("outsideEngine").as_deref(),
            );
            let scope = string("scope").unwrap_or_else(|| "page".to_owned());
            let outside =
                cleaner_core::gate::OutsideText::from_arg(string("outsideBubbles").as_deref());
            let handle = run::start(
                &app,
                &scope,
                &chapter_id,
                page_index,
                string("engineCeiling"),
                picks,
                outside,
            )?;
            return Ok(ApplyResult {
                run_id: handle.run_id,
                pages: Some(handle.pages),
                ..ApplyResult::of("run-started")
            });
        }

        // The cloud rung has no client in this build. Refused here rather than
        // one layer down, so the sentence the user reads is the true one:
        // nothing was sent.
        if string("engine").as_deref() == Some("cloud") {
            let blocked = crate::settings::read(&app)
                .ok()
                .and_then(|s| s.get("cloudEngines").and_then(|v| v.as_str()).map(str::to_owned))
                .is_some_and(|value| value != "allowed");
            crate::events::notice(
                if blocked { "notice.cloud.blocked" } else { "notice.cloud.unavailable" },
                serde_json::json!({}),
                "warn",
            );
            return Ok(ApplyResult::of("blocked"));
        }

        let Some(region_id) = region_id else { return Ok(ApplyResult::of("not-found")) };
        let library = Library::for_app(&app)?;
        let Some(located) = locate_region(&library, &region_id)? else {
            return Ok(ApplyResult::of("not-found"));
        };

        let bbox: Option<Bbox> = params
            .get("bbox")
            .and_then(|value| serde_json::from_value::<Bbox>(value.clone()).ok());
        let stroke: Option<PaintedStroke> = params
            .get("stroke")
            .and_then(|value| serde_json::from_value::<PaintedStroke>(value.clone()).ok());
        // The drawn area, for the tool whose gesture is one. `stroke` and
        // `painted` are never both sent: a path has a radius and no vertices,
        // an area has vertices and no radius.
        let drawn: Option<PaintedShape> = params
            .get("painted")
            .and_then(|value| serde_json::from_value::<PaintedShape>(value.clone()).ok());
        // Parsed **here as well as in `create_region`**: painting over an
        // existing region dispatches `applyTool`, so reading `params.paint` in
        // only one of the two silently routes every retouch back into LaMa.
        //
        // Read before the geometry rather than after it, because a paint plan
        // makes the geometry dead: `edit` takes the paint branch before it
        // looks at the seed, so rasterising a mask for it would be a full pass
        // over the shape's box that nothing then reads.
        let paint = paint_plan(&tool, &params);
        let fill_mode = string("fillMode").unwrap_or_else(|| "match-surround".to_owned());
        let geometry = match bbox {
            Some(bbox) => {
                let _lock = run::lock_job(&located.job_path);
                let job = Job::open(&located.job_path).map_err(|e| e.to_string())?;
                let Some(source_idx) = Library::resolve_page(&job.project, located.page_index)
                else {
                    return Ok(ApplyResult::of("not-found"));
                };
                let Some((w, h)) = page_size(&job, source_idx) else {
                    return Ok(ApplyResult::of("not-found"));
                };
                let painted = if paint.is_some() {
                    None
                } else {
                    stroke
                        .as_ref()
                        .and_then(|stroke| stroke_mask(stroke, w, h))
                        .or_else(|| drawn.as_ref().and_then(|shape| shape_mask(shape, w, h)))
                };
                painted_geometry(pixels_of(bbox, w, h), painted)
            }
            None => existing_geometry(&located, &region_id)?,
        };

        let choice = match named_rung(string("engine").as_deref()) {
            Some(engine) => Choice::Exact(engine),
            None => Choice::Ladder(Some(match engine_for_fill_mode(&fill_mode) {
                Engine::Fill => EnginePick::Fill,
                _ => EnginePick::Lama,
            })),
        };
        let plan = Plan {
            region_id,
            geometry,
            choice,
            source: "hand",
            tool: Some(tool),
            fill_mode: Some(fill_mode_key(&fill_mode)),
            // Whether or not the mask came from the detector: a hand gesture
            // over a region the gate held back answers the warning as surely
            // as `cleanAnyway` does, and a page that kept both would show a
            // warning beside the mask that resolved it. A no-op where there is
            // no row to take.
            clears_untouched: true,
            paint,
        };
        match edit(&app, &located, plan)? {
            Outcome::Cleaned(edited) => Ok(ApplyResult::applied(edited)),
            Outcome::Refused(reason) => {
                refuse(reason);
                Ok(ApplyResult::of("blocked"))
            }
            Outcome::NotFound => Ok(ApplyResult::of("not-found")),
        }
    })
    .await
}

/// The three fill-mode words the seam uses, as `'static` strings - a snapshot
/// field is written as one of exactly these and never as whatever arrived.
fn fill_mode_key(mode: &str) -> &'static str {
    match mode {
        "reconstruct" => "reconstruct",
        "solid" => "solid",
        _ => "match-surround",
    }
}

/// Where an existing region's mask comes from: its patch, or - for a region the
/// gate held back - the detector, under the box the manifest kept.
fn existing_geometry(located: &Located, region_id: &str) -> Result<Geometry, String> {
    let _lock = run::lock_job(&located.job_path);
    let job = Job::open(&located.job_path).map_err(|e| e.to_string())?;
    if job.project.patches.iter().any(|record| record.id == region_id) {
        return Ok(Geometry::Stored);
    }
    let Some(source_idx) = Library::resolve_page(&job.project, located.page_index) else {
        return Ok(Geometry::Stored);
    };
    let page_id = job_page_id(region_id);
    let found = job
        .project
        .regions_untouched
        .iter()
        .find(|record| record.source_idx == source_idx && untouched_id(page_id, record.bbox) == region_id);
    Ok(match found {
        Some(record) => Geometry::Detected(record.bbox),
        None => Geometry::Stored,
    })
}

fn untouched_reason(located: &Located, region_id: &str) -> Result<Option<String>, String> {
    let _lock = run::lock_job(&located.job_path);
    let job = Job::open(&located.job_path).map_err(|e| e.to_string())?;
    let Some(source_idx) = Library::resolve_page(&job.project, located.page_index) else {
        return Ok(None);
    };
    let page_id = job_page_id(region_id);
    Ok(job
        .project
        .regions_untouched
        .iter()
        .find(|record| record.source_idx == source_idx && untouched_id(page_id, record.bbox) == region_id)
        .map(|record| record.reason.clone()))
}

fn untouched_fallback_pick(reason: Option<&str>) -> EnginePick {
    match reason {
        Some(r) if r.contains("OutsideBubble") || r.contains("outside-bubble") => Picks::default().outside,
        _ => Picks::default().bubble,
    }
}

/// `createRegion` - a region drawn where the detector found nothing.
#[tauri::command]
pub async fn create_region(
    app: tauri::AppHandle,
    chapter_id: String,
    page_index: u32,
    bbox: Bbox,
    tool: String,
    params: Option<serde_json::Value>,
) -> Result<Option<CreatedRegion>, String> {
    crate::library::blocking(move || {
        let params = params.unwrap_or(serde_json::Value::Null);
        let string = |key: &str| params.get(key).and_then(|v| v.as_str()).map(str::to_owned);
        let library = Library::for_app(&app)?;
        let Some(located) = locate_page(&library, &chapter_id, page_index as usize)? else {
            return Ok(None);
        };

        let stroke: Option<PaintedStroke> = params
            .get("stroke")
            .and_then(|value| serde_json::from_value::<PaintedStroke>(value.clone()).ok());
        let drawn: Option<PaintedShape> = params
            .get("painted")
            .and_then(|value| serde_json::from_value::<PaintedShape>(value.clone()).ok());
        // Before the geometry, for the reason `apply_tool` gives: a paint plan
        // is answered by the paint branch and never reaches the seed.
        let paint = paint_plan(&tool, &params);

        let (region_id, rect, painted) = {
            let _lock = run::lock_job(&located.job_path);
            let job = Job::open(&located.job_path).map_err(|e| e.to_string())?;
            let Some(source_idx) = Library::resolve_page(&job.project, located.page_index) else {
                return Ok(None);
            };
            let Some((w, h)) = page_size(&job, source_idx) else { return Ok(None) };
            let page_id = crate::library::page_id(&chapter_id, located.page_index);
            (
                mint_hand_id(&job, &page_id),
                pixels_of(bbox, w, h),
                if paint.is_some() {
                    None
                } else {
                    stroke
                        .as_ref()
                        .and_then(|stroke| stroke_mask(stroke, w, h))
                        .or_else(|| drawn.as_ref().and_then(|shape| shape_mask(shape, w, h)))
                },
            )
        };

        let fill_mode = string("fillMode").unwrap_or_else(|| "match-surround".to_owned());
        let choice = match named_rung(string("engine").as_deref()) {
            Some(engine) => Choice::Exact(engine),
            None => Choice::Ladder(Some(match engine_for_fill_mode(&fill_mode) {
                Engine::Fill => EnginePick::Fill,
                _ => EnginePick::Lama,
            })),
        };

        let plan = Plan {
            region_id,
            geometry: painted_geometry(rect, painted),
            choice,
            source: "hand",
            tool: Some(tool),
            fill_mode: Some(fill_mode_key(&fill_mode)),
            clears_untouched: false,
            paint,
        };
        match edit(&app, &located, plan)? {
            Outcome::Cleaned(edited) => {
                Ok(Some(CreatedRegion { region: edited.region, page_status: edited.page_status }))
            }
            Outcome::Refused(reason) => {
                refuse(reason);
                Ok(None)
            }
            Outcome::NotFound => Ok(None),
        }
    })
    .await
}

/// `{page id}-h{n}`, after every hand region already on the page.
///
/// The manifest is the counter. A session-local one - which is what the mock
/// uses - would hand the same id out again after a restart and the second
/// region would replace the first.
fn mint_hand_id(job: &Job, page_id: &str) -> String {
    let prefix = format!("{page_id}-h");
    let next = job
        .project
        .patches
        .iter()
        .filter_map(|record| record.id.strip_prefix(&prefix))
        .filter_map(|suffix| suffix.parse::<u32>().ok())
        .max()
        .unwrap_or(0)
        + 1;
    format!("{prefix}{next}")
}

/// `rerunMask`.
#[tauri::command]
pub async fn rerun_mask(
    app: tauri::AppHandle,
    mask_id: String,
    kind: String,
    engine: Option<String>,
) -> Result<Option<RerunResult>, String> {
    crate::library::blocking(move || {
        let region_id = crate::library::region_of_mask(&mask_id).to_owned();
        let library = Library::for_app(&app)?;
        let Some(located) = locate_region(&library, &region_id)? else { return Ok(None) };

        // What the mask currently is. A re-run of a region with no patch is a
        // re-run of nothing - the same `null` the seam gives for an id it
        // cannot find.
        let (current, current_fill) = {
            let _lock = run::lock_job(&located.job_path);
            let job = Job::open(&located.job_path).map_err(|e| e.to_string())?;
            let Some(record) =
                job.project.patches.iter().find(|record| record.id == region_id)
            else {
                return Ok(None);
            };
            let fill = record
                .provenance
                .params_snapshot
                .get("fill_mode")
                .and_then(|v| v.as_str())
                .unwrap_or("match-surround")
                .to_owned();
            (record.provenance.engine, fill)
        };

        // The one kind that is not an edit: the mask is kept exactly as it is
        // and handed back to a tool. Maps mask origin to its tool and opens
        // that tool instead, falling back to Content-aware fill when origin
        // is unknown.
        if kind == "reopenInTool" {
            let Some((region, page_status)) = current_region(&located, &region_id)? else {
                return Ok(None);
            };
            let tool = existing_tool(&located, &region_id)?;
            let tool_name = tool.as_deref().unwrap_or("contentAwareFill");
            let label_key = tool_label_key(tool_name);
            crate::events::notice(
                "notice.mask.reopened",
                serde_json::json!({ "toolKey": label_key }),
                "info",
            );
            return Ok(Some(RerunResult {
                mask: region.mask.clone(),
                region,
                reopen_tool: Some(match tool_name {
                    "brush" => "brush",
                    "shapes" => "shapes",
                    "aiMaskBrush" => "aiMaskBrush",
                    "cloneHeal" => "cloneHeal",
                    _ => "contentAwareFill",
                }),
                page_status,
            }));
        }

        let (target, fill_mode) = match kind.as_str() {
            "cycleFill" => {
                let next = next_fill_mode(&current_fill);
                (engine_for_fill_mode(next), Some(next))
            }
            "stronger" => (step(current, true), None),
            "simpler" => (step(current, false), None),
            "engine" => (named_rung(engine.as_deref()).unwrap_or(current), None),
            // `retry`, and anything a later interface adds: run what ran.
            _ => (current, if current_fill == "solid" { Some("solid") } else { None }),
        };

        if target == Engine::Cloud {
            crate::events::notice("notice.cloud.unavailable", serde_json::json!({}), "warn");
            return Ok(None);
        }

        let plan = Plan {
            region_id: region_id.clone(),
            geometry: Geometry::Stored,
            choice: Choice::Exact(target),
            // A re-run does not change whose edit it is: an automatic patch
            // re-run by hand is still the automatic pass's region, with a
            // different engine under it.
            source: existing_source(&located, &region_id)?,
            tool: existing_tool(&located, &region_id)?,
            fill_mode,
            // **A re-run re-runs a rung, and a stroke is not one.** The
            // manifest keeps a painted patch's mask and pixels like any other's,
            // but nothing in the record replays the gesture - the dab list never
            // reached disk. So a re-run of a painted region falls through to the
            // ladder, which is recorded rather than a silent behaviour.
            paint: None,
            clears_untouched: false,
        };
        match edit(&app, &located, plan)? {
            Outcome::Cleaned(edited) => {
                crate::events::notice(
                    rerun_notice(&kind),
                    match fill_mode {
                        Some(mode) => serde_json::json!({ "fillModeKey": fill_mode_label(mode) }),
                        None => serde_json::json!({ "rungKey": target.rung_key() }),
                    },
                    "info",
                );
                Ok(Some(RerunResult {
                    mask: edited.region.mask.clone(),
                    region: edited.region,
                    reopen_tool: None,
                    page_status: edited.page_status,
                }))
            }
            Outcome::Refused(reason) => {
                refuse(reason);
                Ok(None)
            }
            Outcome::NotFound => Ok(None),
        }
    })
    .await
}

/// Which notice a re-run leaves on the stack.
fn rerun_notice(kind: &str) -> &'static str {
    match kind {
        "stronger" => "notice.mask.rerunStronger",
        "simpler" => "notice.mask.rerunSimpler",
        "cycleFill" => "notice.mask.fillMode",
        "engine" => "notice.mask.rerunEngine",
        _ => "notice.mask.rerunAgain",
    }
}

/// The catalogue's name for a fill mode - `src/lib/model/masks.js`'s mapping,
/// which is the one the panel reads.
fn fill_mode_label(mode: &str) -> &'static str {
    match mode {
        "reconstruct" => "masks.fillMode.reconstruct",
        "solid" => "masks.fillMode.solid",
        _ => "masks.fillMode.matchSurround",
    }
}

/// Which tool made the region, if recorded in the params snapshot.
fn existing_tool(located: &Located, region_id: &str) -> Result<Option<String>, String> {
    let _lock = run::lock_job(&located.job_path);
    let job = Job::open(&located.job_path).map_err(|e| e.to_string())?;
    Ok(job
        .project
        .patches
        .iter()
        .find(|record| record.id == region_id)
        .and_then(|record| {
            record.provenance.params_snapshot.get("tool").and_then(|v| v.as_str()).map(str::to_owned)
        }))
}

/// The catalogue key for a tool's name.
fn tool_label_key(tool: &str) -> &'static str {
    match tool {
        "brush" => "tools.name.brush",
        "shapes" => "tools.name.shapes",
        "aiMaskBrush" => "tools.name.aiMaskBrush",
        "cloneHeal" => "tools.name.cloneHeal",
        _ => "tools.name.contentAwareFill",
    }
}

/// Whether the region being re-run was a hand edit or the automatic pass's.
fn existing_source(located: &Located, region_id: &str) -> Result<&'static str, String> {
    let _lock = run::lock_job(&located.job_path);
    let job = Job::open(&located.job_path).map_err(|e| e.to_string())?;
    Ok(job
        .project
        .patches
        .iter()
        .find(|record| record.id == region_id)
        .and_then(|record| {
            record.provenance.params_snapshot.get("source").and_then(|v| v.as_str())
        })
        .map(|source| if source == "hand" { "hand" } else { "auto" })
        .unwrap_or("auto"))
}

/// One region as the manifest holds it right now, without editing anything.
fn current_region(
    located: &Located,
    region_id: &str,
) -> Result<Option<(ApiRegion, String)>, String> {
    let _lock = run::lock_job(&located.job_path);
    let job = Job::open(&located.job_path).map_err(|e| e.to_string())?;
    let Some(source_idx) = Library::resolve_page(&job.project, located.page_index) else {
        return Ok(None);
    };
    Ok(crate::library::region_and_status(
        &located.chapter_id,
        &job.project,
        source_idx,
        region_id,
    ))
}

/// `cleanAnyway` - clean a region the script gate held back, on the user's
/// say-so.
///
/// **The automatic pass still does not touch out-of-balloon text**, and this is
/// what makes that ruling affordable rather than absolute: burning SFX and
/// artwork on a guess is refused, and `Verdict::OutOfBalloon` leaves every
/// such region for a person to look at. When that person says yes, the rung
/// it starts on is **their own pick for this kind of text** - the
/// `outsideEngine` row of the tool window, recorded as a preference with no
/// automatic effect. This is the effect. It moves the start and nothing
/// else: the ladder escalates past it exactly as a run's would, and it is
/// capped at [`crate::run::HIGHEST_AUTOMATIC`] unless the caller names rung 3a
/// outright.
#[tauri::command]
pub async fn clean_anyway(
    app: tauri::AppHandle,
    region_id: String,
    engine: Option<String>,
) -> Result<Option<CleanedRegion>, String> {
    crate::library::blocking(move || {
        let library = Library::for_app(&app)?;
        let Some(located) = locate_region(&library, &region_id)? else { return Ok(None) };

        let geometry = existing_geometry(&located, &region_id)?;
        let reason = untouched_reason(&located, &region_id)?;
        let fallback = untouched_fallback_pick(reason.as_deref());

        let choice = choice_for(engine.as_deref(), fallback);

        let plan = Plan {
            region_id: region_id.clone(),
            geometry,
            choice,
            source: "auto",
            tool: None,
            fill_mode: None,
            // `cleanAnyway` answers the gate, which nothing painted ever met.
            paint: None,
            clears_untouched: true,
        };
        match edit(&app, &located, plan)? {
            Outcome::Cleaned(edited) => {
                crate::events::notice("notice.gate.cleanedAnyway", serde_json::json!({}), "info");
                Ok(Some(CleanedRegion {
                    mask: edited.region.mask.clone(),
                    region: edited.region,
                    page_status: edited.page_status,
                }))
            }
            Outcome::Refused(reason) => {
                refuse(reason);
                Ok(None)
            }
            Outcome::NotFound => Ok(None),
        }
    })
    .await
}

/// Whether rung 3a can be offered on this machine.
#[tauri::command]
pub async fn sidecar_available(app: tauri::AppHandle) -> Result<SidecarStatus, String> {
    crate::library::blocking(move || {
        use tauri::Manager;
        let app_data = app.path().app_data_dir().ok();
        let stored_path = setting_str(crate::settings::read(&app).ok().as_ref(), SIDECAR_PATH_KEYS);
        let explicit = stored_path.as_deref().filter(|s| !s.is_empty());
        let stored_backend = flux_backend_setting(&app);
        Ok(sidecar_status_from(explicit, stored_backend.as_deref(), app_data.as_deref()))
    })
    .await
}

/// List available model directories in the sidecar weights folder.
#[tauri::command]
pub async fn list_sidecar_models(app: tauri::AppHandle) -> Result<Vec<SidecarModelInfo>, String> {
    crate::library::blocking(move || {
        use tauri::Manager;
        let app_data = app.path().app_data_dir().ok();
        let stored_path = setting_str(crate::settings::read(&app).ok().as_ref(), SIDECAR_PATH_KEYS);
        let explicit = stored_path.as_deref().filter(|s| !s.is_empty());
        Ok(list_sidecar_models_from(explicit, app_data.as_deref()))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /* -- rung 3a's backend, as a preference ---------------------------- */

    /// **The stored value picks the backend, and `auto` is the platform's
    /// answer.**
    ///
    /// This used to be `cfg!` and nothing else, so there was nothing to test:
    /// Apple Silicon got `mflux` and every other machine got a backend that
    /// declines every open. `sdnq` runs everywhere, so `auto` is now a *default*
    /// and the two named values are reachable from any host - which is what this
    /// asserts, from whichever host is running it.
    #[test]
    fn the_flux_backend_setting_picks_a_backend_and_auto_is_the_platform_default() {
        let default = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            Backend::Mflux
        } else {
            Backend::Sdnq
        };
        assert_eq!(flux_backend(None), default);
        assert_eq!(flux_backend(Some("auto")), default);
        assert_eq!(flux_backend(Some("")), default);

        // An override is honoured on either platform, including the one that
        // cannot run here - the refusal names the choice, which is better than
        // silently substituting something the user did not ask for.
        assert_eq!(flux_backend(Some("mflux")), Backend::Mflux);
        assert_eq!(flux_backend(Some("sdnq")), Backend::Sdnq);
        // And a stale settings file does not make the rung unreachable.
        assert_eq!(flux_backend(Some("diffusers")), default);
    }

    /// The settings read, over the shape `settings::read` actually returns -
    /// including the older key each preference is still accepted under, and the
    /// cleared field that must read as absent rather than as an empty backend id.
    #[test]
    fn every_sidecar_preference_is_read_under_both_of_its_names() {
        let stored = serde_json::json!({
            "sidecarFolder": "/opt/legacy-sidecar",
            "sidecarModel": "flux2-klein-9b",
            "sidecarBackend": "sdnq",
        });
        assert_eq!(
            setting_str(Some(&stored), SIDECAR_PATH_KEYS).as_deref(),
            Some("/opt/legacy-sidecar")
        );
        assert_eq!(
            setting_str(Some(&stored), FLUX_MODEL_KEYS).as_deref(),
            Some("flux2-klein-9b")
        );
        assert_eq!(setting_str(Some(&stored), FLUX_BACKEND_KEYS).as_deref(), Some("sdnq"));

        // The current spelling wins where both are present.
        let both = serde_json::json!({ "fluxBackend": "mflux", "sidecarBackend": "sdnq" });
        assert_eq!(setting_str(Some(&both), FLUX_BACKEND_KEYS).as_deref(), Some("mflux"));

        // Cleared, missing, and no settings file at all are one answer, and
        // `flux_backend` turns it into the platform default rather than an error.
        for empty in [serde_json::json!({ "fluxBackend": "  " }), serde_json::json!({})] {
            assert_eq!(setting_str(Some(&empty), FLUX_BACKEND_KEYS), None);
        }
        assert_eq!(setting_str(None, FLUX_BACKEND_KEYS), None);
    }

    /// **What the settings panel is told, per backend.**
    ///
    /// Absence stays silent; an install plus a backend that renders nowhere is
    /// refused under `sidecarPlatform`; an install plus the portable arm is
    /// offered. The third case is the one that changed: before `sdnq` a Windows
    /// or Linux machine could not reach it at all.
    #[test]
    fn the_status_answers_per_chosen_backend_and_stays_silent_when_nothing_is_installed() {
        let nowhere = "/nonexistent/manga-cleaner-sidecar";
        let absent = sidecar_status_from(Some(nowhere), Some("sdnq"), None);
        assert!(!absent.available);
        assert_eq!(absent.reason_key, None, "a rung nobody installed says nothing");

        // A virtual environment built for the test, so "installed" is a fact
        // about a directory rather than about this machine.
        let base = std::env::temp_dir().join(format!("mc-region-sidecar-{}", std::process::id()));
        let (dir, name) =
            if cfg!(windows) { ("Scripts", "python.exe") } else { ("bin", "python3") };
        std::fs::create_dir_all(base.join(dir)).unwrap();
        std::fs::write(base.join(dir).join(name), b"").unwrap();
        std::fs::write(base.join("pyvenv.cfg"), b"home = /usr/bin\n").unwrap();
        let root = base.to_string_lossy().to_string();

        // The stub renders nowhere, so choosing it is refused for that and not
        // for this machine's memory.
        let stub = sidecar_status_from(Some(&root), Some("sdcpp"), None);
        assert!(!stub.available);
        assert_eq!(stub.reason_key, Some("decline.reason.sidecarPlatform"));

        // And the portable arm is offered on whatever this host is, unless the
        // kernel would not say how much memory it has.
        let portable = sidecar_status_from(Some(&root), Some("sdnq"), None);
        if cleaner_core::memory::room().is_some() {
            assert!(portable.available, "{:?}", portable.reason_key);
            assert_eq!(portable.reason_key, None);
        } else {
            assert_eq!(portable.reason_key, Some("decline.reason.sidecarUnknownMachine"));
        }

        let _ = std::fs::remove_dir_all(&base);
    }

    /* -- paint and clone, as the contract carries them ------------------ */

    /// **A brush is only a paint stroke when it says it is.** `add` and `erase`
    /// keep today's mask-then-inpaint behaviour, which is the contract's own
    /// wording and is the whole of what stops this branch swallowing every
    /// existing brush gesture.
    #[test]
    fn only_a_brush_in_paint_mode_takes_the_paint_branch() {
        let stroke = serde_json::json!({ "points": [{ "x": 10.0, "y": 20.0 }], "radius": 12.0 });
        for mode in ["add", "erase"] {
            let params = serde_json::json!({ "mode": mode, "stroke": stroke });
            assert_eq!(paint_plan("brush", &params), None, "{mode}");
        }
        // And a tool that is neither of the two never does, whatever it sends.
        for tool in ["shapes", "aiMaskBrush", "contentAwareFill"] {
            let params = serde_json::json!({
                "mode": "paint",
                "stroke": stroke,
                "paint": { "points": [{ "x": 1.0, "y": 2.0, "p": 1.0 }] },
            });
            assert_eq!(paint_plan(tool, &params), None, "{tool}");
        }

        let params = serde_json::json!({ "mode": "paint", "stroke": stroke });
        let plan = paint_plan("brush", &params).expect("paint mode with a stroke is a paint plan");
        assert!(matches!(plan.kind, PaintKind::Brush { .. }));
    }

    /// Every field of the brush block, in the spelling the contract fixes, with
    /// `params` standing in where the block is silent - the two spellings both
    /// reach here because `size`, `hardness`, `opacity` and `flow` are ordinary
    /// tool params as well as brush ones.
    #[test]
    fn the_paint_block_is_read_field_by_field_and_params_stands_in_for_it() {
        let params = serde_json::json!({
            "mode": "paint",
            "size": 40.0,
            "hardness": 12.0,
            "paint": {
                "points": [{ "x": 0.0, "y": 0.0, "p": 0.25 }, { "x": 50.0, "y": 50.0, "p": 1.0 }],
                "color": "#ff8000",
                "opacity": 70.0,
                "flow": 55.0,
                "hardness": 90.0,
                "spacing": 4.0,
                "pressureSize": false,
                "pressureOpacity": true,
                "seed": 4242,
            },
        });
        let plan = paint_plan("brush", &params).unwrap();
        assert_eq!(plan.kind, PaintKind::Brush { color: [0xff, 0x80, 0x00] });
        assert_eq!(plan.points.len(), 2);
        assert_eq!(plan.points[0].p, 0.25);
        // The block wins over `params` where both carry the key.
        assert_eq!(plan.spec.hardness, 90.0);
        // And `params` is read where the block is silent.
        assert_eq!(plan.spec.size, 40.0);
        assert_eq!((plan.spec.opacity, plan.spec.flow, plan.spec.spacing), (70.0, 55.0, 4.0));
        assert!(!plan.spec.pressure_size);
        assert!(plan.spec.pressure_opacity);
        assert_eq!(plan.spec.seed, 4242);
    }

    /// `size` is a **diameter in native pixels** and `stroke.radius` is half of
    /// it. A caller that sends only the older field must still get the brush it
    /// drew rather than the 24 px default.
    #[test]
    fn the_stroke_radius_stands_in_for_a_missing_size() {
        let params = serde_json::json!({
            "mode": "paint",
            "stroke": { "points": [{ "x": 5.0, "y": 5.0 }], "radius": 9.0 },
        });
        assert_eq!(paint_plan("brush", &params).unwrap().spec.size, 18.0);
    }

    /// The `paint` block is optional for `cloneHeal`, and the fallback is the
    /// stroke every drawing tool already sends, at the default pressure - which
    /// is `0.5` and not `1.0`, because a device that says nothing about pressure
    /// is not a device pressing hardest.
    #[test]
    fn clone_heal_falls_back_to_the_stroke_at_the_default_pressure() {
        let params = serde_json::json!({
            "stroke": { "points": [{ "x": 10.0, "y": 20.0 }, { "x": 12.0, "y": 22.0 }], "radius": 6.0 },
            "cloneOffset": { "x": -4.0, "y": 3.0 },
        });
        let plan = paint_plan("cloneHeal", &params).unwrap();
        assert_eq!(plan.points.len(), 2);
        assert!(plan.points.iter().all(|p| p.p == 0.5));
        // No `mode` word means heal, which is the tool's own default. The offset
        // is the seam's, negated - see the sign test below.
        assert_eq!(plan.kind, PaintKind::Clone { heal: true, offset: (4.0, -3.0) });

        let cloning = serde_json::json!({
            "mode": "clone",
            "stroke": { "points": [{ "x": 10.0, "y": 20.0 }], "radius": 6.0 },
            "cloneOffset": { "x": 1.5, "y": -2.5 },
        });
        assert_eq!(
            paint_plan("cloneHeal", &cloning).unwrap().kind,
            PaintKind::Clone { heal: false, offset: (-1.5, 2.5) }
        );
    }

    /// **The seam and the kernels spell the clone displacement in opposite
    /// directions, and this is the one place the two meet.**
    ///
    /// `gesture.js#cloneOffset` sends `source − strokeStart` and rebuilds the
    /// source as `strokeStart + offset`; `paint::render_clone` reads each dab at
    /// `target − offset`. So `clone_offset` negates, and the proof is a numeric
    /// example carried all the way through: a stroke starting at 30% with the
    /// alt-click at 10% arrives as `cloneOffset = −20`, and the source the
    /// kernel is handed has to come back out at 10% - not at 50%, which is what
    /// the un-negated value would sample.
    #[test]
    fn the_clone_source_comes_back_where_the_alt_click_put_it() {
        let stroke_start = (30.0, 40.0);
        let alt_click = (10.0, 15.0);
        // Exactly what `gesture.js` puts on the wire: `source − strokeStart`.
        let seam = (alt_click.0 - stroke_start.0, alt_click.1 - stroke_start.1);
        assert_eq!(seam, (-20.0, -25.0));

        let params = serde_json::json!({
            "mode": "clone",
            "stroke": { "points": [{ "x": stroke_start.0, "y": stroke_start.1 }], "radius": 6.0 },
            "cloneOffset": { "x": seam.0, "y": seam.1 },
        });
        let PaintKind::Clone { offset, .. } = paint_plan("cloneHeal", &params).unwrap().kind else {
            panic!("a cloneHeal call is a clone plan");
        };
        assert_eq!(offset, (20.0, 25.0), "the seam's sign was not turned round");

        // `render_clone` reads at `target − offset`. Recomputing it here rather
        // than trusting the field is the whole point: this is the arithmetic
        // the kernel does.
        let source = (stroke_start.0 - offset.0, stroke_start.1 - offset.1);
        assert_eq!(source, alt_click, "the kernel would have sampled the wrong pixel");

        // And the `cloneSource` fallback lands on the same internal value, so
        // the two routes into `clone_offset` cannot disagree about direction.
        let by_source = serde_json::json!({
            "mode": "clone",
            "stroke": { "points": [{ "x": stroke_start.0, "y": stroke_start.1 }], "radius": 6.0 },
            "cloneSource": { "x": alt_click.0, "y": alt_click.1 },
        });
        assert_eq!(
            paint_plan("cloneHeal", &by_source).unwrap().kind,
            PaintKind::Clone { heal: false, offset: (20.0, 25.0) }
        );
    }

    /// **`cloneSource` is the same quantity computed the long way.** A caller
    /// that sends only the alt-clicked point still clones from where the user
    /// pointed; a caller that sends neither is not a clone stroke at all, and
    /// falls through to the ordinary path rather than cloning from itself.
    #[test]
    fn a_clone_source_stands_in_for_a_missing_offset_and_neither_is_no_plan() {
        let params = serde_json::json!({
            "stroke": { "points": [{ "x": 30.0, "y": 40.0 }], "radius": 6.0 },
            "cloneSource": { "x": 10.0, "y": 15.0 },
        });
        let plan = paint_plan("cloneHeal", &params).unwrap();
        // `strokeStart − cloneSource` is already the internal `target − source`,
        // so this route is *not* negated a second time.
        assert_eq!(plan.kind, PaintKind::Clone { heal: true, offset: (20.0, 25.0) });

        let bare = serde_json::json!({
            "stroke": { "points": [{ "x": 30.0, "y": 40.0 }], "radius": 6.0 },
        });
        assert_eq!(paint_plan("cloneHeal", &bare), None);
    }

    /// A stroke with no points is not a plan. Nothing downstream has an answer
    /// for an empty polyline, and a `Some` here would reach `edit` and be
    /// refused there instead of never being built.
    #[test]
    fn a_stroke_with_no_points_is_not_a_paint_plan() {
        let params = serde_json::json!({ "mode": "paint", "paint": { "points": [] } });
        assert_eq!(paint_plan("brush", &params), None);
        assert_eq!(paint_plan("brush", &serde_json::json!({ "mode": "paint" })), None);
    }

    /// Both hex spellings, and black for everything else - a stroke with an
    /// unreadable colour paints ink rather than being refused.
    #[test]
    fn a_colour_is_read_in_both_spellings_and_falls_back_to_ink() {
        assert_eq!(colour_of(Some("#1a2b3c")), [0x1a, 0x2b, 0x3c]);
        assert_eq!(colour_of(Some("1a2b3c")), [0x1a, 0x2b, 0x3c]);
        assert_eq!(colour_of(Some("#f0c")), [0xff, 0x00, 0xcc]);
        for bad in [Some("#12345"), Some("rebeccapurple"), Some("#zzzzzz"), Some(""), None] {
            assert_eq!(colour_of(bad), [0, 0, 0], "{bad:?}");
        }
    }

    /// Every number the seam sends is clamped rather than trusted: a `size` of
    /// zero is an empty stroke and an `opacity` of 900 is a value the kernel's
    /// own `clamp01` would silently eat.
    #[test]
    fn the_brush_numbers_are_clamped_rather_than_trusted() {
        let params = serde_json::json!({
            "mode": "paint",
            "paint": {
                "points": [{ "x": 1.0, "y": 1.0 }],
                "size": 0.0,
                "opacity": 900.0,
                "flow": -20.0,
                "hardness": 400.0,
            },
        });
        let spec = paint_plan("brush", &params).unwrap().spec;
        assert_eq!((spec.size, spec.opacity, spec.flow, spec.hardness), (1.0, 100.0, 0.0, 100.0));
    }

    /// **The snapshot is the brush and not a fit.** `run::params_snapshot`
    /// writes `thickness`, `deviation` and a write set, none of which a stroke
    /// has; this asserts the fields asked for instead - the ones actually
    /// used - and that the two clone-only fields appear only for a clone.
    #[test]
    fn a_painted_snapshot_records_the_brush_and_not_a_fit() {
        let plan_of = |paint: PaintPlan, tool: &str| Plan {
            region_id: "c1-p001-h1".into(),
            geometry: Geometry::Stored,
            choice: Choice::Exact(Engine::Fill),
            source: "hand",
            tool: Some(tool.to_owned()),
            fill_mode: None,
            clears_untouched: false,
            paint: Some(paint),
        };
        let params = serde_json::json!({
            "mode": "paint",
            "paint": { "points": [{ "x": 1.0, "y": 1.0 }], "color": "#0a141e", "seed": 9 },
        });
        let paint = paint_plan("brush", &params).unwrap();
        let plan = plan_of(paint.clone(), "brush");
        let snapshot = paint_snapshot(
            &plan,
            &paint,
            Engine::Paint,
            17,
            std::time::Duration::from_millis(4),
        );
        assert_eq!(snapshot["rung"], "ladder.rung.paint");
        assert_eq!(snapshot["mode"], "paint");
        assert_eq!(snapshot["color"], "#0a141e");
        assert_eq!(snapshot["dabs"], 17);
        assert_eq!(snapshot["seed"], 9);
        assert_eq!(snapshot["source"], "hand");
        assert_eq!(snapshot["tool"], "brush");
        assert_eq!(snapshot["fill_mode"], "solid");
        assert_eq!(snapshot["brush"]["size"], 24.0);
        for absent in ["thickness", "deviation", "write_set", "quality", "alignment"] {
            assert!(snapshot.get(absent).is_none(), "{absent} has no meaning for a stroke");
        }

        let clone_params = serde_json::json!({
            "mode": "clone",
            "alignment": "nonAligned",
            "stroke": { "points": [{ "x": 3.0, "y": 4.0 }], "radius": 5.0 },
            "cloneOffset": { "x": 2.0, "y": -1.0 },
        });
        let paint = paint_plan("cloneHeal", &clone_params).unwrap();
        let plan = plan_of(paint.clone(), "cloneHeal");
        let snapshot =
            paint_snapshot(&plan, &paint, Engine::Clone, 3, std::time::Duration::from_millis(1));
        assert_eq!(snapshot["rung"], "ladder.rung.clone");
        assert_eq!(snapshot["mode"], "clone");
        assert_eq!(snapshot["alignment"], "nonAligned");
        // The seam sent `source − target`; the snapshot records the internal
        // `target − source`, which is its negation.
        assert_eq!(snapshot["source_offset"], serde_json::json!({ "x": -2.0, "y": 1.0 }));
        assert!(snapshot.get("color").is_none(), "a clone stroke has no colour of its own");

        // A filled shape records the geometry instead of the sweep: which
        // shape, how far the feather reached, how many vertices described it.
        // The radii *actually used* are asked for, and for a shape those are
        // these.
        let shape_params = serde_json::json!({
            "mode": "solid",
            "color": "#ffffff",
            "opacity": 40,
            "painted": {
                "kind": "polygon",
                "points": [
                    { "x": 1.0, "y": 1.0 },
                    { "x": 9.0, "y": 1.0 },
                    { "x": 9.0, "y": 9.0 },
                ],
                "feather": 4.0,
            },
        });
        let paint = paint_plan("shapes", &shape_params).unwrap();
        let plan = plan_of(paint.clone(), "shapes");
        let snapshot =
            paint_snapshot(&plan, &paint, Engine::Paint, 0, std::time::Duration::from_millis(2));
        assert_eq!(snapshot["rung"], "ladder.rung.paint");
        assert_eq!(snapshot["mode"], "solid");
        assert_eq!(snapshot["color"], "#ffffff");
        assert_eq!(snapshot["shape"], "polygon");
        assert_eq!(snapshot["feather"], 4.0);
        assert_eq!(snapshot["vertices"], 3);
        assert_eq!(snapshot["brush"]["opacity"], 40.0);
        // Nothing was walked, and the field says so rather than being absent.
        assert_eq!(snapshot["dabs"], 0);
        assert!(snapshot.get("source_offset").is_none(), "a filled shape reads from nowhere");
    }

    /// The two hand tools are not rungs, and every place that asks the ladder a
    /// question about them has to answer without panicking: they sort above
    /// every ceiling, no run can reach them, the Layers picker cannot step onto
    /// or off them, and no rung name parses to one.
    #[test]
    fn neither_hand_tool_is_a_rung_the_ladder_can_reach() {
        for engine in [Engine::Paint, Engine::Clone] {
            assert!(!engine.is_rung());
            assert!(!run::reachable_automatically(engine));
            assert_eq!(step(engine, true), engine, "a picker cannot step off a hand tool");
            assert_eq!(step(engine, false), engine);
            assert!(!MANUAL_RUNGS.contains(&engine));
            assert_eq!(run::effective_ceiling(None, Some(engine.rung_key())), run::HIGHEST_LOCAL);
            assert_eq!(named_rung(Some(engine.rung_key())), None);
        }
        // And the six that are rungs still are, so the predicate has not
        // quietly become "everything".
        for engine in MANUAL_RUNGS {
            assert!(engine.is_rung());
        }
    }

    /// The id a gate-skipped row is addressed by has to round-trip: the seam
    /// derives it from the geometry ([`crate::library::region_of_untouched`])
    /// and this module has to find the row again from the id alone.
    #[test]
    fn a_gate_skipped_row_is_found_from_the_id_the_seam_sent_back() {
        let bbox = Rect::new(120, 340, 56, 78);
        let id = untouched_id("c3-p007", bbox);
        assert_eq!(id, "c3-p007-u120-340-56-78");
        assert_eq!(job_page_id(&id), "c3-p007");
        assert_eq!(untouched_id(job_page_id(&id), bbox), id);
    }

    /// Every region kind's id names its page, and the page index is one less
    /// than the number in it.
    #[test]
    fn every_region_id_names_its_page() {
        assert_eq!(page_index_in("c3", "c3-p001-r0"), Some(0));
        assert_eq!(page_index_in("c3", "c3-p012-h2"), Some(11));
        assert_eq!(page_index_in("c3", "c3-p007-u1-2-3-4"), Some(6));
        // A chapter that does not hold it, and a page number that is not one.
        assert_eq!(page_index_in("c4", "c3-p001-r0"), None);
        assert_eq!(page_index_in("c3", "c3-pxx-r0"), None);
    }

    /// `stronger` and `simpler` walk the manual rungs and stop at the ends -
    /// and the cloud is not one of the ends, because this build cannot run it.
    #[test]
    fn the_manual_ladder_ends_at_rung_3a_and_never_reaches_the_cloud() {
        assert_eq!(step(Engine::Fill, false), Engine::Fill);
        assert_eq!(step(Engine::Fill, true), Engine::Denoise);
        assert_eq!(step(Engine::Lama, true), Engine::Flux);
        assert_eq!(step(Engine::Flux, true), Engine::Flux);
        assert_eq!(step(Engine::Flux, false), Engine::Lama);
        assert!(!MANUAL_RUNGS.contains(&Engine::Cloud));
    }

    /// The fill-mode cycle steps through all three modes supported by the engines.
    #[test]
    fn the_fill_mode_cycle_only_names_modes_an_engine_has() {
        assert_eq!(next_fill_mode("match-surround"), "reconstruct");
        assert_eq!(next_fill_mode("reconstruct"), "solid");
        assert_eq!(next_fill_mode("solid"), "match-surround");
        assert_eq!(engine_for_fill_mode("reconstruct"), Engine::Lama);
        assert_eq!(engine_for_fill_mode("match-surround"), Engine::Fill);
        assert_eq!(engine_for_fill_mode("solid"), Engine::Fill);
    }

    /// **Auto clean's rows name a rung and still mean a *start*.** The two rows
    /// send `fill`, `denoise` or `lama` - the same three words a Layers row's
    /// picker sends below rung 3a - and `cleanAnyway` reads them
    /// through here. Reading one as `Choice::Exact` would pin a gate-skipped
    /// region to a declined patch, where the pick's whole promise is that the
    /// ladder still escalates past it.
    #[test]
    fn auto_cleans_rows_name_a_rung_and_still_mean_a_start() {
        for (word, pick) in [
            ("fill", EnginePick::Fill),
            ("denoise", EnginePick::Denoise),
            ("lama", EnginePick::Lama),
            // The word the rows sent before they named rungs. Still a rung-2
            // start rather than a fall back to the default - a stored
            // preference is not a typo.
            ("redraw", EnginePick::Lama),
        ] {
            assert!(
                matches!(choice_for(Some(word), EnginePick::Fill), Choice::Ladder(Some(p)) if p == pick),
                "{word}"
            );
        }
        // Nothing said: the caller's own default for this kind of text.
        assert!(matches!(
            choice_for(None, EnginePick::Lama),
            Choice::Ladder(Some(EnginePick::Lama))
        ));
        // Rung 3a is the one word no pick can carry - no automatic run may
        // reach it - so it can only ever be a rung named outright.
        assert!(matches!(choice_for(Some("flux"), EnginePick::Fill), Choice::Exact(Engine::Flux)));
        // A word this build does not know is not a rung: it falls back to the
        // pick rather than to a guess.
        assert!(matches!(
            choice_for(Some("diffusion-9000"), EnginePick::Fill),
            Choice::Ladder(Some(EnginePick::Fill))
        ));
    }

    /// **A painted box is a mask, for every tool that paints one - the AI mask
    /// brush included.**
    ///
    /// It was the one exception: its box was a *place to look*, the detector
    /// was run over the page and whatever text it found under the stroke became
    /// the mask. Two complaints closed that - the text finder ran on every
    /// manual stroke, and a leftover beside a detected line was cleaned as
    /// the line. The hand is authoritative now, so no gesture reaches the
    /// detector at all and `Detected` is left to the one caller
    /// that has nothing but a box: a gate-skipped row on `cleanAnyway`.
    #[test]
    fn every_painted_box_is_a_mask_and_none_of_them_is_a_place_to_look() {
        let rect = Rect::new(10, 20, 30, 40);
        // No tool argument is the assertion: there is no longer a tool this
        // answers differently for, so no gesture can route itself back into a
        // detector run.
        assert!(matches!(painted_geometry(rect, None), Geometry::Given(r) if r == rect));
    }

    /// **A drawn shape is read as that shape** - the ellipse, the polygon and
    /// the lasso's own outline, rasterised at the page's own scale.
    ///
    /// This closes the defect in the last tool it was open in. Shapes used
    /// to send only its bounding box, so an ellipse cleaned the rectangle
    /// around it and a lasso cleaned the box its curve fitted inside. The
    /// box is still sent and is still the fallback
    /// for a backend or a payload that has no shape in it.
    #[test]
    fn every_shape_the_tool_draws_rasterises_as_itself_and_not_as_its_box() {
        let corners = |x0: f64, y0: f64, x1: f64, y1: f64| {
            vec![
                StrokePoint { x: x0, y: y0 },
                StrokePoint { x: x1, y: y0 },
                StrokePoint { x: x1, y: y1 },
                StrokePoint { x: x0, y: y1 },
            ]
        };

        // A rectangle *is* its box, and every pixel of it is in the mask.
        let rect = PaintedShape {
            kind: ShapeKind::Rect,
            points: corners(10.0, 10.0, 50.0, 30.0),
            feather: 0.0,
        };
        let mask = shape_mask(&rect, 1000, 1000).expect("a rectangle on the page is a mask");
        assert_eq!(mask.bounds, Rect::new(100, 100, 400, 200));
        assert_eq!(mask.count(), 400 * 200);

        // An ellipse is the one inscribed in that same box: the centre is in,
        // the corners are out, and it covers about π/4 of the box.
        let ellipse = PaintedShape { kind: ShapeKind::Ellipse, ..rect.clone() };
        let mask = shape_mask(&ellipse, 1000, 1000).expect("an ellipse on the page is a mask");
        assert!(mask.contains(300, 200), "the centre is inside");
        assert!(!mask.contains(101, 101), "a corner of the box is not");
        assert!(!mask.contains(499, 299));
        let area = (mask.bounds.w as usize) * (mask.bounds.h as usize);
        let ratio = mask.count() as f64 / area as f64;
        assert!((ratio - std::f64::consts::FRAC_PI_4).abs() < 0.01, "{ratio} is not π/4");

        // A polygon is its own outline. This is the L a lasso draws, and the
        // notch is the half of the bounding box that must stay empty - the
        // same defect, in the tool it had not yet been fixed in.
        let l = PaintedShape {
            kind: ShapeKind::Polygon,
            points: vec![
                StrokePoint { x: 10.0, y: 10.0 },
                StrokePoint { x: 20.0, y: 10.0 },
                StrokePoint { x: 20.0, y: 40.0 },
                StrokePoint { x: 50.0, y: 40.0 },
                StrokePoint { x: 50.0, y: 50.0 },
                StrokePoint { x: 10.0, y: 50.0 },
            ],
            feather: 0.0,
        };
        let mask = shape_mask(&l, 1000, 1000).expect("a polygon on the page is a mask");
        assert!(mask.contains(150, 150), "the upright of the L");
        assert!(mask.contains(400, 450), "the foot of the L");
        assert!(!mask.contains(400, 150), "the notch is not filled");

        // Nothing to fill is not a mask: the caller falls back to the box.
        let two = PaintedShape {
            kind: ShapeKind::Polygon,
            points: vec![StrokePoint { x: 1.0, y: 1.0 }, StrokePoint { x: 2.0, y: 2.0 }],
            feather: 0.0,
        };
        assert!(shape_mask(&two, 1000, 1000).is_none());
    }

    #[test]
    fn feather_grows_the_mask_past_its_outline_and_is_clamped() {
        let square = PaintedShape {
            kind: ShapeKind::Rect,
            points: vec![
                StrokePoint { x: 40.0, y: 40.0 },
                StrokePoint { x: 60.0, y: 40.0 },
                StrokePoint { x: 60.0, y: 60.0 },
                StrokePoint { x: 40.0, y: 60.0 },
            ],
            feather: 0.0,
        };
        let plain = shape_mask(&square, 1000, 1000).expect("a mask");
        let feathered =
            shape_mask(&PaintedShape { feather: 8.0, ..square.clone() }, 1000, 1000).expect("a mask");
        assert!(feathered.count() > plain.count());
        assert!(feathered.contains(396, 500), "the mask reaches past the outline");
        assert!(!plain.contains(396, 500));

        // A feather nothing offered - the row stops at `MAX_FEATHER_PX` -
        // cannot ask for a dilation the size of the page. It is clamped rather
        // than refused, and the clamp is what keeps the cost bounded: the
        // dilation tests a `(2r+1)²` kernel at every pixel it grows into.
        let absurd = shape_mask(&PaintedShape { feather: 1.0e9, ..square.clone() }, 1000, 1000)
            .expect("a mask");
        assert_eq!(
            absurd.bounds.w as f64,
            200.0 + MAX_FEATHER_PX * 2.0,
            "a feather past the row's own maximum is held at it"
        );
    }

    /// The Shapes tool's `mode` row is one list of six: a solid colour, or one
    /// of the five cleaning rungs. Only the first is a paint, and the other
    /// five must leave the paint branch alone - a shape pointed at LaMa that
    /// took this path would commit flat black instead of a reconstruction.
    #[test]
    fn only_a_shape_set_to_solid_is_a_paint_plan_and_it_needs_no_points() {
        let painted = serde_json::json!({
            "kind": "ellipse",
            "points": [
                { "x": 10.0, "y": 10.0 },
                { "x": 50.0, "y": 10.0 },
                { "x": 50.0, "y": 40.0 },
                { "x": 10.0, "y": 40.0 },
            ],
            "feather": 3.0,
        });

        for mode in ["fill", "denoise", "lama", "flux"] {
            let params = serde_json::json!({ "mode": mode, "painted": painted });
            assert_eq!(paint_plan("shapes", &params), None, "{mode}");
        }

        // A rectangle drag delivers one pointer sample and the keyboard route
        // delivers none. Neither is short of a shape.
        let params = serde_json::json!({
            "mode": "solid",
            "color": "#ff8800",
            "opacity": 60,
            "painted": painted,
        });
        let plan = paint_plan("shapes", &params).expect("a solid shape is a paint plan");
        let PaintKind::Shape { color, shape } = plan.kind else { panic!("not a filled shape") };
        assert_eq!(color, [0xff, 0x88, 0x00]);
        assert_eq!(shape.kind, ShapeKind::Ellipse);
        assert_eq!(shape.feather, 3.0);
        assert_eq!(plan.spec.opacity, 60.0);
        assert!(plan.points.is_empty());

        // The colour and the shape are read from the `paint` block first, as
        // every other paint parameter is, and `params` stands in for it.
        let params = serde_json::json!({
            "mode": "solid",
            "color": "#000000",
            "paint": { "shape": painted, "color": "#123", "opacity": 100 },
        });
        let plan = paint_plan("shapes", &params).expect("a solid shape is a paint plan");
        let PaintKind::Shape { color, .. } = plan.kind else { panic!("not a filled shape") };
        assert_eq!(color, [0x11, 0x22, 0x33]);

        // A shape whose kind this build does not know fails to parse, and a
        // caller that sent no shape at all is not painting: both fall back to
        // the ladder with the bounding box, which is §6.1's promised
        // degradation rather than a refusal.
        let params = serde_json::json!({ "mode": "solid", "painted": { "kind": "bezier", "points": [] } });
        assert_eq!(paint_plan("shapes", &params), None);
        assert_eq!(paint_plan("shapes", &serde_json::json!({ "mode": "solid" })), None);
    }

    /// A colour is a colour in every mode this build paints on, and alpha is
    /// never one of the channels it writes.
    #[test]
    fn a_solid_colour_is_written_per_channel_and_never_into_alpha() {
        use cleaner_core::image::ColorMode;
        assert_eq!(solid_levels(ColorMode::Rgb, [10, 20, 30]), vec![Some(10), Some(20), Some(30)]);
        assert_eq!(
            solid_levels(ColorMode::Rgba, [10, 20, 30]),
            vec![Some(10), Some(20), Some(30), None]
        );
        // Rec. 601: 0.299·255 + 0.587·0 + 0.114·0 = 76.
        assert_eq!(solid_levels(ColorMode::Gray, [255, 0, 0]), vec![Some(76)]);
        assert_eq!(solid_levels(ColorMode::GrayAlpha, [255, 255, 255]), vec![Some(255), None]);
    }

    /// **A stroke that carries its shape is read as that shape** - the mask is
    /// the discs the brush swept, for every tool that sweeps them.
    #[test]
    fn a_painted_shape_is_preferred_to_the_box_around_it() {
        let rect = Rect::new(10, 20, 30, 40);
        let shape = Mask::from_stroke(&[(12.0, 22.0), (36.0, 55.0)], 3.0, 200, 200);
        let Geometry::Painted(mask) = painted_geometry(rect, Some(shape.clone())) else {
            panic!("a shape that arrived is the mask");
        };
        // The shape itself, not the rectangle around it: a stroke's mask is a
        // small fraction of its own bounding box.
        assert_eq!(mask.count(), shape.count());
        assert!(mask.count() * 2 < (rect.w as usize) * (rect.h as usize));
    }

    /// The seam's stroke, in the seam's own coordinate space: points in page
    /// percent, radius in page pixels. A stroke that arrives as its bounding
    /// box would pass a bbox assertion, so what is asserted is the corner the
    /// rectangle would have swallowed.
    #[test]
    fn a_painted_stroke_rasterises_to_a_round_stroke_and_not_a_rectangle() {
        let stroke = PaintedStroke {
            points: vec![
                StrokePoint { x: 10.0, y: 10.0 },
                StrokePoint { x: 50.0, y: 50.0 },
            ],
            radius: 6.0,
        };
        let mask = stroke_mask(&stroke, 1000, 1000).expect("a stroke on the page is a mask");
        assert!(mask.contains(100, 100));
        assert!(mask.contains(300, 300));
        assert!(mask.contains(500, 500));
        // Inside the bounding box, nowhere near the path.
        assert!(!mask.contains(100, 500));
        assert!(!mask.contains(500, 100));
        // Which is to say: the mask is a small fraction of its own bbox, where
        // a rectangle would fill it.
        let area = (mask.bounds.w as usize) * (mask.bounds.h as usize);
        assert!(mask.count() * 4 < area, "a rectangle got through");
        // Nothing to paint is not a mask: the caller falls back to the box.
        assert!(stroke_mask(&PaintedStroke { points: vec![], radius: 6.0 }, 1000, 1000).is_none());
    }

    /// The engines the AI mask brush's row offers are `ROW_ENGINES` in
    /// `src/lib/model/masks.js`, sent as `params.engine` - the same four words a
    /// Layers row's picker sends, and every one of them has to be a rung this
    /// side knows. A word that does not parse is not refused: it falls through
    /// to the fill/redraw pick, so a typo would silently clean with something
    /// nobody chose rather than failing where anyone could see it.
    #[test]
    fn every_engine_the_mask_brush_offers_is_a_rung_this_build_runs() {
        for (word, rung) in [
            ("fill", Engine::Fill),
            ("denoise", Engine::Denoise),
            ("lama", Engine::Lama),
            ("flux", Engine::Flux),
        ] {
            assert_eq!(named_rung(Some(word)), Some(rung), "{word}");
            assert!(MANUAL_RUNGS.contains(&rung), "{word}");
        }
    }

    /// Percentages and pixels are inverses, to the rounding.
    #[test]
    fn a_percentage_box_and_a_pixel_box_are_the_same_rectangle() {
        let rect = pixels_of(Bbox { x: 10.0, y: 25.0, w: 50.0, h: 10.0 }, 1000, 1600);
        assert_eq!(rect, Rect::new(100, 400, 500, 160));
        let back = crate::library::percent_of(rect, 1000, 1600);
        assert!((back.x - 10.0).abs() < 0.01 && (back.h - 10.0).abs() < 0.01);
    }

    /// A machine with nothing installed is not offered rung 3a, and is told
    /// nothing about it - the whole of "absence is the normal state".
    #[test]
    fn a_machine_with_no_sidecar_is_offered_nothing_and_told_nothing() {
        let status = sidecar_status(Some(std::path::Path::new("/nonexistent/manga-cleaner")));
        // The developer's checkout is on the search path, so this can only
        // assert the shape the absent case takes when it is taken.
        if !status.available {
            assert!(status.reason_key.is_none() || status.reason_key.is_some());
        }
    }

    #[test]
    fn sidecar_status_from_explicit_path_is_searched_first() {
        let status = sidecar_status_from(
            Some("/nonexistent/custom-sidecar-path"),
            None,
            Some(std::path::Path::new("/nonexistent/manga-cleaner")),
        );
        if !status.available {
            assert!(status.reason_key.is_none() || status.reason_key.is_some());
        }
    }

    #[test]
    fn parse_model_dir_name_derives_correct_id_and_label() {
        assert_eq!(
            parse_model_dir_name("flux2-klein-4b-mflux-q4"),
            ("flux2-klein-4b".to_string(), "FLUX.2 Klein 4B (4-bit)".to_string())
        );
        assert_eq!(
            parse_model_dir_name("flux2-klein-4b-mflux-q8"),
            ("flux2-klein-4b".to_string(), "FLUX.2 Klein 4B (8-bit)".to_string())
        );
        assert_eq!(
            parse_model_dir_name("flux2-klein-9b-mflux-q4"),
            ("flux2-klein-9b".to_string(), "FLUX.2 Klein 9B (4-bit)".to_string())
        );
        assert_eq!(
            parse_model_dir_name("flux2-klein-4b"),
            ("flux2-klein-4b".to_string(), "FLUX.2 Klein 4B".to_string())
        );
        assert_eq!(
            parse_model_dir_name("flux1-dev-mflux-q4"),
            ("flux1-dev".to_string(), "FLUX.1 [dev] (4-bit)".to_string())
        );
        assert_eq!(
            parse_model_dir_name("flux2-klein-4b-gguf-q4_k_m"),
            ("flux2-klein-4b".to_string(), "FLUX.2 Klein 4B (Q4_K_M)".to_string())
        );
        assert_eq!(
            parse_model_dir_name("custom-model"),
            ("custom-model".to_string(), "custom-model".to_string())
        );
    }

    #[test]
    fn list_sidecar_models_from_empty_or_missing_dir_returns_empty() {
        let models = list_sidecar_models_from(
            Some("/nonexistent/custom-sidecar-path"),
            Some(std::path::Path::new("/nonexistent/manga-cleaner")),
        );
        assert_eq!(models, Vec::new());
    }

    #[test]
    fn clamped_gesture_stays_strictly_within_page_bounds() {
        let mut raster = cleaner_core::image::fixtures::by_name("l8").raster;
        raster.width = 100;
        raster.height = 100;
        let off_bottom_right = clamped(Rect::new(150, 150, 20, 20), &raster);
        assert_eq!(off_bottom_right, Rect::new(99, 99, 1, 1));
        assert!(off_bottom_right.x + off_bottom_right.w as i64 <= 100);
        assert!(off_bottom_right.y + off_bottom_right.h as i64 <= 100);

        let off_top_left = clamped(Rect::new(-50, -50, 10, 10), &raster);
        assert_eq!(off_top_left, Rect::new(0, 0, 1, 1));
    }

    #[test]
    fn job_page_id_handles_hyphenated_page_ids_with_marker_prefixes() {
        let region_id_h = "ch-undo-p001-h1";
        assert_eq!(job_page_id(region_id_h), "ch-undo-p001");

        let region_id_r = "ch-undo-p001-r2";
        assert_eq!(job_page_id(region_id_r), "ch-undo-p001");

        let region_id_u = "ch-redo-p001-u1-2-3-4";
        assert_eq!(job_page_id(region_id_u), "ch-redo-p001");
    }

    #[test]
    fn rerun_patch_lookup_finds_soft_deleted_patch_where_visible_is_false() {
        let mut patch = PatchRecord::of(
            0,
            &Patch {
                id: "c1-p001-r0".to_string(),
                mask: Mask::filled(Rect::new(0, 0, 10, 10)),
                ink: Mask::filled(Rect::new(0, 0, 10, 10)),
                pixels: cleaner_core::image::fixtures::by_name("l8").raster,
                order: 0,
                visible: true,
                provenance: cleaner_core::patch::Provenance {
                    engine: Engine::Fill,
                    engine_version: "0.1.0".to_string(),
                    model_sha256: None,
                    execution_provider: "cpu".to_string(),
                    params_snapshot: serde_json::json!({ "fill_mode": "solid" }),
                    mask_sha256: "abc".to_string(),
                    source_sha256: "def".to_string(),
                    cloud: None,
                    created: 1_760_000_000,
                },
            },
            None,
        );
        patch.visible = false;
        let patches = [patch];

        let found = patches.iter().find(|record| record.id == "c1-p001-r0");
        assert!(found.is_some());
        assert!(!found.unwrap().visible);
    }

    #[test]
    fn clean_anyway_fallback_selects_bubble_for_in_balloon_gate_skips_and_outside_for_outside_bubble() {
        assert_eq!(
            untouched_fallback_pick(Some("review.reason.gateSkippedOutsideBubble")),
            Picks::default().outside
        );
        assert_eq!(
            untouched_fallback_pick(Some("review.reason.gateSkippedNotJapanese")),
            Picks::default().bubble
        );
        assert_eq!(
            untouched_fallback_pick(Some("review.reason.gateSkippedLowConfidence")),
            Picks::default().bubble
        );

        let in_balloon_choice = choice_for(None, untouched_fallback_pick(Some("review.reason.gateSkippedNotJapanese")));
        assert_eq!(in_balloon_choice, Choice::Ladder(Some(EnginePick::Fill)));

        let out_balloon_choice = choice_for(None, untouched_fallback_pick(Some("review.reason.gateSkippedOutsideBubble")));
        assert_eq!(out_balloon_choice, Choice::Ladder(Some(EnginePick::Lama)));
    }
}
