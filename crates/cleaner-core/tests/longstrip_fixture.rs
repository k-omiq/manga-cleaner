//! The longstrip fixture, against three rules.
//!
//! `scripts/make-fixture-pages.py` draws `strip-01`…`strip-03` in strip
//! coordinates and slices them, so the two cases Phase 2 names as exit
//! criteria exist as pixels: a bubble deliberately split across a page join,
//! and a bubble placed
//! deliberately at a content-minimum split point. This file asserts that the
//! geometry actually finds them - a fixture nothing measures is a drawing.
//!
//! **It is not a measurement of Phase 2's exit criteria.** Cleaning either
//! bubble end to end needs the detector's weights and the run, and the third
//! criterion - peak RSS under 400 MB - needs a 200-page webtoon, which three
//! synthetic pages are not.

use std::path::PathBuf;

use cleaner_core::detect::{Letterbox, Segmentation};
use cleaner_core::image::Raster;
use cleaner_core::strip::{
    DETECTION_OVERLAP, EdgeRows, JOIN_PROBE_ROWS, JoinAnomaly, JoinState, Joins, RowProfile, Split,
    SplitKind, Strip, check_join, detection_segments, plan_splits,
};

fn pages() -> Vec<Raster> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pages");
    (1..=3)
        .map(|n| {
            let path = dir.join(format!("strip-{n:02}.png"));
            let bytes = std::fs::read(&path).unwrap_or_else(|e| {
                panic!("{}: {e} - run scripts/make-fixture-pages.py", path.display())
            });
            cleaner_core::image::decode(&bytes).expect("the fixture page decoded")
        })
        .collect()
}

fn strip_of(pages: &[Raster]) -> Strip {
    Strip::of_sizes(&pages.iter().map(|p| (p.width, p.height)).collect::<Vec<_>>())
}

/// The dimensions `strip.js` assumes, and rule 2's trigger firing on them.
#[test]
fn the_fixture_is_a_strip_and_not_three_pages() {
    let pages = pages();
    let strip = strip_of(&pages);
    assert_eq!((strip.width(), strip.height()), (800, 12_000));
    assert!(strip.is_longstrip_shaped());
    assert_eq!(strip.joins(), 2);
    assert_eq!(strip.join_row(0), Some(4_000));
}

/// Rule 1's verified join, and the anomaly check.
///
/// The fixture is drawn on one canvas and cut, so join 0 is a genuine
/// continuous cut and the check must not invent a fault in it. Join 1 repeats
/// eight rows, which is the one anomaly the catalogue has a sentence for.
#[test]
fn one_join_verifies_and_the_deliberately_broken_one_does_not() {
    let pages = pages();
    let strip = strip_of(&pages);
    let mut joins = Joins::unchecked(strip.joins());
    for j in 0..strip.joins() {
        joins.set(
            j,
            check_join(
                &EdgeRows::bottom_of(&pages[j], JOIN_PROBE_ROWS),
                &EdgeRows::top_of(&pages[j + 1], JOIN_PROBE_ROWS),
            ),
        );
    }

    assert_eq!(joins.state(0), JoinState::Verified, "the continuous cut was called a fault");
    assert_eq!(joins.state(1), JoinState::Anomalous(JoinAnomaly::DuplicatedRows { rows: 8 }));
    let reported: Vec<_> = joins.anomalies().collect();
    assert_eq!(reported.len(), 1);
    assert_eq!(reported[0].1.reason_key(), Some("notice.input.joinAnomaly"));
}

/// **Exit criterion 2, constructed.** The bubble straddling the join at
/// y = 4000 is read as one region only because the join verified; with the same
/// window on the unverified join the read stops at the page edge.
#[test]
fn the_bubble_across_the_join_is_one_region_and_the_gutter_is_still_clamped() {
    let pages = pages();
    let strip = strip_of(&pages);
    let mut joins = Joins::unchecked(strip.joins());
    joins.set(0, JoinState::Verified);

    // The balloon is drawn at (100, 3700)–(700, 4300).
    let bubble = cleaner_core::mask::Rect::new(100, 3_700, 600, 600);
    let crossed = strip.clamp(bubble, &joins);
    assert_eq!(crossed.bottom(), 4_300, "the read did not cross the verified join");
    assert_eq!(crossed.y, 3_700);

    let stopped = strip.clamp(bubble, &Joins::unchecked(strip.joins()));
    assert_eq!(stopped.bottom(), 4_000, "an unverified join is a page edge");

    // Rule 8: the read crosses, the write splits.
    let parts = strip.split_at_joins(bubble);
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].0, 0);
    assert_eq!(parts[1].0, 1);
    assert_eq!(parts[0].1.h + parts[1].1.h, 600);
}

/// One page's rows, marked as text from `top` to `bottom` across the balloon's
/// x-span.
///
/// Constructed rather than measured. The detector's weights do not exist in
/// this repository, so what its per-pixel mask covers on a drawn balloon -
/// glyph strokes, or the region the glyphs sit in - is not something this file
/// can find out. What the test below needs is a mask that says something about
/// a specific run of rows, so that rule 2's mask term has an assertion to bite
/// on against the fixture's real gradient energy.
fn text_rows(top: u32, bottom: u32) -> Segmentation {
    let (w, h) = (800usize, 4_000usize);
    let mut levels = vec![0u8; w * h];
    for y in top as usize..(bottom as usize).min(h) {
        for x in 110..690 {
            levels[y * w + x] = 255;
        }
    }
    Segmentation { width: 800, height: 4_000, levels, fit: Letterbox::fit(800, 4_000) }
}

/// The profile of the whole fixture strip, one page at a time - rule 2's shape,
/// and the only thing either split test needs from the pixels.
fn strip_profile(pages: &[Raster], strip: &Strip) -> RowProfile {
    let mut profile = RowProfile::zeroed(strip.height());
    for (page, placement) in pages.iter().zip(strip.pages()) {
        profile.add_page(placement.y_offset, page);
    }
    profile
}

/// **Exit criterion 3, constructed.** The first split lands inside the balloon
/// drawn at 1340–2010, which is what "a bubble placed deliberately at a
/// content-minimum split point" means, and the segment that owns the rows above
/// it can still see the whole of it.
///
/// This is the energy term alone. The profile carries no mask coverage, which
/// is deliberate and is what makes the case: rule 2's splitter, given only what
/// the pixels say, cuts this bubble. The mask term is the next test.
#[test]
fn the_first_split_lands_inside_the_bubble_drawn_there_for_it() {
    let pages = pages();
    let strip = strip_of(&pages);
    let profile = strip_profile(&pages, &strip);

    let splits = plan_splits(&strip, &profile);
    let first = splits[0];
    assert_eq!(first.kind, SplitKind::Minimum, "the fixture's quiet row was not accepted");
    assert!(
        (1_340..2_010).contains(&first.y),
        "the first split is at {}, outside the balloon at 1340–2010",
        first.y
    );

    // Rule 2: a join inside the band wins, and the second band reaches 4000.
    assert_eq!(splits[1], Split { y: 4_000, kind: SplitKind::Join });

    // Rule 3: the segment above the split sees the whole balloon, which is what
    // recovers it as one region.
    let segments = detection_segments(strip.height(), &splits);
    assert!(segments[0].detect_end >= 2_010);
    assert_eq!(segments[0].detect_end - segments[1].start, DETECTION_OVERLAP);
}

/// **Rule 2's mask term, against the fixture.**
///
/// > Add coverage by the detector's mask from a cheap low-resolution pre-pass,
/// > so a row crossing known text is **never** chosen.
///
/// The test above is the same fixture with the term absent, and it asserts that
/// the splitter cuts the balloon. This one adds the term and pins two things
/// about it, which are different and are easy to conflate.
///
/// **The term binds the search.** With the balloon's rows called text, not one
/// of them is accepted: `TEXT_ROW_PENALTY` is a number large against a mean
/// gradient in 0–255 levels rather than a tuned weight, so every covered row
/// leaves the acceptance floor unreachable and the whole band - the balloon
/// below 2010, dense hatching above it - has no acceptable row left.
///
/// **The term does not bind the fallback,** and rule 2 is explicit that nothing
/// does: "otherwise widen, then fall back to a hard split at exactly 2000 px,
/// recorded in `strip.splits` and surfaced in review". So the plan comes back
/// with a `Fallback` at exactly the target, which on this fixture is 2000 - ten
/// rows above the balloon's own bottom edge, and still inside it. That is the
/// honest outcome rule 2 asks for rather than a silent least-bad row, and rule
/// 3's detection overlap is what recovers the box it cuts.
///
/// Marking only the four lines of text has no effect at all, and that is worth
/// asserting rather than assuming: the row the energy term chooses is 1500,
/// which is the gap between the first and second lines. A mask that follows
/// glyphs has nothing to say about the paper between them.
#[test]
fn a_row_the_mask_calls_text_is_never_the_split_even_on_the_fixture() {
    let pages = pages();
    let strip = strip_of(&pages);
    let bare = plan_splits(&strip, &strip_profile(&pages, &strip));
    assert_eq!(bare[0], Split { y: 1_500, kind: SplitKind::Minimum });

    // The four lines of text, at 1395, 1555, 1715 and 1875, 60 px tall.
    let mut glyphs = strip_profile(&pages, &strip);
    for line in [1_395u32, 1_555, 1_715, 1_875] {
        glyphs.add_mask_coverage(0, &text_rows(line, line + 60));
    }
    assert_eq!(
        plan_splits(&strip, &glyphs)[0],
        bare[0],
        "the chosen row is the gap between two lines; a glyph mask does not cover it"
    );

    // The balloon's rows, 1340–2010.
    let mut covered = strip_profile(&pages, &strip);
    covered.add_mask_coverage(0, &text_rows(1_340, 2_010));
    let splits = plan_splits(&strip, &covered);

    assert_ne!(splits[0], bare[0], "the mask term changed nothing");
    assert_eq!(
        splits[0],
        Split { y: 2_000, kind: SplitKind::Fallback },
        "no row of the band was acceptable, so rule 2 requires the hard cut at the target"
    );
    assert!(
        splits.iter().all(|s| s.kind != SplitKind::Minimum || !(1_340..2_010).contains(&s.y)),
        "a covered row was accepted as a minimum: {splits:?}"
    );

    // Everything downstream of the first band is unmoved: the join at 4000 still
    // wins its band, which is rule 2's "page joins win unconditionally".
    assert_eq!(splits[1], Split { y: 4_000, kind: SplitKind::Join });
}

