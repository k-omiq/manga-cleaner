//! Rule 3: detection overlap, then a global box list.
//!
//! **Detection segments overlap by 900 px** (>= the largest expected box
//! height). Overlap is for *detection only* - no pixels are processed twice -
//! and it exists because the IoU dedupe cannot fire without it: with
//! disjoint segments a clipped bubble appears as a top half in one and a
//! bottom half in the next, two **disjoint** boxes, IoU = 0, and both produce
//! a half-evidence patch.
//!
//! > Boxes are merged into a **single global list in strip coordinates**, then:
//! > union any pair whose edges lie within 8 px of the same split line;
//! > deduplicate by IoU ≥ 0.5; merge at >20% of the smaller box's area, sorted
//! > on strip `(y1, x1)`.
//!
//! > Ownership is decided on that list. **Never by per-tile centroid** - a
//! > clipped box has a different centroid in each tile, so both tiles claim it.
//!
//! The last paragraph is the one this module is arranged around. Ownership is a
//! field on [`GlobalBox`], decided from the one rectangle a merged box has.
//!
//! [`DetectedInSegment`] carries its rectangle and its segment **privately**,
//! and has no accessor for either. A caller builds one with
//! [`DetectedInSegment::new`] and hands it to [`merge_global`]; it cannot read
//! the rectangle back out, so there is no expression outside this module that
//! computes a per-segment centroid - the input to that question is a box that
//! exists twice, and any function of it answers twice. This is the whole
//! guarantee: it is enforced by the field's visibility rather than by the
//! absence of a convenience method, and it stops at the module boundary. The
//! tests below are inside that boundary and one of them deliberately computes
//! the forbidden answer, because a rule nothing demonstrates is a rule nobody
//! can see.
//!
//! **Nothing outside this module calls any of it yet.** See
//! [`super`]'s own note: the geometry is built and is not on the pipeline's
//! path.

use crate::mask::Rect;

use super::{DEDUPE_IOU, DETECTION_OVERLAP, MERGE_AREA_FRACTION, SPLIT_EDGE_TOLERANCE, Split};

/// One unit of work, and the rows it is allowed to *look* at.
///
/// The two ranges differ and that is the point. `start..end` is what this
/// segment processes - the pixels no other segment touches. `start..detect_end`
/// is what the detector is run over, and it reaches [`DETECTION_OVERLAP`] rows
/// into the next segment's work so that a box clipped by the split at `end`
/// arrives whole from *somewhere*.
///
/// The extension is downward only. A box up to 900 px tall that straddles the
/// split at `end` lies entirely inside `start..detect_end`, so extending the
/// next segment upward as well would buy a second copy of a box that is already
/// whole, at the cost of a second inference over the same rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub index: usize,
    pub start: u32,
    pub end: u32,
    pub detect_end: u32,
}

impl Segment {
    /// Whether this segment *processes* a strip row - the ownership range, not
    /// the detection range.
    ///
    /// Crate-private. Ownership is answered by [`merge_global`] on the global
    /// list and nowhere else; an exported predicate is half of the forbidden
    /// question already spelled out, and the other half - a per-segment
    /// rectangle to feed it - is what [`DetectedInSegment`]'s private fields
    /// withhold.
    pub(crate) fn owns_row(&self, y: i64) -> bool {
        y >= self.start as i64 && y < self.end as i64
    }
}

/// The segments a split plan produces. `splits` are rule 2's rows; the last
/// segment runs to the end of the strip.
pub fn detection_segments(strip_height: u32, splits: &[Split]) -> Vec<Segment> {
    let mut bounds: Vec<u32> = splits.iter().map(|s| s.y).collect();
    bounds.retain(|y| *y > 0 && *y < strip_height);
    bounds.sort_unstable();
    bounds.dedup();

    let mut out: Vec<Segment> = Vec::with_capacity(bounds.len() + 1);
    let mut start = 0u32;
    for end in bounds.iter().copied().chain(std::iter::once(strip_height)) {
        if end <= start && !out.is_empty() {
            continue;
        }
        // `out.len()` rather than the position in the iteration. `index` is
        // read back as a position in this vector - [`owner_of`] returns one and
        // callers index with it - so a skipped bound must not leave a hole in
        // the numbering. The guard above cannot fire today, because the bounds
        // are deduplicated and strictly inside `0..strip_height`; a counter
        // that is right by construction costs nothing and does not depend on
        // that staying true.
        out.push(Segment {
            index: out.len(),
            start,
            end,
            detect_end: end.saturating_add(DETECTION_OVERLAP).min(strip_height),
        });
        start = end;
    }
    out
}

/// A box as one segment's detector saw it, already converted to **strip**
/// coordinates.
///
/// **Write-only from outside this module.** There is a constructor and there is
/// no accessor: the rectangle goes in and comes back out only through
/// [`merge_global`], as part of a [`GlobalBox`] that exists once. That is rule
/// 3's "never by per-tile centroid" expressed as a visibility rather than as a
/// convention - a caller holding one of these has nothing to compute a centroid
/// from.
///
/// ```
/// use cleaner_core::mask::Rect;
/// use cleaner_core::strip::{DetectedInSegment, detection_segments, merge_global};
///
/// let segments = detection_segments(9_000, &[]);
/// let seen = DetectedInSegment::new(0, Rect::new(100, 1_800, 300, 200), 0.9);
/// // The rectangle comes back only as part of a merged box, which has one
/// // owner however many segments saw it.
/// let merged = merge_global(&[seen], &[], &segments);
/// assert_eq!(merged[0].owner, 0);
/// ```
///
/// and the question rule 3 forbids does not compile:
///
/// ```compile_fail
/// use cleaner_core::mask::Rect;
/// use cleaner_core::strip::DetectedInSegment;
///
/// let seen = DetectedInSegment::new(0, Rect::new(100, 1_800, 300, 200), 0.9);
/// let centre = seen.rect.y + seen.rect.h as i64 / 2;
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DetectedInSegment {
    segment: usize,
    rect: Rect,
    confidence: f32,
}

impl DetectedInSegment {
    /// One detector's box, in strip coordinates, and which segment ran the
    /// detector.
    pub fn new(segment: usize, rect: Rect, confidence: f32) -> DetectedInSegment {
        DetectedInSegment { segment, rect, confidence }
    }
}

/// A box on the global list: one box, one owner, and the segments that saw it.
#[derive(Debug, Clone, PartialEq)]
pub struct GlobalBox {
    pub rect: Rect,
    pub confidence: f32,
    /// Every detection segment that contributed evidence, ascending. Length
    /// above one is the overlap doing its job.
    pub segments: Vec<usize>,
    /// The segment that will process it. Decided from `rect`, which exists
    /// once.
    pub owner: usize,
}

fn area(rect: &Rect) -> i64 {
    rect.w as i64 * rect.h as i64
}

fn intersection(a: &Rect, b: &Rect) -> i64 {
    let w = (a.right().min(b.right()) - a.x.max(b.x)).max(0);
    let h = (a.bottom().min(b.bottom()) - a.y.max(b.y)).max(0);
    w * h
}

fn hull(a: &Rect, b: &Rect) -> Rect {
    let x = a.x.min(b.x);
    let y = a.y.min(b.y);
    let right = a.right().max(b.right());
    let bottom = a.bottom().max(b.bottom());
    Rect { x, y, w: (right - x) as u32, h: (bottom - y) as u32 }
}

fn iou(a: &Rect, b: &Rect) -> f32 {
    let inter = intersection(a, b);
    let union = area(a) + area(b) - inter;
    if union <= 0 { 0.0 } else { inter as f32 / union as f32 }
}

fn overlaps_horizontally(a: &Rect, b: &Rect) -> bool {
    a.right().min(b.right()) > a.x.max(b.x)
}

/// Step 1: the two halves of a box the split cut.
///
/// Both facing edges within [`SPLIT_EDGE_TOLERANCE`] of the *same* split line,
/// and overlapping in x. The x test is what stops two unrelated boxes that
/// happen to end at the split - a caption above it and a bubble below - from
/// being welded into one region spanning the width of the page.
fn cut_by_the_same_split(a: &Rect, b: &Rect, splits: &[u32]) -> bool {
    if !overlaps_horizontally(a, b) {
        return false;
    }
    let (upper, lower) = if a.y <= b.y { (a, b) } else { (b, a) };
    splits.iter().any(|split| {
        let split = *split as i64;
        (upper.bottom() - split).abs() <= SPLIT_EDGE_TOLERANCE
            && (lower.y - split).abs() <= SPLIT_EDGE_TOLERANCE
    })
}

/// Rule 3's three merge steps and the ownership decision.
///
/// **Every step is decided on the rectangles as they were detected**, never on
/// a hull an earlier merge produced. Rule 3 is one pass over the list, and a
/// fixpoint over hulls is a different rule with a different answer: A and B
/// overlapping by a quarter make a hull, and a third box that touched neither
/// original then lies wholly inside that hull's rectangle and is swallowed at
/// 100%. Two genuinely distinct bubbles merged into one region is a wrong
/// clean, not a cosmetic defect - one fill tone measured across both, one
/// engine crop spanning the gap between them.
///
/// So the three steps are three *relations* over the original rectangles, and
/// what comes back is the partition they induce: two detections end up in the
/// same box exactly when a chain of **detected** rectangles connects them. That
/// is independent of which segment reported a box first, and needs no
/// fixpoint to make it so; it is why this does not follow
/// [`crate::detect`]'s per-page merge - that one reproduces upstream, because the size statistics are
/// calibrated against the box population it produces, and this one has no
/// upstream to reproduce.
pub fn merge_global(found: &[DetectedInSegment], splits: &[Split], segments: &[Segment]) -> Vec<GlobalBox> {
    let split_rows: Vec<u32> = splits.iter().map(|s| s.y).collect();

    // Strip order first: it is the order rule 3 names, it is what lets the
    // sweep below stop early, and it is what nothing after this point depends
    // on the reporting order for.
    let mut seen: Vec<&DetectedInSegment> = found.iter().collect();
    seen.sort_by(|a, b| in_strip_order(&a.rect, &b.rect));

    let mut groups = Partition::new(seen.len());
    // Step 1 - union any pair whose edges lie within 8 px of the same split.
    relate(&seen, &mut groups, |a, b| cut_by_the_same_split(a, b, &split_rows));
    // Step 2 - deduplicate by IoU.
    relate(&seen, &mut groups, |a, b| iou(a, b) >= DEDUPE_IOU);
    // Step 3 - merge at >20% of the smaller box's area.
    relate(&seen, &mut groups, |a, b| {
        let smaller = area(a).min(area(b));
        smaller > 0 && intersection(a, b) as f64 > MERGE_AREA_FRACTION as f64 * smaller as f64
    });

    let mut at: Vec<Option<usize>> = vec![None; seen.len()];
    let mut boxes: Vec<GlobalBox> = Vec::new();
    for (i, detection) in seen.iter().enumerate() {
        let root = groups.root(i);
        match at[root] {
            Some(index) => {
                let global = &mut boxes[index];
                global.rect = hull(&global.rect, &detection.rect);
                global.confidence = global.confidence.max(detection.confidence);
                global.segments.push(detection.segment);
            }
            None => {
                at[root] = Some(boxes.len());
                boxes.push(GlobalBox {
                    rect: detection.rect,
                    confidence: detection.confidence,
                    segments: vec![detection.segment],
                    owner: 0,
                });
            }
        }
    }

    for global in boxes.iter_mut() {
        global.segments.sort_unstable();
        global.segments.dedup();
    }
    boxes.sort_by(|a, b| in_strip_order(&a.rect, &b.rect));
    for global in boxes.iter_mut() {
        global.owner = owner_of(&global.rect, segments);
    }
    boxes
}

/// Strip `(y1, x1)` order, which rule 3 names and which is the only reason two
/// runs over the same page agree.
fn in_strip_order(a: &Rect, b: &Rect) -> std::cmp::Ordering {
    a.y.cmp(&b.y).then(a.x.cmp(&b.x)).then(a.h.cmp(&b.h)).then(a.w.cmp(&b.w))
}

/// Union every pair the relation holds for, in one sweep over a list already in
/// strip order.
///
/// The inner loop stops early, and the bound is a property of all three of rule
/// 3's relations rather than a heuristic: steps 2 and 3 both need a positive
/// intersection, so the lower box must begin above the upper box's bottom edge,
/// and step 1 needs both facing edges within [`SPLIT_EDGE_TOLERANCE`] of one
/// split line, so the lower box begins at most twice that below it. Past
/// `bottom + 2 × 8` no box in the rest of the list can be related to this one.
///
/// This is the difference between a few thousand boxes down a 200-page strip
/// costing one sweep and costing a rescan-and-resort per merge.
fn relate(
    seen: &[&DetectedInSegment],
    groups: &mut Partition,
    related: impl Fn(&Rect, &Rect) -> bool,
) {
    for (i, above) in seen.iter().enumerate() {
        let upper = &above.rect;
        let reach = upper.bottom() + 2 * SPLIT_EDGE_TOLERANCE;
        for (j, below) in seen.iter().enumerate().skip(i + 1) {
            if below.rect.y > reach {
                break;
            }
            if related(upper, &below.rect) {
                groups.join(i, j);
            }
        }
    }
}

/// Disjoint sets over the detected boxes. The root of a set is its lowest
/// member index, so which box a group is built from is decided by strip order
/// and not by the order the relations happened to fire in.
struct Partition {
    parent: Vec<usize>,
}

impl Partition {
    fn new(count: usize) -> Partition {
        Partition { parent: (0..count).collect() }
    }

    fn root(&mut self, mut i: usize) -> usize {
        while self.parent[i] != i {
            self.parent[i] = self.parent[self.parent[i]];
            i = self.parent[i];
        }
        i
    }

    fn join(&mut self, a: usize, b: usize) {
        let (a, b) = (self.root(a), self.root(b));
        if a != b {
            self.parent[a.max(b)] = a.min(b);
        }
    }
}

/// **Ownership, decided on the global list.**
///
/// The point is the input, not the arithmetic: this is a function of one
/// rectangle in strip coordinates, and there is exactly one of those per box
/// however many segments saw it. The rule rule 3 forbids - a centroid computed
/// in each tile's own frame - is not wrong because a centroid is a bad
/// statistic; it is wrong because it is asked of a box that exists twice, and
/// it answers twice.
///
/// The top edge rather than the centre, because it is the coordinate the list
/// is already sorted on, so ownership and order agree by construction.
///
/// A row no segment owns is off the strip, and the fallback is **split by
/// sign**: past the end it is the last segment, above the start it is the
/// first. One fallback for both is right in one direction and wrong in the
/// other - a box at a negative `y` would be handed to the segment at the far
/// end of a 200-page chapter, which is the worst answer available rather than
/// the nearest one.
fn owner_of(rect: &Rect, segments: &[Segment]) -> usize {
    let Some(first) = segments.first() else { return 0 };
    if let Some(owner) = segments.iter().find(|s| s.owns_row(rect.y)) {
        return owner.index;
    }
    if rect.y < first.start as i64 {
        first.index
    } else {
        segments[segments.len() - 1].index
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strip::SplitKind;

    fn split(y: u32) -> Split {
        Split { y, kind: SplitKind::Minimum }
    }

    fn seen(segment: usize, x: i64, y: i64, w: u32, h: u32) -> DetectedInSegment {
        DetectedInSegment::new(segment, Rect::new(x, y, w, h), 0.9)
    }

    #[test]
    fn a_segment_processes_less_than_it_looks_at() {
        let segments = detection_segments(9_000, &[split(2000), split(4000), split(6000)]);
        assert_eq!(segments.len(), 4);
        assert_eq!(segments[0], Segment { index: 0, start: 0, end: 2000, detect_end: 2900 });
        assert_eq!(segments[3].end, 9_000);
        assert_eq!(segments[3].detect_end, 9_000, "the last segment has nowhere to reach");
        // The overlap is exactly rule 3's, and it is where the next segment's
        // work begins.
        assert_eq!(segments[0].detect_end - segments[1].start, DETECTION_OVERLAP);
        assert!(!segments[0].owns_row(2000), "the split row belongs to the segment below");
        assert!(segments[1].owns_row(2000));
    }

    /// Rule 3's whole reason for the overlap, stated as the failure it removes:
    /// with disjoint segments the two halves are **disjoint**, IoU = 0, and the
    /// dedupe cannot fire.
    #[test]
    fn two_disjoint_halves_of_a_clipped_bubble_have_no_iou_to_dedupe_on() {
        let top = Rect::new(100, 1800, 300, 200); // …1800–2000
        let bottom = Rect::new(100, 2000, 300, 240); // 2000–2240
        assert_eq!(iou(&top, &bottom), 0.0);

        // Step 1 is what catches them: both edges within 8 px of the split.
        let splits = [split(2000)];
        let segments = detection_segments(9_000, &splits);
        let merged = merge_global(
            &[seen(0, 100, 1800, 300, 200), seen(1, 100, 2000, 300, 240)],
            &splits,
            &segments,
        );
        assert_eq!(merged.len(), 1, "the clipped bubble is one region");
        assert_eq!(merged[0].rect, Rect::new(100, 1800, 300, 440));
        assert_eq!(merged[0].segments, vec![0, 1]);
    }

    /// Step 1's x test. A caption ending at the split and a bubble starting at
    /// it are two regions, not one band across the page.
    #[test]
    fn two_unrelated_boxes_that_merely_touch_the_split_stay_apart() {
        let splits = [split(2000)];
        let segments = detection_segments(9_000, &splits);
        let merged = merge_global(
            &[seen(0, 40, 1900, 200, 100), seen(1, 500, 2000, 200, 100)],
            &splits,
            &segments,
        );
        assert_eq!(merged.len(), 2);
    }

    /// Step 2. The overlap means the same bubble is detected twice, once per
    /// segment, at very slightly different coordinates.
    #[test]
    fn the_same_box_seen_by_two_overlapping_segments_becomes_one() {
        let splits = [split(2000)];
        let segments = detection_segments(9_000, &splits);
        let merged = merge_global(
            &[seen(0, 100, 2100, 300, 400), seen(1, 104, 2106, 300, 396)],
            &splits,
            &segments,
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].segments, vec![0, 1]);
    }

    /// **Rule 3's "one pass over the list", and what a closure over hulls does
    /// instead.**
    ///
    /// `a` and `b` overlap at a quarter of the smaller, so step 3 merges them
    /// and their hull is the square `0..150`. `c` sits inside that square and
    /// touches neither original - no x overlap with `a`, no y overlap with `b`.
    /// Merging hulls transitively then finds `c` 100% inside the hull and
    /// swallows it, which is two distinct bubbles cleaned as one region.
    #[test]
    fn a_third_box_inside_the_hull_of_two_others_is_not_swallowed_by_it() {
        let segments = detection_segments(9_000, &[]);
        let a = Rect::new(0, 0, 100, 100);
        let b = Rect::new(50, 50, 100, 100);
        let c = Rect::new(105, 5, 40, 40);

        assert!(intersection(&a, &b) as f64 > MERGE_AREA_FRACTION as f64 * area(&a) as f64);
        assert_eq!(intersection(&a, &c), 0, "c touches neither original");
        assert_eq!(intersection(&b, &c), 0);
        let swallowing = hull(&a, &b);
        assert_eq!(intersection(&swallowing, &c), area(&c), "…and is inside their hull");

        let merged = merge_global(
            &[seen(0, 0, 0, 100, 100), seen(0, 50, 50, 100, 100), seen(0, 105, 5, 40, 40)],
            &[],
            &segments,
        );
        assert_eq!(merged.len(), 2, "the third box was merged into a hull it never touched");
        assert_eq!(merged[0].rect, swallowing);
        assert_eq!(merged[1].rect, c);
    }

    /// The same rule from the other side: a chain of boxes that **do** overlap
    /// each other in turn is one region, because every link is a relation
    /// between two rectangles that were detected rather than between hulls.
    #[test]
    fn a_chain_of_genuinely_overlapping_boxes_is_still_one_region() {
        let segments = detection_segments(9_000, &[]);
        let merged = merge_global(
            &[seen(0, 0, 0, 100, 100), seen(0, 0, 50, 100, 100), seen(0, 0, 100, 100, 100)],
            &[],
            &segments,
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].rect, Rect::new(0, 0, 100, 200));
    }

    /// The one thing in this geometry that has to survive the exit criterion's
    /// corpus. A 200-page strip's global list is a few thousand boxes; rescanning
    /// every pair and re-sorting after each merge is O(n³ log n) and this input -
    /// a long quiet run followed by a pile that all merges - is its worst
    /// case, because every merge rescans the whole quiet run before reaching the
    /// pile. The sweep does it in one pass per relation.
    #[test]
    fn a_few_thousand_boxes_do_not_cost_a_rescan_per_merge() {
        let segments = detection_segments(9_000_000, &[]);
        let mut found: Vec<DetectedInSegment> =
            (0..2_000).map(|i| seen(0, 0, i * 400, 100, 100)).collect();
        found.extend((0..1_000).map(|_| seen(1, 0, 8_000_000, 300, 300)));

        let merged = merge_global(&found, &[], &segments);
        assert_eq!(merged.len(), 2_001, "the quiet run stayed apart and the pile became one");
        assert_eq!(merged[2_000].rect, Rect::new(0, 8_000_000, 300, 300));
        assert_eq!(merged[2_000].segments, vec![1]);
    }

    /// **The sentence rule 3 is written for.** A clipped box has a different
    /// centroid in each tile, so both tiles claim it; the global list has one
    /// box and therefore one owner.
    ///
    /// The forbidden answer is computed here, from inside the module, which is
    /// the only place it can be computed: [`DetectedInSegment`]'s rectangle and
    /// [`Segment::owns_row`] are both out of reach of a caller. The doctest on
    /// [`DetectedInSegment`] is the other half of that claim - it asserts the
    /// same three lines do not compile outside.
    #[test]
    fn ownership_comes_from_the_global_box_and_not_from_a_per_tile_centroid() {
        let splits = [split(2000)];
        let segments = detection_segments(9_000, &splits);
        let halves = [seen(0, 100, 1800, 300, 200), seen(1, 100, 2000, 300, 240)];

        // What the forbidden rule produces, computed here so the test says what
        // it is protecting against rather than only asserting the good case.
        let by_tile_centroid: Vec<usize> = halves
            .iter()
            .map(|h| {
                let centre = h.rect.y + h.rect.h as i64 / 2;
                segments.iter().find(|s| s.owns_row(centre)).map(|s| s.index).unwrap()
            })
            .collect();
        assert_eq!(by_tile_centroid, vec![0, 1], "both segments claim the same bubble");

        let merged = merge_global(&halves, &splits, &segments);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].owner, 0, "one box, one owner, decided on the strip rectangle");
    }

    #[test]
    fn the_global_list_does_not_depend_on_the_order_the_segments_reported_in() {
        let splits = [split(2000), split(4200)];
        let segments = detection_segments(9_000, &splits);
        let found = vec![
            seen(1, 100, 2100, 300, 400),
            seen(0, 100, 1800, 300, 200),
            seen(1, 104, 2106, 300, 396),
            seen(1, 100, 2000, 300, 240),
            seen(2, 600, 5000, 200, 200),
        ];
        let mut reversed = found.clone();
        reversed.reverse();
        assert_eq!(merge_global(&found, &splits, &segments), merge_global(&reversed, &splits, &segments));
        let merged = merge_global(&found, &splits, &segments);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged.iter().map(|b| b.owner).collect::<Vec<_>>(), vec![0, 2]);
    }

    #[test]
    fn a_strip_with_no_splits_is_one_segment() {
        let segments = detection_segments(3_000, &[]);
        assert_eq!(segments, vec![Segment { index: 0, start: 0, end: 3_000, detect_end: 3_000 }]);
    }

    /// `Segment::index` is read back as a position in the returned list - it is
    /// what [`owner_of`] answers - so it has to be one on every shape the
    /// function accepts, including the degenerate ones.
    #[test]
    fn a_segments_index_is_its_position_in_the_list() {
        for (height, splits) in [
            (9_000u32, vec![split(2_000), split(4_000)]),
            (9_000, vec![split(4_000), split(2_000), split(2_000)]),
            (9_000, vec![split(0), split(9_000), split(12_000)]),
            (2_000, vec![split(2_000)]),
            (0, vec![]),
            (0, vec![split(1_000)]),
        ] {
            let segments = detection_segments(height, &splits);
            assert!(!segments.is_empty(), "{height} / {splits:?} produced no segment at all");
            for (position, segment) in segments.iter().enumerate() {
                assert_eq!(segment.index, position, "{height} / {splits:?}");
            }
        }
    }

    /// A row off the strip has no owner, and the fallback is split by sign: the
    /// last segment past the end, the **first** above the start. One fallback
    /// for both hands a box at a negative `y` to the segment at the far end of
    /// the chapter.
    #[test]
    fn a_row_off_the_strip_falls_back_to_the_nearer_end() {
        let segments = detection_segments(9_000, &[split(2_000), split(4_000)]);
        assert_eq!(owner_of(&Rect::new(0, 5_000, 10, 10), &segments), 2);
        assert_eq!(owner_of(&Rect::new(0, 20_000, 10, 10), &segments), 2, "past the end");
        assert_eq!(owner_of(&Rect::new(0, -50, 10, 10), &segments), 0, "above the start");
        assert_eq!(owner_of(&Rect::new(0, -50, 10, 10), &[]), 0, "and no segments at all");
    }
}
