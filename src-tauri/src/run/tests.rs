//! The ordering contract, pinned rule by rule.
//!
//! Four rules govern the event stream, and a scheduler that satisfies them
//! under a happy path and races under cancellation is not done. So each rule
//! has a test, and the cancel path, the resume path and a chapter whose pages
//! have no regions at all are all exercised against a real [`Job`] on disk.
//!
//! What is *not* exercised here is the model-backed pipeline: opening three
//! ONNX sessions needs the weights, which are not in the repository. That is
//! why [`Cleaner`] is a trait - every rule below is a property of the
//! scheduler, and the scheduler under test is the one the window runs.

use super::*;
use cleaner_core::image::{BitDepth, ColorMode, Format, encode, fixtures};
use cleaner_core::project::StripMode;
use cleaner_core::strip::{EdgePad, Strip};

/* ------------------------------------------------------------------ */
/* Scaffolding                                                         */
/* ------------------------------------------------------------------ */

/// A scratch directory that removes itself - the same shape the project store's
/// tests and `library.rs`'s use, and for the same reason: a unique path, not a
/// dependency.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        use std::sync::atomic::AtomicU32;
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir()
            .join("manga-cleaner-run")
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

/// A folder of scans: `count` distinct PNG pages, naturally ordered.
fn scans(dir: &Path, count: usize) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    for n in 1..=count {
        let mut raster = fixtures::by_name("l8").raster;
        // Distinct bytes per page, or ingest deduplicates them.
        raster.data[0] = n as u8;
        let bytes = encode(&raster, Format::Png).unwrap();
        std::fs::write(dir.join(format!("{n:03}.png")), bytes).unwrap();
    }
    dir.to_path_buf()
}

/// A library holding one project of one chapter, over `pages` scans.
fn a_chapter(scratch: &Scratch, pages: usize) -> (Library, String, String) {
    let library = Library::at(scratch.join("library"));
    let source = scans(&scratch.join("raws"), pages);
    let project =
        library.create_project("Tsuki to Hane", StripMode::Single, Some(source), None).unwrap();
    let chapter = library.create_chapter(&project.id, "Ch. 1", None, None).unwrap().created().unwrap();
    (library, project.id, chapter.id)
}

/// A patch covering a small rectangle of the fixture page.
fn a_patch(id: &str, order: u32, engine: Engine) -> Patch {
    let bounds = Rect::new(2, 2, 4, 4);
    let mut pixels = fixtures::by_name("l8").raster;
    pixels.width = bounds.w;
    pixels.height = bounds.h;
    pixels.data = vec![220; (bounds.w * bounds.h) as usize];
    Patch {
        id: id.to_owned(),
        mask: Mask::filled(bounds),
        ink: Mask::filled(bounds),
        pixels,
        order,
        visible: true,
        provenance: Provenance {
            engine,
            engine_version: "test".into(),
            model_sha256: None,
            execution_provider: "cpu".into(),
            params_snapshot: serde_json::json!({
                "rung": engine.rung_key(),
                "fill_mode": "match-surround",
                "elapsed_ms": 41,
            }),
            mask_sha256: String::new(),
            source_sha256: "0".repeat(64),
            cloud: None,
            created: 1_760_000_000,
        },
    }
}

/// A cleaner that produces a fixed shape of result, and records what it was
/// asked for.
///
/// Everything the scheduler is responsible for - the order of events, the
/// manifest writes, the cancellation checks - is visible through this without
/// a single ONNX session.
struct Stub {
    cleaned: usize,
    untouched: usize,
    /// Regions declined rather than gate-skipped, which is the other counter.
    declined: usize,
    pages: Arc<Mutex<Vec<String>>>,
    ceilings: Arc<Mutex<Vec<Engine>>>,
    /// What the strip said about each page it was asked for: the placement, how
    /// many detection segments the page has, and how many joins the chapter
    /// has.
    contexts: Arc<Mutex<Vec<(usize, usize, usize)>>>,
    /// Pages whose clean fails outright, by page id.
    fails: Vec<String>,
    /// Pages whose clean **panics**, by page id. A model that produced a shape
    /// nothing expected is this, and it is not a `Result`.
    panics: Vec<String>,
    /// Report the held-back regions before the cleaned ones. The order a page's
    /// regions come back in is the pipeline's business and not the scheduler's,
    /// and it decides what a cancel mid-page leaves written down.
    untouched_first: bool,
}

impl Stub {
    fn new(cleaned: usize, untouched: usize) -> Stub {
        Stub {
            cleaned,
            untouched,
            declined: 0,
            pages: Arc::new(Mutex::new(Vec::new())),
            ceilings: Arc::new(Mutex::new(Vec::new())),
            contexts: Arc::new(Mutex::new(Vec::new())),
            fails: Vec::new(),
            panics: Vec::new(),
            untouched_first: false,
        }
    }

    /// A page whose held-back regions are written down before its cleaned ones,
    /// with one declined region among them.
    fn held_back_first(cleaned: usize, untouched: usize, declined: usize) -> Stub {
        Stub { declined, untouched_first: true, ..Stub::new(cleaned, untouched) }
    }

    fn held_back(&self) -> Vec<RegionOutcome> {
        let mut regions = Vec::new();
        for index in 0..self.untouched {
            regions.push(RegionOutcome::Untouched {
                bbox: Rect::new(20 + index as i64, 20, 4, 4),
                reason: "review.reason.gateSkippedNotJapanese".into(),
            });
        }
        for index in 0..self.declined {
            regions.push(RegionOutcome::Untouched {
                bbox: Rect::new(40 + index as i64, 40, 4, 4),
                reason: "review.reason.declined".into(),
            });
        }
        regions
    }
}

impl Cleaner for Stub {
    fn clean_page(
        &mut self,
        page_id: &str,
        _bytes: &[u8],
        ceiling: Engine,
        context: &PageContext,
    ) -> Result<PageOutcome, String> {
        self.pages.lock().unwrap().push(page_id.to_owned());
        self.ceilings.lock().unwrap().push(ceiling);
        // The geometry is handed to every page and every page is somewhere in
        // the strip. Asserted here rather than in one test, so a scheduler that
        // stopped building the context would fail every test that runs a page
        // rather than the one that looks for it.
        assert!(
            context.strip.pages().get(context.placement).is_some(),
            "{page_id} was cleaned with no place in the strip"
        );
        assert!(
            !context.page_segments().is_empty(),
            "{page_id} was cleaned with no detection segment"
        );
        self.contexts
            .lock()
            .unwrap()
            .push((context.placement, context.page_segments().len(), context.joins.len()));
        assert!(!self.panics.iter().any(|id| id == page_id), "the pipeline panicked on {page_id}");
        if self.fails.iter().any(|id| id == page_id) {
            return Err("this page could not be read".into());
        }
        let mut outcome = PageOutcome::default();
        if self.untouched_first {
            outcome.regions.extend(self.held_back());
        }
        for index in 0..self.cleaned {
            outcome.regions.push(RegionOutcome::Cleaned(
                Box::new(a_patch(&region_id(page_id, index), index as u32, Engine::Fill)),
                None,
            ));
        }
        if !self.untouched_first {
            outcome.regions.extend(self.held_back());
        }
        Ok(outcome)
    }
}

/// The scheduler's globals - the active slot and the notice counter - are
/// process-wide, and `cargo test` is parallel. The tests that touch them take
/// this first, the same shape `events.rs`'s own tests use and for the same
/// reason.
fn exclusively() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Run a body with panic reporting silenced, for the tests whose subject *is* a
/// panic. The hook goes back immediately after, so a genuine panic elsewhere
/// still says so.
fn quietly<T>(body: impl FnOnce() -> T) -> T {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let answer = body();
    std::panic::set_hook(hook);
    answer
}

/// Whether a job is free to be written right now. The non-blocking half of
/// [`lock_job`], which a caller never wants and a test does.
fn job_is_free(path: &Path) -> bool {
    let (held, _) = held_jobs();
    let held = held.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    !held.contains(path)
}

/// The worker, and the active slot it will hand back, wired to one recorder.
fn a_worker(chapter_id: &str, recorder: &Recorder) -> Worker {
    let worker = Worker {
        run_id: "backend-run-1".to_owned(),
        chapter_id: chapter_id.to_owned(),
        cancel: Arc::clone(&recorder.cancel),
        finished: Arc::new((Mutex::new(false), Condvar::new())),
        outcome: Arc::new(Mutex::new(None)),
    };
    *active().lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Active {
        run_id: worker.run_id.clone(),
        cancel: Arc::clone(&worker.cancel),
        finished: Arc::clone(&worker.finished),
        outcome: Arc::clone(&worker.outcome),
    });
    worker
}

/// Collects the stream, and can pull the cancel lever at a chosen moment -
/// which is how a cancel arriving *while* the run is between two things is
/// simulated without a second thread and the flakiness that comes with one.
struct Recorder {
    seen: Arc<Mutex<Vec<Event>>>,
    cancel: Arc<AtomicBool>,
    /// Raise the cancel flag the first time an event of this kind goes out.
    trip_on: Option<&'static str>,
}

impl Recorder {
    fn new() -> Recorder {
        Recorder {
            seen: Arc::new(Mutex::new(Vec::new())),
            cancel: Arc::new(AtomicBool::new(false)),
            trip_on: None,
        }
    }

    fn tripping_on(kind: &'static str) -> Recorder {
        Recorder { trip_on: Some(kind), ..Recorder::new() }
    }

    fn sink(&self) -> impl Fn(Event) + '_ {
        move |event| {
            if let Some(kind) = self.trip_on {
                if kind_of(&event) == kind && !self.cancel.load(Ordering::SeqCst) {
                    self.cancel.store(true, Ordering::SeqCst);
                }
            }
            self.seen.lock().unwrap().push(event);
        }
    }

    fn events(&self) -> Vec<Event> {
        self.seen.lock().unwrap().clone()
    }

    fn kinds(&self) -> Vec<&'static str> {
        self.events().iter().map(kind_of).collect()
    }
}

fn kind_of(event: &Event) -> &'static str {
    match event {
        Event::PageStarted { .. } => "page-started",
        Event::RegionDone { .. } => "region-done",
        Event::PageDone { .. } => "page-done",
        Event::RunFinished { .. } => "run-finished",
        Event::Notice { .. } => "notice",
        // A download's progress. Never emitted by a run, and named here rather
        // than swept into a wildcard so that the next event type added to the
        // seam fails this match instead of being silently mislabelled.
        Event::ModelProgress { .. } => "model-progress",
    }
}

fn page_index_of(event: &Event) -> Option<u32> {
    match event {
        Event::PageStarted { page_index, .. }
        | Event::RegionDone { page_index, .. }
        | Event::PageDone { page_index, .. } => Some(*page_index),
        _ => None,
    }
}

/* ------------------------------------------------------------------ */
/* The four ordering rules                                             */
/* ------------------------------------------------------------------ */

/// Rule 1: every `region-done` for a page falls **strictly** between that
/// page's `page-started` and its `page-done`.
#[test]
fn every_region_done_falls_strictly_between_its_pages_start_and_end() {
    let scratch = Scratch::new("ordering");
    let (library, _, chapter_id) = a_chapter(&scratch, 4);
    let entries = plan(&library, "chapter", &chapter_id, None).unwrap();
    assert_eq!(entries.len(), 4);

    let recorder = Recorder::new();
    let mut stub = Stub::new(3, 1);
    let summary = execute(
        "run-1",
        &chapter_id,
        &entries,
        &mut stub,
        Engine::Flux,
        &recorder.cancel,
        &recorder.sink(),
    );

    assert_eq!(summary.reason, "completed");
    let events = recorder.events();
    for page in 0..4u32 {
        let opened = events
            .iter()
            .position(|e| kind_of(e) == "page-started" && page_index_of(e) == Some(page))
            .expect("a page was never started");
        let closed = events
            .iter()
            .position(|e| kind_of(e) == "page-done" && page_index_of(e) == Some(page))
            .expect("a page was never finished");
        assert!(opened < closed);
        let regions: Vec<usize> = events
            .iter()
            .enumerate()
            .filter(|(_, e)| kind_of(e) == "region-done" && page_index_of(e) == Some(page))
            .map(|(at, _)| at)
            .collect();
        assert_eq!(regions.len(), 3, "a page's regions were not all reported");
        for at in regions {
            assert!(at > opened && at < closed, "a region escaped its page's bracket");
        }
    }
    // The pages themselves are in queue order, and nothing interleaves them.
    let page_order: Vec<u32> = events
        .iter()
        .filter(|e| kind_of(e) == "page-started")
        .filter_map(page_index_of)
        .collect();
    assert_eq!(page_order, vec![0, 1, 2, 3]);
}

/// Rule 2: exactly one `run-finished` per run, whatever happened. Asserted on
/// both exits, because one of them is the one a race would double.
#[test]
fn exactly_one_run_finished_whatever_happened() {
    for trip in [None, Some("region-done"), Some("page-done")] {
        let scratch = Scratch::new("one-finish");
        let (library, _, chapter_id) = a_chapter(&scratch, 4);
        let entries = plan(&library, "chapter", &chapter_id, None).unwrap();
        let recorder = match trip {
            None => Recorder::new(),
            Some(kind) => Recorder::tripping_on(kind),
        };
        let mut stub = Stub::new(2, 0);
        execute(
            "run-1",
            &chapter_id,
            &entries,
            &mut stub,
            Engine::Flux,
            &recorder.cancel,
            &recorder.sink(),
        );
        let finished = recorder.kinds().iter().filter(|kind| **kind == "run-finished").count();
        assert_eq!(finished, 1, "trip {trip:?} produced {finished} run-finished events");
        assert_eq!(
            recorder.kinds().last(),
            Some(&"run-finished"),
            "something was emitted after the run finished"
        );
    }
}

/// Rule 3: `nextPageIndex` is set **only** on cancel, and it is where a resume
/// picks up - the page the run stopped at, not the last one it finished.
#[test]
fn next_page_index_is_set_only_on_cancel() {
    let scratch = Scratch::new("next-page");
    let (library, _, chapter_id) = a_chapter(&scratch, 4);
    let entries = plan(&library, "chapter", &chapter_id, None).unwrap();

    let completed = Recorder::new();
    let summary = execute(
        "run-1",
        &chapter_id,
        &entries,
        &mut Stub::new(1, 0),
        Engine::Flux,
        &completed.cancel,
        &completed.sink(),
    );
    assert_eq!(summary.reason, "completed");
    assert_eq!(summary.next_page_index, None);
    assert!(matches!(
        completed.events().last(),
        Some(Event::RunFinished { next_page_index: None, .. })
    ));

    // A second chapter, because the first one is now finished and a finished
    // chapter has an empty queue.
    let other = Scratch::new("next-page-cancel");
    let (library, _, chapter_id) = a_chapter(&other, 4);
    let entries = plan(&library, "chapter", &chapter_id, None).unwrap();
    let cancelled = Recorder::tripping_on("page-done");
    let summary = execute(
        "run-2",
        &chapter_id,
        &entries,
        &mut Stub::new(1, 0),
        Engine::Flux,
        &cancelled.cancel,
        &cancelled.sink(),
    );
    assert_eq!(summary.reason, "cancelled");
    // Page 0 finished; the cancel landed as it did, so page 1 is the resume
    // point and page 1 is what the manifest records.
    assert_eq!(summary.next_page_index, Some(1));
    assert_eq!(summary.pages_cleaned, 1);
    let job = Job::open(&entries[0].job_path).unwrap();
    assert_eq!(job.project.interrupted_at, Some(1));
}

/// Rule 4 lives in `events.rs` - `page-started` is a fifth event *type*, not a
/// flag - and here is the half a scheduler can get wrong: a page that is being
/// cleaned announces itself before anything about it is known, so the mark can
/// go up while the work happens rather than after it.
#[test]
fn a_page_announces_itself_before_any_of_its_regions() {
    let scratch = Scratch::new("announce");
    let (library, _, chapter_id) = a_chapter(&scratch, 1);
    let recorder = Recorder::new();
    execute(
        "run-1",
        &chapter_id,
        &plan(&library, "chapter", &chapter_id, None).unwrap(),
        &mut Stub::new(2, 0),
        Engine::Flux,
        &recorder.cancel,
        &recorder.sink(),
    );
    assert_eq!(
        recorder.kinds(),
        vec!["page-started", "region-done", "region-done", "page-done", "run-finished"]
    );
}

/* ------------------------------------------------------------------ */
/* Cancel                                                              */
/* ------------------------------------------------------------------ */

/// A cancel keeps every completed region, and the page it landed in the
/// middle of is left unfinished rather than rolled back - no `page-done`,
/// not marked examined, and still queued.
#[test]
fn a_cancel_mid_page_keeps_what_finished_and_leaves_the_page_queued() {
    let scratch = Scratch::new("cancel-mid-page");
    let (library, _, chapter_id) = a_chapter(&scratch, 3);
    let entries = plan(&library, "chapter", &chapter_id, None).unwrap();

    // Trips on the first `region-done`, which is inside page 0.
    let recorder = Recorder::tripping_on("region-done");
    let summary = execute(
        "run-1",
        &chapter_id,
        &entries,
        &mut Stub::new(3, 0),
        Engine::Flux,
        &recorder.cancel,
        &recorder.sink(),
    );

    assert_eq!(summary.reason, "cancelled");
    assert_eq!(summary.next_page_index, Some(0));
    assert_eq!(summary.pages_cleaned, 0);
    assert_eq!(summary.regions_cleaned, 1);
    assert!(
        !recorder.kinds().contains(&"page-done"),
        "a page cancelled part-way was reported finished"
    );

    let job = Job::open(&entries[0].job_path).unwrap();
    assert_eq!(job.project.patches.len(), 1, "the completed region was not kept");
    assert!(job.project.examined.is_empty(), "an unfinished page was marked examined");
    assert_eq!(job.project.interrupted_at, Some(0));

    // And the next run picks the page up again.
    let again = plan(&library, "chapter", &chapter_id, None).unwrap();
    assert_eq!(again.iter().map(|e| e.page_index).collect::<Vec<_>>(), vec![0, 1, 2]);
}

/// Nothing is emitted after the cancel is observed. A scheduler that kept
/// walking would report regions for a run the interface has already closed.
#[test]
fn a_cancelled_run_stops_emitting() {
    let scratch = Scratch::new("cancel-stops");
    let (library, _, chapter_id) = a_chapter(&scratch, 6);
    let recorder = Recorder::tripping_on("page-done");
    execute(
        "run-1",
        &chapter_id,
        &plan(&library, "chapter", &chapter_id, None).unwrap(),
        &mut Stub::new(1, 0),
        Engine::Flux,
        &recorder.cancel,
        &recorder.sink(),
    );
    assert_eq!(
        recorder.kinds(),
        vec!["page-started", "region-done", "page-done", "run-finished"],
        "a cancelled run kept walking its queue"
    );
}

/* ------------------------------------------------------------------ */
/* Resume                                                              */
/* ------------------------------------------------------------------ */

/// A resumed run picks up at the page the cancel stopped at, redoes it, and
/// clears the offer when it finishes - a job that records being interrupted
/// and cannot say it no longer is would offer a resume for ever.
#[test]
fn a_resume_starts_at_the_page_the_cancel_stopped_at_and_clears_the_offer() {
    let scratch = Scratch::new("resume");
    let (library, project_id, chapter_id) = a_chapter(&scratch, 4);
    let entries = plan(&library, "chapter", &chapter_id, None).unwrap();

    let first = Recorder::tripping_on("page-done");
    let summary = execute(
        "run-1",
        &chapter_id,
        &entries,
        &mut Stub::new(1, 0),
        Engine::Flux,
        &first.cancel,
        &first.sink(),
    );
    assert_eq!(summary.next_page_index, Some(1));

    // What Home reads to offer the resume.
    let project = library
        .list_projects()
        .unwrap()
        .into_iter()
        .find(|p| p.id == project_id)
        .unwrap();
    let offer = project.interrupted_job.expect("no resume was offered");
    assert_eq!(offer.chapter_id, chapter_id);
    assert_eq!(offer.page_index, 1);

    let resumed = plan(&library, "chapter", &chapter_id, None).unwrap();
    assert_eq!(
        resumed.iter().map(|entry| entry.page_index).collect::<Vec<_>>(),
        vec![1, 2, 3],
        "the resume did not pick up where the cancel stopped"
    );

    let second = Recorder::new();
    let summary = execute(
        "run-2",
        &chapter_id,
        &resumed,
        &mut Stub::new(1, 0),
        Engine::Flux,
        &second.cancel,
        &second.sink(),
    );
    assert_eq!(summary.reason, "completed");
    assert_eq!(summary.pages_cleaned, 3);
    assert_eq!(Job::open(&entries[0].job_path).unwrap().project.interrupted_at, None);
    assert!(
        library
            .list_projects()
            .unwrap()
            .into_iter()
            .find(|p| p.id == project_id)
            .unwrap()
            .interrupted_job
            .is_none(),
        "the resume was still offered after the run completed"
    );
}

/* ------------------------------------------------------------------ */
/* A chapter with nothing in it                                        */
/* ------------------------------------------------------------------ */

/// A chapter whose pages have no regions at all. Every page is still started
/// and finished, no `region-done` goes out, and the run reports the empty
/// result - which is also the only thing that can tell "detected nothing"
/// from "never looked".
#[test]
fn a_chapter_with_no_regions_at_all_reports_every_page_and_no_regions() {
    let scratch = Scratch::new("empty");
    let (library, _, chapter_id) = a_chapter(&scratch, 5);
    let entries = plan(&library, "chapter", &chapter_id, None).unwrap();

    let recorder = Recorder::new();
    let summary = execute(
        "run-1",
        &chapter_id,
        &entries,
        &mut Stub::new(0, 0),
        Engine::Flux,
        &recorder.cancel,
        &recorder.sink(),
    );

    assert_eq!(summary.reason, "completed");
    assert_eq!(summary.pages_cleaned, 5);
    assert_eq!(summary.regions_cleaned, 0);
    let kinds = recorder.kinds();
    assert_eq!(kinds.iter().filter(|k| **k == "region-done").count(), 0);
    assert_eq!(kinds.iter().filter(|k| **k == "page-started").count(), 5);
    assert_eq!(kinds.iter().filter(|k| **k == "page-done").count(), 5);
    assert_eq!(kinds.iter().filter(|k| **k == "run-finished").count(), 1);

    // The chapter now says so at the seam, and its pages are finished rather
    // than permanently unclean.
    let chapter = library
        .list_projects()
        .unwrap()
        .into_iter()
        .flat_map(|project| project.chapters)
        .find(|chapter| chapter.id == chapter_id)
        .unwrap();
    assert!(chapter.no_text_detected, "an examined and empty chapter did not say so");
    assert!(chapter.pages.iter().all(|page| page.status == "cleaned"));

    // And nothing is queued a second time.
    assert!(plan(&library, "chapter", &chapter_id, None).unwrap().is_empty());
}

/// The complementary case: a chapter nobody has run does **not** claim that
/// no text was detected. The two states were the same manifest before
/// `Project::examined` existed.
#[test]
fn a_chapter_nobody_has_run_does_not_claim_an_empty_result() {
    let scratch = Scratch::new("unexamined");
    let (library, _, chapter_id) = a_chapter(&scratch, 3);
    let chapter = library
        .list_projects()
        .unwrap()
        .into_iter()
        .flat_map(|project| project.chapters)
        .find(|chapter| chapter.id == chapter_id)
        .unwrap();
    assert!(!chapter.no_text_detected);
    assert!(chapter.pages.iter().all(|page| page.status == "unclean"));
}

/// A cancelled run has not established that a chapter is empty, however many
/// of its pages came back with nothing.
#[test]
fn a_partly_examined_chapter_does_not_claim_an_empty_result() {
    let scratch = Scratch::new("part-examined");
    let (library, _, chapter_id) = a_chapter(&scratch, 4);
    let recorder = Recorder::tripping_on("page-done");
    execute(
        "run-1",
        &chapter_id,
        &plan(&library, "chapter", &chapter_id, None).unwrap(),
        &mut Stub::new(0, 0),
        Engine::Flux,
        &recorder.cancel,
        &recorder.sink(),
    );
    let chapter = library
        .list_projects()
        .unwrap()
        .into_iter()
        .flat_map(|project| project.chapters)
        .find(|chapter| chapter.id == chapter_id)
        .unwrap();
    assert!(!chapter.no_text_detected, "one page of four settled the whole chapter");
}

/* ------------------------------------------------------------------ */
/* The queue                                                           */
/* ------------------------------------------------------------------ */

#[test]
fn scope_page_queues_one_page_and_scope_chapter_queues_them_all() {
    let scratch = Scratch::new("scope");
    let (library, _, chapter_id) = a_chapter(&scratch, 6);
    let one = plan(&library, "page", &chapter_id, Some(3)).unwrap();
    assert_eq!(one.iter().map(|entry| entry.page_index).collect::<Vec<_>>(), vec![3]);
    assert_eq!(plan(&library, "chapter", &chapter_id, None).unwrap().len(), 6);
}

#[test]
fn scope_project_walks_every_chapter_of_the_project() {
    let scratch = Scratch::new("scope-project");
    let (library, project_id, first) = a_chapter(&scratch, 2);
    scans(&scratch.join("raws/Ch. 2"), 3);
    let second = library.create_chapter(&project_id, "Ch. 2", None, None).unwrap().created().unwrap();

    let entries = plan(&library, "project", &first, None).unwrap();
    let chapters: Vec<&str> = entries.iter().map(|entry| entry.chapter_id.as_str()).collect();
    assert_eq!(chapters.iter().filter(|id| **id == second.id).count(), 3);
    assert_eq!(chapters.iter().filter(|id| **id == first).count(), 2);
}

/// A page with a patch on it is finished. A run over a chapter that is already
/// half done queues the other half and nothing else.
#[test]
fn a_page_that_has_already_been_cleaned_is_not_queued_again() {
    let scratch = Scratch::new("requeue");
    let (library, _, chapter_id) = a_chapter(&scratch, 3);
    let entries = plan(&library, "chapter", &chapter_id, None).unwrap();
    let recorder = Recorder::new();
    execute(
        "run-1",
        &chapter_id,
        &entries[..1],
        &mut Stub::new(2, 0),
        Engine::Flux,
        &recorder.cancel,
        &recorder.sink(),
    );
    assert_eq!(
        plan(&library, "chapter", &chapter_id, None)
            .unwrap()
            .iter()
            .map(|entry| entry.page_index)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
}

/// A chapter of an unknown id is an error rather than an empty queue: an empty
/// queue is `notice.run.nothingInScope`, which is a different thing to tell a
/// user.
#[test]
fn an_unknown_chapter_is_an_error_rather_than_an_empty_queue() {
    let scratch = Scratch::new("unknown");
    let (library, _, _) = a_chapter(&scratch, 1);
    assert!(matches!(
        plan(&library, "chapter", "nope", None),
        Err(LibraryError::Unknown { .. })
    ));
}

/* ------------------------------------------------------------------ */
/* The ceiling                                                         */
/* ------------------------------------------------------------------ */

/// **Auto clean is local-only.** A batch run can never reach the cloud rung,
/// whatever it is asked for - `needs-confirmation` lives on `applyTool` and
/// `runClean` has no equivalent, so a run at the cloud ceiling would spend with
/// neither the transmission statement nor the cost confirmation.
#[test]
fn a_run_can_never_reach_the_cloud_rung() {
    for asked in ["cloud", "ladder.rung.cloud"] {
        assert_eq!(effective_ceiling(Some(asked), None), Engine::Flux);
        assert_eq!(effective_ceiling(None, Some(asked)), Engine::Flux);
        assert_eq!(effective_ceiling(Some(asked), Some(asked)), Engine::Flux);
    }
}

/// **`engineCeiling` caps, never raises.** Introduced twice, caught twice.
#[test]
fn the_ceiling_only_ever_moves_down() {
    // The caller lowers the stored setting.
    assert_eq!(effective_ceiling(Some("fill"), Some("flux")), Engine::Fill);
    // And cannot lift it.
    assert_eq!(effective_ceiling(Some("flux"), Some("fill")), Engine::Fill);
    // Neither side named one: the highest local rung, not the highest rung.
    assert_eq!(effective_ceiling(None, None), HIGHEST_LOCAL);
    // A name this build does not know is ignored rather than read as "no
    // ceiling" - the failure that matters is a typo unlocking a rung.
    assert_eq!(effective_ceiling(Some("supermax"), Some("denoise")), Engine::Denoise);
    assert_eq!(effective_ceiling(Some("supermax"), None), HIGHEST_LOCAL);
}

/// The cap with an effect today: a ceiling below rung 1 sends a ring too noisy
/// for a flat fill back to a flat fill, rather than refusing the region.
#[test]
fn a_ceiling_below_rung_one_downgrades_rather_than_declining() {
    assert_eq!(engine_for(Route::FillAndDenoise, Engine::Flux), Some(Engine::Denoise));
    assert_eq!(engine_for(Route::FillAndDenoise, Engine::Denoise), Some(Engine::Denoise));
    assert_eq!(engine_for(Route::FillAndDenoise, Engine::Fill), Some(Engine::Fill));
    assert_eq!(engine_for(Route::Fill, Engine::Fill), Some(Engine::Fill));
}

/// The ladder exists now, and an inpaint route reaches it - at every ceiling
/// that permits rung 2, and at none that does not.
///
/// The asymmetry with the test above is the point. A `FillAndDenoise` region
/// under a ceiling of rung 0 gets a slightly worse patch; an `Inpaint` region
/// under one gets a hole where the screentone was, and §6's metric would not
/// catch it - a flat plate has no strokes to fail the edge test and its level
/// sits inside a halftone's own p5..p95.
#[test]
fn an_inpaint_route_reaches_rung_two_and_declines_below_it() {
    for ceiling in [Engine::Lama, Engine::Flux] {
        assert_eq!(engine_for(Route::Inpaint, ceiling), Some(Engine::Lama));
    }
    for ceiling in [Engine::Fill, Engine::Denoise] {
        assert_eq!(engine_for(Route::Inpaint, ceiling), None);
    }
}

/// **The picks move where a region starts and nothing else.** Bubble text
/// starts on the fill family even where the fit sent it to the inpainter -
/// which is the whole point of the row, a balloon being flat paper - and text
/// outside a balloon starts on the inpainter even where the fit would have
/// filled it.
#[test]
fn a_pick_moves_the_starting_rung_for_its_kind_of_text() {
    let picks = Picks::default();
    // The default the tool window ships: a bubble is filled, whatever the fit
    // made of the paper around the text.
    assert_eq!(
        start_rung(Route::Inpaint, Engine::Flux, Some(picks.bubble)),
        Some(Engine::Fill)
    );
    // And the rung the row names is the rung that runs, downward too: the
    // "rung 1 is part of the fill family, leave it alone" rule was retired,
    // because a row that says *Fill* and produces a denoise is a picker that
    // cannot be used to move between models.
    assert_eq!(
        start_rung(Route::FillAndDenoise, Engine::Flux, Some(picks.bubble)),
        Some(Engine::Fill)
    );
    // The other rows the window offers, each starting exactly where it says on
    // a route that would have gone somewhere else.
    assert_eq!(
        start_rung(Route::Fill, Engine::Flux, Some(EnginePick::Denoise)),
        Some(Engine::Denoise)
    );
    // Outside a balloon the art has to be put back, so the inpainter is where
    // it starts rather than where it escalates to.
    assert_eq!(
        start_rung(Route::Fill, Engine::Flux, Some(picks.outside)),
        Some(Engine::Lama)
    );
    // No pick is the route's own answer, unchanged.
    assert_eq!(start_rung(Route::Fill, Engine::Flux, None), Some(Engine::Fill));
    assert_eq!(start_rung(Route::Inpaint, Engine::Flux, None), Some(Engine::Lama));
}

/// **A pick is not a ceiling, in either direction.** It cannot reach past
/// `engineCeiling`, and it cannot talk an inpaint route down past the decline
/// [`engine_for`] states - §5's argument is about the paper under the region,
/// and a preference does not change what is under it.
#[test]
fn a_pick_can_neither_lift_the_ceiling_nor_undo_a_decline() {
    // A rung-2 pick under a ceiling that cannot inpaint keeps the routed rung.
    assert_eq!(
        start_rung(Route::Fill, Engine::Fill, Some(EnginePick::Lama)),
        Some(Engine::Fill)
    );
    assert_eq!(
        start_rung(Route::FillAndDenoise, Engine::Denoise, Some(EnginePick::Lama)),
        Some(Engine::Denoise)
    );
    // And an inpaint route under one still declines rather than being filled.
    for ceiling in [Engine::Fill, Engine::Denoise] {
        assert_eq!(start_rung(Route::Inpaint, ceiling, Some(EnginePick::Fill)), None);
        assert_eq!(start_rung(Route::Inpaint, ceiling, Some(EnginePick::Lama)), None);
    }
}

/// The seam's two names, and what an absent or mistyped one means: **this
/// type's own default**, not "no preference". The interface always sends both,
/// so an unreadable one is a typo and the defaults are what the window shows.
#[test]
fn the_picks_read_the_seams_names_and_default_rather_than_falling_open() {
    assert_eq!(
        Picks::from_args(Some("lama"), Some("fill")),
        Picks { bubble: EnginePick::Lama, outside: EnginePick::Fill }
    );
    assert_eq!(Picks::from_args(None, None), Picks::default());
    assert_eq!(Picks::from_args(Some("supermax"), Some("")), Picks::default());
    // Catalogue keys are accepted too, and so is `redraw` - the word the rows
    // used to send, kept readable so a stored preference is not silently
    // replaced by the default.
    assert_eq!(
        Picks::from_args(Some("ladder.rung.lama"), Some("denoise")),
        Picks { bubble: EnginePick::Lama, outside: EnginePick::Denoise }
    );
    assert_eq!(
        Picks::from_args(Some("redraw"), Some("inpaint")),
        Picks { bubble: EnginePick::Lama, outside: EnginePick::Lama }
    );
    // Neither rung above `HIGHEST_AUTOMATIC` is a pick: a run may not reach
    // them, so reading one as a start would be a promise the ceiling takes
    // straight back.
    assert_eq!(Picks::from_args(Some("flux"), Some("cloud")), Picks::default());
}

/// The escalation loop's list: at or above the routed rung, at or below the
/// ceiling, and only rungs that can carry this source.
#[test]
fn the_ladder_climbs_from_the_routed_rung_and_never_above_the_ceiling() {
    let page = fixtures::by_name("l8").raster;
    assert_eq!(
        ladder_from(Engine::Fill, Engine::Flux, &page),
        vec![Engine::Fill, Engine::Denoise, Engine::Lama]
    );
    // A region that started at rung 2 never falls back to a simpler engine:
    // §5's table is a floor and not a suggestion - and rung 2 is now the top of
    // the list, so this is also where the escalation ends.
    assert_eq!(ladder_from(Engine::Lama, Engine::Flux, &page), vec![Engine::Lama]);
    assert_eq!(
        ladder_from(Engine::Fill, Engine::Denoise, &page),
        vec![Engine::Fill, Engine::Denoise]
    );
    assert_eq!(ladder_from(Engine::Fill, Engine::Fill, &page), vec![Engine::Fill]);

    // Rung 3a is never on any list, at any ceiling.
    assert!(!ladder_from(Engine::Fill, Engine::Flux, &page).contains(&Engine::Flux));

    // And a source a rung cannot carry is not a rung the escalation names -
    // enforced one level up from the router.
    let bitonal = fixtures::by_name("bitonal").raster;
    assert_eq!(ladder_from(Engine::Fill, Engine::Flux, &bitonal), vec![Engine::Fill]);
    let indexed = fixtures::by_name("indexed-p").raster;
    assert_eq!(ladder_from(Engine::Fill, Engine::Flux, &indexed), vec![Engine::Fill]);
}

/// **Nothing above rung 2 is reachable from an automatic run**, at any ceiling,
/// from any starting rung, on any source.
///
/// This is the same ruling the cloud rung carries: `runClean` has no
/// confirmation protocol, and a batch job must not silently spend ten to
/// sixty seconds a region on a heavy local model, nor spawn a multi-gigabyte
/// child to do it. The rung is reached per region, deliberately, from
/// Content-aware fill.
///
/// This existed as a comment on [`ladder_from`] and as one `contains` check
/// inside another test. Neither survives somebody adding a rung to a list, and
/// the rung now exists - `cleaner_core::engines::flux` is callable today. So it
/// is pinned three ways, because there are three ways in:
///
/// 1. **Nothing starts there.** [`engine_for`] names a region's first rung.
/// 2. **Nothing escalates there.** [`ladder_from`] is the escalation's list.
/// 3. **Nothing renders there.** [`render_rung`] used to answer an unreachable
///    rung with a *wildcard* that produced rung 0's pixels - so a `Flux` that
///    somehow arrived would have been written to the manifest as a FLUX patch
///    over a planar fill. It refuses now.
///
/// **And [`HIGHEST_AUTOMATIC`] is LaMa**, which is what makes the three checks
/// above statements about the whole ladder rather than about two named rungs.
/// MI-GAN held rung 3 and the line between automatic and not ran above it; the
/// rung is gone and the line has come down to rung 2, so the strongest thing a
/// batch job can spend is the inpainter. Asserted on the constant *and* on
/// every list this loop produces, because the constant is the rule and the
/// lists are whether anything reads it.
#[test]
fn rung_3a_is_not_reachable_from_an_automatic_run() {
    let sources = [
        fixtures::by_name("l8").raster,
        fixtures::by_name("l16").raster,
        fixtures::by_name("rgb8").raster,
        fixtures::by_name("bitonal").raster,
        fixtures::by_name("indexed-p").raster,
    ];
    let rungs = [Engine::Fill, Engine::Denoise, Engine::Lama, Engine::Flux, Engine::Cloud];

    // The constant itself, and the predicate every filter reads it through.
    assert_eq!(HIGHEST_AUTOMATIC, Engine::Lama);
    for engine in [Engine::Fill, Engine::Denoise, Engine::Lama] {
        assert!(reachable_automatically(engine), "{engine:?}");
    }
    for engine in [Engine::Flux, Engine::Cloud] {
        assert!(!reachable_automatically(engine), "{engine:?}");
    }

    for page in &sources {
        for start in rungs {
            for ceiling in rungs {
                let ladder = ladder_from(start, ceiling, page);
                assert!(
                    !ladder.contains(&Engine::Flux),
                    "rung 3a reached the ladder from {start:?} under {ceiling:?}"
                );
                assert!(
                    !ladder.contains(&Engine::Cloud),
                    "rung 4 reached the ladder from {start:?} under {ceiling:?}"
                );
                // The same statement without naming a rung, so that a rung
                // added above LaMa is caught by this test rather than by the
                // two `contains` checks somebody would have to remember to
                // extend.
                for engine in &ladder {
                    assert!(
                        rung(*engine) <= rung(HIGHEST_AUTOMATIC),
                        "{engine:?} is above the automatic ceiling, from {start:?} under {ceiling:?}"
                    );
                }
            }
        }
    }

    // 1 - and the router is asked at every ceiling, including the two that
    // name rung 3a and above, because "highest permitted rung wins" is the
    // reading that would get this wrong.
    for ceiling in rungs {
        for route in [Route::Fill, Route::FillAndDenoise, Route::Inpaint] {
            assert_ne!(engine_for(route, ceiling), Some(Engine::Flux));
            assert_ne!(engine_for(route, ceiling), Some(Engine::Cloud));
        }
    }

    // And the stored setting cannot lift it either: `effective_ceiling` is
    // free to answer `Flux` - the rung is local, and a patch made by hand at
    // that rung is a real patch - but a ceiling of `Flux` buys an automatic run
    // exactly what a ceiling of `Lama` buys it.
    let page = &sources[0];
    assert_eq!(
        ladder_from(Engine::Fill, effective_ceiling(Some("flux"), None), page),
        ladder_from(Engine::Fill, Engine::Lama, page),
    );

    // 3 - the rung that cannot be reached is refused rather than filled.
    let page = gray_page(|x, y| if tone(x, y) { 40 } else { 235 });
    let fitted = fitted_over(&page, Rect::new(60, 60, 40, 30));
    for engine in [Engine::Flux, Engine::Cloud] {
        let rendered = render_rung(&mut no_rung_two(), engine, &page, &fitted, 0.0)
            .expect("an unreachable rung faulted rather than refusing");
        match rendered {
            Rendered::Refused(key) => assert_eq!(key, "decline.reason.rungUnavailable"),
            Rendered::Made(_) => panic!("{engine:?} produced a patch from an automatic run"),
        }
    }
}

/// The ceiling the scheduler is given is the ceiling the cleaner sees, on every
/// page. A run that lifted it after the first page would be the same defect
/// with a longer fuse.
#[test]
fn every_page_of_a_run_is_cleaned_under_the_same_ceiling() {
    let scratch = Scratch::new("ceiling-per-page");
    let (library, _, chapter_id) = a_chapter(&scratch, 3);
    let recorder = Recorder::new();
    let mut stub = Stub::new(1, 0);
    let ceilings = Arc::clone(&stub.ceilings);
    execute(
        "run-1",
        &chapter_id,
        &plan(&library, "chapter", &chapter_id, None).unwrap(),
        &mut stub,
        Engine::Denoise,
        &recorder.cancel,
        &recorder.sink(),
    );
    assert_eq!(*ceilings.lock().unwrap(), vec![Engine::Denoise; 3]);
}

/* ------------------------------------------------------------------ */
/* Try, escalate, decline                                              */
/* ------------------------------------------------------------------ */

// The escalation loop is written against [`Rung2`] rather than against
// [`Pipeline`] for the reason this file's header gives about [`Cleaner`]: rung
// 2's weights are not in the repository, and every rule below is a property
// of the loop rather than of the model.
// So the loop is exercised here with the session deliberately absent, and
// `Rung2::state` says afterwards whether one was ever asked for - which is the
// memory claim itself, not a proxy for it.

/// A rung 2 that is not on this machine: every checkout that has not run
/// `scripts/fetch-models.sh`, and every CI runner.
fn no_rung_two() -> Rung2 {
    Rung2::new(Path::new("no-weights-live-here"), Preference::CpuOnly)
}

const SYNTHETIC: u32 = 200;

/// An 8 px halftone grid - `fit::ring`'s own screentone, which routes to the
/// ladder by construction.
fn tone(x: u32, y: u32) -> bool {
    x % 8 < 3 && y % 8 < 3
}

fn gray_page(f: impl Fn(u32, u32) -> u8) -> Raster {
    let mut data = Vec::with_capacity((SYNTHETIC * SYNTHETIC) as usize);
    for y in 0..SYNTHETIC {
        for x in 0..SYNTHETIC {
            data.push(f(x, y));
        }
    }
    Raster {
        width: SYNTHETIC,
        height: SYNTHETIC,
        mode: ColorMode::Gray,
        depth: BitDepth::Eight,
        icc: None,
        palette: None,
        trns: None,
        srgb_intent: None,
        data,
    }
}

/// The same halftone at one bit per sample, packed the way a 1-bit PNG is.
fn bitonal_page() -> Raster {
    let stride = (SYNTHETIC as usize).div_ceil(8);
    let mut data = vec![0u8; stride * SYNTHETIC as usize];
    for y in 0..SYNTHETIC {
        for x in 0..SYNTHETIC {
            if !tone(x, y) {
                data[y as usize * stride + x as usize / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    Raster {
        width: SYNTHETIC,
        height: SYNTHETIC,
        mode: ColorMode::Gray,
        depth: BitDepth::One,
        icc: None,
        palette: None,
        trns: None,
        srgb_intent: None,
        data,
    }
}

fn fitted_over(page: &Raster, rect: Rect) -> fit::Fitted {
    fit::fit(
        page,
        &Mask::filled(rect),
        1.0,
        fit::page_noise_sigma(page),
        &EdgeMap::none(page.width, page.height),
        false,
    )
}

fn attempt(rung2: &mut Rung2, page: &Raster, fitted: &fit::Fitted, ceiling: Engine) -> Attempt {
    clean_region(
        rung2,
        page,
        fitted,
        ceiling,
        // No pick: these are assertions about the route's own rung and the
        // escalation above it, which is what an unpicked region gets.
        None,
        fit::page_noise_sigma(page),
    )
    .expect("no rung faulted")
}

/// The memory claim, asserted rather than described. A manga-LaMa session
/// costs 2.1 s and about 510 MB, and a page of flat-paper dialogue must not
/// pay a byte of it.
#[test]
fn a_page_of_flat_paper_never_pays_for_a_lama_session() {
    let page = gray_page(|_, _| 246);
    let mut rung2 = no_rung_two();
    for x in [40, 88, 140] {
        let fitted = fitted_over(&page, Rect::new(x, 90, 24, 20));
        assert_eq!(fitted.route, Route::Fill);
        match attempt(&mut rung2, &page, &fitted, Engine::Flux) {
            Attempt::Cleaned(made, verdict) => {
                assert_eq!(made.engine, Engine::Fill);
                assert_eq!(verdict.outcome, quality::Outcome::Accepted, "{verdict:?}");
                // No model ran, so there is no digest and there are no tiles.
                assert_eq!(made.model_sha256, None);
                assert_eq!(made.tiles, None);
                assert_eq!(made.provider, None);
            }
            Attempt::Declined(reason) => panic!("flat paper was declined: {reason}"),
        }
    }
    assert!(
        matches!(rung2.state, Session::Unopened),
        "a page of flat-paper dialogue opened an inpainter"
    );
}

/// A region §5 sent to the ladder, on a machine that has no ladder to send it
/// to. It is **left alone** rather than filled by a rung that was already
/// refused - and the session is asked for once, not once per region.
#[test]
fn a_ladder_region_with_no_rung_two_is_left_alone_and_says_why() {
    let page = gray_page(|x, y| if tone(x, y) { 40 } else { 250 });
    let fitted = fitted_over(&page, Rect::new(88, 90, 24, 20));
    assert_eq!(fitted.route, Route::Inpaint, "the screentone did not reach the ladder");

    let mut rung2 = no_rung_two();
    match attempt(&mut rung2, &page, &fitted, Engine::Flux) {
        Attempt::Declined(reason) => assert_eq!(reason, "decline.reason.rungUnavailable"),
        Attempt::Cleaned(made, _) => {
            panic!("a screentone region was filled by {:?} with no inpainter", made.engine)
        }
    }
    assert!(matches!(rung2.state, Session::Unavailable), "the failure was not remembered");
}

/// The same region under a ceiling below rung 2: declined **without asking any
/// engine at all**, which is why it carries the reason it does rather than
/// §6's.
///
/// It does not downgrade to rung 0, and that asymmetry with `FillAndDenoise` is
/// deliberate: a flat plate over a halftone is a hole in the tone, and §6's
/// metric passes it - the plate has no strokes to fail the edge test and its
/// level sits inside the tone's own p5..p95.
#[test]
fn a_ladder_region_under_a_low_ceiling_declines_without_asking_any_engine() {
    let page = gray_page(|x, y| if tone(x, y) { 40 } else { 250 });
    let fitted = fitted_over(&page, Rect::new(88, 90, 24, 20));
    let mut rung2 = no_rung_two();
    for ceiling in [Engine::Fill, Engine::Denoise] {
        match attempt(&mut rung2, &page, &fitted, ceiling) {
            Attempt::Declined(reason) => assert_eq!(reason, "decline.reason.rungUnavailable"),
            Attempt::Cleaned(made, _) => {
                panic!("{:?} filled a screentone region at ceiling {ceiling:?}", made.engine)
            }
        }
    }
    assert!(matches!(rung2.state, Session::Unopened), "a capped run reached for the session");
}

/// **A bitonal scan cleaned nothing at all.** Every annulus on a 1-bit source is
/// dither, so §5 routed every region of the page to the ladder, and rung 2
/// declines a 1-bit source outright ([`lama::Decline::Depth`]) - the engine
/// right and the router wrong. Rung 0's planar fill serves bitonal and always
/// has.
#[test]
fn a_bitonal_page_cleans_at_rung_zero_rather_than_declining_wholesale() {
    let page = bitonal_page();
    let mut rung2 = no_rung_two();
    for x in [40, 88, 140] {
        let fitted = fitted_over(&page, Rect::new(x, 90, 24, 20));
        match attempt(&mut rung2, &page, &fitted, Engine::Flux) {
            Attempt::Cleaned(made, _) => assert_eq!(made.engine, Engine::Fill),
            Attempt::Declined(reason) => panic!("a bitonal region was declined: {reason}"),
        }
    }
    assert!(
        matches!(rung2.state, Session::Unopened),
        "a bitonal page reached for the one rung that refuses it"
    );
}

/// §6's two tests are two facts, and a reviewer acting on them acts
/// differently: one says the engine put strokes on the paper, the other says it
/// put a tone that is not the paper's.
#[test]
fn the_two_halves_of_the_quality_metric_are_two_reasons() {
    assert_eq!(decline_key(quality::Cause::EdgeEnergy), "decline.reason.edgeEnergy");
    assert_eq!(decline_key(quality::Cause::Histogram), "decline.reason.histogram");
    assert_ne!(decline_key(quality::Cause::EdgeEnergy), decline_key(quality::Cause::Histogram));
}

/* ------------------------------------------------------------------ */
/* What reaches the seam                                               */
/* ------------------------------------------------------------------ */

/// `fill_mode` and `elapsed_ms` were reconstructed - one
/// guessed from the rung, the other defaulted to zero - because nothing wrote
/// them. The run writes both into `params_snapshot`, and this pins that the
/// recorded values are what reach the seam.
#[test]
fn a_recorded_fill_mode_and_elapsed_time_reach_the_mask_on_the_seam() {
    let scratch = Scratch::new("provenance");
    let (library, _, chapter_id) = a_chapter(&scratch, 1);
    let recorder = Recorder::new();
    execute(
        "run-1",
        &chapter_id,
        &plan(&library, "chapter", &chapter_id, None).unwrap(),
        &mut Stub::new(1, 0),
        Engine::Flux,
        &recorder.cancel,
        &recorder.sink(),
    );

    let region = recorder
        .events()
        .into_iter()
        .find_map(|event| match event {
            Event::RegionDone { region, .. } => Some(*region),
            _ => None,
        })
        .expect("no region reached the seam");
    let mask = region.mask.expect("a cleaned region with no mask");
    assert_eq!(mask.fill_mode, "match-surround");
    assert_eq!(mask.elapsed_ms, 41);
    assert_eq!(region.outcome, "cleaned");
    assert_eq!(region.id, format!("{chapter_id}-p001-r0"));
    assert_eq!(mask.id, format!("{chapter_id}-p001-r0-m1"));
}

/// The snapshot the real pipeline writes, over a ring sampled from a fixture.
/// Same two fields, at the other end of the same path.
#[test]
fn the_pipeline_writes_the_fill_mode_and_the_time_it_took() {
    let page = fixtures::by_name("l8").raster;
    let mask = Mask::filled(Rect::new(4, 4, 6, 6));
    let edges = EdgeMap::none(page.width, page.height);
    let paper = cleaner_core::fit::ring::paper_annulus(&page, &mask, 0, &edges);
    let fitted = fit::Fitted {
        ring: cleaner_core::fit::ring::sample_over(&page, &paper),
        paper,
        // Rung 0's own mask, and a rung-0 snapshot: `ink` is only ever read by a
        // model rung, and here it is the whole of the mask anyway.
        ink: mask.clone(),
        mask,
        route: Route::Fill,
        thickness: 9,
        best_deviation: 1.5,
    };
    let measured = quality::assess(
        &page,
        &fitted.mask,
        &cleaner_core::engines::fill::render(&page, &fitted),
        0.0,
    );
    let snapshot = params_snapshot(
        Engine::Fill,
        &fitted,
        Duration::from_millis(37),
        EdgePad::Replicate,
        None,
        &measured,
    );
    assert_eq!(snapshot["fill_mode"], "match-surround");
    assert_eq!(snapshot["elapsed_ms"], 37);
    assert_eq!(snapshot["rung"], "ladder.rung.fill");
    assert_eq!(snapshot["thickness"], 9);
    assert_eq!(snapshot["source"], "auto");
    // Which edge pad was used, recorded in `params_snapshot`.
    assert_eq!(snapshot["edge_pad"], "edge-replicate");
    // No model ran, so there are no tiles - absent rather than zero, which
    // would read as a model that did nothing.
    assert_eq!(snapshot.get("tiles"), None);
    assert_eq!(
        params_snapshot(Engine::Fill, &fitted, Duration::ZERO, EdgePad::None, None, &measured)
            ["edge_pad"],
        "none"
    );

    // Rung 2's half of the same record: a reconstruction, and the number of
    // model runs it cost.
    let inpainted =
        params_snapshot(Engine::Lama, &fitted, Duration::ZERO, EdgePad::None, Some(4), &measured);
    assert_eq!(inpainted["fill_mode"], "reconstruct");
    assert_eq!(inpainted["tiles"], 4);
}

/// The metric's silence is recorded rather than passed off as a verdict -
/// "a check that silently answers accepted" is the failure this whole family
/// is at risk of.
#[test]
fn a_patch_nothing_could_measure_says_so_in_its_snapshot() {
    let page = fixtures::by_name("l8").raster;
    let mask = Mask::filled(Rect::new(4, 4, 6, 6));
    let edges = EdgeMap::none(page.width, page.height);
    let paper = cleaner_core::fit::ring::paper_annulus(&page, &mask, 0, &edges);
    let fitted = fit::Fitted {
        ring: cleaner_core::fit::ring::sample_over(&page, &paper),
        paper,
        // Rung 0's own mask, and a rung-0 snapshot: `ink` is only ever read by a
        // model rung, and here it is the whole of the mask anyway.
        ink: mask.clone(),
        mask,
        route: Route::Fill,
        thickness: 9,
        best_deviation: 1.5,
    };
    let measured = quality::assess(
        &page,
        &fitted.mask,
        &cleaner_core::engines::fill::render(&page, &fitted),
        0.0,
    );
    assert_eq!(
        params_snapshot(Engine::Fill, &fitted, Duration::ZERO, EdgePad::None, None, &measured)
            ["quality"],
        "measured"
    );

    let bitonal = fixtures::by_name("bitonal").raster;
    let unmeasured = quality::assess(&bitonal, &fitted.mask, &page, 0.0);
    assert_eq!(
        params_snapshot(Engine::Fill, &fitted, Duration::ZERO, EdgePad::None, None, &unmeasured)
            ["quality"],
        "unmeasured"
    );
}

/// A region the gate held back is written down and reaches the seam as a
/// gate-skipped region with the cause the gate gave - it is the review list's
/// other half, and a page that silently lost it would show a clean page with no
/// record that anything was deliberately left alone.
#[test]
fn a_region_the_gate_held_back_is_recorded_and_flagged() {
    let scratch = Scratch::new("gated");
    let (library, _, chapter_id) = a_chapter(&scratch, 1);
    let entries = plan(&library, "chapter", &chapter_id, None).unwrap();
    let recorder = Recorder::new();
    let summary = execute(
        "run-1",
        &chapter_id,
        &entries,
        &mut Stub::new(0, 2),
        Engine::Flux,
        &recorder.cancel,
        &recorder.sink(),
    );

    assert_eq!(summary.regions_cleaned, 0, "a held-back region was counted as cleaned");
    let job = Job::open(&entries[0].job_path).unwrap();
    assert_eq!(job.project.regions_untouched.len(), 2);
    assert_eq!(job.project.counters.gate_dropped, 2);

    let page = match recorder.events().into_iter().find(|e| kind_of(e) == "page-done") {
        Some(Event::PageDone { page, .. }) => *page,
        _ => panic!("no page-done"),
    };
    assert_eq!(page.regions.len(), 2);
    assert!(page.regions.iter().all(|region| region.outcome == "gate-skipped"));
    assert!(page.regions.iter().all(|region| region.gate_skip_cause == Some("not-japanese")));
    // Held back is not cleaned, and a page of held-back regions is not a
    // cleaned page in the sense of having had anything done to it - but it has
    // been examined, so it is finished.
    assert_eq!(page.status, "cleaned");
    assert!(plan(&library, "chapter", &chapter_id, None).unwrap().is_empty());
}

/// A page that could not be read is counted, reported finished so the `●` comes
/// off it, and left in the queue for the next run.
#[test]
fn a_page_that_could_not_be_cleaned_is_counted_and_stays_queued() {
    let scratch = Scratch::new("errored");
    let (library, _, chapter_id) = a_chapter(&scratch, 2);
    let entries = plan(&library, "chapter", &chapter_id, None).unwrap();
    let mut stub = Stub::new(1, 0);
    stub.fails = vec![library::page_id(&chapter_id, 0)];

    let recorder = Recorder::new();
    let summary = execute(
        "run-1",
        &chapter_id,
        &entries,
        &mut stub,
        Engine::Flux,
        &recorder.cancel,
        &recorder.sink(),
    );

    assert_eq!(summary.errored, 1);
    assert_eq!(summary.pages_cleaned, 1);
    assert_eq!(summary.reason, "completed");
    assert_eq!(recorder.kinds().iter().filter(|k| **k == "page-done").count(), 2);
    let job = Job::open(&entries[0].job_path).unwrap();
    assert_eq!(job.project.counters.errored, 1);
    assert_eq!(
        plan(&library, "chapter", &chapter_id, None)
            .unwrap()
            .iter()
            .map(|entry| entry.page_index)
            .collect::<Vec<_>>(),
        vec![0]
    );
}

/// Every region is flushed as it completes, so a crash mid-run loses the region
/// in flight and nothing else. Read back off disk rather than out of the job in
/// memory, because that is the only version a crash would leave.
#[test]
fn every_completed_region_is_on_disk_before_the_next_one_starts() {
    let scratch = Scratch::new("flush");
    let (library, _, chapter_id) = a_chapter(&scratch, 2);
    let entries = plan(&library, "chapter", &chapter_id, None).unwrap();
    let job_path = entries[0].job_path.clone();
    let counted = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&counted);
    let cancel = AtomicBool::new(false);
    execute(
        "run-1",
        &chapter_id,
        &entries,
        &mut Stub::new(2, 0),
        Engine::Flux,
        &cancel,
        &|event| {
            if kind_of(&event) == "region-done" {
                let on_disk = Job::open(&job_path).unwrap().project.patches.len();
                seen.lock().unwrap().push(on_disk);
            }
        },
    );
    assert_eq!(
        *counted.lock().unwrap(),
        vec![1, 2, 3, 4],
        "a region was reported before it was written down"
    );
}

/* ------------------------------------------------------------------ */
/* A run that does not come back                                       */
/* ------------------------------------------------------------------ */

/// A panic in the pipeline is still a run that finished, and the seam's rule
/// says so: **exactly one `run-finished`, whatever happened**. Without the
/// guard the event never went out - the page kept its `●` for the session, the
/// active slot stayed claimed so every later `runClean` answered
/// `alreadyRunning`, and `cancelRun` waited the full `CANCEL_WAIT` before naming
/// a run that had already stopped.
#[test]
fn a_panicking_pipeline_still_finishes_the_run_and_frees_the_scheduler() {
    let _exclusive = exclusively();
    let scratch = Scratch::new("panic");
    let (library, _, chapter_id) = a_chapter(&scratch, 3);
    let entries = plan(&library, "chapter", &chapter_id, None).unwrap();
    let mut stub = Stub::new(1, 0);
    stub.panics = vec![library::page_id(&chapter_id, 1)];

    let recorder = Recorder::new();
    let worker = a_worker(&chapter_id, &recorder);
    let summary = quietly(|| worker.run(&entries, &mut stub, Engine::Flux, &recorder.sink()));

    let finished = recorder.kinds().iter().filter(|kind| **kind == "run-finished").count();
    assert_eq!(finished, 1, "a panicked run emitted {finished} run-finished events");
    assert_eq!(
        recorder.kinds().last(),
        Some(&"notice"),
        "the run ended on something other than its notice"
    );
    assert_eq!(summary.reason, "cancelled", "a panicked run has not completed");
    assert_eq!(summary.next_page_index, Some(1), "the page it stopped inside is the resume point");
    assert_eq!(summary.pages_cleaned, 1, "the page before it was still finished");

    // The slot is free, so the next run can start. This is the whole of what
    // `alreadyRunning` for the rest of the session was.
    assert!(
        active().lock().unwrap().is_none(),
        "a panicked run kept the scheduler for the rest of the session"
    );
    assert!(*worker.finished.0.lock().unwrap(), "a cancelRun would have waited out the timeout");

    // And the manifest can say where to pick up, which needs the walk's own
    // `close` to have been stood in for.
    assert_eq!(Job::open(&entries[0].job_path).unwrap().project.interrupted_at, Some(1));
    assert_eq!(
        plan(&library, "chapter", &chapter_id, None)
            .unwrap()
            .iter()
            .map(|entry| entry.page_index)
            .collect::<Vec<_>>(),
        vec![1, 2],
        "the page the panic landed on was not left in the queue"
    );
}

/// The exit `execute` cannot catch: a subscriber that panics on the way out.
/// The slot is handed back by a guard rather than by a line at the end, so it
/// is handed back on this path too.
#[test]
fn a_panic_on_the_way_out_still_hands_the_scheduler_back() {
    let _exclusive = exclusively();
    let scratch = Scratch::new("panic-sink");
    let (library, _, chapter_id) = a_chapter(&scratch, 1);
    let entries = plan(&library, "chapter", &chapter_id, None).unwrap();
    let recorder = Recorder::new();
    let worker = a_worker(&chapter_id, &recorder);

    let panicked = quietly(|| {
        std::panic::catch_unwind(AssertUnwindSafe(|| {
            worker.run(&entries, &mut Stub::new(1, 0), Engine::Flux, &|event| {
                assert_ne!(kind_of(&event), "run-finished", "a sink that throws on the way out");
            })
        }))
    });
    assert!(panicked.is_err(), "the sink did not panic and the guard was not exercised");
    assert!(
        active().lock().unwrap().is_none(),
        "a panic outside the scheduler kept the slot for the rest of the session"
    );
}

/* ------------------------------------------------------------------ */
/* One writer per job                                                  */
/* ------------------------------------------------------------------ */

/// The run holds the job it is writing for as long as it has it open, so no
/// other command in the window can read it, mutate it and flush over what the
/// run has since written. The exposure is what made this worth closing: one
/// `Job` is held for a **whole chapter** and the manifest is rewritten after
/// every region.
#[test]
fn the_run_holds_its_job_for_as_long_as_it_is_open() {
    let scratch = Scratch::new("job-held");
    let (library, _, chapter_id) = a_chapter(&scratch, 2);
    let entries = plan(&library, "chapter", &chapter_id, None).unwrap();
    let job_path = entries[0].job_path.clone();
    assert!(job_is_free(&job_path), "the job was held before the run started");

    let free_during = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&free_during);
    let cancel = AtomicBool::new(false);
    execute(
        "backend-run-1",
        &chapter_id,
        &entries,
        &mut Stub::new(1, 0),
        Engine::Flux,
        &cancel,
        &|event| {
            if kind_of(&event) == "region-done" {
                seen.lock().unwrap().push(job_is_free(&job_path));
            }
        },
    );
    assert_eq!(*free_during.lock().unwrap(), vec![false, false], "the run left its job writable");
    assert!(job_is_free(&job_path), "the run kept the job after it had finished with it");
}

/// And a second writer waits rather than interleaving. Within one process: two
/// copies of the application still race, which is the single-instance lock
/// and is Phase 6's.
#[test]
fn a_second_writer_waits_for_the_job_rather_than_interleaving() {
    let scratch = Scratch::new("job-lock");
    let path = scratch.join("c1.mtclean");
    let order = Arc::new(Mutex::new(Vec::new()));

    let held = lock_job(&path);
    let second = {
        let path = path.clone();
        let order = Arc::clone(&order);
        std::thread::spawn(move || {
            let _held = lock_job(&path);
            order.lock().unwrap().push("second");
        })
    };
    // Long enough that a lock that did not hold would let the second writer
    // through first, which is the failure this pins.
    std::thread::sleep(Duration::from_millis(80));
    order.lock().unwrap().push("first");
    drop(held);
    second.join().unwrap();

    assert_eq!(*order.lock().unwrap(), vec!["first", "second"]);
    assert!(job_is_free(&path), "the lock was not given back");
}

/* ------------------------------------------------------------------ */
/* Honest counters                                                     */
/* ------------------------------------------------------------------ */

/// `counters` is the basis of honest statistics. A cancel-and-resume must
/// therefore end on the same numbers an uninterrupted run does: the resumed
/// page's records are dropped and written again, and a counter that only
/// counted the writing climbed on every cycle.
#[test]
fn a_cancel_and_resume_ends_on_the_same_counters_as_an_uninterrupted_run() {
    let straight = Scratch::new("counters-straight");
    let (library, _, chapter_id) = a_chapter(&straight, 3);
    let recorder = Recorder::new();
    execute(
        "backend-run-1",
        &chapter_id,
        &plan(&library, "chapter", &chapter_id, None).unwrap(),
        &mut Stub::held_back_first(2, 2, 1),
        Engine::Flux,
        &recorder.cancel,
        &recorder.sink(),
    );
    let uninterrupted = Job::open(&library.resolve_chapter(&chapter_id).unwrap())
        .unwrap()
        .project
        .counters;
    assert_eq!(uninterrupted.gate_dropped, 6);
    assert_eq!(uninterrupted.declined, 3);

    // The same work, cancelled inside page 0 - after its held-back regions were
    // written down and before the page was finished - and then resumed.
    let interrupted = Scratch::new("counters-resumed");
    let (library, _, chapter_id) = a_chapter(&interrupted, 3);
    let first = Recorder::tripping_on("region-done");
    let summary = execute(
        "backend-run-1",
        &chapter_id,
        &plan(&library, "chapter", &chapter_id, None).unwrap(),
        &mut Stub::held_back_first(2, 2, 1),
        Engine::Flux,
        &first.cancel,
        &first.sink(),
    );
    assert_eq!(summary.next_page_index, Some(0), "the cancel did not land inside page 0");

    let resumed = plan(&library, "chapter", &chapter_id, None).unwrap();
    assert_eq!(resumed.first().map(|entry| entry.page_index), Some(0), "page 0 was not redone");
    let second = Recorder::new();
    execute(
        "backend-run-2",
        &chapter_id,
        &resumed,
        &mut Stub::held_back_first(2, 2, 1),
        Engine::Flux,
        &second.cancel,
        &second.sink(),
    );

    let job = Job::open(&library.resolve_chapter(&chapter_id).unwrap()).unwrap();
    assert_eq!(job.project.counters, uninterrupted, "a resumed run counted page 0 twice");
    assert_eq!(job.project.regions_untouched.len(), 9, "the rows were doubled too");
    assert_eq!(job.project.patches.len(), 6);
}

/// The other half: a page that fails is counted once however many runs try it.
/// It is deliberately never marked examined, so every run picks it up again -
/// and every run used to add to `counters.errored` for the same corrupt file.
#[test]
fn a_page_that_fails_on_every_run_is_counted_once() {
    let scratch = Scratch::new("errored-twice");
    let (library, _, chapter_id) = a_chapter(&scratch, 2);
    let mut stub = Stub::new(1, 0);
    stub.fails = vec![library::page_id(&chapter_id, 0)];

    for run in 1..=3 {
        let recorder = Recorder::new();
        let entries = plan(&library, "chapter", &chapter_id, None).unwrap();
        let summary = execute(
            &format!("backend-run-{run}"),
            &chapter_id,
            &entries,
            &mut stub,
            Engine::Flux,
            &recorder.cancel,
            &recorder.sink(),
        );
        assert_eq!(summary.errored, 1, "run {run} reported the wrong number of failures");
        let job = Job::open(&entries[0].job_path).unwrap();
        assert_eq!(
            job.project.counters.errored, 1,
            "the manifest counted one corrupt page {run} times"
        );
        assert_eq!(job.project.errored, vec![0]);
    }

    // And it goes back down when the page succeeds.
    stub.fails.clear();
    let recorder = Recorder::new();
    let entries = plan(&library, "chapter", &chapter_id, None).unwrap();
    execute(
        "backend-run-4",
        &chapter_id,
        &entries,
        &mut stub,
        Engine::Flux,
        &recorder.cancel,
        &recorder.sink(),
    );
    let job = Job::open(&entries[0].job_path).unwrap();
    assert_eq!(job.project.counters.errored, 0, "a page that was fixed still counts as errored");
    assert!(job.project.errored.is_empty());
}

/* ------------------------------------------------------------------ */
/* A region that cannot be written                                     */
/* ------------------------------------------------------------------ */

/// A full disk, in the shape it actually takes: every region of every page
/// fails to write. The failure used to be swallowed - no event, no counter -
/// and the page was marked examined anyway, so the regions were lost for good
/// and **the chapter reported that no text was detected**, which is the
/// field saying the opposite of the truth.
#[test]
fn a_region_that_cannot_be_written_leaves_the_page_queued_and_the_chapter_honest() {
    let scratch = Scratch::new("full-disk");
    let (library, _, chapter_id) = a_chapter(&scratch, 2);
    let entries = plan(&library, "chapter", &chapter_id, None).unwrap();
    // The sidecar directory is where a region's mask and pixels go. Without it
    // every `complete_region` fails at its first write, one region at a time,
    // which is what a disk with nothing left on it does.
    let sidecar = PathBuf::from(format!("{}.d", entries[0].job_path.display()));
    std::fs::remove_dir_all(&sidecar).unwrap();

    let recorder = Recorder::new();
    let summary = execute(
        "backend-run-1",
        &chapter_id,
        &entries,
        &mut Stub::new(2, 0),
        Engine::Flux,
        &recorder.cancel,
        &recorder.sink(),
    );

    assert_eq!(summary.errored, 2, "a region that could not be written was not counted");
    assert_eq!(summary.pages_cleaned, 0);
    assert_eq!(summary.regions_cleaned, 0);
    // Every page still opens and closes: the `●` comes off even here.
    assert_eq!(recorder.kinds().iter().filter(|kind| **kind == "page-started").count(), 2);
    assert_eq!(recorder.kinds().iter().filter(|kind| **kind == "page-done").count(), 2);

    let job = Job::open(&entries[0].job_path).unwrap();
    assert!(job.project.examined.is_empty(), "a page whose regions were lost was marked examined");
    assert_eq!(job.project.counters.errored, 2);

    let chapter = library
        .list_projects()
        .unwrap()
        .into_iter()
        .flat_map(|project| project.chapters)
        .find(|chapter| chapter.id == chapter_id)
        .unwrap();
    assert!(
        !chapter.no_text_detected,
        "a chapter whose every region failed to write reported that it had no text in it"
    );

    // And the work is still there to be done when there is somewhere to put it.
    assert_eq!(
        plan(&library, "chapter", &chapter_id, None)
            .unwrap()
            .iter()
            .map(|entry| entry.page_index)
            .collect::<Vec<_>>(),
        vec![0, 1],
    );
}

/* ------------------------------------------------------------------ */
/* Events with what is known                                           */
/* ------------------------------------------------------------------ */

/// A `page-started` with no `page-done` leaves the page marked as cleaning for
/// the rest of the session. So a page the manifest can no longer describe is
/// reported with what the run itself knows, rather than not reported at all.
#[test]
fn a_page_the_manifest_cannot_describe_is_still_finished() {
    let scratch = Scratch::new("no-source");
    let (library, _, chapter_id) = a_chapter(&scratch, 2);
    let entries = plan(&library, "chapter", &chapter_id, None).unwrap();
    // A manifest that lists the pages and no longer knows what is under them.
    let mut job = Job::open(&entries[0].job_path).unwrap();
    job.project.sources.clear();
    job.flush().unwrap();

    let recorder = Recorder::new();
    execute(
        "backend-run-1",
        &chapter_id,
        &entries,
        &mut Stub::new(1, 0),
        Engine::Flux,
        &recorder.cancel,
        &recorder.sink(),
    );
    assert_eq!(
        recorder.kinds(),
        vec!["page-started", "page-done", "page-started", "page-done", "run-finished"],
        "a page that was started was never finished"
    );
    let page = match recorder.events().into_iter().find(|event| kind_of(event) == "page-done") {
        Some(Event::PageDone { page, .. }) => *page,
        _ => panic!("no page-done"),
    };
    assert_eq!(page.id, library::page_id(&chapter_id, 0));
    assert_eq!(page.index, 0);
    assert_eq!(page.number, 1);
    assert!(page.regions.is_empty());
}

/// `run-finished.regionsCleaned` counts the regions the interface was told
/// about, and not one more: the counter is incremented beside the event, and
/// the event goes out with what is known rather than being dropped.
#[test]
fn every_counted_region_reached_the_stream() {
    let scratch = Scratch::new("counted-regions");
    let (library, _, chapter_id) = a_chapter(&scratch, 3);
    let recorder = Recorder::new();
    let summary = execute(
        "backend-run-1",
        &chapter_id,
        &plan(&library, "chapter", &chapter_id, None).unwrap(),
        &mut Stub::new(2, 1),
        Engine::Flux,
        &recorder.cancel,
        &recorder.sink(),
    );
    let reported = recorder.kinds().iter().filter(|kind| **kind == "region-done").count();
    assert_eq!(summary.regions_cleaned as usize, reported);
    assert_eq!(reported, 6);
}

/* ------------------------------------------------------------------ */
/* The commands                                                        */
/* ------------------------------------------------------------------ */

/// A cancel that arrives after the run has already finished names nothing. The
/// window is real - `run-finished` has gone out saying `completed` and the slot
/// is not cleared until the worker's release runs - and answering with the run
/// id in it tells a caller its cancel took, a moment after the event stream
/// told it the run completed.
#[test]
fn a_cancel_that_arrives_after_the_run_completed_names_nothing() {
    let _exclusive = exclusively();
    let slot = |reason: Option<&'static str>| Active {
        run_id: "backend-run-7".to_owned(),
        cancel: Arc::new(AtomicBool::new(false)),
        finished: Arc::new((Mutex::new(true), Condvar::new())),
        outcome: Arc::new(Mutex::new(reason)),
    };

    *active().lock().unwrap() = Some(slot(Some("completed")));
    assert_eq!(request_cancel(None).unwrap(), None, "a completed run was reported as cancelled");
    assert_eq!(request_cancel(Some("backend-run-7".into())).unwrap(), None);

    // A run that is still going, and one that stopped because of a cancel, are
    // both cancellable and both name themselves.
    *active().lock().unwrap() = Some(slot(None));
    assert_eq!(request_cancel(None).unwrap(), Some("backend-run-7".to_owned()));
    *active().lock().unwrap() = Some(slot(Some("cancelled")));
    assert_eq!(request_cancel(None).unwrap(), Some("backend-run-7".to_owned()));

    // Somebody else's run id is not this one, whatever state it is in.
    assert_eq!(request_cancel(Some("run-7".into())).unwrap(), None);
    *active().lock().unwrap() = None;
    assert_eq!(request_cancel(None).unwrap(), None);
}

/// The ids the backend mints cannot be the mock's. `subscribe` is a merge of
/// both streams, `applyTool({tool: 'autoClean'})` is still the mock's and starts
/// a real mock run, and `editor.run` holds exactly one run id - so two live
/// schedulers minting `run-1` each would be one run as far as the interface is
/// concerned.
#[test]
fn a_run_id_cannot_collide_with_the_mocks() {
    let first = next_run_id();
    let second = next_run_id();
    assert!(first.starts_with("backend-run-"), "{first} could be the mock's");
    assert!(second.starts_with("backend-run-"));
    assert_ne!(first, second);
}


/// The same check `library.rs` makes, for the same reason: a command without
/// `async` is expanded through `body_blocking` and runs *inline* on the invoke
/// handler's thread. A run that opened three ONNX sessions there would hold the
/// webview for the whole of it.
#[test]
fn every_command_is_async_and_so_never_runs_on_the_invoke_handlers_thread() {
    fn check<A, B, C, D, E, F, G, H, R: std::future::Future>(_: fn(A, B, C, D, E, F, G, H) -> R) {}
    fn check1<A, R: std::future::Future>(_: fn(A) -> R) {}
    fn check3<A, B, C, R: std::future::Future>(_: fn(A, B, C) -> R) {}
    check(run_clean);
    check1(cancel_run);
    check3(resume_job);
}

/// The seam's own names, on the wire. `runClean` resolves with the queue and
/// `runId` is null when nothing was queued.
#[test]
fn the_run_handle_is_serialised_in_the_names_the_seam_declares() {
    let handle = RunHandle {
        run_id: Some("run-1".into()),
        pages: vec![QueuedPage {
            chapter_id: "c1".into(),
            page_id: "c1-p001".into(),
            page_index: 0,
        }],
        already_running: None,
    };
    let value = serde_json::to_value(&handle).unwrap();
    assert_eq!(value["runId"], "run-1");
    assert_eq!(value["pages"][0]["chapterId"], "c1");
    assert_eq!(value["pages"][0]["pageId"], "c1-p001");
    assert_eq!(value["pages"][0]["pageIndex"], 0);
    assert!(value.get("alreadyRunning").is_none(), "alreadyRunning was sent when it is not one");

    let nothing =
        serde_json::to_value(RunHandle { run_id: None, pages: Vec::new(), already_running: None })
            .unwrap();
    assert!(nothing["runId"].is_null());
    assert_eq!(nothing["pages"].as_array().unwrap().len(), 0);
}

/// The model search order, which is the one thing about the weights this
/// machine can check without them: an override first, then the per-user
/// directory a first-launch download writes to, then the installed copy.
#[test]
fn the_weights_are_looked_for_where_they_are_put() {
    let paths = model_search_paths(Some(Path::new("/data")));
    assert!(paths.contains(&PathBuf::from("/data/models")));
    assert_eq!(paths.last(), Some(&PathBuf::from("models")));
}

/// The dylib `scripts/fetch-runtime.sh` unpacks into the checkout's
/// `runtimes/<package>/lib/`.
fn fetched_runtime() -> Option<PathBuf> {
    let name = if cfg!(target_os = "windows") {
        "onnxruntime.dll"
    } else if cfg!(target_os = "macos") {
        "libonnxruntime.dylib"
    } else {
        "libonnxruntime.so"
    };
    std::fs::read_dir("../runtimes")
        .ok()?
        .flatten()
        .map(|entry| entry.path().join("lib").join(name))
        .find(|path| path.exists())
}

/// The real pipeline, over the real weights, on one page - **when they are
/// there**.
///
/// It skips rather than fails when they are not, and that is deliberate: the
/// weights are not in the repository and the ONNX Runtime is fetched rather
/// than bundled, so a machine that has run neither `scripts/fetch-models.sh`
/// nor `scripts/fetch-runtime.sh` cannot run this and should not be told it
/// has broken something.
///
/// What it establishes is narrow and worth having: that the scheduler's
/// [`Cleaner`] is satisfied by the same sequence `spikes/clean-page` runs, that
/// a region reaches a patch with a well-formed pixel buffer, and that the
/// provenance a run writes is the provenance the seam reads back. It is **not**
/// a throughput measurement and not a flag-rate one - Phase 1's exit criteria
/// need a licence-cleared corpus, and `fixtures/pages/` is synthetic.
/// The real pipeline, or `None` when this machine cannot build one.
///
/// It skips rather than fails, and that is deliberate: the weights are not in
/// the repository and the ONNX Runtime is fetched rather than bundled, so a
/// machine that has run neither `scripts/fetch-models.sh` nor
/// `scripts/fetch-runtime.sh` cannot run these and should not be told it has
/// broken something.
///
/// **One at a time.** Every test that builds one of these holds
/// [`one_gpu_at_a_time`] for as long as it uses it. Rung 2 lands on WebGPU on
/// this machine, `cargo test` runs its tests on as many threads as
/// the machine has cores, and two Metal command buffers encoded from two
/// threads through one process abort the whole binary - *"A command encoder is
/// already encoding to this command buffer"*, `SIGABRT`, every test in the
/// binary lost. That is a property of driving one GPU from several threads and
/// not of the pipeline: the application holds one [`Pipeline`] per run and has
/// no path that opens two.
fn the_real_pipeline() -> Option<Pipeline> {
    let models = model_dir(None).or_else(|| {
        let up = PathBuf::from("../models");
        models_ready(&up).then_some(up)
    });
    let Some(models) = models else {
        eprintln!("skipped: no model weights - run scripts/fetch-models.sh");
        return None;
    };
    // `runtime::find` looks where an *installed* application would; a test
    // binary sits in `target/debug/deps`, where nothing was ever unpacked. So
    // the checkout's own `runtimes/` is searched too, and these run on a
    // machine that has fetched the runtime without needing `ORT_DYLIB_PATH` in
    // the environment - which the runtime module's own tests assert is unset.
    let Some(runtime) = cleaner_core::runtime::find(None).ok().or_else(fetched_runtime) else {
        eprintln!("skipped: no ONNX Runtime - run scripts/fetch-runtime.sh");
        return None;
    };
    if cleaner_core::runtime::load(&runtime).is_err() {
        eprintln!("skipped: the ONNX Runtime would not load");
        return None;
    }
    Pipeline::open(&models, Preference::Automatic).ok()
}

/// The lock every real-pipeline test takes, for the reason
/// [`the_real_pipeline`] gives. Poisoning is ignored: a panicking test has
/// already failed, and taking the rest of the binary down with it turns one
/// red line into a hundred.
/// `open_gate` with the reader's three files absent, and with them present but
/// unreadable. Both open a gate, and neither has a reader - the second is the
/// fallback for a reader that will not open, which had no test.
///
/// Skips like [`the_real_pipeline`] and for the same reason: the gate's own
/// weights are needed to open it at all.
#[test]
fn a_reader_that_is_absent_or_will_not_open_leaves_the_gate_standing() {
    let _gpu = one_gpu_at_a_time();
    let Some(pipeline_models) = model_dir(None).or_else(|| {
        let up = PathBuf::from("../models");
        models_ready(&up).then_some(up)
    }) else {
        eprintln!("skipped: no model weights - run scripts/fetch-models.sh");
        return;
    };
    let Some(runtime) = cleaner_core::runtime::find(None).ok().or_else(fetched_runtime) else {
        eprintln!("skipped: no ONNX Runtime - run scripts/fetch-runtime.sh");
        return;
    };
    if cleaner_core::runtime::load(&runtime).is_err() {
        eprintln!("skipped: the ONNX Runtime would not load");
        return;
    }

    let scratch = Scratch::new("gate-without-reader");
    let models = scratch.join("models");
    std::fs::create_dir_all(&models).unwrap();
    for name in [GATE_MODEL, GATE_LABELS] {
        std::fs::copy(pipeline_models.join(name), models.join(name)).unwrap();
    }

    let bare = open_gate(&models, Preference::Automatic).expect("the gate opens without a reader");
    assert!(!bare.has_reader(), "no reader files, no reader");

    for name in OCR_MODELS {
        std::fs::write(models.join(name), b"not a model").unwrap();
    }
    let fallen_back =
        open_gate(&models, Preference::Automatic).expect("a reader that will not open is not a failure");
    assert!(!fallen_back.has_reader(), "a corrupt reader must fall back to the bare gate");
}

fn one_gpu_at_a_time() -> std::sync::MutexGuard<'static, ()> {
    static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());
    GPU.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// One fixture page through it, with the geometry a run would hand it: one
/// page, its own strip, one segment, no joins. A single-page project is the
/// degenerate case of rules 1 to 4 and it has to come out where it came out
/// before they were wired.
fn clean_fixture(pipeline: &mut Pipeline, fixture: &str, ceiling: Engine) -> Option<PageOutcome> {
    let page = PathBuf::from("../fixtures/pages").join(fixture);
    let Ok(bytes) = std::fs::read(&page) else {
        eprintln!("skipped: no {}", page.display());
        return None;
    };
    let decoded = cleaner_core::image::decode(&bytes).expect("the fixture decoded");
    let strip = Strip::of_sizes(&[(decoded.width, decoded.height)]);
    let survey = cleaner_core::strip::survey(&strip, |_| None);
    let joins = survey.joins.clone();
    let context =
        PageContext { strip: &strip, joins: &joins, segments: &survey.segments, placement: 0 };
    Some(pipeline.clean_page("c1-p001", &bytes, ceiling, &context).expect("the page cleaned"))
}

/// What one region came out as, in one line. The evidence an end-to-end run
/// reports, and the reason these tests are worth running with `--nocapture`.
fn one_line(region: &RegionOutcome) -> String {
    match region {
        RegionOutcome::Cleaned(patch, review) => {
            let provenance = &patch.provenance;
            let params = &provenance.params_snapshot;
            format!(
                "{}: {:?} on {} - {}×{} mask, {} ms, tiles {}, pad {}, quality {}, digest {}{}",
                patch.id,
                provenance.engine,
                provenance.execution_provider,
                patch.mask.bounds.w,
                patch.mask.bounds.h,
                params["elapsed_ms"],
                params.get("tiles").map(|t| t.to_string()).unwrap_or_else(|| "-".into()),
                params["edge_pad"],
                params["quality"],
                if provenance.model_sha256.is_some() { "yes" } else { "none" },
                review.as_deref().map(|r| format!(" - {r}")).unwrap_or_default(),
            )
        }
        RegionOutcome::Untouched { bbox, reason } => {
            format!("({}, {}) {}×{}: untouched - {reason}", bbox.x, bbox.y, bbox.w, bbox.h)
        }
    }
}

#[test]
fn the_pipeline_cleans_a_page_when_the_weights_and_the_runtime_are_present() {
    let _serial = one_gpu_at_a_time();
    let Some(mut pipeline) = the_real_pipeline() else { return };
    let Some(outcome) = clean_fixture(&mut pipeline, "page-dialogue.png", HIGHEST_LOCAL) else {
        return;
    };
    assert!(!outcome.regions.is_empty(), "a page of dialogue produced no regions at all");

    let cleaned: Vec<&Patch> = outcome
        .regions
        .iter()
        .filter_map(|region| match region {
            RegionOutcome::Cleaned(patch, _) => Some(&**patch),
            RegionOutcome::Untouched { .. } => None,
        })
        .collect();
    assert!(!cleaned.is_empty(), "no region of a page of dialogue was cleaned");
    for patch in &cleaned {
        assert!(patch.is_well_formed(), "{}: the pixels do not cover the mask", patch.id);
        assert!(patch.id.starts_with("c1-p001-r"));
        assert_eq!(patch.provenance.mask_sha256.len(), 64);
        assert!(matches!(patch.provenance.engine, Engine::Fill | Engine::Denoise));
        assert_eq!(patch.provenance.params_snapshot["fill_mode"], "match-surround");
        assert!(patch.provenance.params_snapshot["elapsed_ms"].is_u64());
        assert!(patch.provenance.cloud.is_none(), "a local run recorded a cloud request");
    }
}

/// Region ids are list indices, and the list has two sources.
///
/// The pipeline adopts the balloon detector's uncovered text boxes as regions
/// and then sorts the joined list by `(masking.y, masking.x)`, because
/// appending would give them the last ids on the page instead of their own
/// place in reading order. What a run emits is a
/// subsequence of that list - a region can be skipped for an empty seed or for
/// belonging to another segment - so the property survives into the outcome,
/// and this is where a lost `sort_regions` would show up.
#[test]
fn the_regions_a_page_reports_are_in_reading_order() {
    let _serial = one_gpu_at_a_time();
    let Some(mut pipeline) = the_real_pipeline() else { return };
    let Some(outcome) = clean_fixture(&mut pipeline, "page-crowded.png", HIGHEST_LOCAL) else {
        return;
    };
    let boxes: Vec<Rect> = outcome
        .regions
        .iter()
        .map(|region| match region {
            RegionOutcome::Cleaned(patch, _) => patch.mask.bounds,
            RegionOutcome::Untouched { bbox, .. } => *bbox,
        })
        .collect();
    assert!(boxes.len() > 1, "a crowded page reported {} regions", boxes.len());
    for pair in boxes.windows(2) {
        // The mask a rung wrote is grown inside the masking box, so the
        // comparison is on the row and not on the exact origin: what must hold
        // is that the page is walked downwards.
        assert!(
            pair[0].y <= pair[1].bottom(),
            "region at ({}, {}) came out after one at ({}, {})",
            pair[1].x,
            pair[1].y,
            pair[0].x,
            pair[0].y
        );
    }
}

/// The ladder, end to end, on the fixture it exists for.
///
/// `page-crowded.png` is the fixture that exercises it: twenty detected
/// regions, of which **seven** reach the ladder. `page-screentone.png` does
/// **not**, despite the name: the two regions the detector emits on it sit on
/// flat paper and fit at deviation 0. Run this with `--nocapture` and it prints
/// what every region actually did, which is the only form the evidence takes: a
/// wiring change that compiles is not one.
///
/// **None of the seven is on screentone**, and an earlier version of this
/// docstring said all eleven of them were. What actually puts them here is that
/// the generator's balloons are too small for their lettering: on each of the
/// seven a glyph overruns its own outline, so the fitted mask must cover a piece
/// of a 6 px ink stroke, §4's fifth correction refuses every candidate in the
/// series, and [`fit::fit_within`] falls back to the ladder. Four more regions
/// used to be here for a different reason - the mask cleared the stroke but the
/// four-pixel annulus did not - and [`cleaner_core::fit::ring::paper_annulus`]
/// is why they are at rung 0 now.
///
/// So this test rests on a **fixture defect**, and it should not rest on one.
/// The seven are exactly the regions the ladder handles worst: a hole inside a
/// speech balloon is where manga-LaMa invents Japanese text
/// ([`cleaner_core::engines::lama::model_hole`]), so the only end-to-end
/// evidence Phase 3 has that the ladder runs comes from seven regions the ladder
/// should arguably never have been given. A fixture with text genuinely on
/// screentone - which the detector currently finds on none of the four toned
/// panels - would replace it.
///
/// It is not a quality measurement. Phase 3's quality exit is a blind human A/B
/// on a screentone-heavy fixture, and `fixtures/pages/` is synthetic.
#[test]
fn a_crowded_page_reaches_rung_two_and_records_where_it_ran() {
    // A host under memory pressure surrenders rung 2 before this test's first
    // assertion, which is the ladder doing its job on the wrong machine.
    let level = memory::pressure();
    if level.is_elevated() {
        eprintln!("skipped: host is under memory pressure ({level:?})");
        return;
    }
    let _serial = one_gpu_at_a_time();
    let Some(mut pipeline) = the_real_pipeline() else { return };
    let started = Instant::now();
    let Some(outcome) = clean_fixture(&mut pipeline, "page-crowded.png", HIGHEST_LOCAL) else {
        return;
    };
    eprintln!("page-crowded.png in {:.2} s", started.elapsed().as_secs_f64());
    for region in &outcome.regions {
        eprintln!("  {}", one_line(region));
    }

    let inpainted: Vec<&Patch> = outcome
        .regions
        .iter()
        .filter_map(|region| match region {
            RegionOutcome::Cleaned(patch, _) => Some(&**patch),
            RegionOutcome::Untouched { .. } => None,
        })
        .filter(|patch| patch.provenance.engine == Engine::Lama)
        .collect();
    assert!(!inpainted.is_empty(), "no region of a crowded page reached rung 2");

    for patch in &inpainted {
        assert!(patch.is_well_formed(), "{}: the pixels do not cover the mask", patch.id);
        let provenance = &patch.provenance;
        // A model ran, so there **is** a digest and there is a provider that
        // chose itself - `None` here would be a missing answer rather than a
        // true one, which is what it is one rung down.
        assert_eq!(
            provenance.model_sha256.as_ref().map(|digest| digest.len()),
            Some(64),
            "{}: rung 2 recorded no model digest",
            patch.id
        );
        assert!(!provenance.execution_provider.is_empty());
        assert_eq!(provenance.params_snapshot["rung"], "ladder.rung.lama");
        assert_eq!(provenance.params_snapshot["fill_mode"], "reconstruct");
        assert_eq!(provenance.params_snapshot["quality"], "measured");
        assert!(
            provenance.params_snapshot["tiles"].as_u64().is_some_and(|tiles| tiles >= 1),
            "{}: a rung-2 patch that ran no tiles",
            patch.id
        );
        assert!(provenance.cloud.is_none(), "a local run recorded a cloud request");
    }

    // **Auto clean is local-only, and rung 2 is the top of the automatic
    // path.** At the highest local ceiling there is still nothing above it.
    for region in &outcome.regions {
        if let RegionOutcome::Cleaned(patch, _) = region {
            assert!(
                rung(patch.provenance.engine) <= rung(Engine::Lama),
                "{} ran at {:?}",
                patch.id,
                patch.provenance.engine
            );
        }
    }
}

/// **The shipped picks, on the page the ladder actually fires on.**
///
/// The test above is the same fixture with nobody asking: seven regions reach
/// rung 2 because the fit could not settle them, and every one of those seven
/// is text inside a speech balloon - which is where manga-LaMa invents Japanese
/// text, and is exactly the place a translator wants a fill and not a model.
/// With the tool window's own defaults those regions start on the fill family
/// instead, and the page comes back with **no rung-2 patch at all**: not a
/// decline, not a hole - the fill's pixels pass the metric, which is the
/// whole argument for offering the row.
///
/// The unit tests either side of [`start_rung`] pin the arithmetic. This pins
/// that the arithmetic reaches the page, which is the claim the row makes to
/// the user.
#[test]
fn the_shipped_picks_keep_bubble_text_off_the_inpainter() {
    let _serial = one_gpu_at_a_time();
    let Some(pipeline) = the_real_pipeline() else { return };
    let mut pipeline = pipeline.with_picks(Picks::default());
    let Some(outcome) = clean_fixture(&mut pipeline, "page-crowded.png", HIGHEST_LOCAL) else {
        return;
    };

    let mut cleaned = 0usize;
    for region in &outcome.regions {
        if let RegionOutcome::Cleaned(patch, _) = region {
            cleaned += 1;
            assert!(
                rung(patch.provenance.engine) <= rung(Engine::Denoise),
                "{} is bubble text and ran at {:?}",
                patch.id,
                patch.provenance.engine
            );
        }
    }
    // And the regions are still *cleaned* - a pick that turned seven patches
    // into seven declines would pass the assertion above and fail the user.
    assert!(cleaned > 0, "the picked run cleaned nothing at all");
}

/// **The pressure ladder, fired.**
///
/// A machine under memory pressure is not a state a test can arrange, so the
/// kernel's reading is supplied rather than waited for - everything downstream
/// of that one value is the real thing, on the real fixture, through the real
/// sessions.
///
/// What the step has to be is **recoverable**, and this is what that means in
/// outcomes: the regions that would have gone to rung 2 are left exactly as they
/// were and listed with `decline.reason.rungUnavailable`, every region
/// rungs 0 and 1 can carry is still cleaned, and the page still comes back. A
/// run is a rung worse, not over.
///
/// The step used to have a second half - rung 3's session had to *not* be
/// opened instead, since reaching for the preview tier on a machine that had
/// just asked for memory back would be the step undoing itself. Rung 3 is
/// gone, so there is one session to give back and nothing left to reach for.
#[test]
fn a_run_under_pressure_gives_back_the_inpainter_and_keeps_going() {
    // The "before" half of this test asserts the host is *not* under pressure;
    // a host that already is fails that assertion before the test's own signal
    // is ever supplied.
    let level = memory::pressure();
    if level.is_elevated() {
        eprintln!("skipped: host is under memory pressure ({level:?})");
        return;
    }
    let _serial = one_gpu_at_a_time();
    let Some(mut pipeline) = the_real_pipeline() else { return };

    // First, with no pressure, so the comparison below is against this
    // machine's own answer rather than against an assumption.
    let Some(before) = clean_fixture(&mut pipeline, "page-crowded.png", HIGHEST_LOCAL) else {
        return;
    };
    let inpainted = before
        .regions
        .iter()
        .filter(|region| {
            matches!(region, RegionOutcome::Cleaned(patch, _)
                if rung(patch.provenance.engine) >= rung(Engine::Lama))
        })
        .count();
    assert!(inpainted > 0, "the fixture reached no model rung, so there is nothing to give back");
    assert!(before.pressure.is_empty(), "the ladder fired on a machine that is not under pressure");
    assert!(matches!(pipeline.rung2.state, Session::Held(_)), "rung 2 was never held");

    // The kernel says the machine is short.
    assert_eq!(
        pipeline.take_pressure(cleaner_core::memory::Pressure::Critical),
        Some("notice.memory.unloadedInpainter")
    );
    assert!(matches!(pipeline.rung2.state, Session::Surrendered));

    let Some(after) = clean_fixture(&mut pipeline, "page-crowded.png", HIGHEST_LOCAL) else {
        return;
    };
    let mut left_alone = 0;
    for region in &after.regions {
        match region {
            RegionOutcome::Cleaned(patch, _) => assert!(
                rung(patch.provenance.engine) < rung(Engine::Lama),
                "{} ran at {:?} after the inpainter was given back",
                patch.id,
                patch.provenance.engine
            ),
            RegionOutcome::Untouched { reason, .. } => {
                if reason == "decline.reason.rungUnavailable" {
                    left_alone += 1;
                }
            }
        }
    }
    assert!(!after.regions.is_empty(), "a page under pressure produced nothing at all");
    assert_eq!(left_alone, inpainted, "the model regions were not the ones left alone");
    // The session is not rebuilt on the next region, or the next page: the step
    // is a latch and `Unopened` would be the state that reopens.
    assert!(matches!(pipeline.rung2.state, Session::Surrendered));

    // And the second step is the one rule 6 names, reported on the same stream.
    assert_eq!(
        pipeline.take_pressure(cleaner_core::memory::Pressure::Warn),
        Some("notice.memory.oneWindow")
    );
}

/// **Opening a pipeline opens no sessions.**
///
/// This needs no ONNX Runtime and no inference, which is the point: the whole
/// of `Pipeline::open` is now a filesystem check, so a run that is about to be
/// declined for some other reason never pays for a session. The three models it
/// used to build come to about 110 MB of weights and several seconds of session
/// build between them.
#[test]
fn opening_a_pipeline_costs_no_session() {
    let Some(models) = model_dir(None).or_else(|| {
        let up = PathBuf::from("../models");
        models_ready(&up).then_some(up)
    }) else {
        eprintln!("skipped: no model weights - run scripts/fetch-models.sh");
        return;
    };
    let pipeline = Pipeline::open(&models, Preference::Automatic).expect("the weights are there");
    assert!(!pipeline.detector.is_held(), "the detector was opened before a page asked");
    assert!(!pipeline.balloons.is_held());
    assert!(!pipeline.gate.is_held());
    assert!(matches!(pipeline.rung2.state, Session::Unopened));

    // And the failure that *is* worth reporting early still is.
    let empty = Scratch::new("no-models");
    assert!(Pipeline::open(&empty.join("."), Preference::Automatic).is_err());
}

/// **A partial model directory is not a model directory.**
///
/// `model_dir` used to decide whether weights were present by checking only
/// `DETECTOR`. If a user had downloaded only part of the suite (or if a
/// download was interrupted), `model_dir` picked that directory, `Pipeline::open`
/// failed on the next required file, and `start` propagated the error - turning
/// a missing-model condition into a rejected IPC promise instead of the warning
/// notice the frontend expects.
///
/// Every file required to open a pipeline (`DETECTOR`, `BALLOONS`, `GATE_MODEL`,
/// and `GATE_LABELS`) must be present before a candidate path is accepted.
#[test]
fn a_partial_model_directory_is_not_a_model_directory() {
    let scratch = Scratch::new("partial-models");
    let models = scratch.join("models");
    std::fs::create_dir_all(&models).unwrap();

    // With only the detector file, the directory is incomplete and must not be chosen.
    std::fs::write(models.join(DETECTOR), b"").unwrap();
    assert!(!models_ready(&models), "only detector should not be ready");
    assert_eq!(
        model_dir(Some(&scratch.0)),
        None,
        "partial directory was chosen when incomplete"
    );

    // With some but not all required files present, still incomplete.
    std::fs::write(models.join(BALLOONS), b"").unwrap();
    std::fs::write(models.join(GATE_MODEL), b"").unwrap();
    assert!(!models_ready(&models), "missing labels should not be ready");
    assert_eq!(
        model_dir(Some(&scratch.0)),
        None,
        "partial directory without labels was chosen"
    );

    // Once all required files are present, the directory is accepted.
    std::fs::write(models.join(GATE_LABELS), b"").unwrap();
    assert!(models_ready(&models), "all required models should be ready");
    assert_eq!(
        model_dir(Some(&scratch.0)),
        Some(models),
        "complete directory was not chosen"
    );
}

/// **A missing model file is a variant, and its wording is unchanged.**
///
/// `start` uses this as defence in depth: if `Pipeline::open` fails because a
/// required model file went missing between the `model_dir` check and the open -
/// a download deleted, a volume unmounted - it emits
/// `notice.run.modelsMissing` and answers an empty handle rather than rejecting
/// the IPC promise. It used to tell that failure apart by reading the front and
/// the back of the error *string*; now it matches `MissingModel`, and the
/// variant carries the path rather than leaving it to be parsed back out.
///
/// The message itself is asserted here because it is the one thing the type
/// change was not allowed to alter: it is what a `Result<_, String>` caller
/// shows, so `Display` must still produce exactly the sentence it produced when
/// it was a `format!`.
#[test]
fn a_partial_or_missing_model_error_is_a_typed_variant() {
    let empty = Scratch::new("missing-model-err");
    let models = empty.join(".");
    // `Pipeline` has no `Debug`, so `expect_err` is not available here.
    let error = match Pipeline::open(&models, Preference::Automatic) {
        Ok(_) => panic!("empty directory should fail to open"),
        Err(error) => error,
    };

    // The first required file, named - not a prefix a caller has to strip.
    let OpenError::MissingModel { path } = &error;
    assert_eq!(path, &models.join(DETECTOR), "the missing file was not named");

    // And the sentence the seam carries, character for character.
    let expected = format!("the model file {} is missing", models.join(DETECTOR).display());
    assert_eq!(error.to_string(), expected, "the message a caller shows changed");
    assert_eq!(String::from(error), expected, "the `String` conversion disagrees with `Display`");
}

/// **The close button in the loaded-models tab, end to end.**
///
/// A page is cleaned, which loads whatever that page needs; every row is then
/// asked for back the way `unload_model` asks; and the next page finds the
/// sessions gone - and reopens them, because a run cannot proceed without the
/// detector and this is not the pressure ladder's latch.
///
/// The distinction against
/// `a_run_under_pressure_gives_back_the_inpainter_and_keeps_going` is the whole
/// point: that one leaves rung 2 `Surrendered` and the page a rung worse, and
/// this one leaves everything `Unopened` and the page unchanged.
#[test]
fn a_requested_unload_is_taken_at_the_next_safe_point_and_the_run_goes_on() {
    let _serial = one_gpu_at_a_time();
    let Some(mut pipeline) = the_real_pipeline() else { return };
    let Some(before) = clean_fixture(&mut pipeline, "page-crowded.png", HIGHEST_LOCAL) else {
        return;
    };
    assert!(pipeline.detector.is_held(), "a cleaned page held no detector");
    let cleaned = |outcome: &PageOutcome| {
        outcome.regions.iter().filter(|r| matches!(r, RegionOutcome::Cleaned(..))).count()
    };
    assert!(cleaned(&before) > 0, "the fixture cleaned nothing, so there is nothing to compare");

    // What the tab does: one request per row, and not one of them touches a
    // session.
    let rows = cleaner_core::registry::loaded();
    assert!(rows.len() >= 3, "the detectors and the gate are not on the tab: {rows:?}");
    for row in &rows {
        assert!(cleaner_core::registry::request_unload(row.id));
    }
    // Still loaded: an unload is a request, and the run is between pages.
    assert!(pipeline.detector.is_held());

    let Some(after) = clean_fixture(&mut pipeline, "page-crowded.png", HIGHEST_LOCAL) else {
        return;
    };
    // Every row the tab was showing has gone. The ones that came back are new
    // sessions with new ids, which is what makes "gone" checkable at all.
    let ids: std::collections::HashSet<u64> = rows.iter().map(|row| row.id).collect();
    for row in cleaner_core::registry::loaded() {
        assert!(!ids.contains(&row.id), "a session that was asked for back is still loaded");
    }
    // And the page is unchanged: this is a reopen, not a rung lost.
    assert_eq!(cleaned(&after), cleaned(&before), "unloading cost the page its rungs");
}

/// **A model, once loaded, outlives the run that loaded it.**
///
/// The ruling this whole mechanism exists for, stated against the only thing
/// that can prove it: the registry's row ids. An id is minted per *session*, so
/// a second run finding the same ids is a second run that reopened nothing -
/// and a second run finding different ones would be exactly the reload cost
/// still being paid.
#[test]
fn a_session_outlives_the_run_that_opened_it_and_the_next_run_gets_it_back() {
    let _serial = one_gpu_at_a_time();
    let Some(mut pipeline) = the_real_pipeline() else { return };
    if clean_fixture(&mut pipeline, "page-crowded.png", HIGHEST_LOCAL).is_none() {
        return;
    }
    let opened: Vec<u64> = cleaner_core::registry::loaded().iter().map(|row| row.id).collect();
    assert!(opened.len() >= 3, "a cleaned page held fewer models than it needs: {opened:?}");

    // The run ends. **Nothing is unloaded**: the sessions go to the residency
    // cache, and their rows stay up because the sessions are still resident.
    drop(pipeline);
    let parked: Vec<u64> = cleaner_core::registry::loaded().iter().map(|row| row.id).collect();
    for id in &opened {
        assert!(parked.contains(id), "the end of a run unloaded session {id}");
    }

    // And the next run gets those very sessions rather than building new ones.
    let Some(mut second) = the_real_pipeline() else { return };
    if clean_fixture(&mut second, "page-crowded.png", HIGHEST_LOCAL).is_none() {
        return;
    }
    let now: Vec<u64> = cleaner_core::registry::loaded().iter().map(|row| row.id).collect();
    for id in &opened {
        assert!(now.contains(id), "the second run reopened session {id}");
    }
}

/// **One page, detected once**, however many region edits land on it.
///
/// The memo is keyed by the source's digest and the detection is a pure
/// function of the source's pixels, so the second call is the *same* detection -
/// asserted by pointer, which is the only assertion that cannot pass by
/// accident on a page that would have detected identically anyway.
#[test]
fn a_page_is_detected_once_however_many_edits_land_on_it() {
    let _serial = one_gpu_at_a_time();
    // For the runtime and the weights, which this borrows rather than restates.
    let Some(pipeline) = the_real_pipeline() else { return };
    drop(pipeline);
    let models = model_dir(None).or_else(|| {
        let up = PathBuf::from("../models");
        models_ready(&up).then_some(up)
    });
    let Some(models) = models else { return };
    let page = PathBuf::from("../fixtures/pages").join("page-crowded.png");
    let Ok(bytes) = std::fs::read(&page) else { return };
    let decoded = cleaner_core::image::decode(&bytes).expect("the fixture decoded");
    let digest = cleaner_core::ingest::sha256_hex(&bytes);

    cleaner_core::detect::forget();
    let mut detector = Detector::open(&models.join(DETECTOR), Preference::Automatic)
        .expect("the detector opened");
    let first = cleaner_core::detect::detect_page(&mut detector, &digest, &decoded)
        .expect("the page detected");
    let again = cleaner_core::detect::detect_page(&mut detector, &digest, &decoded)
        .expect("the page detected twice");
    assert!(std::sync::Arc::ptr_eq(&first, &again), "the second edit detected the page again");

    // A different source is a different page, memo or no memo.
    let other = cleaner_core::detect::detect_page(&mut detector, "not-this-page", &decoded)
        .expect("the other page detected");
    assert!(!std::sync::Arc::ptr_eq(&first, &other));
    cleaner_core::detect::forget();
}

/// The same page under a ceiling of rung 1: every region the ladder wanted is
/// **left alone**, and none of them is quietly filled by a rung §5 already
/// ruled out.
#[test]
fn a_capped_run_leaves_the_ladders_regions_alone_rather_than_filling_them() {
    let _serial = one_gpu_at_a_time();
    let Some(mut pipeline) = the_real_pipeline() else { return };
    let Some(outcome) = clean_fixture(&mut pipeline, "page-crowded.png", Engine::Denoise) else {
        return;
    };
    for region in &outcome.regions {
        eprintln!("  {}", one_line(region));
    }
    let declined = outcome
        .regions
        .iter()
        .filter(|region| {
            matches!(region, RegionOutcome::Untouched { reason, .. }
                if reason == "decline.reason.rungUnavailable")
        })
        .count();
    assert!(declined > 0, "a capped run cleaned every region of a crowded page");
    for region in &outcome.regions {
        if let RegionOutcome::Cleaned(patch, _) = region {
            assert!(
                rung(patch.provenance.engine) <= rung(Engine::Denoise),
                "{} ran at {:?} under a rung-1 ceiling",
                patch.id,
                patch.provenance.engine
            );
        }
    }
}

/* ------------------------------------------------------------------ */
/* Rules 1 to 4, wired                                                */
/* ------------------------------------------------------------------ */

/// A longstrip chapter over `fixtures/pages/strip-01…03.png`.
///
/// Those three pages are one 800×12000 canvas cut into three, and the cut
/// between page 2 and page 3 deliberately repeats eight rows - a duplicated
/// overlap, the one join anomaly the catalogue has a sentence for.
fn a_longstrip_chapter(scratch: &Scratch) -> Option<(Library, String, String)> {
    let raws = scratch.join("strip-raws");
    std::fs::create_dir_all(&raws).unwrap();
    for n in 1..=3 {
        let from = PathBuf::from(format!("../fixtures/pages/strip-{n:02}.png"));
        let bytes = std::fs::read(&from).ok()?;
        std::fs::write(raws.join(format!("{n:03}.png")), bytes).unwrap();
    }
    let library = Library::at(scratch.join("library"));
    let project = library
        .create_project("Webtoon", StripMode::Longstrip, Some(raws), None)
        .unwrap();
    let chapter = library.create_chapter(&project.id, "Ch. 1", None, None).unwrap().created().unwrap();
    Some((library, project.id, chapter.id))
}

/// **Satisfies one half of what's needed.**
///
/// `check_join` and `Joins::anomalies` had no caller outside their own tests,
/// so "report them" was unmet for **four of four** variants, including the
/// one the catalogue can say. The survey is the caller, and this is the
/// notice arriving on the same stream the run's events ride.
///
/// What it does not close: the other three variants still have no key, so a
/// misregistration, a width mismatch and a 1-px seam are still found, still
/// treated as page edges, and still unreportable.
#[test]
fn a_duplicated_join_is_reported_on_the_stream() {
    let scratch = Scratch::new("join-anomaly");
    let Some((library, _, chapter_id)) = a_longstrip_chapter(&scratch) else {
        eprintln!("skipped: no fixtures/pages/strip-*.png - run scripts/make-fixture-pages.py");
        return;
    };
    let entries = plan(&library, "chapter", &chapter_id, None).unwrap();
    assert_eq!(entries.len(), 3);

    let recorder = Recorder::new();
    let mut stub = Stub::new(0, 0);
    execute(
        "run-1",
        &chapter_id,
        &entries,
        &mut stub,
        Engine::Flux,
        &recorder.cancel,
        &recorder.sink(),
    );

    let anomalies: Vec<serde_json::Value> = recorder
        .events()
        .iter()
        .filter_map(|event| match event {
            Event::Notice { key, params, tone, .. } if key == "notice.input.joinAnomaly" => {
                assert_eq!(*tone, "warn");
                Some(params.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(anomalies.len(), 1, "the deliberately broken join was not reported");
    // The catalogue's sentence is "Duplicated overlap rows between positions
    // {first} and {second}", and the positions a reader counts from are 1.
    assert_eq!(anomalies[0]["first"], 2);
    assert_eq!(anomalies[0]["second"], 3);

    // And the notice is a notice: it does not disturb the five ordered events.
    let ordered: Vec<&'static str> =
        recorder.kinds().into_iter().filter(|kind| *kind != "notice").collect();
    assert_eq!(
        ordered,
        vec![
            "page-started", "page-done", "page-started", "page-done", "page-started", "page-done",
            "run-finished",
        ]
    );
}

/// **Rule 2's plan, recorded.** `strip.splits` had no writer at all - the
/// field's own doc comment says so - and rule 2 requires a fallback to be
/// "recorded in `strip.splits` and surfaced in review".
///
/// The fixture is three 4000 px pages, so every join is inside a tolerance band
/// at some point and rule 2's "page joins win unconditionally" is what the plan
/// is mostly made of.
#[test]
fn the_split_plan_reaches_the_manifest() {
    let scratch = Scratch::new("splits");
    let Some((library, project_id, chapter_id)) = a_longstrip_chapter(&scratch) else {
        eprintln!("skipped: no fixtures/pages/strip-*.png - run scripts/make-fixture-pages.py");
        return;
    };
    let job_path = library.job_path(&project_id, &chapter_id);
    assert!(Job::open(&job_path).unwrap().project.strip.splits.is_empty(), "before the run");

    let entries = plan(&library, "chapter", &chapter_id, None).unwrap();
    let recorder = Recorder::new();
    let mut stub = Stub::new(0, 0);
    execute(
        "run-1",
        &chapter_id,
        &entries,
        &mut stub,
        Engine::Flux,
        &recorder.cancel,
        &recorder.sink(),
    );

    let splits = Job::open(&job_path).unwrap().project.strip.splits;
    assert!(!splits.is_empty(), "rule 2's plan was not recorded");
    let rows: Vec<u32> = splits.iter().map(|s| s.y).collect();
    assert!(rows.windows(2).all(|w| w[0] < w[1]), "the plan runs backwards: {rows:?}");
    assert!(rows.iter().all(|y| *y > 0 && *y < 12_000), "{rows:?}");
    // The two page joins are free, guaranteed text-free split points and rule 2
    // takes them unconditionally.
    for join in [4_000u32, 8_000] {
        let taken = splits.iter().find(|s| s.y == join).expect("a join was not taken");
        assert_eq!(taken.kind, SplitKind::Join, "{taken:?}");
    }
}

/// The cost of the survey, stated as the rule that decides whether to pay it.
///
/// A chapter of ordinary pages pays nothing: its pages are its segments, which
/// is rule 2's own sentence for the case its trigger does not fire, and it
/// needs no join check because the join check scopes that to longstrip.
#[test]
fn an_ordinary_chapter_is_not_surveyed_and_a_longstrip_one_is() {
    let scratch = Scratch::new("survey-cost");
    let (library, project_id, chapter_id) = a_chapter(&scratch, 3);
    let job = Job::open(&library.job_path(&project_id, &chapter_id)).unwrap();
    assert!(!survey_is_worth_it(&job.project));
    // …and it still gets a strip, a segment per page and unchecked joins.
    let strip = strip_of(&job.project);
    assert_eq!(strip.pages().len(), 3);
    assert_eq!(strip.joins(), 2);

    let strip_scratch = Scratch::new("survey-cost-strip");
    let Some((library, project_id, chapter_id)) = a_longstrip_chapter(&strip_scratch) else {
        eprintln!("skipped: no fixtures/pages/strip-*.png - run scripts/make-fixture-pages.py");
        return;
    };
    let job = Job::open(&library.job_path(&project_id, &chapter_id)).unwrap();
    assert!(survey_is_worth_it(&job.project));
    assert_eq!(strip_of(&job.project).height(), 12_000);
}

/// **Rule 1's clamp, on the path a region actually takes.** The window is
/// computed in strip coordinates and ends in `Strip::clamp`, so a box on the
/// narrow page of a strip cannot reach the gutter beside it and a box beside an
/// unverified join cannot reach across it.
///
/// Asserted here rather than only in `cleaner_core::strip` because what this is
/// about is the *caller*: the numbers below are the ones `PageContext` hands to
/// `decode_window`.
#[test]
fn a_regions_decode_window_is_clamped_by_the_strip_it_sits_in() {
    let strip = Strip::of_sizes(&[(800, 4_000), (700, 4_000), (800, 4_000)]);
    let survey = cleaner_core::strip::survey(&strip, |_| None);
    let context = PageContext {
        strip: &strip,
        joins: &survey.joins,
        segments: &survey.segments,
        placement: 1,
    };

    // A box hard against the narrow page's left edge, in that page's own
    // coordinates, in a segment that starts at the top of the page.
    let on_page = Rect::new(0, 100, 120, 60);
    let in_strip = context.to_strip(on_page, 0);
    assert_eq!(in_strip, Rect::new(50, 4_100, 120, 60), "the page is centred, so x is inset by 50");

    let window = cleaner_core::strip::decode_window(
        context.strip,
        context.joins,
        in_strip,
        2.0,
        cleaner_core::strip::EngineContext::None,
    );
    assert_eq!(window.rect.x, 50, "the window reached into the gutter");
    assert!(window.requested.x < 50);
    assert_eq!(window.pad, EdgePad::Replicate);
    // …and back in the page's own coordinates it starts at zero, which is what
    // the fit is clamped to.
    let decoded = fixtures::by_name("l8").raster;
    let bounds = context.to_crop(window.rect, 0, &decoded);
    assert_eq!(bounds.x, 0);
}
