//! `runClean`, `cancelRun`, `resumeJob` - and the four run events.
//!
//! The core already cleans a page end to end; `spikes/clean-page` is the whole
//! of that in about 150 lines. What was missing is everything around it: a
//! queue, a manifest to write into, a cancellation that keeps what it has, and
//! the ordering makes a contract rather than a courtesy.
//!
//! ## The shape: a scheduler and a cleaner, separately
//!
//! Every ordering rule the seam states is a property of the **scheduler** -
//! when events go out, how many, in what order, and what the manifest holds
//! when they do. None of them is a property of the pipeline that turns pixels
//! into patches. So the two are separated by [`Cleaner`]: [`Pipeline`] is the
//! real one and holds three ONNX sessions, and the scheduler is written against
//! the trait.
//!
//! That is not a testing convenience bolted on. It is what makes the ordering
//! rules testable *at all* on a machine without the weights, and it means the
//! tests exercise the same scheduler the window runs, with a different last
//! inch. The pipeline's own correctness is pinned elsewhere - by the core's
//! unit tests, and end to end by `spikes/clean-page`, which asserts the
//! fidelity contract against the file it wrote.
//!
//! ## Sessions are held; regions are never batched
//!
//! [`Pipeline`] opens the detector,
//! the balloon detector and the script gate **once per run** and carries them
//! across every page. There is no batching path here and there should not be
//! one: a run is a sequence of pages, each of which is a sequence of regions,
//! and each region's result is flushed before the next one starts.
//!
//! Rung 2's session is held the same way and opened differently: **not until a
//! region actually routes to it**, because it costs 2.1 s to build and about
//! 510 MB to hold, and a chapter of flat-paper dialogue never reaches it. See
//! [`Rung2`].
//!
//! ## The ladder is a loop, and its bottom rung is "leave it alone"
//!
//! The decline outcome is the state
//! where "no engine's output passes the uniform quality metric" - which is a
//! loop over rungs and not a property of one of them. [`clean_region`]
//! is that loop: the routed rung runs, [`quality::assess`] scores what came
//! back, and a decline escalates to the next rung [`ladder_from`] permits. When
//! the top permitted rung is declined too, the region is **left exactly as it
//! was** and listed in review with the reason - never half-cleaned, and never
//! written with the output of a rung that already failed. Leaving text is
//! recoverable; a bad fill is not.
//!
//! ## Auto clean is local-only
//!
//! It is enforced in one place: [`effective_ceiling`] never answers
//! [`Engine::Cloud`], whatever it is asked for. `needs-confirmation` lives on
//! `applyTool` and `runClean` has no equivalent, so a batch run at the cloud
//! ceiling would spend with neither the transmission statement nor the cost
//! confirmation. If cloud batch runs are ever wanted, this grows the
//! confirmation protocol **first**.
//!
//! And `engineCeiling` **caps, never raises**: both the stored setting and the
//! caller's argument can only lower the rung, never lift it.
//!
//! ## What is written down, and when
//!
//! The "flushed after every
//! completed region" rule is [`Job::complete_region`]'s reason for existing, and
//! this module calls nothing else to record one. A page is marked
//! [examined](cleaner_core::project::Project::examined) **after** its regions
//! are recorded, so a crash between the two repeats one page rather than
//! skipping it. A page that could not be written is marked
//! [errored](cleaner_core::project::Project::errored) instead of examined, so it
//! is still the next run's work.
//!
//! ## One writer per job, and what that does not cover
//!
//! A manifest is rewritten whole after every region, and the run holds one open
//! for a whole chapter - so the span in which another command could interleave
//! on it went from milliseconds to the length of a run. [`lock_job`] closes
//! that **inside this process**: every read-modify-write of a manifest in this
//! crate takes it. Two copies of the application over one library still race,
//! and that is the single-instance lock,
//! which is Phase 6 and is not attempted here.
//!
//! ## Nothing gets out of here without a `run-finished`
//!
//! The seam's "exactly one per run, whatever happened" includes a panic in the
//! pipeline, so [`execute`]'s walk runs under
//! [`catch_unwind`](std::panic::catch_unwind) and a panicked run reports itself
//! cancelled at the page it stopped on. The active slot is handed back by a
//! guard for the same reason one rung further out: a run whose slot is never
//! cleared makes every later `runClean` answer `alreadyRunning` for the rest of
//! the session.

use std::collections::HashSet;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use cleaner_core::accel::Preference;
use cleaner_core::detect::{Detector, build_regions_separated};
use cleaner_core::engines::{denoise, fill, lama};
use cleaner_core::fit::{self, EdgeMap, Route};
use cleaner_core::gate::{OutsideText, ScriptGate};
use cleaner_core::image::{Raster, decode};
use cleaner_core::ingest::sha256_hex;
use cleaner_core::mask::{Mask, Rect};
use cleaner_core::memory;
use cleaner_core::patch::{Engine, Patch, Provenance};
use cleaner_core::project::{Job, PatchRecord, Project, StripMode, buffers};
use cleaner_core::quality;
use cleaner_core::registry;
use cleaner_core::residency::{self, Resident};
use cleaner_core::strip::{
    self, DetectedInSegment, EngineContext, GlobalBox, Joins, LONGSTRIP_ASPECT, Segment, Split,
    SplitKind, Strip, Survey, merge_global,
};

use crate::events::{self, Event};
use crate::library::{self, ApiChapter, ApiPage, ApiProject, Library, LibraryError};

/* ------------------------------------------------------------------ */
/* The ladder ceiling                                                  */
/* ------------------------------------------------------------------ */

/// The rungs in order, so "no higher than" is a comparison rather than a match
/// arm somebody has to keep in the right sequence.
pub(crate) fn rung(engine: Engine) -> u8 {
    match engine {
        Engine::Fill => 0,
        Engine::Denoise => 1,
        Engine::Lama => 2,
        // 3 is vacant. It was MI-GAN's, and the removal left the number where
        // it was rather than sliding rung 3a and rung 4 down into it: every
        // stored `engineCeiling` is a *name*, but every comparison in this file
        // is on these numbers, and renumbering would silently reorder a ceiling
        // somebody stored. A gap costs nothing; a shuffle costs correctness.
        Engine::Flux => 4,
        Engine::Cloud => 5,
        // **Not rungs, and sorted above every ceiling so that they cannot be
        // mistaken for one.** A brush stroke and a clone stroke record
        // themselves in the same `engine` field a rung does, and that is the
        // whole of the resemblance: there is nothing to escalate to, nothing
        // to escalate from, and no quality verdict to escalate on. Answering
        // `u8::MAX` makes every "may this run here?" comparison - the ceiling
        // fold, `reachable_automatically`, `ladder_from`'s range filter -
        // answer no by arithmetic rather than by an arm somebody has to
        // remember to add. `Engine::is_rung` is the predicate that says so in
        // words; this is the number that makes it operational.
        Engine::Paint | Engine::Clone => u8::MAX,
    }
}

/// The highest rung that runs on this machine.
///
/// The whole of "auto clean is local-only". `Engine::Cloud` sits above it and is therefore unreachable from a run,
/// by construction rather than by a check somebody could forget.
pub const HIGHEST_LOCAL: Engine = Engine::Flux;

/// The highest rung an **automatic** run may reach, whatever ceiling it is
/// given.
///
/// [`HIGHEST_LOCAL`] answers "which rungs run on this machine" and this answers
/// "which rungs a batch job may spend", and rung 3a is the one place the two
/// differ: it is local, it is not automatic. Same ruling the cloud rung carries,
/// and for the same reason - `runClean` has no confirmation protocol, and
/// a batch job must not silently spend ten to sixty seconds a region on a heavy
/// local model, nor spawn a multi-gigabyte child to do it.
///
/// LaMa is the boundary because it is now the top of the automatic ladder: MI-GAN
/// held this line while it existed and no longer does. Rung 3a (FLUX) and
/// rung 4 (cloud) are still above it, and still for the reason the paragraph
/// above gives.
///
/// It is a **constant compared against**, rather than a rung left out of a
/// list, because a list is a thing somebody adds to. [`ladder_from`] filters on
/// this and `rung_3a_is_not_reachable_from_an_automatic_run` in
/// [`tests`](super::run::tests) pins it across every start and every ceiling.
pub const HIGHEST_AUTOMATIC: Engine = Engine::Lama;

/// Whether an automatic run may reach a rung at all.
///
/// The two rungs above [`HIGHEST_AUTOMATIC`] are the two that need a
/// confirmation `runClean` does not have: rung 4 spends the user's money and
/// sends their scan off the machine, rung 3a spends their memory and their
/// afternoon. Neither is a judgement about quality.
pub fn reachable_automatically(engine: Engine) -> bool {
    rung(engine) <= rung(HIGHEST_AUTOMATIC)
}

/// A rung name to a rung. Accepts the seam's bare names and the catalogue's
/// `ladder.rung.*` keys, because `engineCeiling` is a stored setting and the
/// two spellings both appear in the interface.
pub(crate) fn parse_rung(name: &str) -> Option<Engine> {
    match name.strip_prefix("ladder.rung.").unwrap_or(name) {
        "fill" => Some(Engine::Fill),
        "denoise" => Some(Engine::Denoise),
        "lama" => Some(Engine::Lama),
        // **A stored `"migan"` still means something, and it means LaMa.**
        // MI-GAN was rung 3 and is gone, but a ceiling is a *setting*, written
        // to disk on machines that ran the old build. Dropping the arm outright
        // would make those installs fall to the `_ => None` case below, and an
        // unparseable ceiling is ignored - so the one setting whose whole job
        // is to hold a rung back would have silently released it, up to
        // `HIGHEST_LOCAL`. Harmless for an automatic run, which
        // `HIGHEST_AUTOMATIC` caps at LaMa anyway, and a real lift for a region
        // edit, which reads the same setting. LaMa is where "no higher than the
        // strongest automatic local rung" now points.
        "migan" => Some(Engine::Lama),
        "flux" => Some(Engine::Flux),
        "cloud" => Some(Engine::Cloud),
        _ => None,
    }
}

/// The rung a run may not go above.
///
/// **Caps, never raises.** Introduced twice and caught twice, so it is written as a fold that can
/// only ever move the ceiling down: the stored setting caps it, the caller's
/// argument caps it again, and [`HIGHEST_LOCAL`] caps both. An unparseable name
/// is ignored rather than treated as "no ceiling", because the failure that
/// matters is a typo silently unlocking a rung.
pub fn effective_ceiling(requested: Option<&str>, stored: Option<&str>) -> Engine {
    let mut ceiling = HIGHEST_LOCAL;
    for candidate in [stored, requested].into_iter().flatten().filter_map(parse_rung) {
        if rung(candidate) < rung(ceiling) {
            ceiling = candidate;
        }
    }
    ceiling
}

/// The stored key naming which accelerator the user wants.
///
/// The vocabulary is the interface's, as [`crate::settings`] insists, and this
/// is where it is read rather than defined - the same shape `engineCeiling`
/// already has a few lines above. The values are
/// [`cleaner_core::accel::Accelerator::label_key`] without the `accel.` prefix,
/// plus `auto`, so a settings panel can build the list from
/// `list_accelerators` and store the `id` it was given back verbatim.
pub const ACCELERATOR_KEY: &str = "accelerator";

/// What the user asked their models to run on.
///
/// Three answers and no fourth: `auto` is [`Preference::Automatic`], `cpu` is
/// [`Preference::CpuOnly`] - absolute, and the setting
/// the golden tests are written against -
/// and any provider name is [`Preference::Force`].
///
/// **An unreadable value is `Automatic`, not an error.** A settings file
/// written by a newer build, or by hand, must not stop the application from
/// cleaning a page; and `Force` of a provider that is not on this machine is
/// already handled one layer down, where it is reported rather than guessed at
/// ([`cleaner_core::accel::choose_within`]).
pub fn preference_from(settings: &serde_json::Value) -> Preference {
    let Some(name) = settings.get(ACCELERATOR_KEY).and_then(|value| value.as_str()) else {
        return Preference::Automatic;
    };
    accelerator_preference(name)
}

/// The same, from the bare name. Split out so the mapping is testable without a
/// settings document.
pub fn accelerator_preference(name: &str) -> Preference {
    let name = name.strip_prefix("accel.").unwrap_or(name);
    match name {
        "auto" | "automatic" | "" => Preference::Automatic,
        "cpu" => Preference::CpuOnly,
        other => cleaner_core::accel::KNOWN
            .into_iter()
            .find(|accelerator| accelerator.label_key() == format!("accel.{other}"))
            .map(Preference::Force)
            .unwrap_or(Preference::Automatic),
    }
}

/// The rung that will actually run for a route, under a ceiling.
///
/// `None` is a decline. Rung 0 is always available - it is the bottom of the
/// ladder and needs no model - so a ceiling below the routed rung has two
/// different answers, and the difference is not a nicety:
///
/// - [`Route::FillAndDenoise`] under a ceiling of rung 0 **downgrades**. Rung 1
///   restores the grain a flat fill flattens; without it the fill is merely
///   slightly too smooth, which is a worse patch and not a wrong one.
/// - [`Route::Inpaint`] under a ceiling below rung 2 **declines**. §5 sent the
///   region to the ladder because the paper around it is periodic, multimodal
///   or unfittable, and rung 0 over that is a hole in the screentone. §6's
///   metric would not catch it either - a flat plate has no strokes to fail the
///   edge test, and its level sits inside a halftone's own p5..p95 - so the
///   downgrade would be a bad fill made silently. Leaving text is recoverable;
///   a bad fill is not.
///
/// The escalation above this is [`clean_region`]'s: this names where
/// a region **starts**, not where it ends.
pub(crate) fn engine_for(route: Route, ceiling: Engine) -> Option<Engine> {
    match route {
        Route::Fill => Some(Engine::Fill),
        Route::FillAndDenoise => Some(if rung(Engine::Denoise) <= rung(ceiling) {
            Engine::Denoise
        } else {
            Engine::Fill
        }),
        Route::Inpaint => (rung(Engine::Lama) <= rung(ceiling)).then_some(Engine::Lama),
    }
}

/* ------------------------------------------------------------------ */
/* Which engine a kind of text starts on                               */
/* ------------------------------------------------------------------ */

/// What the user asked for, for one kind of text: **the rung it starts on**.
///
/// It used to be two answers - *fill* and *redraw* - each standing for a family
/// of rungs, on the reasoning that a translator should never have to read the
/// ladder's own words. The user ruled the other way: an engine picker exists so that
/// somebody can move between models, and a word that covers two of them makes
/// that impossible. So the tool window's two rows now name a rung each, and
/// this is that rung.
///
/// Rung 3a is deliberately not a variant. [`HIGHEST_AUTOMATIC`] caps every
/// automatic run at LaMa, so a `Flux` pick could only ever be a control that
/// did nothing; the tool window does not offer it and [`parse_pick`] does not
/// read it.
///
/// **Still a start and never a ceiling.** [`clean_region`] escalates past a
/// declined rung exactly as it did when this was two words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnginePick {
    /// Rung 0 - paint the hole from the paper around it.
    Fill,
    /// Rung 1 - the same fill with the page's grain put back.
    Denoise,
    /// Rung 2 - the inpainter.
    Lama,
}

impl EnginePick {
    /// The rung this pick starts a region on.
    fn engine(self) -> Engine {
        match self {
            EnginePick::Fill => Engine::Fill,
            EnginePick::Denoise => Engine::Denoise,
            EnginePick::Lama => Engine::Lama,
        }
    }
}

/// The two picks a run carries: one for text inside a speech balloon, one for
/// text outside one.
///
/// The defaults are the two kinds of text's own answers. A balloon is flat
/// white or flat black paper and the fill family matches it exactly for no
/// model at all; text over art has to have the art put back, and only the
/// inpainter can do that. The run used to decide both from the fit alone, so a
/// bubble whose fit would not settle went to a 510 MB session and a second a
/// region to redraw paper a fill would have got right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Picks {
    pub bubble: EnginePick,
    pub outside: EnginePick,
}

impl Default for Picks {
    fn default() -> Self {
        Picks { bubble: EnginePick::Fill, outside: EnginePick::Lama }
    }
}

impl Picks {
    /// The seam's two fields. An absent or unreadable name is **this type's own
    /// default**, not "no preference": the interface always sends both, so an
    /// unparseable one is a typo somewhere and the defaults are what the
    /// window shows.
    pub fn from_args(bubble: Option<&str>, outside: Option<&str>) -> Picks {
        let fallback = Picks::default();
        Picks {
            bubble: parse_pick(bubble).unwrap_or(fallback.bubble),
            outside: parse_pick(outside).unwrap_or(fallback.outside),
        }
    }

    /// Which pick a region answers to. The gate's own question
    /// ([`cleaner_core::balloon::in_balloon`]), asked once per region and used
    /// for both.
    fn for_region(&self, in_balloon: bool) -> EnginePick {
        if in_balloon { self.bubble } else { self.outside }
    }
}

/// A pick's name to a pick.
///
/// The seam's names are the ladder's own rung names, bare or as a
/// `ladder.rung.*` catalogue key. `redraw` and `inpaint` are the two words the
/// rows used to send and are still read as rung 2: a preference stored before
/// the rename is not a typo, and silently
/// falling back to the default would move a user's run without telling them.
///
/// `flux` and `cloud` are **not** picks. Neither is reachable from an automatic
/// run ([`HIGHEST_AUTOMATIC`]), so reading either one as a start would be a
/// promise the ceiling immediately takes back.
pub(crate) fn parse_pick(name: Option<&str>) -> Option<EnginePick> {
    let name = name?;
    match name.strip_prefix("ladder.rung.").unwrap_or(name) {
        "fill" => Some(EnginePick::Fill),
        "denoise" => Some(EnginePick::Denoise),
        "lama" | "redraw" | "inpaint" => Some(EnginePick::Lama),
        _ => None,
    }
}

/// The rung a region **starts** on: the route's own answer, moved by the user's
/// pick for this kind of text.
///
/// **A pick is a starting point and never a ceiling.** It cannot reach past
/// `engineCeiling` - a `Lama` pick under a ceiling below rung 2 keeps the routed
/// rung - and it cannot remove a rung from [`ladder_from`], so a `Fill` over
/// screentone is tried, declined by [`quality::assess`], and escalated to the
/// inpainter exactly as an unpicked run would be. That is the whole reason the
/// pick moves the start rather than the ceiling: the cheap answer is tried
/// first and the expensive one is still there when it is wrong.
///
/// **The rung the row names is the rung that runs**, in both directions, which
/// is what was asked for: picking *Fill*
/// on a `Route::FillAndDenoise` region now starts on rung 0 rather than being
/// quietly kept at rung 1 because rung 1 is "part of the fill family". The one
/// thing that still overrides it is the ceiling.
///
/// The decline that [`engine_for`] returns is left exactly where it was. A
/// `Route::Inpaint` under a ceiling that cannot inpaint still declines rather
/// than being talked down to rung 0 by a pick - the routing argument is about the
/// paper under the region and a preference does not change it.
pub(crate) fn start_rung(route: Route, ceiling: Engine, pick: Option<EnginePick>) -> Option<Engine> {
    let routed = engine_for(route, ceiling)?;
    let Some(pick) = pick else { return Some(routed) };
    let preferred = pick.engine();
    Some(if rung(preferred) <= rung(ceiling) { preferred } else { routed })
}

/// The rungs a region may be tried on, in the order they are tried.
///
/// A decline is the state where "**no
/// engine's output** passes the uniform quality metric", which is a loop and not
/// a single call - `quality.rs`'s own report says so in as many words: "the
/// try-escalate-then-decline loop is the caller's". This is that loop's list.
///
/// Three filters, and each one is a rule from somewhere else:
///
/// - **At or above the routed rung.** The routing table is a floor, not a suggestion.
///   A region the fit could not settle does not get a *simpler* engine as a
///   second chance.
/// - **At or below the ceiling.** `engineCeiling` caps and never raises, and
///   it caps escalation for the same reason it caps routing.
/// - **The rung can carry this source.** The engines answer that themselves, so
///   an escalation cannot name a rung that would refuse the page,
///   one level up from [`fit::route_for`], which applies the same test when it
///   names the starting rung.
///
/// **Rung 2 is the top of this list**, and there is nothing above it a region
/// can escalate *to*. Rung 3 used to sit here - MI-GAN, the optional preview
/// tier, reached only when LaMa declined a region - and the user removed it on
/// the ground that a rung which only ever ran after a stronger model had
/// already refused the region was buying nothing. So the escalation this loop
/// walks now ends at the inpainter:
/// a region LaMa declines is declined, listed in review with its reason, and
/// left exactly as it was, which is §6's own answer for the end of the ladder.
///
/// **Rung 3a's FLUX sidecar is never reachable from an automatic run at all**,
/// and that is now a filter rather than an absence. It used to be true because
/// the array below did not mention it, which is a guarantee that lasts until
/// somebody adds a rung to a list; it is true now because
/// [`reachable_automatically`] says so and
/// `rung_3a_is_not_reachable_from_an_automatic_run` checks every start against
/// every ceiling. The rung exists - `cleaner_core::engines::flux` and
/// `cleaner_core::sidecar` - and it is reached per region, deliberately, from
/// Content-aware fill.
fn ladder_from(start: Engine, ceiling: Engine, page: &Raster) -> Vec<Engine> {
    [Engine::Fill, Engine::Denoise, Engine::Lama, Engine::Flux, Engine::Cloud]
        .into_iter()
        .filter(|engine| reachable_automatically(*engine))
        .filter(|engine| rung(*engine) >= rung(start) && rung(*engine) <= rung(ceiling))
        .filter(|engine| match engine {
            // Rung 0 is arithmetic over the page's own samples and serves every
            // mode and depth this application reads.
            Engine::Fill => true,
            Engine::Denoise => denoise::applies(page),
            // The model rung answers for itself - a source it cannot carry is
            // a source it drops off the ladder rather than one it faults on.
            Engine::Lama => lama::applies(page),
            // Already gone, by the filter above. Written out rather than left
            // to a wildcard so that this match cannot silently start answering
            // for a rung it has no business answering for.
            Engine::Flux | Engine::Cloud => false,
            // The two hand tools are not on the ladder at all: `rung` sorts
            // them above every ceiling, so the range filter has already dropped
            // them and this arm is unreachable. Written out for the same reason
            // as the line above it.
            Engine::Paint | Engine::Clone => false,
        })
        .collect()
}

/* ------------------------------------------------------------------ */
/* What a page produced                                                */
/* ------------------------------------------------------------------ */

/// One region's result, in the two shapes the manifest has rows for.
pub enum RegionOutcome {
    /// A patch, and the `review.reason.*` key it is flagged under, if any.
    Cleaned(Box<Patch>, Option<String>),
    /// Deliberately left alone: gate-skipped, or declined. `reason` is an i18n
    /// key - no English is written to disk either.
    Untouched { bbox: Rect, reason: String },
}

#[derive(Default)]
pub struct PageOutcome {
    pub regions: Vec<RegionOutcome>,
    /// Steps down the pressure ladder taken
    /// while this page was cleaned, as i18n keys.
    ///
    /// Here rather than emitted where it happens, because the pipeline has no
    /// event sink and should not grow one: the scheduler owns the stream, and
    /// the pipeline reporting through its return value is the same shape a
    /// region outcome already takes. A run whose ladder fired and said nothing
    /// would be a run that silently got slower.
    pub pressure: Vec<&'static str>,
}

/* ------------------------------------------------------------------ */
/* Where the page sits in the strip                                    */
/* ------------------------------------------------------------------ */

/// What the geometry knows about the page a [`Cleaner`] is about to clean.
///
/// Rules 1 to 3's geometry is built
/// and has no caller; this is how the caller reaches it. Everything here comes
/// from one [`strip::survey`] per job, taken before the walk starts, and it is
/// borrowed rather than owned because a 200-page chapter has exactly one of it.
pub struct PageContext<'a> {
    pub strip: &'a Strip,
    /// Rule 1's third state matters: an **unchecked** join is not verified, so
    /// reads do not cross it, and is not an anomaly, so nothing is reported.
    pub joins: &'a Joins,
    /// Every segment of the whole strip, because ownership
    /// ([`merge_global`]) is decided against all of them and not against this
    /// page's.
    pub segments: &'a [Segment],
    /// This page's position in `strip.pages()`, which is also its position in
    /// the manifest's `strip.order`.
    pub placement: usize,
}

impl PageContext<'_> {
    /// The page's own origin in strip coordinates.
    fn origin(&self) -> (i64, i64) {
        self.strip
            .pages()
            .get(self.placement)
            .map(|p| (p.x_offset, p.y_offset))
            .unwrap_or((0, 0))
    }

    /// The segments that lie inside this page. Segments are cut at page
    /// boundaries ([`strip::survey`]), so this is the whole of the page's
    /// detection work and never a fraction of a segment.
    pub fn page_segments(&self) -> Vec<Segment> {
        let Some(page) = self.strip.pages().get(self.placement) else { return Vec::new() };
        let (top, bottom) = (page.y_offset, page.rect().bottom());
        self.segments
            .iter()
            .copied()
            .filter(|s| (s.start as i64) >= top && (s.start as i64) < bottom)
            .collect()
    }

    /// The rows every segment boundary sits on, as rule 3 step 1's split lines.
    ///
    /// Step 1 unions two boxes whose facing edges lie within 8 px of the *same*
    /// split, which is how the two halves of a bubble a cut went through become
    /// one region. The lines that matter for that are the ones detection was
    /// actually cut at, which is every segment start - rule 2's plan and the
    /// page boundaries together.
    fn cuts(&self) -> Vec<Split> {
        self.segments
            .iter()
            .skip(1)
            .map(|s| Split { y: s.start, kind: SplitKind::Join })
            .collect()
    }

    /// A rectangle in a segment crop's coordinates, in strip coordinates.
    /// `top` is where the crop begins in the page.
    fn to_strip(&self, rect: Rect, top: u32) -> Rect {
        let (x, y) = self.origin();
        Rect::new(rect.x + x, rect.y + y + top as i64, rect.w, rect.h)
    }

    /// The reverse, clamped to the crop: a decode window may reach onto another
    /// page through a verified join, and this build's reader holds one page.
    fn to_crop(&self, rect: Rect, top: u32, crop: &Raster) -> Rect {
        let (x, y) = self.origin();
        let x0 = (rect.x - x).max(0);
        let y0 = (rect.y - y - top as i64).max(0);
        let x1 = (rect.right() - x).min(crop.width as i64);
        let y1 = (rect.bottom() - y - top as i64).min(crop.height as i64);
        Rect::new(x0, y0, (x1 - x0).max(0) as u32, (y1 - y0).max(0) as u32)
    }
}

/// What turns one page of pixels into region outcomes.
///
/// See the module docs for why this is a trait. `page_id` is passed in because
/// region ids are page-scoped and the manifest keeps one record per id across
/// the whole chapter, and `context` because
/// rules 1 to 4 are all statements
/// about where the page sits in the strip rather than about the page.
pub trait Cleaner: Send {
    fn clean_page(
        &mut self,
        page_id: &str,
        bytes: &[u8],
        ceiling: Engine,
        context: &PageContext,
    ) -> Result<PageOutcome, String>;
}

/* ------------------------------------------------------------------ */
/* The real pipeline                                                   */
/* ------------------------------------------------------------------ */

/// The model files a run opens, by the names `scripts/fetch-models.sh` writes.
pub(crate) const DETECTOR: &str = "comictextdetector.onnx";
pub(crate) const BALLOONS: &str = "comic-text-and-bubble-detector-detector-v4-s_int8.onnx";
pub(crate) const GATE_MODEL: &str = "image-script-identification-osd_lstm.onnx";
pub(crate) const GATE_LABELS: &str = "image-script-identification-osd_labels.json";

/// Rung 2's weights. The fifth model, and the one a run does not open until
/// something needs it - see [`Rung2`].
pub(crate) const INPAINTER: &str = "lama-manga.onnx";

/// The gate's rescue reader - `manga-ocr`, three files
/// ([`cleaner_core::gate::ocr`]), and **optional in a way none of the others
/// are**. It is not in [`REQUIRED_MODELS`], nothing falls back to it, and a
/// machine without it gates exactly as this application did before it existed:
/// [`open_gate`] attaches it when all three files are present and otherwise
/// opens the gate alone. 460 MB is too much to make a precondition of cleaning
/// a page for the 4% of regions it recovers.
pub(crate) const OCR_ENCODER: &str = "manga-ocr-encoder_model.onnx";
pub(crate) const OCR_DECODER: &str = "manga-ocr-decoder_model.onnx";
pub(crate) const OCR_VOCAB: &str = "manga-ocr-vocab.txt";

/// The three, in one place, so a caller asking "is the reader installed" asks
/// the same question [`open_gate`] answers.
pub(crate) const OCR_MODELS: [&str; 3] = [OCR_ENCODER, OCR_DECODER, OCR_VOCAB];

/// The model files a run requires to open a pipeline.
///
/// `DETECTOR`, `BALLOONS`, `GATE_MODEL`, and `GATE_LABELS` are required eagerly
/// by [`Pipeline::open`]. `INPAINTER` is opened lazily by rung 2 and falls back
/// to `Unavailable` if missing on disk, so it is not required at open time.
pub(crate) const REQUIRED_MODELS: [&str; 4] = [DETECTOR, BALLOONS, GATE_MODEL, GATE_LABELS];

/// Whether a candidate directory contains all files required to open a [`Pipeline`].
pub(crate) fn models_ready(dir: &Path) -> bool {
    REQUIRED_MODELS.iter().all(|name| dir.join(name).exists())
}

/// Why [`Pipeline::open`] said no.
///
/// One variant, because [`Pipeline::open`] has exactly one way to fail: the
/// files are not there. It used to answer a `String`, and [`start`] told the
/// missing-weights failure apart from every other one by reading the front and
/// the back of that string - which
/// made the *wording* of a message load-bearing, and needed a test to pin it so
/// that rephrasing the message could not silently turn a notice into a rejected
/// promise.
///
/// A type instead. The wording stays exactly what it was, because
/// the remedy is what the user reads, but
/// nothing branches on it any more. And when `open` grows a second failure -
/// a runtime that will not load, a provider that refuses - the match in
/// [`start`] stops compiling until someone decides whether that one is a notice
/// or a rejection, which is the decision the prefix was making by accident.
#[derive(Debug)]
pub enum OpenError {
    /// A file in [`REQUIRED_MODELS`] is not on disk. The remedy is Settings ›
    /// Models, and [`start`] turns this into `notice.run.modelsMissing`.
    MissingModel { path: PathBuf },
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::MissingModel { path } => {
                write!(f, "the model file {} is missing", path.display())
            }
        }
    }
}

impl std::error::Error for OpenError {}

/// For the callers that answer `Result<_, String>` - every Tauri command in
/// this crate does - so that `?` still reaches them without each one spelling
/// out `map_err(|e| e.to_string())`.
impl From<OpenError> for String {
    fn from(error: OpenError) -> String {
        error.to_string()
    }
}

/// Where the weights are, in the order they are looked for.
///
/// The same shape as [`cleaner_core::runtime::search_paths`] and for the same
/// reasons: an environment override so a developer can point at a checkout, the
/// per-user directory a first-launch download writes to,
/// then the installed copy beside the
/// executable and inside the macOS bundle's `Resources`. The working directory
/// is last, and it is what the spikes and `cargo test` use.
pub fn model_search_paths(app_data: Option<&Path>) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(explicit) = std::env::var("MANGA_CLEANER_MODELS") {
        if !explicit.is_empty() {
            paths.push(PathBuf::from(explicit));
        }
    }
    if let Some(dir) = app_data {
        paths.push(dir.join("models"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            paths.push(dir.join("models"));
            paths.push(dir.join("../Resources/models"));
        }
    }
    paths.push(PathBuf::from("models"));
    paths
}

pub(crate) fn model_dir(app_data: Option<&Path>) -> Option<PathBuf> {
    model_search_paths(app_data).into_iter().find(|dir| models_ready(dir))
}

/// Every session a run may need, and **not one of them opened until a page
/// asks for it**.
///
/// This used to open the detector, the balloon detector and the script gate in
/// [`Pipeline::open`] and hold all three for the length of the run. Both halves
/// of that were wrong for a run's memory:
///
/// - **Opening them eagerly** put three sessions on a run before it had decided
///   there was anything to do, and on the *only* code path that can fail for a
///   reason the user can act on - missing weights. So `open` now checks that
///   the files are there, which is the failure worth reporting early, and opens
///   nothing.
/// - **Holding them unconditionally** meant a session with nothing pending was
///   indistinguishable from one in use. [`OnDemand`] closes that: every model
///   carries a [`cleaner_core::registry`] row, and a row can be asked for back -
///   by the user, from the loaded-models tab, or by the idle grace - at the
///   region boundary this file already established as the safe point for
///   giving a session back.
pub struct Pipeline {
    detector: OnDemand<Detector>,
    balloons: OnDemand<cleaner_core::balloon::BalloonDetector>,
    gate: OnDemand<ScriptGate>,
    engine_version: &'static str,
    rung2: Rung2,
    /// The pressure ladder, latched for the life of the pipeline. See
    /// [`Pipeline::under_pressure`].
    ladder: memory::Ladder,
    /// Which rung each kind of text starts on. A property of the run and not of
    /// the page, so it is held here rather than passed through [`Cleaner`]: the
    /// trait's arguments are the ones that change page by page.
    ///
    /// `None` is **not** [`Picks::default`]: it is "nobody asked", and a
    /// pipeline nobody asked routes every region by the fit alone, which is
    /// what this did before the rows existed. `run_clean` always asks - the
    /// tool window always sends both - so the empty case is `spikes/clean-page`
    /// and the tests, where the fit's own answer is the thing under test.
    picks: Option<Picks>,
    /// What this run does with text the balloon question puts outside a
    /// balloon. [`cleaner_core::gate::OutsideText::Review`] unless the tool
    /// window's row said otherwise, and a property of the run for the same
    /// reason `picks` is.
    outside: OutsideText,
}

/// A session borrowed the first time something needs it and handed back to
/// [`cleaner_core::residency`] the moment this run does not.
///
/// **This no longer owns the session, and that is the whole change.** It used
/// to: the detector belonged to the `Pipeline`, the `Pipeline` was a local of
/// the run's thread, and so the end of a run - or of a region edit, which built
/// its own bench - dropped every model the user's next click was about to want.
/// Now `get` asks the residency cache first and `Drop` puts the session back
/// there, so two consecutive runs, a page turn and a hand edit in between all
/// share one 94 MB session and one CoreML build.
///
/// The three models a page needs before it can route anything - the detector,
/// the balloon detector, the gate - differ from rung 2 in one way that
/// matters here: **a run cannot proceed without them**, so giving one back is
/// never a decision about the output, only about when the session is rebuilt.
/// That is why this has no `Surrendered` state and [`Rung2`] does. A step down
/// the pressure ladder costs a run its rungs and is latched; a spent detector
/// costs a run one reopen and is not.
///
/// `open` is a function pointer rather than a closure so that this stays one
/// plain type rather than a generic over a callable - every instance of it is
/// built from a model directory and a preference, which is exactly what a run
/// has.
pub(crate) struct OnDemand<T: Resident> {
    models: PathBuf,
    preference: Preference,
    /// Which parked session is this one's. See [`session_key`].
    key: residency::Key,
    open: fn(&Path, Preference) -> Result<T, String>,
    held: Option<T>,
}

impl<T: Resident> OnDemand<T> {
    fn new(
        kind: registry::Kind,
        models: &Path,
        preference: Preference,
        open: fn(&Path, Preference) -> Result<T, String>,
    ) -> OnDemand<T> {
        OnDemand {
            models: models.to_path_buf(),
            preference,
            key: session_key(kind, models, preference),
            open,
            held: None,
        }
    }

    /// The session: the one already in memory if there is one, and a new one
    /// otherwise.
    ///
    /// A failure here is the page's, not the run's: [`clean_page`] turns it
    /// into a page that could not be cleaned, which
    /// leaves it queued for the next run.
    pub(crate) fn get(&mut self) -> Result<&mut T, String> {
        if self.held.is_none() {
            self.held = Some(match residency::checkout::<T>(&self.key) {
                Some(resident) => resident,
                None => (self.open)(&self.models, self.preference)?,
            });
        }
        Ok(self.held.as_mut().expect("just opened"))
    }

    /// Give it back if the registry says so - somebody pressed the close
    /// button, or nothing has used it for
    /// [`cleaner_core::registry::ONNX_IDLE_GRACE`]. Answers whether a session
    /// actually went.
    ///
    /// **A drop here is a real drop**, not a hand-back: `spent` is exactly the
    /// state the cache would refuse to park anyway, and routing it through the
    /// cache would only mean the memory went one sweep later than the user was
    /// told.
    fn release_if_spent(&mut self) -> bool {
        let spent = self.held.as_ref().is_some_and(Resident::spent);
        if spent {
            self.held = None;
        }
        spent
    }

    #[cfg(test)]
    fn is_held(&self) -> bool {
        self.held.is_some()
    }
}

impl<T: Resident> Drop for OnDemand<T> {
    /// The end of a run is **not** a reason to unload a model.
    /// Whatever this is still
    /// holding goes back to the residency cache, where the next run, the next
    /// page and the next region edit will find it.
    fn drop(&mut self) {
        if let Some(held) = self.held.take() {
            residency::checkin(self.key.clone(), held);
        }
    }
}

/// Which parked session a holder means: the kind, the directory the weights
/// were read from, and the accelerator preference they were built under. Two
/// different model directories are two different sessions, and handing back one
/// opened against the other would be answering a question nobody asked.
fn session_key(kind: registry::Kind, models: &Path, preference: Preference) -> residency::Key {
    residency::Key::new(kind, format!("{}|{preference:?}", models.display()))
}

/// Rung 2's session, and the two states that are not "held".
///
/// **It is not opened until a region routes to it.** A manga-LaMa session costs
/// about 2.1 s to build and holds roughly 510 MB resident,
/// and a chapter of
/// flat-paper dialogue never needs one: every region of it routes to rung 0 and
/// passes. Opening the inpainter with the other three would put that 510 MB and
/// those 2.1 s on every run of every chapter, for a rung most of them never
/// reach. So the run pays for it exactly when it uses it.
///
/// **And once opened it is held for the rest of the run**: building
/// costs seconds and running costs one, so a session rebuilt per region would
/// spend most of a job in the loader. It goes when the [`Pipeline`] goes, at the
/// end of the run, the same as the
/// "unload LaMa" pressure step does by hand.
///
/// It is a value of its own rather than three fields on [`Pipeline`] for the
/// same reason [`Cleaner`] is a trait: the escalation loop is written against
/// this, so every rung below 2 can be exercised on a machine that has no
/// weights, with [`Rung2::state`] saying afterwards whether a session was ever
/// asked for. It was one of a pair - rung 3 held its own MI-GAN session on
/// these terms - and it is the only one left.
pub(crate) struct Rung2 {
    /// Where [`INPAINTER`] is looked for, kept because the session that reads
    /// it is not built until something needs it.
    models: PathBuf,
    preference: Preference,
    /// Which parked session is this one's, so that "held for the rest of the
    /// run" has become "held until somebody asks for it back" - the run's end
    /// hands the session to [`cleaner_core::residency`] rather than dropping
    /// it.
    key: residency::Key,
    state: Session<lama::Inpainter>,
}

/// What a lazily-opened rung's session currently is.
///
/// Generic over the engine, because the three facts it records - when the
/// session is built, what "never needed" means, and what happens when the
/// pressure ladder takes it away - are facts about a *lazily opened rung* and
/// not about the model. Rung 3 was the second user of it and is gone; the
/// generic stays, because the alternative is inlining these four states into
/// [`Rung2`] and writing them out again the next time a rung is opened on
/// demand.
enum Session<E> {
    /// Nothing has needed it yet. Not the same as [`Session::Unavailable`]:
    /// this is the state a run of pure rung-0 pages ends in, and it is the one
    /// that says the 510 MB was never paid.
    Unopened,
    Held(Box<Held<E>>),
    /// It was needed, and it could not be had - no weights on this machine, or
    /// no session would build. Recorded so the failure is paid once per run
    /// rather than once per region.
    Unavailable,
    /// The pressure ladder took it back, or refused to open it in the first
    /// place. Distinct from
    /// `Unavailable`, because the weights are fine and the machine is not, and
    /// distinct from `Unopened`, because it must **not** be opened again: a
    /// step down that ladder is a latch, and rebuilding 510 MB on the next
    /// region is how a machine under pressure gets driven back into the
    /// compressor.
    Surrendered,
}

impl Rung2 {
    pub(crate) fn new(models: &Path, preference: Preference) -> Rung2 {
        Rung2 {
            models: models.to_path_buf(),
            preference,
            key: session_key(registry::Kind::Inpainter, models, preference),
            state: Session::Unopened,
        }
    }

    /// The held inpainter, opening it if this is the first region to want one.
    ///
    /// `None` is "this run has no rung 2", not "this region cannot use it". The
    /// two are told apart at the call site because they carry different reasons
    /// into review.
    fn held(&mut self) -> Option<&mut Held<lama::Inpainter>> {
        if matches!(self.state, Session::Unopened) {
            // The parked session first: 2.1 s and 510 MB that a second run, or
            // a hand edit after one, does not have to pay again.
            let resident = residency::checkout::<Held<lama::Inpainter>>(&self.key);
            self.state = match resident.or_else(|| open_inpainter(&self.models, self.preference)) {
                Some(held) => Session::Held(Box::new(held)),
                None => Session::Unavailable,
            };
        }
        match &mut self.state {
            Session::Held(held) => Some(held),
            _ => None,
        }
    }

    /// Give the session back, under the
    /// pressure ladder. Answers whether there was one to give.
    ///
    /// The state it lands in is [`Session::Surrendered`], which [`Rung2::held`]
    /// does not reopen - so every later region routes down to rungs 0 and 1, or
    /// is left alone and listed in review, and the run continues. That is what
    /// "each step is recoverable" means here: the run is a rung worse, not over.
    fn surrender(&mut self) -> bool {
        let held = matches!(self.state, Session::Held(_));
        self.state = Session::Surrendered;
        held
    }

    /// Give the session back because **nothing is using it** - the close button
    /// in the loaded-models tab, or
    /// [`cleaner_core::registry::IDLE_GRACE`] with no region having routed here.
    ///
    /// It lands in [`Session::Unopened`] and not [`Session::Surrendered`], and
    /// that is the whole difference between this and the pressure ladder: the
    /// ladder's step is a latch because the machine is short of memory and
    /// rebuilding 510 MB is how it gets driven into the compressor, whereas
    /// this is a session nobody wanted on a machine that is fine. A later
    /// region gets a rung 2 again, and pays the 2.1 s once.
    fn release_if_spent(&mut self) -> bool {
        let spent = match &self.state {
            Session::Held(held) => held.inpainter.spent(),
            _ => false,
        };
        if spent {
            self.state = Session::Unopened;
        }
        spent
    }
}

impl Drop for Rung2 {
    /// A held session goes back to the residency cache, and a **surrendered**
    /// one does not exist to go anywhere: [`Rung2::surrender`] already dropped
    /// it, which is what the pressure ladder asked for.
    fn drop(&mut self) {
        if let Session::Held(held) = std::mem::replace(&mut self.state, Session::Unopened) {
            residency::checkin(self.key.clone(), *held);
        }
    }
}

/// A held model session, with the two facts about it that go into every patch
/// it produces.
struct Held<E> {
    inpainter: E,
    /// Where the session landed, from the engine's own `selection` and not from
    /// the detector's - the models choose their providers separately and a
    /// patch names the one that made it.
    provider: String,
    /// The weights' digest. `None` on every rung below this one because there
    /// is no model to digest; here, a missing digest would be a missing answer
    /// rather than a true one.
    model_sha256: Option<String>,
}

/// **The provider and the digest are parked with the session, not recomputed.**
/// They are facts about *this* session - where it landed, and the bytes it was
/// built from - so a cache that kept the session and threw them away would make
/// the next run read 207 MB off disk to digest weights it never opened.
impl Resident for Held<lama::Inpainter> {
    fn spent(&self) -> bool {
        self.inpainter.spent()
    }
}

impl Pipeline {
    /// **The pressure ladder's trigger.**
    ///
    /// Called once per region, between regions, which is the only moment a step
    /// is safe to take: rule 6's discipline means the last region's buffers are
    /// gone and the next one's do not exist, so what is resident here is the
    /// sessions - and the sessions are what the ladder gives back.
    ///
    /// The signal is [`cleaner_core::memory::pressure`], the kernel's own
    /// `kern.memorystatus_vm_pressure_level`, and **not**
    /// `os_proc_available_memory`, which answers 0 for
    /// a process with no per-process limit - every macOS process this
    /// application will ever be. That measurement is in
    /// [`cleaner_core::memory`]'s own documentation.
    ///
    /// Two steps, and rung 3a's step is missing because rung 3a is refused
    /// before it is ever opened - the order is "refuse and
    /// unload rung 3a, then unload LaMa, then drop to one window in flight":
    ///
    /// - **Unload the inpainter.** Rung 2's held session goes back, and the
    ///   parked copies of it go with it. This step used to unload two model
    ///   rungs because there were two; rung 3 is gone and the step is the same
    ///   step with one fewer session to give.
    /// - **One window in flight**, rule 6's own remedy.
    ///
    /// Both are recoverable, which is the property that makes refusing better
    /// than absorbing: a region with no model routes down to rungs 0 and 1, or
    /// is left exactly as it was and listed in review with its reason. Nothing
    /// fails, and nothing is half-cleaned.
    fn under_pressure(&mut self) -> Option<&'static str> {
        // **Rung 3a's step, wired at last.** It could not
        // have one before: a run never spawned a sidecar and a region edit
        // killed its own child at the end of the click, so there was never a
        // moment when this process was holding one and a run was going. Now
        // that a child survives the edit that spawned it, there is - and the
        // first step down the ladder is the 4.6 GB one rather than the 510 MB
        // one.
        if registry::any_loaded(registry::Kind::Sidecar) {
            self.ladder.holding_sidecar();
        }
        self.take_pressure(memory::pressure())
    }

    /// [`Pipeline::under_pressure`] against a reading the caller supplies.
    ///
    /// A machine under memory pressure is not a state a test can arrange, and a
    /// ladder nothing has ever climbed is a ladder that works until the first
    /// time it is needed. So the reading is a parameter one call in, and
    /// `a_run_under_pressure_gives_back_the_inpainter_and_keeps_going` drives
    /// the whole pipeline through it on a real page.
    fn take_pressure(&mut self, reading: memory::Pressure) -> Option<&'static str> {
        let step = self.ladder.take(reading)?;
        if step == memory::Step::UnloadInpainter {
            // `surrender` is idempotent, and the session may never have been
            // opened at all.
            self.rung2.surrender();
            // And the parked copy with it. A ladder that gave back the session
            // this run was holding while leaving an idle one of the same kind
            // in the cache would be a ladder that freed nothing on the run that
            // never opened rung 2 - which is the run most likely to be the one
            // that hit the pressure.
            residency::evict(registry::Kind::Inpainter);
        }
        if step == memory::Step::RefuseSidecar {
            // Refusing the rung is only half of it: the child is already up and
            // holding gigabytes, and this step exists to get them back.
            residency::evict(registry::Kind::Sidecar);
        }
        Some(memory::step_key(step))
    }

    /// **Checks the weights and opens nothing.**
    ///
    /// The failure this used to catch by opening three sessions is "the model
    /// files are not on this machine", and that is a question about the
    /// filesystem - asking it by building a session cost a run 100 MB and
    /// several seconds before it had looked at a page. Every other failure a
    /// session can have is one the page reports, through
    /// [`OnDemand::get`], and it already
    /// leaves such a page queued rather than losing it.
    /// Give back every session nothing is using, at a point where dropping one
    /// is safe.
    ///
    /// **The same safe point [`Pipeline::under_pressure`] uses**, and for the
    /// same reason spelled out there: between regions the last region's buffers
    /// are gone and the next one's do not exist, so what is resident is the
    /// sessions. Two things put a session on this list, and
    /// [`cleaner_core::registry`] is what makes them one question:
    ///
    /// - the user pressed the close button on its row in the loaded-models tab;
    /// - nothing has used it for [`cleaner_core::registry::IDLE_GRACE`], which
    ///   in a run that is flowing never happens to the detectors and does
    ///   happen to rung 2 across a stretch of pages that never reach it.
    ///
    /// Nothing here is a latch. A page that needs the detector again opens it
    /// again, which is the honest cost of a close button on a model a run
    /// cannot proceed without.
    fn reap(&mut self) {
        self.detector.release_if_spent();
        self.balloons.release_if_spent();
        self.gate.release_if_spent();
        self.rung2.release_if_spent();
        // And what *nobody* is holding: a session parked by the last run or the
        // last region edit, whose grace has elapsed while this run was busy
        // with rungs it never needed.
        residency::sweep();
    }

    pub fn open(models: &Path, preference: Preference) -> Result<Pipeline, OpenError> {
        for name in REQUIRED_MODELS {
            let path = models.join(name);
            if !path.exists() {
                return Err(OpenError::MissingModel { path });
            }
        }
        Ok(Pipeline {
            detector: OnDemand::new(registry::Kind::TextDetector, models, preference, open_detector),
            balloons: OnDemand::new(
                registry::Kind::BalloonDetector,
                models,
                preference,
                open_balloons,
            ),
            gate: OnDemand::new(registry::Kind::ScriptGate, models, preference, open_gate),
            engine_version: env!("CARGO_PKG_VERSION"),
            rung2: Rung2::new(models, preference),
            ladder: memory::Ladder::new(),
            picks: None,
            outside: OutsideText::Review,
        })
    }

    /// The run's engine picks, which arrive with the command rather than with
    /// the model directory. Separate from [`Pipeline::open`] so that every
    /// other caller of it - `spikes/clean-page`, the tests - goes on routing by
    /// the fit alone rather than silently acquiring the interface's defaults.
    pub fn with_picks(mut self, picks: Picks) -> Pipeline {
        self.picks = Some(picks);
        self
    }

    /// The run's answer for text outside a balloon, from the same row of the
    /// tool window as the picks and separate from [`Pipeline::open`] for the
    /// same reason: a pipeline nobody asked reviews that text, as §3 defaults.
    pub fn with_outside(mut self, outside: OutsideText) -> Pipeline {
        self.outside = outside;
        self
    }
}

/* The three sessions a page needs before it can route anything. Free
 * functions rather than closures because `OnDemand` holds a function
 * pointer - see the type. */

/// A detector holder for a caller that is not a run.
///
/// [`crate::region`]'s bench wants exactly what a pipeline wants - the resident
/// session if there is one, a new one if there is not, and a hand-back at the
/// end - and the way to make a hand edit and a run share one session is for
/// both to ask the same type for it rather than for the bench to open its own.
pub(crate) fn detector_on_demand(models: &Path, preference: Preference) -> OnDemand<Detector> {
    OnDemand::new(registry::Kind::TextDetector, models, preference, open_detector)
}

fn open_detector(models: &Path, preference: Preference) -> Result<Detector, String> {
    Detector::open(&models.join(DETECTOR), preference).map_err(|e| e.to_string())
}

fn open_balloons(
    models: &Path,
    preference: Preference,
) -> Result<cleaner_core::balloon::BalloonDetector, String> {
    cleaner_core::balloon::BalloonDetector::open(&models.join(BALLOONS), preference)
        .map_err(|e| e.to_string())
}

/// The gate, and the rescue reader beside it **if this machine has one**.
///
/// The reader is 460 MB of optional download ([`OCR_MODELS`]), so its absence
/// is the ordinary case and must cost nothing: no error, no diagnostic, and a
/// gate that reaches exactly the verdicts it reached before this existed
/// ([`ScriptGate::with_ocr`] is what makes that true rather than a promise).
///
/// **A reader that is present but will not open is also not a failure.** The
/// three files could be truncated, or the provider could refuse the graph on
/// some machine nobody has tried; either way the region the reader would have
/// rescued goes to review, which is where it went before, and a page that
/// cleans is worth more than a page that refuses to start over a model nothing
/// requires. What is lost is 4% of regions, and it is lost in the recoverable
/// direction, which is the gate's whole asymmetry.
fn open_gate(models: &Path, preference: Preference) -> Result<ScriptGate, String> {
    let gate = ScriptGate::open(&models.join(GATE_MODEL), &models.join(GATE_LABELS), preference)
        .map_err(|e| e.to_string())?;
    if !OCR_MODELS.iter().all(|name| models.join(name).exists()) {
        return Ok(gate);
    }
    match cleaner_core::gate::Ocr::open(
        &models.join(OCR_ENCODER),
        &models.join(OCR_DECODER),
        &models.join(OCR_VOCAB),
        preference,
    ) {
        Ok(ocr) => Ok(gate.with_ocr(ocr)),
        Err(error) => {
            // Optional, so a corrupt download costs the rescue and not the
            // run; said on stderr so the loss is at least findable.
            eprintln!("manga-cleaner: the Japanese text reader could not be opened, cleaning without it: {error}");
            Ok(gate)
        }
    }
}

/// Build rung 2's session, and digest the weights it was built from.
///
/// The digest is taken **before** the session, so the 207 MB read and the
/// 510 MB session are not resident at the same moment. It is the only place a
/// run reads the weights itself, and it happens once.
fn open_inpainter(models: &Path, preference: Preference) -> Option<Held<lama::Inpainter>> {
    let model = models.join(INPAINTER);
    let model_sha256 = std::fs::read(&model).ok().map(|bytes| sha256_hex(&bytes));
    let inpainter = lama::Inpainter::open(&model, preference).ok()?;
    let provider = format!("{:?}", inpainter.selection().accelerator).to_lowercase();
    Some(Held { inpainter, provider, model_sha256 })
}

impl Cleaner for Pipeline {
    /// `spikes/clean-page`'s body, region for region, with the counting
    /// replaced by records the manifest can hold - and with rules 3 to 5
    /// around it.
    ///
    /// **Detection is per segment** (rule 3). A segment is a range of strip
    /// rows and this build's segments are cut at page boundaries, so a page is
    /// one segment unless rule 2 split it - which happens exactly when a source
    /// is a long strip in its own right. That case is not a nicety: an
    /// 800×12000 source letterboxed whole into the detector's 1024² input is
    /// scaled by k ≈ 11.7 and there is nothing left to detect. Cut into six
    /// segments it is k ≈ 2.8, which is an ordinary page.
    ///
    /// **Ownership is decided on the global list** (rule 3's last paragraph),
    /// never per segment, and the evidence for it is this segment's detections
    /// together with the previous segment's - the only other segment whose
    /// detection range can overlap this one's.
    ///
    /// **Every read is bounded by a decode window** (rule 4), which ends in
    /// [`Strip::clamp`] (rule 1). The window is what the candidate ladder grows
    /// inside, so the ring it is measured against is inside it too.
    ///
    /// **Detect small, measure large** (rule 5): the detector sees the 1024²
    /// letterbox of the crop, and the gate, the ring statistics, the fit and
    /// the fill all run on the crop's own pixels.
    fn clean_page(
        &mut self,
        page_id: &str,
        bytes: &[u8],
        ceiling: Engine,
        context: &PageContext,
    ) -> Result<PageOutcome, String> {
        let page = decode(bytes).map_err(|e| e.to_string())?;
        // A page begins by giving back whatever the last one finished with.
        // The first thing a run does is therefore to open what it needs and
        // nothing else, and a run that has been asked for a model back gets no
        // further than one page holding it.
        self.reap();
        let balloons =
            self.balloons.get()?.detect(&page).map_err(|e| e.to_string())?;

        // Page-level statistics, measured once and handed to every region.
        // **Per page and not per segment**, because the noise floor is one of
        // the two bounded exceptions to "no global operations" and it is
        // bounded at the page.
        let noise = fit::page_noise_sigma(&page);
        let source_sha256 = sha256_hex(bytes);
        // The run's provider, from the detector's session - which is opened
        // here rather than at the first segment because this is where the name
        // is needed, and the first segment wants it anyway.
        let provider =
            format!("{:?}", self.detector.get()?.selection().accelerator).to_lowercase();
        // Rule 4's engine-context term is a property of the rung that could
        // run, and the ceiling is what decides that.
        let engine_context = if rung(ceiling) >= rung(Engine::Lama) {
            EngineContext::Local
        } else {
            EngineContext::None
        };
        let cuts = context.cuts();

        let mut outcome = PageOutcome::default();
        let mut previous: Vec<DetectedInSegment> = Vec::new();
        let mut index = 0usize;

        for segment in context.page_segments() {
            let Some((top, bottom)) = strip::crop_rows(context.strip, context.placement, &segment)
            else {
                continue;
            };
            // The whole page is the common case and it is *the same object*
            // rather than a copy of itself: a page that is one segment costs
            // exactly what it cost before this rule was wired.
            let cropped;
            let crop: &Raster = if top == 0 && bottom == page.height {
                &page
            } else {
                cropped = page.rows(top, bottom);
                &cropped
            };

            let detection = self.detector.get()?.detect(crop).map_err(|e| e.to_string())?;
            let mut regions = build_regions_separated(detection.boxes.clone(), crop.width, crop.height, |a, b| {
                cleaner_core::balloon::merge_crosses_a_balloon(crop, &detection.segmentation, a, b)
            });
            // **The second source of boxes.** The balloon detector has already
            // run for this page, and outside a balloon it sees text the text
            // detector does not emit a box for - narration boxes, caption
            // lines, a chapter title strip. A box nothing already covers
            // becomes a region here, and the gate below asks it exactly what it
            // asks every other region.
            //
            // The balloon boxes are the **page's**, in page coordinates, and
            // everything here is the segment crop's. Rule 3 is what makes the
            // translation a filter rather than a clip: a segment's detection
            // range runs past the next segment's start, so a box straddling a
            // cut is whole in one of the two crops and is adopted there, by
            // whichever segment `owned_by` gives it to below. A box clipped to
            // this crop would be a different box.
            let in_crop: Vec<cleaner_core::balloon::BalloonBox> = balloons
                .iter()
                .filter(|b| b.rect.y >= top as i64 && b.rect.bottom() <= bottom as i64)
                .map(|b| cleaner_core::balloon::BalloonBox {
                    rect: Rect::new(b.rect.x, b.rect.y - top as i64, b.rect.w, b.rect.h),
                    ..*b
                })
                .collect();
            let median = cleaner_core::detect::median_box_area(&detection.boxes);
            let adopted = cleaner_core::balloon::adopt_uncovered_text(
                &regions,
                &in_crop,
                crop.width,
                crop.height,
                median,
            );
            regions.extend(adopted);
            // Sorted rather than appended, because `region_id` names a region
            // by its index in this list: appending would give the adopted
            // regions the last ids on the page instead of their own place in
            // reading order, and two runs have to agree on the order.
            cleaner_core::detect::sort_regions(&mut regions);
            let scale = detection.segmentation.proxy_scale();
            let edges = EdgeMap::sobel(crop);

            let found: Vec<DetectedInSegment> = regions
                .iter()
                .map(|region| {
                    DetectedInSegment::new(
                        segment.index,
                        context.to_strip(region.masking, top),
                        region.members.iter().map(|m| m.confidence).fold(0.0, f32::max),
                    )
                })
                .collect();
            let mut evidence = previous.clone();
            evidence.extend(found.iter().copied());
            let globals = merge_global(&evidence, &cuts, context.segments);

            for region in regions.iter() {
                let strip_rect = context.to_strip(region.masking, top);
                if !owned_by(&globals, strip_rect, segment.index) {
                    // Some other segment's work. A box in this segment's
                    // detection overlap is whole here *and* whole there, and
                    // rule 3's whole point is that exactly one of them cleans
                    // it.
                    continue;
                }
                let started = Instant::now();
                let id = region_id(page_id, index);
                let order = index as u32;
                index += 1;

                // Everything the manifest records is in **page** coordinates,
                // so a bbox reported to the seam means the same thing whether
                // the page was one segment or six.
                let on_page = Rect::new(
                    region.masking.x,
                    region.masking.y + top as i64,
                    region.masking.w,
                    region.masking.h,
                );

                let detected = cleaner_core::balloon::detected(on_page, &balloons);
                let inside = detected.inside();
                let verdict = self
                    .gate
                    .get()?
                    .judge(crop, &detection.segmentation, region, detected, self.outside)
                    .map_err(|e| e.to_string())?;
                if !verdict.cleans() {
                    if let Some(reason) = verdict.reason_key() {
                        outcome.regions.push(RegionOutcome::Untouched {
                            bbox: on_page,
                            reason: reason.to_owned(),
                        });
                    }
                    continue;
                }

                let seed = fit::seed_mask(&detection.segmentation, region, crop.width, crop.height);
                if seed.is_empty() {
                    // A box with no segmentation under it. Not a region the
                    // detector emitted a mask for, so there is nothing to
                    // record and nothing to leave alone.
                    continue;
                }

                // **Rule 4.** Every term native, and the clamp is rule 1's.
                let window = strip::decode_window(
                    context.strip,
                    context.joins,
                    strip_rect,
                    scale,
                    engine_context,
                );
                let bounds = context.to_crop(window.rect, top, crop);
                if bounds.w == 0 || bounds.h == 0 {
                    continue;
                }
                let fitted = fit::fit_within(crop, &seed, scale, noise, &edges, false, bounds);

                // **The pressure ladder, between regions.** The last region's
                // buffers are gone and this one's model tensors do not exist
                // yet, so a step taken here gives back a session rather than
                // interrupting an inference.
                if let Some(step) = self.under_pressure() {
                    outcome.pressure.push(step);
                }
                // And the voluntary half of the same moment: a session nobody
                // is using, or one somebody asked for back. See
                // [`Pipeline::reap`].
                self.reap();

                // Which kind of text this is, which is the same question the
                // gate asked above and the only thing the two picks are told
                // apart by.
                let pick = self.picks.map(|picks| picks.for_region(inside));

                let attempt = clean_region(
                    &mut self.rung2,
                    crop,
                    &fitted,
                    ceiling,
                    pick,
                    noise,
                )?;
                let (made, verdict) = match attempt {
                    Attempt::Declined(reason) => {
                        // §6: a declined region is **left exactly as it was**
                        // and listed in review with its reason. Not
                        // half-cleaned, and not cleaned by a rung that already
                        // failed - leaving text is recoverable and a bad fill
                        // is not.
                        outcome.regions.push(RegionOutcome::Untouched {
                            bbox: on_page,
                            reason: reason.to_owned(),
                        });
                        continue;
                    }
                    Attempt::Cleaned(made, verdict) => (*made, verdict),
                };
                let mask = lowered(made.mask, top as i64);
                let ink = lowered(made.ink, top as i64);

                // Rule 4's term is the window's, and rung 2 reports its own -
                // the tiles it ran are what actually met an edge. Either one
                // replicating is a replicated patch.
                let pad = if made.pad == strip::EdgePad::None { window.pad } else { made.pad };
                let mask_sha256 = mask_digest(&mask);
                outcome.regions.push(RegionOutcome::Cleaned(
                    Box::new(Patch {
                        id,
                        mask,
                        ink,
                        pixels: made.pixels,
                        order,
                        visible: true,
                        provenance: Provenance {
                            engine: made.engine,
                            engine_version: self.engine_version.into(),
                            model_sha256: made.model_sha256,
                            // A rung with a session of its own names where that
                            // session landed; rungs 0 and 1 have none, and the
                            // run's provider is the only one there is to give.
                            execution_provider: made
                                .provider
                                .unwrap_or_else(|| provider.clone()),
                            params_snapshot: params_snapshot(
                                made.engine,
                                &fitted,
                                started.elapsed(),
                                pad,
                                made.tiles,
                                &verdict,
                            ),
                            mask_sha256,
                            source_sha256: source_sha256.clone(),
                            cloud: None,
                            created: now(),
                        },
                    }),
                    if region.flagged_large {
                        Some("review.reason.unusuallyLarge".into())
                    } else {
                        None
                    },
                ));
            }
            // Rule 6's shape at the segment scale: the crop, its segmentation
            // and its edge map go here, and only the rectangles survive into
            // the next iteration.
            previous = found;
        }
        Ok(outcome)
    }
}

/* ------------------------------------------------------------------ */
/* Try, escalate, decline                                              */
/* ------------------------------------------------------------------ */

/// What the ladder answered for one region.
pub(crate) enum Attempt {
    /// A rung produced pixels and [`quality::assess`] did not decline them.
    Cleaned(Box<Made>, quality::Assessment),
    /// No rung the region could reach produced pixels the metric would accept.
    /// The key is the **last** refusal's, which is the highest rung's: that is
    /// the reason the region is untouched, where the rungs below it are only
    /// the reason it got that far.
    Declined(&'static str),
}

/// One rung's output, and everything about how it was made that the provenance
/// record wants.
pub(crate) struct Made {
    pub(crate) engine: Engine,
    pub(crate) mask: Mask,
    /// [`Patch::ink`]: the lettering, as the rung saw it. `Fitted::ink` for
    /// every rung - rung 2's hole is cut to exactly that - and the stroke
    /// itself for a hand tool.
    pub(crate) ink: Mask,
    pub(crate) pixels: Raster,
    /// The rung's own execution provider, where the rung has a session.
    pub(crate) provider: Option<String>,
    pub(crate) model_sha256: Option<String>,
    pub(crate) pad: strip::EdgePad,
    /// How many model runs the region cost. `None` below rung 2, where there
    /// are no tiles because there is no model - not zero, which would read as a
    /// model that ran nothing.
    pub(crate) tiles: Option<u32>,
}

/// What one rung answered, before it was scored.
pub(crate) enum Rendered {
    Made(Box<Made>),
    /// The rung refused the region before it ran, under its own key.
    Refused(&'static str),
}

/// Try the routed rung, score it, and escalate until something passes or
/// the ladder runs out.
///
/// The rule is one sentence and a loop: "a region is declined when **no
/// engine's output** passes the uniform quality metric". So each rung's
/// pixels are scored by
/// [`quality::assess`] - the same function for every rung, which is what
/// makes a review row saying *declined* mean the same thing whichever rung
/// produced them - and a declining rung hands the region to the next one
/// [`ladder_from`] permits rather than being written to disk.
///
/// **A decline is not a fault.** A rung that refuses before it runs - a box
/// past 2048 px, a mode a model cannot write into - is a decision with a key
/// of its own, and the escalation continues past it. A rung the model would
/// not *answer* is a fault, and it comes back as `Err`, which fails the page
/// and leaves it queued (the two are kept apart).
///
/// **An unmeasurable region is accepted, not declined.** [`quality`]'s own
/// argument: a declined region leaves text on the page, and the
/// justification for accepting that cost is an argument for declining when
/// the metric *fails*, not when it is absent.
///
/// **Where it starts is the user's**, through [`start_rung`]: `pick` is the
/// answer they gave for this kind of text, and `None` is the route's own rung.
/// It moves the start and nothing else - the list this loop walks, and every
/// rung above the start in it, are the same either way.
pub(crate) fn clean_region(
    rung2: &mut Rung2,
    crop: &Raster,
    fitted: &fit::Fitted,
    ceiling: Engine,
    pick: Option<EnginePick>,
    noise: f32,
) -> Result<Attempt, String> {
    let Some(start) = start_rung(fitted.route, ceiling, pick) else {
        return Ok(Attempt::Declined("decline.reason.rungUnavailable"));
    };
    // The answer if the ladder is empty, which is the same statement the
    // empty list makes: the rung this region needs is not one this run has.
    let mut refused = "decline.reason.rungUnavailable";

    for engine in ladder_from(start, ceiling, crop) {
        let made = match render_rung(rung2, engine, crop, fitted, noise)? {
            Rendered::Refused(key) => {
                refused = key;
                continue;
            }
            Rendered::Made(made) => made,
        };
        // The **applied** mask and the pixels that would ship, which is
        // exactly the pair `assess` asks for: the surround has to begin
        // outside everything the edit wrote.
        let verdict = quality::assess(crop, &made.mask, &made.pixels, noise);
        if let Some(cause) = verdict.cause() {
            refused = decline_key(cause);
            // Rule 6 at the rung scale: this rung's buffers go before the
            // next rung's are built, so escalating costs one patch and not
            // one per rung tried.
            drop(made);
            continue;
        }
        return Ok(Attempt::Cleaned(made, verdict));
    }
    Ok(Attempt::Declined(refused))
}

/// One rung, run once.
///
/// `crop` is the segment's own pixels rather than the decode window's, and
/// that is safe in this build for the reason [`PageContext::to_crop`] gives
/// from the other side: a crop never spans a join, so the window's clamp and
/// the crop's edge fall in the same place, and rung 2's "page edge" is rule
/// 4's page edge. A reader that could hold two pages across a verified join
/// would have to hand this the window itself.
pub(crate) fn render_rung(
    rung2: &mut Rung2,
    engine: Engine,
    crop: &Raster,
    fitted: &fit::Fitted,
    noise: f32,
) -> Result<Rendered, String> {
    let plain = |mask: Mask, pixels: Raster| {
        Rendered::Made(Box::new(Made {
            engine,
            mask,
            ink: fitted.ink.clone(),
            pixels,
            provider: None,
            // Arithmetic over the page's own pixels. There is no model, so
            // there is no digest - `None` is the true answer, not a missing
            // one.
            model_sha256: None,
            pad: strip::EdgePad::None,
            tiles: None,
        }))
    };
    match engine {
        // `FillAndDenoise` produces one patch, not two: the region's edit
        // is one thing to delete or re-run.
        Engine::Denoise => {
            let (mask, pixels) = denoise::render(crop, fitted, noise);
            Ok(plain(mask, pixels))
        }
        Engine::Lama => {
            let Some(held) = rung2.held() else {
                return Ok(Rendered::Refused("decline.reason.rungUnavailable"));
            };
            match held.inpainter.render(crop, fitted) {
                Ok(rendered) => Ok(Rendered::Made(Box::new(Made {
                    engine,
                    mask: rendered.mask,
                    ink: lama::model_hole(fitted),
                    pixels: rendered.pixels,
                    provider: Some(held.provider.clone()),
                    model_sha256: held.model_sha256.clone(),
                    pad: rendered.pad,
                    tiles: Some(rendered.tiles),
                }))),
                Err(lama::Error::Declined(declined)) => {
                    Ok(Rendered::Refused(declined.reason_key()))
                }
                Err(lama::Error::Run(fault)) => Err(fault),
            }
        }
        // Rung 3a and rung 4, neither of which an automatic run can reach.
        // `ladder_from` never names either, so this arm is unreachable - and
        // it is a **refusal** rather than a fill for exactly that reason. The
        // wildcard that used to stand here would have answered a `Flux` or a
        // `Cloud` with rung 0's pixels wearing rung 3a's or rung 4's name, and
        // a patch whose provenance says `flux` over a planar fill is a lie in
        // the manifest that no later reader could detect.
        Engine::Flux | Engine::Cloud => {
            Ok(Rendered::Refused("decline.reason.rungUnavailable"))
        }
        // **A painted stroke never arrives here, and if it does the answer is a
        // refusal rather than pixels.** `region::edit` intercepts paint and
        // clone before the fit runs, because neither has a mask to fit or a
        // ring to sample; this function is the ladder's renderer and has
        // nothing to render for either. Falling through to rung 0 would write a
        // planar fill wearing the word `paint` in its provenance, which is the
        // same lie in the manifest the `Flux`/`Cloud` arm above exists to
        // prevent.
        Engine::Paint | Engine::Clone => {
            Ok(Rendered::Refused("decline.reason.rungUnavailable"))
        }
        // Rung 0. A fill is the answer that cannot be wrong for a region that
        // got here.
        Engine::Fill => Ok(plain(fitted.mask.clone(), fill::render(crop, fitted))),
    }
}

/// The catalogue key a quality decline is recorded under.
///
/// **This belongs in [`quality::Cause::reason_key`]**, and that function's own
/// documentation says so: it answers one key for both causes because the
/// catalogue held one entry, and "splitting them costs one line on the day a
/// second entry exists". The catalogue now holds both entries, and the line is
/// owed there rather than here - the core names the key for the outcome the core
/// decided. It is written here for now because rung 2's two new modules are
/// being worked on concurrently, and a run that reported an edge-energy failure
/// and a histogram failure under one key would have a reviewer looking for
/// strokes that are not there.
pub(crate) fn decline_key(cause: quality::Cause) -> &'static str {
    match cause {
        quality::Cause::EdgeEnergy => "decline.reason.edgeEnergy",
        quality::Cause::Histogram => "decline.reason.histogram",
    }
}

/// Rule 3's ownership question, asked of the **global** list.
///
/// A detection is in exactly one global box, and that box has one owner however
/// many segments saw it. A rectangle no global box covers is a box
/// [`merge_global`] never received, which cannot happen for a detection that
/// was in its input - the fallback is "clean it here" because losing a region
/// is worse than cleaning it in the wrong segment.
fn owned_by(globals: &[GlobalBox], rect: Rect, segment: usize) -> bool {
    globals
        .iter()
        .find(|global| {
            global.rect.x <= rect.x
                && global.rect.y <= rect.y
                && global.rect.right() >= rect.right()
                && global.rect.bottom() >= rect.bottom()
        })
        .map(|global| global.owner == segment)
        .unwrap_or(true)
}

/// A mask fitted in a segment crop's coordinates, in the page's.
///
/// The patch's pixels need no move: [`fill::render`] and [`denoise::render`]
/// size their raster to the mask's bounds and the compositor places it there,
/// so the mask's rectangle is the patch's position.
fn lowered(mask: Mask, by: i64) -> Mask {
    Mask { bounds: Rect { y: mask.bounds.y + by, ..mask.bounds }, bits: mask.bits }
}

/// The provenance snapshot a run writes.
///
/// `fill_mode` and `elapsed_ms` were left *reconstructed* - the first guessed
/// from the rung, the second defaulted to zero - because nothing wrote them.
/// The run is what knows both, and §3 calls this field "the thresholds,
/// dilations and radii **actually used**", so both go in. No manifest format
/// change was needed: `params_snapshot` is an opaque object and
/// `library::fill_mode` already prefers a recorded value over a derived one.
pub(crate) fn params_snapshot(
    engine: Engine,
    fitted: &fit::Fitted,
    elapsed: Duration,
    pad: strip::EdgePad,
    tiles: Option<u32>,
    quality: &quality::Assessment,
) -> serde_json::Value {
    let mut snapshot = serde_json::json!({
        "rung": engine.rung_key(),
        "thickness": fitted.thickness,
        "deviation": fitted.ring.deviation,
        // Rungs 0 and 1 fit the paper around the mask; every rung above them
        // reconstructs what was under it. `library::fill_mode` derives the same
        // thing from the rung when the field is absent, and §3 asks for what was
        // actually used rather than for what can be inferred.
        "fill_mode": match engine {
            Engine::Fill | Engine::Denoise => "match-surround",
            _ => "reconstruct",
        },
        // Which of the fit's two masks this rung wrote through
        // ("The growth is rung 0's"). Rungs 0 and 1 write the whole grown
        // mask; every rung above them writes
        // `Fitted::ink`, the lettering and its first growth step. `thickness`
        // above is the fit's own answer either way, so without this field a
        // snapshot from a model rung reads as a 64 px edit when the edit was
        // 9 - and §3 asks for the radii *actually used*. The exact set is
        // `mask_sha256`; this is the word that says which one it is.
        "write_set": match engine {
            Engine::Fill | Engine::Denoise => "fitted",
            _ => "ink",
        },
        // The **region's** cost and not the accepted rung's: a region that was
        // tried at rung 0, declined, and cleaned at rung 2 cost all of it.
        "elapsed_ms": elapsed.as_millis() as u64,
        // A hand mask and an automatic mask are identical in kind, so this is
        // one word in the snapshot rather than a field.
        "source": "auto",
        // Record which edge pad was used in `params_snapshot`. `none` for an
        // interior window, which is most of them, and never `reflect` -
        // there is no such variant.
        "edge_pad": pad.as_str(),
        // Whether §6's metric actually measured this patch or had nothing to
        // measure it with: the failure to avoid is "a check that silently
        // answers accepted", and a snapshot that recorded only the accepted
        // verdicts would be it: on a bitonal source every rung passes by
        // construction, and the record should say which of the two happened.
        "quality": match quality.outcome {
            quality::Outcome::Unmeasured(_) => "unmeasured",
            _ => "measured",
        },
    });
    // Only where there is a model to have run tiles. Rungs 0 and 1 would report
    // zero, and zero reads as a model that did nothing.
    if let Some(tiles) = tiles {
        snapshot["tiles"] = tiles.into();
    }
    snapshot
}

/// `{page id}-r{n}`. Unique across the chapter, because the manifest keeps one
/// patch record per id and `library::region_of_patch` hands the id straight to
/// the seam as the region's own.
fn region_id(page_id: &str, index: usize) -> String {
    format!("{page_id}-r{index}")
}

/// The digest of the mask a patch was applied with.
///
/// Over the encoded buffer, which is what actually lands in the sidecar
/// directory, so the recorded digest and the file on disk are the same bytes.
pub(crate) fn mask_digest(mask: &Mask) -> String {
    sha256_hex(&buffers::encode_mask(mask))
}

pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0)
}

/* ------------------------------------------------------------------ */
/* One writer per job                                                  */
/* ------------------------------------------------------------------ */

/// The job manifests currently held for writing, and the wait on them.
fn held_jobs() -> &'static (Mutex<HashSet<PathBuf>>, Condvar) {
    static HELD: OnceLock<(Mutex<HashSet<PathBuf>>, Condvar)> = OnceLock::new();
    HELD.get_or_init(|| (Mutex::new(HashSet::new()), Condvar::new()))
}

/// A job held for writing. Releases it when it goes.
pub struct JobLock {
    path: PathBuf,
}

impl Drop for JobLock {
    fn drop(&mut self) {
        let (held, waiting) = held_jobs();
        let mut held = held.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        held.remove(&self.path);
        // Every waiter is woken because they are waiting on *different* paths
        // through one condition variable, and the one this wakes may not be the
        // one this frees.
        waiting.notify_all();
    }
}

/// Take a job for writing, waiting for whoever has it.
///
/// **What this is, exactly.** A manifest is rewritten whole, so every write
/// to one is a read-modify-write over the entire file, and two of them that
/// interleave lose
/// the earlier one entirely. Two things made that worth closing now rather than
/// later: the run holds one [`Job`] in memory for a **whole chapter** and
/// rewrites the manifest after every region, so the window it is exposed for
/// went from milliseconds to the length of a run; and the run is the first
/// writer that is not on the command thread, so "two commands cannot overlap"
/// stopped being true by construction.
///
/// **What this is not.** It is a lock *within one process*. Two copies of the
/// application over one library still race, and the loser's completed regions
/// are still lost - that is the single-instance lock, and it stays Phase 6
/// because it needs a lock file with an owner, a staleness rule and a way to
/// tell the user. Nothing here pretends to it.
///
/// **On the key.** The path as the caller spells it. Every caller in this crate
/// builds one through [`Library::job_path`], so they agree; `canonicalize`
/// would not help, because a job is locked before it is created.
pub fn lock_job(path: &Path) -> JobLock {
    let (held, waiting) = held_jobs();
    let mut set = held.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    while set.contains(path) {
        set = waiting.wait(set).unwrap_or_else(|poisoned| poisoned.into_inner());
    }
    set.insert(path.to_path_buf());
    JobLock { path: path.to_path_buf() }
}

/* ------------------------------------------------------------------ */
/* The queue                                                           */
/* ------------------------------------------------------------------ */

/// One page of a run: which chapter it belongs to, which job holds it, and
/// where it sits in that job's `strip.order`.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanEntry {
    pub chapter_id: String,
    pub job_path: PathBuf,
    pub page_index: usize,
    pub source_idx: usize,
}

/* ------------------------------------------------------------------ */
/* The strip, surveyed once per job                                    */
/* ------------------------------------------------------------------ */

/// A job's strip and everything one walk over its pages could establish.
///
/// One of these per job per run, built when the job is opened and handed to
/// every page as a [`PageContext`].
pub struct JobStrip {
    pub strip: Strip,
    pub survey: Survey,
}

/// The manifest's `sources` and `strip.order`, as rule 1's coordinate system.
///
/// A position in `order` that names no source keeps its place as a zero-sized
/// page rather than being dropped, so a placement index is a position in
/// `strip.order` - which is what [`PlanEntry::page_index`] is, and what every
/// event on the seam carries.
pub fn strip_of(project: &Project) -> Strip {
    let sizes: Vec<(u32, u32)> = project
        .strip
        .order
        .iter()
        .map(|source| project.sources.get(*source).map(|s| (s.w, s.h)).unwrap_or((0, 0)))
        .collect();
    Strip::of_sizes(&sizes)
}

/// Whether this job's pages have to be read before they are cleaned.
///
/// The survey costs one decode of every page, so it is taken where it buys
/// something and not otherwise:
///
/// - a **longstrip** project, because that is where the join check applies
///   and where rule 1 lets a read cross a join at all;
/// - a project any of whose **sources is itself a long strip**, because rule 2
///   has to split it and rule 3 has to segment it, and an unsurveyed profile
///   would fall back to a hard cut every 2000 px on evidence nothing gathered.
///
/// A chapter of ordinary pages is neither, and pays nothing: its pages are its
/// segments, which is rule 2's own sentence for the case its trigger does not
/// fire.
fn survey_is_worth_it(project: &Project) -> bool {
    project.strip.mode == StripMode::Longstrip
        || project
            .sources
            .iter()
            .any(|s| s.h > 0 && (s.w as f32 / s.h as f32) < LONGSTRIP_ASPECT)
}

/// Survey a job, report the joins that failed, and record the split plan.
///
/// **This report satisfies one half of what's needed.** `check_join` and
/// `Joins::anomalies` had no caller outside the tests, so "report them" was
/// unmet for four of four variants - including the one the catalogue can say.
/// It is met here for that one. The other three still have no sentence to be
/// reported in and `JoinAnomaly::reason_key` still answers `None` for them,
/// which is the remaining half - a vocabulary decision the core may not take.
/// What is *not* silent either way is the behaviour: an anomalous join is not
/// verified, so no read crosses it, notice or no notice.
///
/// The plan is written to `strip.splits` only for a `Longstrip` project,
/// because [`cleaner_core::project::Strip::splits`] is defined as "always empty
/// for `Single`, where the page boundaries are the splits" - and on a `Single`
/// project every split rule 2 finds *is* a page boundary, so the field would
/// say nothing the order does not.
fn survey_job(job: &mut Job, emit: &dyn Fn(Event)) -> JobStrip {
    let strip = strip_of(&job.project);
    let longstrip = job.project.strip.mode == StripMode::Longstrip;

    let survey = if survey_is_worth_it(&job.project) {
        let sources: Vec<Option<PathBuf>> = (0..strip.pages().len())
            .map(|page| {
                library::Library::resolve_page(&job.project, page)
                    .and_then(|source| job.source_path(source))
            })
            .collect();
        strip::survey(&strip, |placement| {
            let path = sources.get(placement)?.as_ref()?;
            let bytes = std::fs::read(path).ok()?;
            decode(&bytes).ok()
        })
    } else {
        // The joins stay unchecked - which rule 1 reads as a page edge, and no
        // read crosses one - and the profile stays empty, which rule 2 only
        // ever consults for a strip its trigger fires on.
        strip::survey(&strip, |_| None)
    };

    if longstrip {
        for (join, anomaly) in survey.anomalies() {
            if let Some(key) = anomaly.reason_key() {
                emit(events::notice_event(
                    key,
                    serde_json::json!({ "first": join + 1, "second": join + 2 }),
                    "warn",
                ));
            }
        }
        if job.project.strip.splits != survey.splits {
            job.project.strip.splits = survey.splits.clone();
            let _ = job.flush();
        }
    }

    JobStrip { strip, survey }
}

/// Whether a page is work a run has left to do.
///
/// **Examined, and nothing else.** Having a patch on it is not the test, and
/// that is the whole point of the field: a page the run got three regions into
/// before it was cancelled has patches and is not finished, and a page with no
/// text on it has none and is. Reading "has patches" as "done" would strand the
/// first case - cancel keeps the completed regions and the resume must still
/// redo the page - and reading "has no patches" as "to do" would requeue the
/// second one on every run for ever.
fn runnable(project: &Project, source_idx: usize) -> bool {
    !project.examined.contains(&source_idx)
}

/// Drop the automatic outcomes a previous pass left on this page.
///
/// `Job::complete_region` replaces a patch record with the same id, so re-running
/// a page cannot double its patches - but `Job::leave_untouched` appends, and a
/// page redone after a cancel would collect a second copy of every region the
/// gate held back. So a page starts from a clean slate for the work a run owns:
/// this source's untouched rows, and the patches whose ids this run issued.
///
/// Nothing else is touched. A hand-drawn region carries an id a run never
/// mints, and a hand mask and an automatic mask are identical in kind -
/// identical in kind, not in provenance, and a run has no business deleting
/// an edit a person made.
///
/// **The counters come back down with the rows.** `counters` is the basis
/// of honest statistics, and a counter that only ever goes up is not one:
/// dropping a page's `regions_untouched` rows while leaving `gate_dropped`
/// where it was meant that every cancel-and-resume added the same regions
/// again, for ever, and a page that failed added to `errored` on every
/// attempt. So each row's contribution is subtracted as the row goes, by the
/// same test that put it there - the counters state what the manifest
/// currently holds, and a resumed run ends on the same numbers an
/// uninterrupted one does.
fn reset_page(job: &mut Job, source_idx: usize, page_id: &str) {
    let prefix = format!("{page_id}-r");
    let before = (job.project.patches.len(), job.project.regions_untouched.len());
    job.project
        .patches
        .retain(|record| record.source_idx != source_idx || !record.id.starts_with(&prefix));

    let (mut gate_dropped, mut declined) = (0u32, 0u32);
    job.project.regions_untouched.retain(|record| {
        if record.source_idx != source_idx {
            return true;
        }
        // The same classification `execute` counted it under, so the two
        // cannot drift into counting one thing up and another down.
        if is_gate_skip(&record.reason) {
            gate_dropped += 1;
        } else {
            declined += 1;
        }
        false
    });
    let counters = &mut job.project.counters;
    counters.gate_dropped = counters.gate_dropped.saturating_sub(gate_dropped);
    counters.declined = counters.declined.saturating_sub(declined);

    let unerrored = job.clear_errored(source_idx);
    if unerrored || before != (job.project.patches.len(), job.project.regions_untouched.len()) {
        let _ = job.flush();
    }
}

/// Whether an untouched region's reason is the gate holding it back, rather
/// than a route nothing would run. One test, used where a region is counted and
/// again where it is uncounted.
fn is_gate_skip(reason: &str) -> bool {
    reason.contains("gateSkipped")
}

/// The queue a scope asks for, in order.
///
/// `'project'` walks every chapter of the project the chapter belongs to;
/// `'chapter'` walks one; `'page'` walks the one page. A chapter with no job on
/// disk contributes nothing rather than failing the call - it is a chapter
/// nobody has put pages in, which is a listing state, not an error.
pub fn plan(
    library: &Library,
    scope: &str,
    chapter_id: &str,
    page_index: Option<u32>,
) -> Result<Vec<PlanEntry>, LibraryError> {
    let index = library.index()?;
    let Some(project) =
        index.projects.iter().find(|p| p.chapters.iter().any(|c| c.id == chapter_id))
    else {
        return Err(LibraryError::Unknown { id: chapter_id.to_owned() });
    };

    let chapters: Vec<&str> = if scope == "project" {
        project.chapters.iter().map(|c| c.id.as_str()).collect()
    } else {
        vec![chapter_id]
    };

    let mut entries = Vec::new();
    for id in chapters {
        let job_path = library.job_path(&project.id, id);
        let Ok(job) = Job::open(&job_path) else { continue };
        for page in 0..job.project.strip.order.len() {
            if scope == "page" && Some(page as u32) != page_index {
                continue;
            }
            let Some(source_idx) = Library::resolve_page(&job.project, page) else { continue };
            if runnable(&job.project, source_idx) {
                entries.push(PlanEntry {
                    chapter_id: id.to_owned(),
                    job_path: job_path.clone(),
                    page_index: page,
                    source_idx,
                });
            }
        }
    }
    Ok(entries)
}

/* ------------------------------------------------------------------ */
/* The scheduler                                                       */
/* ------------------------------------------------------------------ */

/// What [`execute`] answers with. The event stream is the product; this is what
/// the caller needs to update the manifest's interrupted marker and to notice.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub reason: &'static str,
    pub pages_queued: u32,
    pub pages_cleaned: u32,
    pub regions_cleaned: u32,
    pub next_page_index: Option<u32>,
    /// Pages whose clean failed outright. Counted into the manifest's
    /// `counters.errored` and reported nowhere else - see the note on the
    /// missing catalogue key in [`run_clean`].
    pub errored: u32,
}

/// Run a queue, emitting as it goes.
///
/// The whole ordering contract lives in this function, and each rule is one
/// line of it:
///
/// - `page-started` goes out before any of that page's regions, and
///   `page-done` after all of them, so **every `region-done` falls strictly
///   between them**.
/// - `run-finished` is emitted **exactly once**, on the single exit, whatever
///   happened. There is no early return.
/// - `next_page_index` is `Some` **only** when the run was cancelled, and it is
///   the page the run stopped *at* rather than the last one it finished - that
///   page is unfinished, and a resume must redo it.
/// - a cancel that lands mid-page emits **no** `page-done` for that page, and
///   does not mark it examined, so the page is still queued afterwards. The
///   regions already recorded stay recorded: cancel keeps completed regions.
///
/// "Whatever happened" includes a **panic**, and that is what the
/// [`catch_unwind`](std::panic::catch_unwind) here is for. A panic in the
/// pipeline - a model that produced a shape nothing expected, an arithmetic
/// overflow in a fit - used to unwind straight past this function's single
/// exit, and the consequences were all of the seam's: no `run-finished` ever,
/// so the page kept its `●` and the run never ended; the active slot never
/// cleared, so every later `runClean` answered `alreadyRunning` for the rest of
/// the session; and `cancelRun` waiting the full [`CANCEL_WAIT`] before naming a
/// run that had already stopped. A panicked run reports itself as **cancelled**,
/// which is the true half of what happened and the only other word `reason`
/// can give: the run stopped early, every completed region is kept, and the
/// page it stopped inside is the resume point. What it cannot say is that it
/// *failed* - the catalogue has no key for it.
pub fn execute(
    run_id: &str,
    chapter_id: &str,
    entries: &[PlanEntry],
    cleaner: &mut dyn Cleaner,
    ceiling: Engine,
    cancel: &AtomicBool,
    emit: &dyn Fn(Event),
) -> Summary {
    let mut progress = Progress {
        summary: Summary {
            reason: "completed",
            pages_queued: entries.len() as u32,
            pages_cleaned: 0,
            regions_cleaned: 0,
            next_page_index: None,
            errored: 0,
        },
        in_flight: None,
    };

    let walked = std::panic::catch_unwind(AssertUnwindSafe(|| {
        walk(run_id, entries, cleaner, ceiling, cancel, emit, &mut progress);
    }));

    let mut summary = progress.summary;
    if walked.is_err() {
        summary.reason = "cancelled";
        // The walk's own `close` never ran, so the job on disk does not know it
        // was interrupted and Home would offer no resume for a run that stopped
        // half way through a chapter. The page in flight is where one picks up:
        // it was not marked examined, so it is queued either way, and this is
        // what puts the offer in front of the user.
        if let Some((job_path, page_index)) = progress.in_flight {
            summary.next_page_index = Some(page_index);
            let _lock = lock_job(&job_path);
            if let Ok(mut job) = Job::open(&job_path) {
                let _ = job.mark_interrupted(Some(page_index));
            }
        }
    }

    emit(Event::RunFinished {
        run_id: run_id.to_owned(),
        chapter_id: chapter_id.to_owned(),
        reason: summary.reason,
        pages_queued: summary.pages_queued,
        pages_cleaned: summary.pages_cleaned,
        regions_cleaned: summary.regions_cleaned,
        next_page_index: summary.next_page_index,
    });
    summary
}

/// What the walk has done, and where it is.
///
/// One struct rather than seven arguments, and the second field is the reason
/// it exists at all: a run that does not come back has to leave behind the page
/// it was inside, or nothing can say where to resume.
struct Progress {
    summary: Summary,
    /// The job and the `strip.order` position the walk is inside, or `None`
    /// between pages.
    in_flight: Option<(PathBuf, u32)>,
}

/// The queue itself. Every ordering rule [`execute`] documents is here; the one
/// rule that is not - exactly one `run-finished` - is there, because it has to
/// hold for the exits this function does not take.
fn walk(
    run_id: &str,
    entries: &[PlanEntry],
    cleaner: &mut dyn Cleaner,
    ceiling: Engine,
    cancel: &AtomicBool,
    emit: &dyn Fn(Event),
    progress: &mut Progress,
) {
    let summary = &mut progress.summary;
    let mut open: Option<(PathBuf, JobLock, Job, JobStrip)> = None;

    for entry in entries {
        if cancel.load(Ordering::SeqCst) {
            summary.next_page_index = Some(entry.page_index as u32);
            break;
        }

        // One job open at a time. A `'project'` run walks chapters in order, so
        // this reopens once per chapter rather than once per page. The lock is
        // taken with the job and held for as long as it is open, because that
        // whole span is one read-modify-write over the manifest.
        //
        // The strip is surveyed here, once per job, for the same reason: rules
        // 1 to 3 are properties of the chapter and not of the page, and a
        // survey per page would decode the chapter once per page.
        if open.as_ref().is_none_or(|(path, _, _, _)| path != &entry.job_path) {
            if let Some((_, _lock, job, _)) = open.take() {
                close(job, None);
            }
            let lock = lock_job(&entry.job_path);
            match Job::open(&entry.job_path) {
                Ok(mut job) => {
                    let geometry = survey_job(&mut job, emit);
                    open = Some((entry.job_path.clone(), lock, job, geometry));
                }
                Err(_) => {
                    summary.errored += 1;
                    continue;
                }
            }
        }
        let Some((_, _, job, geometry)) = open.as_mut() else { continue };

        let page_id = library::page_id(&entry.chapter_id, entry.page_index);
        reset_page(job, entry.source_idx, &page_id);
        progress.in_flight = Some((entry.job_path.clone(), entry.page_index as u32));
        emit(Event::PageStarted {
            run_id: run_id.to_owned(),
            chapter_id: entry.chapter_id.clone(),
            page_id: page_id.clone(),
            page_index: entry.page_index as u32,
        });

        let bytes = job
            .source_path(entry.source_idx)
            .ok_or_else(|| "no such source".to_owned())
            .and_then(|path| std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display())));
        let context = PageContext {
            strip: &geometry.strip,
            joins: &geometry.survey.joins,
            segments: &geometry.survey.segments,
            placement: entry.page_index,
        };
        let outcome = match bytes
            .and_then(|bytes| cleaner.clean_page(&page_id, &bytes, ceiling, &context))
        {
            Ok(outcome) => outcome,
            Err(_) => {
                fail_page(emit, run_id, entry, &page_id, job, summary);
                progress.in_flight = None;
                continue;
            }
        };

        // The pressure ladder is reported before the regions it changed the
        // outcome of are: a run that quietly stopped
        // using the inpainter looks like a run whose pages got easier.
        for step in &outcome.pressure {
            emit(events::notice_event(step, serde_json::json!({}), "warn"));
        }

        let mut cancelled_here = false;
        let mut failed_here = false;
        for region in outcome.regions {
            if cancel.load(Ordering::SeqCst) {
                cancelled_here = true;
                break;
            }
            match region {
                RegionOutcome::Cleaned(patch, review_state) => {
                    let mut patch = *patch;
                    patch.provenance.mask_sha256 = mask_digest(&patch.mask);
                    let record = PatchRecord::of(entry.source_idx, &patch, review_state.clone());
                    if job.complete_region(entry.source_idx, &patch, review_state).is_err() {
                        // The region's pixels or the manifest could not be
                        // written - a full disk is the shape this takes, and on
                        // one every region of every page fails. Swallowing it
                        // lost the region for good: the page was marked
                        // examined regardless, so no later run would ever
                        // re-queue it, and a chapter every region of which had
                        // failed reported `noTextDetected` - the field saying the
                        // opposite of what happened. It is the same failure as
                        // a page that could not be read, so it takes the same
                        // path: counted, **not** examined, and
                        // left in the queue with what it managed to write.
                        failed_here = true;
                        break;
                    }
                    summary.regions_cleaned += 1;
                    let page = page_or_stub(entry, &page_id, job);
                    emit(Event::RegionDone {
                        run_id: run_id.to_owned(),
                        chapter_id: entry.chapter_id.clone(),
                        page_id: page_id.clone(),
                        page_index: entry.page_index as u32,
                        region: Box::new(library::region_of_patch(&record, &page)),
                    });
                }
                RegionOutcome::Untouched { bbox, reason } => {
                    if is_gate_skip(&reason) {
                        job.project.counters.gate_dropped += 1;
                    } else {
                        job.project.counters.declined += 1;
                    }
                    let _ = job.leave_untouched(entry.source_idx, bbox, &reason);
                }
            }
        }

        if cancelled_here {
            summary.next_page_index = Some(entry.page_index as u32);
            break;
        }
        if failed_here {
            fail_page(emit, run_id, entry, &page_id, job, summary);
            progress.in_flight = None;
            continue;
        }

        let _ = job.mark_examined(entry.source_idx);
        emit_page_done(emit, run_id, entry, &page_id, job);
        summary.pages_cleaned += 1;
        progress.in_flight = None;
    }

    if summary.next_page_index.is_some() {
        summary.reason = "cancelled";
    }
    if let Some((_, _lock, job, _)) = open.take() {
        close(job, summary.next_page_index);
    }
}

/// A page the run could not finish: it is counted, it is **not** marked
/// examined - so a later run tries it again - and `page-done` still goes out, or
/// the interface would leave a `●` on it for the rest of the session.
///
/// The counter is moved by [`Job::mark_errored`] rather than by hand, because
/// the page will be attempted again and a counter incremented per attempt
/// climbs for ever over one corrupt file.
fn fail_page(
    emit: &dyn Fn(Event),
    run_id: &str,
    entry: &PlanEntry,
    page_id: &str,
    job: &mut Job,
    summary: &mut Summary,
) {
    summary.errored += 1;
    let _ = job.mark_errored(entry.source_idx);
    emit_page_done(emit, run_id, entry, page_id, job);
}

fn emit_page_done(
    emit: &dyn Fn(Event),
    run_id: &str,
    entry: &PlanEntry,
    page_id: &str,
    job: &Job,
) {
    emit(Event::PageDone {
        run_id: run_id.to_owned(),
        chapter_id: entry.chapter_id.clone(),
        page_id: page_id.to_owned(),
        page_index: entry.page_index as u32,
        page: Box::new(page_or_stub(entry, page_id, job)),
    });
}

/// The page a run's events carry, or the little that is known about it.
///
/// [`library::page_of`] answers `None` when the manifest cannot describe the
/// page - a `strip.order` position with no source behind it, which is a
/// manifest another writer has moved under the run. **Emitting nothing was the
/// wrong answer to that**, and in two ways the interface feels: a
/// `page-started` with no matching `page-done` leaves the page marked as
/// cleaning for the rest of the session, and a dropped `region-done` after
/// `regions_cleaned` had already been incremented let `run-finished` claim more
/// regions than the events ever reported. So the event goes out with what the
/// run itself knows - the page's id, its position, and no regions, which is
/// exactly the claim "the manifest cannot describe this page" amounts to.
fn page_or_stub(entry: &PlanEntry, page_id: &str, job: &Job) -> ApiPage {
    library::page_of(&entry.chapter_id, &job.project, entry.page_index).unwrap_or(ApiPage {
        id: page_id.to_owned(),
        chapter_id: entry.chapter_id.clone(),
        index: entry.page_index as u32,
        number: entry.page_index as u32 + 1,
        file: String::new(),
        source_sha: String::new(),
        width: 0,
        height: 0,
        status: "unclean",
        skip_reason: None,
        regions: Vec::new(),
        // A stub is a page the manifest cannot describe, so it has nothing to
        // count and nothing was loaded - `resident: true` with an empty list is
        // the honest reading of "no regions", and it is what stops the
        // interface asking for a window it will never get an answer to.
        region_count: 0,
        done_count: 0,
        review_count: 0,
        resident: true,
    })
}

/// Record where a job stopped, or that it did not stop.
///
/// `mark_interrupted(None)` is not a no-op and has to happen: a job that
/// records being interrupted with no way to say it no longer is would offer a
/// resume for ever.
fn close(mut job: Job, interrupted_at: Option<u32>) {
    let _ = job.mark_interrupted(interrupted_at);
}

/* ------------------------------------------------------------------ */
/* The active run                                                      */
/* ------------------------------------------------------------------ */

/// The one run in flight, if there is one.
///
/// One at a time, which is the mock's behaviour and the interface's assumption:
/// `runClean` answers `alreadyRunning` rather than starting a second, and
/// `editor.run` holds exactly one run id.
struct Active {
    run_id: String,
    cancel: Arc<AtomicBool>,
    /// Raised when the worker has emitted `run-finished` and cleared itself.
    /// `cancelRun` waits on it, so that - as in the mock - the call does not
    /// answer before the event the caller is about to look for has gone out.
    finished: Arc<(Mutex<bool>, Condvar)>,
    /// The `reason` the run's `run-finished` carried, once it has gone out.
    ///
    /// `finished` says the worker has let go; this says what it said on the way
    /// out, and they are not the same question. `cancelRun` needs the second
    /// one: between `execute` returning and the slot being cleared there is a
    /// window in which the run is over, `run-finished` has already gone out
    /// saying `completed`, and the slot still holds it - and a cancel that
    /// arrived in that window used to set the flag on a dead run, wait, and
    /// then name it as cancelled to a caller whose event stream says otherwise.
    outcome: Arc<Mutex<Option<&'static str>>>,
}

fn active() -> &'static Mutex<Option<Active>> {
    static ACTIVE: OnceLock<Mutex<Option<Active>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(None))
}

/// `backend-run-1`, `backend-run-2`, ….
///
/// **The prefix is load-bearing.** `subscribe` is a merge of this stream and
/// the mock's, and the mock mints `run-1`,
/// `run-2`, … from a counter of its own - `applyTool({tool: 'autoClean'})` is
/// still the mock's and starts a real mock run, so two schedulers can be live at
/// once on one stream while `editor.run` holds exactly one run id. "A run
/// belongs entirely to one implementation" is true and does not make the two
/// counters disjoint; a prefix does.
fn next_run_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!("backend-run-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

/// How long `cancelRun` will wait for the worker to stop.
///
/// A bound rather than a promise: the worker checks for cancellation between
/// regions, and one region of one page is the longest it can be from noticing.
/// If that were ever to exceed this, answering late is better than never
/// answering - the run id is still correct and `run-finished` still arrives on
/// the channel.
const CANCEL_WAIT: Duration = Duration::from_secs(120);

/* ------------------------------------------------------------------ */
/* The seam's shapes                                                   */
/* ------------------------------------------------------------------ */

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedPage {
    pub chapter_id: String,
    pub page_id: String,
    pub page_index: u32,
}

/// `runClean` resolves with the **queue**, not the result.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunHandle {
    pub run_id: Option<String>,
    pub pages: Vec<QueuedPage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub already_running: Option<bool>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumedJob {
    pub project: ApiProject,
    pub chapter: ApiChapter,
    pub resumed_from: u32,
    pub run_id: Option<String>,
    pub pages: Vec<QueuedPage>,
}

fn queued(entries: &[PlanEntry]) -> Vec<QueuedPage> {
    entries
        .iter()
        .map(|entry| QueuedPage {
            chapter_id: entry.chapter_id.clone(),
            page_id: library::page_id(&entry.chapter_id, entry.page_index),
            page_index: entry.page_index as u32,
        })
        .collect()
}

/* ------------------------------------------------------------------ */
/* Starting one                                                        */
/* ------------------------------------------------------------------ */

/// Build the queue, claim the active slot, and put the run on a thread.
///
/// A thread of its own rather than `spawn_blocking`: a run over a 200-page
/// chapter occupies its worker for minutes, and the blocking pool is what every
/// other command in this crate resolves on ([`crate::library::blocking`]).
pub(crate) fn start(
    app: &tauri::AppHandle,
    scope: &str,
    chapter_id: &str,
    page_index: Option<u32>,
    engine_ceiling: Option<String>,
    picks: Picks,
    outside: OutsideText,
) -> Result<RunHandle, String> {
    {
        let guard = active().lock().map_err(|e| e.to_string())?;
        if let Some(run) = guard.as_ref() {
            return Ok(RunHandle {
                run_id: Some(run.run_id.clone()),
                pages: Vec::new(),
                already_running: Some(true),
            });
        }
    }

    let library = Library::for_app(app).map_err(|e| e.to_string())?;
    let entries = plan(&library, scope, chapter_id, page_index).map_err(|e| e.to_string())?;
    if entries.is_empty() {
        events::notice("notice.run.nothingInScope", serde_json::json!({}), "warn");
        return Ok(RunHandle { run_id: None, pages: Vec::new(), already_running: None });
    }

    // The ONNX Runtime is downloaded after install, so "is it there" is a
    // question with an answer that changes over a session's life. The four
    // outcomes are told apart because their remedies are, and the catalogue
    // already has a key for each - `diagnostics::onnx_runtime` reports the same
    // four to the About window.
    let app_data = {
        use tauri::Manager;
        app.path().app_data_dir().ok()
    };
    if let Err(error) = cleaner_core::runtime::find(app_data.as_deref())
        .and_then(|path| cleaner_core::runtime::load(&path))
    {
        use cleaner_core::runtime::LoadError;
        events::notice(
            match error {
                LoadError::NotFound { .. } => "diagnostics.runtime.missing",
                LoadError::Quarantined { .. } => "diagnostics.runtime.quarantined",
                LoadError::Refused { .. } => "diagnostics.runtime.refused",
                _ => "diagnostics.runtime.unloadable",
            },
            serde_json::json!({}),
            "warn",
        );
        return Ok(RunHandle { run_id: None, pages: Vec::new(), already_running: None });
    }

    // The weights, and the one failure this reports as a notice rather than as
    // an error. A run that cannot start for want of the model files used to
    // answer `Err`, and the interface had nowhere to put it - the doc comment
    // on `run_clean` said so. Now that Settings › Models can *fetch* them there
    // is somewhere for the user to go, so the gap is closed the same way the
    // missing-runtime gap already was: a warning that names the remedy, and an
    // empty handle rather than a rejected promise.
    let Some(models) = model_dir(app_data.as_deref()) else {
        events::notice("notice.run.modelsMissing", serde_json::json!({}), "warn");
        return Ok(RunHandle { run_id: None, pages: Vec::new(), already_running: None });
    };
    // Read before the pipeline is opened, because the pipeline's sessions are
    // built here and the accelerator is a property of a session rather than of
    // a run. A setting read afterwards would take effect one run late.
    let settings = crate::settings::read(app).unwrap_or(serde_json::Value::Null);
    // The same notice as the `model_dir` miss above, and it is reached when a
    // required file went away between that check and this one. Matched on the
    // variant rather than on the message: a second failure mode added
    // to `OpenError` will fail to compile here rather than falling into the
    // notice or out of it by the shape of its wording.
    let pipeline = match Pipeline::open(&models, preference_from(&settings)) {
        Ok(pipeline) => pipeline.with_picks(picks).with_outside(outside),
        Err(OpenError::MissingModel { .. }) => {
            events::notice("notice.run.modelsMissing", serde_json::json!({}), "warn");
            return Ok(RunHandle { run_id: None, pages: Vec::new(), already_running: None });
        }
    };

    let stored =
        settings.get("engineCeiling").and_then(|value| value.as_str()).map(|s| s.to_owned());
    let ceiling = effective_ceiling(engine_ceiling.as_deref(), stored.as_deref());

    let run_id = next_run_id();
    let cancel = Arc::new(AtomicBool::new(false));
    let finished = Arc::new((Mutex::new(false), Condvar::new()));
    let outcome = Arc::new(Mutex::new(None));
    {
        let mut guard = active().lock().map_err(|e| e.to_string())?;
        if guard.is_some() {
            return Ok(RunHandle {
                run_id: guard.as_ref().map(|run| run.run_id.clone()),
                pages: Vec::new(),
                already_running: Some(true),
            });
        }
        *guard = Some(Active {
            run_id: run_id.clone(),
            cancel: Arc::clone(&cancel),
            finished: Arc::clone(&finished),
            outcome: Arc::clone(&outcome),
        });
    }

    let handle = RunHandle {
        run_id: Some(run_id.clone()),
        pages: queued(&entries),
        already_running: None,
    };
    let worker = Worker { run_id, chapter_id: chapter_id.to_owned(), cancel, finished, outcome };
    let settings_snapshot = settings.clone();
    std::thread::spawn(move || {
        let mut cleaner = pipeline;
        // The settings a job ran under, recorded on the job it ran over. Under
        // the job's own lock, because this is a read-modify-write like any
        // other and the run is about to take the same lock for the run itself.
        for path in
            entries.iter().map(|entry| &entry.job_path).collect::<std::collections::BTreeSet<_>>()
        {
            let _lock = lock_job(path);
            if let Ok(mut job) = Job::open(path) {
                job.project.settings = settings_snapshot.clone();
                let _ = job.flush();
            }
        }
        worker.run(&entries, &mut cleaner, ceiling, &|event| events::emit(&event));
    });
    Ok(handle)
}

/// The handles the thread that owns a run needs to finish it and hand the slot
/// back.
struct Worker {
    run_id: String,
    chapter_id: String,
    cancel: Arc<AtomicBool>,
    finished: Arc<(Mutex<bool>, Condvar)>,
    outcome: Arc<Mutex<Option<&'static str>>>,
}

impl Worker {
    /// Run to completion, and hand the slot back **whatever happens on the
    /// way**.
    ///
    /// The release is a guard rather than a line at the end, because the exits
    /// are not all visible here: [`execute`] contains a panic in the pipeline,
    /// but a panic in a *sink* - a subscriber that throws on the way out -
    /// unwinds through this frame, and a `release` that only ran on the happy
    /// path would leave `active()` occupied for the life of the session. Every
    /// later `runClean` would answer `alreadyRunning` for a run that has
    /// stopped, and every `cancelRun` would wait the full [`CANCEL_WAIT`] and
    /// then name it.
    fn run(
        &self,
        entries: &[PlanEntry],
        cleaner: &mut dyn Cleaner,
        ceiling: Engine,
        emit: &dyn Fn(Event),
    ) -> Summary {
        let _release = Release(Arc::clone(&self.finished));
        let summary = execute(
            &self.run_id,
            &self.chapter_id,
            entries,
            cleaner,
            ceiling,
            &self.cancel,
            emit,
        );
        // Recorded before the slot is released, so a `cancelRun` woken by the
        // release finds the reason the run actually ended on.
        if let Ok(mut recorded) = self.outcome.lock() {
            *recorded = Some(summary.reason);
        }
        report(&summary, emit);
        summary
    }
}

/// Hands the active slot back when it goes. See [`Worker::run`].
struct Release(Arc<(Mutex<bool>, Condvar)>);

impl Drop for Release {
    fn drop(&mut self) {
        release(&self.0);
    }
}

/// The notice a finished run leaves on the stack. The mock's `onFinished`,
/// key for key.
///
/// Emitted through the run's own sink rather than through the global registry,
/// so that the notice a run ends on is one of the things the scheduler's tests
/// can see. In the window the sink is `events::emit`, which is where it went
/// before.
fn report(summary: &Summary, emit: &dyn Fn(Event)) {
    if summary.reason == "cancelled" {
        emit(events::notice_event("notice.run.cancelled", serde_json::json!({}), "warn"));
    } else if summary.regions_cleaned == 0 {
        emit(events::notice_event(
            "notice.chapter.emptyResult",
            serde_json::json!({ "regions": 0, "pages": summary.pages_cleaned }),
            "warn",
        ));
    } else {
        emit(events::notice_event(
            "notice.run.finished",
            serde_json::json!({
                "pages": summary.pages_cleaned,
                "regions": summary.regions_cleaned,
            }),
            "info",
        ));
    }
}

/// Clear the active slot and wake anything waiting on it.
///
/// Through a poisoned lock, both times: this is the call that has to happen for
/// the *next* run to be startable at all, and a slot left occupied by a panic
/// somewhere else is the failure it exists to prevent.
fn release(finished: &Arc<(Mutex<bool>, Condvar)>) {
    *active().lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    let (lock, condvar) = &**finished;
    *lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
    condvar.notify_all();
}

/* ------------------------------------------------------------------ */
/* Commands                                                            */
/* ------------------------------------------------------------------ */

/// `runClean`. Resolves with the queue; every result arrives on the channel.
///
/// `async`, with the body off the invoke handler's thread, for the reason
/// [`crate::library::blocking`] gives in full: a sync command is expanded
/// through `body_blocking` and runs *inline* on the thread the invoke arrives
/// on, which here would hold the webview through a whole-library index read and
/// three ONNX session opens.
///
/// **Both preconditions are now notices.** A missing runtime has always been
/// one - `diagnostics.runtime.*` says exactly that - and missing *weights*
/// used to be an `Err` the interface had nowhere to put. Settings › Models
/// can fetch both now, so `notice.run.modelsMissing` names that remedy and
/// `start` answers with an empty handle rather than a rejected promise.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn run_clean(
    app: tauri::AppHandle,
    scope: Option<String>,
    chapter_id: String,
    page_index: Option<u32>,
    engine_ceiling: Option<String>,
    bubble_engine: Option<String>,
    outside_engine: Option<String>,
    outside_bubbles: Option<String>,
) -> Result<RunHandle, String> {
    crate::library::blocking(move || {
        let picks = Picks::from_args(bubble_engine.as_deref(), outside_engine.as_deref());
        let outside = OutsideText::from_arg(outside_bubbles.as_deref());
        start(
            &app,
            scope.as_deref().unwrap_or("chapter"),
            &chapter_id,
            page_index,
            engine_ceiling,
            picks,
            outside,
        )
    })
    .await
}

/// `cancelRun`. Keeps every completed region.
///
/// Answers only once the worker has stopped and `run-finished` has gone out, so
/// that a caller which cancels and then reads the event stream sees the same
/// thing the mock shows it. `null` when there is no run, or when the id names a
/// run that is not the active one.
#[tauri::command]
pub async fn cancel_run(run_id: Option<String>) -> Result<Option<String>, String> {
    crate::library::blocking(move || request_cancel(run_id)).await
}

/// `cancelRun`'s body, off the Tauri boundary so the race it is careful about
/// can be set up in a test.
///
/// **A run that has already finished is not cancellable, and saying it was is a
/// lie the caller can catch.** The window is small and real: `execute` has
/// returned and its `run-finished` has gone out saying `completed`, and the slot
/// is not cleared until the worker's release runs. A cancel arriving in it used
/// to set the flag on a dead run, wait out the release, and answer with the run
/// id - a caller that had just been told the run completed, told a moment later
/// that its cancel took. So the outcome is checked on both sides of the wait,
/// and `null` - "there was nothing to cancel" - is the honest answer.
fn request_cancel(run_id: Option<String>) -> Result<Option<String>, String> {
    let (id, cancel, finished, outcome) = {
        let guard = active().lock().map_err(|e| e.to_string())?;
        let Some(run) = guard.as_ref() else { return Ok(None) };
        if run_id.as_deref().is_some_and(|asked| asked != run.run_id) {
            return Ok(None);
        }
        (
            run.run_id.clone(),
            Arc::clone(&run.cancel),
            Arc::clone(&run.finished),
            Arc::clone(&run.outcome),
        )
    };
    if completed_already(&outcome) {
        return Ok(None);
    }
    cancel.store(true, Ordering::SeqCst);

    let (lock, condvar) = &*finished;
    let mut done = lock.lock().map_err(|e| e.to_string())?;
    while !*done {
        let (guard, timeout) = condvar.wait_timeout(done, CANCEL_WAIT).map_err(|e| e.to_string())?;
        done = guard;
        if timeout.timed_out() {
            break;
        }
    }
    drop(done);
    if completed_already(&outcome) {
        return Ok(None);
    }
    Ok(Some(id))
}

/// Whether this run's `run-finished` has already gone out saying it completed.
///
/// A run still going has no outcome yet, and one that stopped because of this
/// cancel - or because it panicked, which reports itself the same way - carries
/// `cancelled`. Only `completed` means the cancel had nothing to act on.
fn completed_already(outcome: &Mutex<Option<&'static str>>) -> bool {
    outcome.lock().map(|reason| *reason == Some("completed")).unwrap_or(false)
}

/// `resumeJob`. Re-verifies the sources, then starts the run.
///
/// "Resume re-verifies source hashes before continuing", and a source that
/// changed loses its patches - the page
/// is stale, and compositing at coordinates taken from a page that has since
/// been re-cropped is the failure the rule exists to prevent. The user is meant
/// to be told, and cannot be: the catalogue has no key for "some pages were
/// dropped because their files changed". Recorded, not invented.
#[tauri::command]
pub async fn resume_job(
    app: tauri::AppHandle,
    project_id: String,
    chapter_id: Option<String>,
) -> Result<Option<ResumedJob>, String> {
    crate::library::blocking(move || {
        let library = Library::for_app(&app).map_err(|e| e.to_string())?;
        let projects = library.list_projects().map_err(|e| e.to_string())?;
        let Some(project) = projects.into_iter().find(|p| p.id == project_id) else {
            return Ok(None);
        };
        let Some(interrupted) = project.interrupted_job.clone() else { return Ok(None) };
        let chapter_id = chapter_id.unwrap_or_else(|| interrupted.chapter_id.clone());
        let Some(chapter) = project.chapters.iter().find(|c| c.id == chapter_id).cloned() else {
            return Ok(None);
        };

        // `ApiProject.interrupted_job` names the *first* interrupted chapter of
        // the project, which is what Home offers. A caller that named a
        // different chapter is resuming that one, and its own manifest is where
        // its resume point is - taking the project's would send the editor to a
        // page of the wrong chapter.
        let mut resumed_from = interrupted.page_index;
        {
            let job_path = library.job_path(&project_id, &chapter_id);
            // Read, verify, drop, flush - one read-modify-write, so it is held
            // for the whole of it and released before `start` takes it again.
            let _lock = lock_job(&job_path);
            if let Ok(mut job) = Job::open(&job_path) {
                if let Some(at) = job.project.interrupted_at {
                    resumed_from = at;
                }
                let states = job.verify_sources();
                let _ = job.drop_stale(&states);
            }
        }

        // A resume carries no picks of its own: `resumeJob` names a chapter and
        // nothing else, so it runs on the defaults rather than on whatever the
        // tool window happened to hold when the job was interrupted. The same
        // for the outside-bubble row: a resume reviews that text, whatever the
        // interrupted run was told.
        let handle = start(
            &app,
            "chapter",
            &chapter_id,
            None,
            None,
            Picks::default(),
            OutsideText::default(),
        )?;
        events::notice(
            "notice.job.resumed",
            serde_json::json!({ "page": resumed_from + 1 }),
            "info",
        );
        Ok(Some(ResumedJob {
            project,
            chapter,
            resumed_from,
            run_id: handle.run_id,
            pages: handle.pages,
        }))
    })
    .await
}

#[cfg(test)]
mod tests;
