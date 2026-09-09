//! Whether a page boundary may be read across, and what to say when it may not.
//!
//! A join is verified when the anomaly check finds no duplicated overlap
//! rows and no misregistration. Unverified joins are treated as page edges
//! and reported. In longstrip, that means detecting duplicated overlap rows
//! and 1-px seams between consecutive pages and reporting them, and never
//! silently misregistering.
//!
//! The check reads [`JOIN_PROBE_ROWS`] rows either side of one boundary and
//! nothing else. That is the whole reason [`EdgeRows`] exists rather than a
//! `&Raster` pair: two 800×4000 pages resident to compare thirty-two rows is
//! the shape this is written against.
//!
//! ## Every check abstains rather than guessing
//!
//! Each of the three below has a precondition under which it cannot tell, and
//! each returns "verified" in that case rather than a maybe. A false anomaly is
//! not free: an unverified join is treated as a page edge, so a bubble split
//! across it stops being read as one region - which is the defect rule 1 exists
//! to prevent, produced by the check meant to protect it.

use crate::image::Raster;

/// How many rows either side of a join the check reads. Enough to establish
/// what the pages look like *away* from the boundary, which is what every
/// abstention below is decided on, and small enough that a join costs two
/// row-strips rather than two pages.
pub const JOIN_PROBE_ROWS: u32 = 16;

/// Mean absolute difference, in 0–255 levels, below which two rows are the same
/// row. Not zero: a page pair round-tripped through a lossy encoder is the
/// common case, and an exact-equality test would find no duplicate anywhere.
const ROW_TOLERANCE: f32 = 1.0;

/// A candidate duplicate block must vary spatially by at least this much,
/// averaged over its rows. **Blank paper repeats trivially**: the last row of a
/// white margin equals the first row of the next page's white margin on every
/// webtoon ever exported, and without this floor every join in the corpus is an
/// anomaly.
const CONTENT_FLOOR: f32 = 2.0;

/// How far sideways the misregistration test looks.
const SHIFT_LIMIT: i64 = 8;

/// A shifted match must beat the unshifted one by this factor to count as
/// misregistration rather than as noise.
const SHIFT_MARGIN: f32 = 2.0;

/// A shifted match must itself be this good, in levels, or the two pages simply
/// do not continue into each other and there is nothing to misregister.
const SHIFT_MATCH: f32 = 4.0;

/// How far a row's mean may sit from its neighbourhood before it is a seam.
const SEAM_DEVIATION: f32 = 12.0;

/// If the neighbourhood itself varies by more than this, the seam test abstains
/// - a row that differs from a varying background is not evidence of anything.
const SEAM_SPREAD: f32 = 6.0;

/// A few rows at one end of a page, as luma in 0–255.
///
/// Levels rather than the raw 16-bit luma so that every constant above reads in
/// the same units thresholds are stated in elsewhere.
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeRows {
    pub width: u32,
    /// Top-to-bottom, in page order, whichever end they came from.
    pub rows: Vec<Vec<f32>>,
}

impl EdgeRows {
    fn of(page: &Raster, first: u32, count: u32) -> EdgeRows {
        let rows = (first..(first + count).min(page.height))
            .map(|y| (0..page.width).map(|x| page.luma16_at(x, y) as f32 / 257.0).collect())
            .collect();
        EdgeRows { width: page.width, rows }
    }

    pub fn top_of(page: &Raster, count: u32) -> EdgeRows {
        EdgeRows::of(page, 0, count)
    }

    pub fn bottom_of(page: &Raster, count: u32) -> EdgeRows {
        EdgeRows::of(page, page.height.saturating_sub(count), count)
    }
}

fn mean(row: &[f32]) -> f32 {
    if row.is_empty() {
        return 0.0;
    }
    row.iter().sum::<f32>() / row.len() as f32
}

fn spatial_std(row: &[f32]) -> f32 {
    if row.is_empty() {
        return 0.0;
    }
    let m = mean(row);
    (row.iter().map(|v| (v - m) * (v - m)).sum::<f32>() / row.len() as f32).sqrt()
}

fn row_difference(a: &[f32], b: &[f32], shift: i64) -> f32 {
    let mut total = 0.0f32;
    let mut counted = 0usize;
    for (x, av) in a.iter().enumerate() {
        let bx = x as i64 + shift;
        if bx < 0 || bx as usize >= b.len() {
            continue;
        }
        total += (av - b[bx as usize]).abs();
        counted += 1;
    }
    if counted == 0 { f32::INFINITY } else { total / counted as f32 }
}

/// What the check found. Every variant means the same thing operationally -
/// the join is not verified, so it is treated as a page edge - and they are
/// distinguished because the remedies differ and because §2 asks for them to be
/// reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinAnomaly {
    /// The last rows of the page above are the first rows of the page below. A
    /// webtoon sliced with overlap, or a page exported twice.
    DuplicatedRows { rows: u32 },
    /// The content continues, but sideways by `dx` pixels.
    Misregistered { dx: i64 },
    /// The pages are not the same width, so the join cannot be established at
    /// all.
    WidthMismatch { above: u32, below: u32 },
    /// §2's "1-px seam": one or two rows at the boundary that belong to neither
    /// page's neighbourhood - the bright or dark line a slicer leaves behind.
    Seam { rows: u32 },
}

impl JoinAnomaly {
    /// The catalogue key this is reported under, or `None` where the catalogue
    /// has no way to say it.
    ///
    /// **Only the first variant has a key.** `notice.input.joinAnomaly` reads
    /// *"Duplicated overlap rows between positions {first} and {second}."* -
    /// which is exactly [`JoinAnomaly::DuplicatedRows`] and is a false statement
    /// about the other three. Reporting a misregistration under it would
    /// repeat a known failure: telling the user the opposite of what
    /// happened because the vocabulary had room for one case.
    ///
    /// A key is a vocabulary decision and not the core's to invent: a
    /// key the core makes up fails `catalogue.test.js` in direction 1. So three
    /// of the four have no sentence to be reported in, and that is
    /// **recorded as an open gap** - the same kind of vocabulary decision
    /// made elsewhere in this crate.
    ///
    /// The gap in the *catalogue* is three of four. The gap in **reporting** is
    /// four of four, and it is a different thing: nothing in this crate or in
    /// `src-tauri` calls [`check_join`] or [`Joins::anomalies`], so no anomaly
    /// of any variant reaches a user today, including the one that does have a
    /// key. That is the module note in [`super`] - the geometry is built and
    /// has no callers - and it is why the open gap is about the report and
    /// not only about the vocabulary.
    ///
    /// What is not at stake either way is the behaviour, because there is no
    /// behaviour yet to be wrong: when the reads are wired, an unverified join
    /// is a page edge whether or not a notice was shown, so the read is bounded
    /// even where the sentence is missing.
    pub fn reason_key(&self) -> Option<&'static str> {
        match self {
            JoinAnomaly::DuplicatedRows { .. } => Some("notice.input.joinAnomaly"),
            JoinAnomaly::Misregistered { .. }
            | JoinAnomaly::WidthMismatch { .. }
            | JoinAnomaly::Seam { .. } => None,
        }
    }
}

/// What is known about one join. `Unchecked` is a third state on purpose: it is
/// **not** verified, so reads do not cross it, and it is **not** an anomaly, so
/// nothing is reported. A strip whose joins have not been probed yet is in
/// exactly that position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinState {
    Unchecked,
    Verified,
    Anomalous(JoinAnomaly),
}

/// The state of every join in a strip, indexed by the page above it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Joins {
    states: Vec<JoinState>,
}

impl Joins {
    /// `count` joins, none of them probed. The safe default: rule 1 makes an
    /// unverified join a page edge, so a caller that forgets to run the check
    /// gets the conservative read rather than the optimistic one.
    pub fn unchecked(count: usize) -> Joins {
        Joins { states: vec![JoinState::Unchecked; count] }
    }

    pub fn len(&self) -> usize {
        self.states.len()
    }

    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }

    pub fn set(&mut self, join: usize, state: JoinState) {
        if let Some(slot) = self.states.get_mut(join) {
            *slot = state;
        }
    }

    pub fn state(&self, join: usize) -> JoinState {
        self.states.get(join).copied().unwrap_or(JoinState::Unchecked)
    }

    pub fn is_verified(&self, join: usize) -> bool {
        matches!(self.state(join), JoinState::Verified)
    }

    /// Every join that failed, with the index of the page above it.
    ///
    /// What "report them" would be read from. **It is read from nowhere
    /// yet**: this has no caller outside the tests, and neither does
    /// [`check_join`], so the requirement is unmet for all four variants and
    /// not only for the three the catalogue cannot say.
    pub fn anomalies(&self) -> impl Iterator<Item = (usize, JoinAnomaly)> + '_ {
        self.states.iter().enumerate().filter_map(|(i, s)| match s {
            JoinState::Anomalous(a) => Some((i, *a)),
            _ => None,
        })
    }
}

/// §2's anomaly check, over one boundary.
///
/// The order of the tests is the order of their certainty. A width mismatch is
/// a fact about the headers; a duplicate is a fact about the pixels; a
/// misregistration and a seam are inferences, and each abstains where it cannot
/// tell.
pub fn check_join(above: &EdgeRows, below: &EdgeRows) -> JoinState {
    if above.width != below.width {
        return JoinState::Anomalous(JoinAnomaly::WidthMismatch {
            above: above.width,
            below: below.width,
        });
    }
    if above.rows.is_empty() || below.rows.is_empty() {
        return JoinState::Unchecked;
    }

    if let Some(rows) = duplicated_rows(above, below) {
        return JoinState::Anomalous(JoinAnomaly::DuplicatedRows { rows });
    }
    if let Some(dx) = misregistration(above, below) {
        return JoinState::Anomalous(JoinAnomaly::Misregistered { dx });
    }
    if let Some(rows) = seam(above, below) {
        return JoinState::Anomalous(JoinAnomaly::Seam { rows });
    }
    JoinState::Verified
}

/// The longest run of rows the two pages share at the boundary.
///
/// Longest rather than shortest because a two-row overlap is also a one-row
/// overlap, and the number is reported.
fn duplicated_rows(above: &EdgeRows, below: &EdgeRows) -> Option<u32> {
    let limit = above.rows.len().min(below.rows.len());
    for n in (1..=limit).rev() {
        let tail = &above.rows[above.rows.len() - n..];
        let head = &below.rows[..n];
        if tail.iter().zip(head).any(|(a, b)| row_difference(a, b, 0) > ROW_TOLERANCE) {
            continue;
        }
        // Blank paper repeats trivially. Only a block that carries content is
        // evidence that anything was duplicated.
        let content = tail.iter().map(|r| spatial_std(r)).sum::<f32>() / n as f32;
        if content < CONTENT_FLOOR {
            continue;
        }
        return Some(n as u32);
    }
    None
}

/// Whether the content continues across the join, but sideways.
///
/// Two pages that are simply different - the ordinary case, and what a page
/// boundary in a paginated chapter *is* - match at no offset, so the test
/// abstains. It fires only where a shifted match is both good in absolute terms
/// and clearly better than the unshifted one, which is what a mis-cut slice
/// looks like and what an unrelated page pair cannot produce.
fn misregistration(above: &EdgeRows, below: &EdgeRows) -> Option<i64> {
    let last = above.rows.last()?;
    let first = below.rows.first()?;
    if spatial_std(last) < CONTENT_FLOOR || spatial_std(first) < CONTENT_FLOOR {
        return None;
    }

    let straight = row_difference(last, first, 0);
    let mut best = (0i64, straight);
    for shift in -SHIFT_LIMIT..=SHIFT_LIMIT {
        let d = row_difference(last, first, shift);
        if d < best.1 {
            best = (shift, d);
        }
    }
    let (dx, matched) = best;
    if dx != 0 && matched < SHIFT_MATCH && matched * SHIFT_MARGIN < straight {
        Some(dx)
    } else {
        None
    }
}

/// §2's "1-px seam": rows at the boundary that belong to neither side.
///
/// Abstains when the neighbourhood is itself varying, which is most of a busy
/// page. What it catches is the case it is named for - a flat run of rows with
/// one bright or dark line dropped into it at the cut.
fn seam(above: &EdgeRows, below: &EdgeRows) -> Option<u32> {
    let context: Vec<f32> = above.rows[..above.rows.len() - 1]
        .iter()
        .rev()
        .take(4)
        .chain(below.rows[1..].iter().take(4))
        .map(|r| mean(r))
        .collect();
    if context.len() < 4 {
        return None;
    }
    let lo = context.iter().copied().fold(f32::INFINITY, f32::min);
    let hi = context.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    if hi - lo > SEAM_SPREAD {
        return None;
    }
    let reference = mean(&context);

    let mut rows = 0u32;
    if (mean(above.rows.last()?) - reference).abs() > SEAM_DEVIATION {
        rows += 1;
    }
    if (mean(below.rows.first()?) - reference).abs() > SEAM_DEVIATION {
        rows += 1;
    }
    (rows > 0).then_some(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rows with structure, so the content floor is cleared.
    ///
    /// Hashed rather than drawn from a lattice like `(x * 7 + y * 31) % 97`.
    /// That shape is periodic in x, so the misregistration test finds an exact
    /// match at a nonzero shift and every join in the file comes back
    /// misregistered - the same trap that once caught the unit fixture
    /// through the periodicity check.
    fn textured(seed: u32, count: u32, width: u32) -> Vec<Vec<f32>> {
        (0..count)
            .map(|y| {
                (0..width)
                    .map(|x| {
                        let mut h = x
                            .wrapping_mul(0x9E37_79B1)
                            ^ (y + seed).wrapping_mul(0x85EB_CA6B)
                            ^ 0xC2B2_AE35;
                        h ^= h >> 15;
                        h = h.wrapping_mul(0x2545_F491);
                        h ^= h >> 13;
                        100.0 + (h % 200) as f32 * 0.5
                    })
                    .collect()
            })
            .collect()
    }

    fn flat(level: f32, count: u32, width: u32) -> Vec<Vec<f32>> {
        (0..count).map(|_| vec![level; width as usize]).collect()
    }

    #[test]
    fn a_clean_join_between_two_different_pages_verifies() {
        let above = EdgeRows { width: 40, rows: textured(0, 8, 40) };
        let below = EdgeRows { width: 40, rows: textured(500, 8, 40) };
        assert_eq!(check_join(&above, &below), JoinState::Verified);
    }

    /// §2's first case, and the only one the catalogue can say.
    #[test]
    fn duplicated_overlap_rows_are_found_and_counted() {
        let shared = textured(11, 3, 40);
        let mut above_rows = textured(0, 5, 40);
        above_rows.extend(shared.clone());
        let mut below_rows = shared;
        below_rows.extend(textured(90, 5, 40));

        let above = EdgeRows { width: 40, rows: above_rows };
        let below = EdgeRows { width: 40, rows: below_rows };
        let state = check_join(&above, &below);
        assert_eq!(state, JoinState::Anomalous(JoinAnomaly::DuplicatedRows { rows: 3 }));
        let JoinState::Anomalous(anomaly) = state else { unreachable!() };
        assert_eq!(anomaly.reason_key(), Some("notice.input.joinAnomaly"));
    }

    /// The floor that keeps the corpus out of the report. Every webtoon join
    /// sits in a white margin, and a white row equals a white row.
    #[test]
    fn a_blank_margin_repeating_is_not_a_duplicate() {
        let above = EdgeRows { width: 40, rows: flat(246.0, 8, 40) };
        let below = EdgeRows { width: 40, rows: flat(246.0, 8, 40) };
        assert_eq!(check_join(&above, &below), JoinState::Verified);
    }

    #[test]
    fn content_that_continues_sideways_is_misregistration() {
        let mut above_rows = textured(0, 7, 40);
        let seam_row: Vec<f32> = (0..40).map(|x| 200.0 - ((x * 7) % 97) as f32).collect();
        above_rows.push(seam_row.clone());
        // The same row, shifted right by 3.
        let shifted: Vec<f32> =
            (0..40).map(|x| seam_row[((x as i64 - 3).rem_euclid(40)) as usize]).collect();
        let mut below_rows = vec![shifted];
        below_rows.extend(textured(0, 7, 40));

        let above = EdgeRows { width: 40, rows: above_rows };
        let below = EdgeRows { width: 40, rows: below_rows };
        assert!(matches!(
            check_join(&above, &below),
            JoinState::Anomalous(JoinAnomaly::Misregistered { .. })
        ));
    }

    /// §2's "1-px seam". Flat paper either side, one dark line at the cut.
    #[test]
    fn a_one_pixel_seam_at_the_cut_is_reported() {
        let mut above_rows = flat(246.0, 8, 40);
        *above_rows.last_mut().unwrap() = vec![120.0; 40];
        let below = EdgeRows { width: 40, rows: flat(246.0, 8, 40) };
        let above = EdgeRows { width: 40, rows: above_rows };
        assert_eq!(check_join(&above, &below), JoinState::Anomalous(JoinAnomaly::Seam { rows: 1 }));
    }

    /// The abstention that keeps the seam test off busy pages: a row that
    /// differs from a varying background is not evidence of a cut.
    #[test]
    fn the_seam_test_abstains_where_the_neighbourhood_is_already_varying() {
        let mut above_rows: Vec<Vec<f32>> =
            (0..8).map(|y| vec![100.0 + (y as f32) * 12.0; 40]).collect();
        *above_rows.last_mut().unwrap() = vec![40.0; 40];
        let above = EdgeRows { width: 40, rows: above_rows };
        let below = EdgeRows { width: 40, rows: flat(190.0, 8, 40) };
        assert_eq!(check_join(&above, &below), JoinState::Verified);
    }

    #[test]
    fn pages_of_different_widths_cannot_be_joined_at_all() {
        let above = EdgeRows { width: 40, rows: textured(0, 4, 40) };
        let below = EdgeRows { width: 36, rows: textured(1, 4, 36) };
        assert_eq!(
            check_join(&above, &below),
            JoinState::Anomalous(JoinAnomaly::WidthMismatch { above: 40, below: 36 })
        );
    }

    /// The three the catalogue has no sentence for. Named as a gap rather than
    /// papered over with the one key that exists, which says something else.
    #[test]
    fn three_anomalies_have_no_catalogue_key_and_say_so() {
        assert_eq!(JoinAnomaly::Misregistered { dx: 3 }.reason_key(), None);
        assert_eq!(JoinAnomaly::Seam { rows: 1 }.reason_key(), None);
        assert_eq!(JoinAnomaly::WidthMismatch { above: 8, below: 7 }.reason_key(), None);
    }

    #[test]
    fn an_unprobed_join_is_neither_verified_nor_an_anomaly() {
        let joins = Joins::unchecked(3);
        assert!(!joins.is_verified(0));
        assert_eq!(joins.anomalies().count(), 0);
        assert_eq!(joins.state(9), JoinState::Unchecked, "past the end is unchecked too");
    }

    #[test]
    fn edge_rows_come_off_the_right_end_of_the_page() {
        let mut raster = crate::image::fixtures::by_name("l8").raster;
        let (w, h) = (raster.width, raster.height);
        for x in 0..w {
            raster.data[((h - 1) as usize) * (w as usize) + x as usize] = 7;
        }
        let bottom = EdgeRows::bottom_of(&raster, 4);
        assert_eq!(bottom.rows.len(), 4);
        assert!(bottom.rows.last().unwrap().iter().all(|v| *v < 8.0));
        assert_eq!(EdgeRows::top_of(&raster, 4).rows.len(), 4);
    }
}
