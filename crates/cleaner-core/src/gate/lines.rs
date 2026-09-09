//! Orientation and line splitting, from the mask alone.
//!
//! The rule:
//!
//! > Line orientation is determined **without knowing the script**: compute
//! > horizontal and vertical projection profiles of the box's mask, take the
//! > axis with deeper normalised minima, split at minima below 15% of the
//! > profile peak; if neither axis is decisive, cluster components by centroid
//! > at ε = 1.5 × median component height.
//!
//! Without knowing the script is the point. A gate that needed the orientation
//! to decide the script, and the script to decide the orientation, would have
//! nothing to start from.

use crate::detect::Segmentation;
use crate::mask::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    /// Lines run left to right; they stack vertically.
    Horizontal,
    /// Columns run top to bottom; they stack right to left.
    Vertical,
}

/// Below this fraction of the profile's peak, a position is a gap between
/// lines rather than a thin part of one.
const GAP_FRACTION: f32 = 0.15;

/// A line, in page coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct TextLine {
    pub rect: Rect,
    pub ink: u64,
}

/// Ink per column and per row of the region, from the segmentation mask.
fn profiles(seg: &Segmentation, rect: Rect) -> (Vec<u32>, Vec<u32>) {
    let mut columns = vec![0u32; rect.w as usize];
    let mut rows = vec![0u32; rect.h as usize];
    for y in rect.y..rect.bottom() {
        for x in rect.x..rect.right() {
            if x < 0 || y < 0 || x >= seg.width as i64 || y >= seg.height as i64 {
                continue;
            }
            if seg.is_text(x as u32, y as u32) {
                columns[(x - rect.x) as usize] += 1;
                rows[(y - rect.y) as usize] += 1;
            }
        }
    }
    (columns, rows)
}

/// How deep the gaps in a profile are, as the fraction of positions below the
/// gap threshold. A profile with clean separations between lines scores high;
/// one that is ink all the way across scores near zero.
fn gap_depth(profile: &[u32]) -> f32 {
    let peak = profile.iter().copied().max().unwrap_or(0);
    if peak == 0 {
        return 0.0;
    }
    let threshold = peak as f32 * GAP_FRACTION;
    let gaps = profile.iter().filter(|v| (**v as f32) < threshold).count();
    gaps as f32 / profile.len() as f32
}

/// The axis whose profile has the deeper gaps.
///
/// Vertical text leaves gaps **between columns**, which show in the *column*
/// profile; horizontal text leaves gaps between lines, which show in the row
/// profile. So a deep column profile means vertical text.
pub fn orientation(seg: &Segmentation, rect: Rect) -> Orientation {
    let (columns, rows) = profiles(seg, rect);
    if gap_depth(&columns) > gap_depth(&rows) {
        Orientation::Vertical
    } else {
        Orientation::Horizontal
    }
}

/// Split a region into lines along the given orientation.
///
/// Returns them in reading order: top to bottom for horizontal text, **right
/// to left** for vertical, which is the order Japanese columns are read in and
/// therefore the order a reviewer expects to see them listed.
pub fn split(seg: &Segmentation, rect: Rect, orientation: Orientation) -> Vec<TextLine> {
    let (columns, rows) = profiles(seg, rect);
    let profile = match orientation {
        Orientation::Vertical => &columns,
        Orientation::Horizontal => &rows,
    };
    let peak = profile.iter().copied().max().unwrap_or(0);
    if peak == 0 {
        return Vec::new();
    }
    let threshold = peak as f32 * GAP_FRACTION;

    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut start: Option<usize> = None;
    for (i, value) in profile.iter().enumerate() {
        let inked = (*value as f32) >= threshold;
        match (inked, start) {
            (true, None) => start = Some(i),
            (false, Some(from)) => {
                runs.push((from, i));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(from) = start {
        runs.push((from, profile.len()));
    }

    let mut lines: Vec<TextLine> = runs
        .into_iter()
        .map(|(from, to)| {
            let rect = match orientation {
                Orientation::Vertical => Rect {
                    x: rect.x + from as i64,
                    y: rect.y,
                    w: (to - from) as u32,
                    h: rect.h,
                },
                Orientation::Horizontal => Rect {
                    x: rect.x,
                    y: rect.y + from as i64,
                    w: rect.w,
                    h: (to - from) as u32,
                },
            };
            let ink = ink_in(seg, rect);
            TextLine { rect, ink }
        })
        .filter(|line| line.ink > 0)
        .collect();

    if orientation == Orientation::Vertical {
        // Right to left.
        lines.sort_by_key(|line| std::cmp::Reverse(line.rect.x));
    } else {
        lines.sort_by_key(|line| line.rect.y);
    }
    lines
}

fn ink_in(seg: &Segmentation, rect: Rect) -> u64 {
    let mut total = 0u64;
    for y in rect.y.max(0)..rect.bottom().min(seg.height as i64) {
        for x in rect.x.max(0)..rect.right().min(seg.width as i64) {
            if seg.is_text(x as u32, y as u32) {
                total += 1;
            }
        }
    }
    total
}

/// How much of the glyph size to add back around a line before recognition.
///
/// The mask bounds the **ink**, and a recogniser wants the glyph: side
/// bearings, the anti-aliasing fringe, and the white space a stroke is read
/// against. Cropping to the ink alone costs real accuracy - a hiragana column
/// that reads `Japanese_vert` at 60 px wide reads `Latin` at the 43 px its mask
/// occupies. Proportional rather than fixed, because the cross-axis size of a
/// line *is* the glyph size.
const RECOGNITION_MARGIN: f32 = 0.15;

/// A line's box for recognition: trimmed to where its ink starts and stops
/// along the line's own direction, then grown by [`RECOGNITION_MARGIN`].
///
/// The trim matters because the split gives a column the region's full height
/// even when the text stops half way, and a strip of empty paper scaled to
/// 48 px tall is noise the recogniser has to see past. The growth matters for
/// the opposite reason.
pub fn tighten(seg: &Segmentation, line: &TextLine, orientation: Orientation) -> Rect {
    let rect = line.rect;
    let (columns, rows) = profiles(seg, rect);
    let along = match orientation {
        Orientation::Vertical => &rows,
        Orientation::Horizontal => &columns,
    };
    let first = along.iter().position(|v| *v > 0);
    let last = along.iter().rposition(|v| *v > 0);
    let trimmed = match (first, last) {
        (Some(from), Some(to)) => match orientation {
            Orientation::Vertical => Rect {
                x: rect.x,
                y: rect.y + from as i64,
                w: rect.w,
                h: (to - from + 1) as u32,
            },
            Orientation::Horizontal => Rect {
                x: rect.x + from as i64,
                y: rect.y,
                w: (to - from + 1) as u32,
                h: rect.h,
            },
        },
        _ => rect,
    };

    let across = match orientation {
        Orientation::Vertical => trimmed.w,
        Orientation::Horizontal => trimmed.h,
    };
    let margin = ((across as f32) * RECOGNITION_MARGIN).round().max(1.0) as u32;
    trimmed.grown(margin, seg.width, seg.height)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mask with ink in the given rectangles.
    fn seg_with(w: u32, h: u32, blocks: &[Rect]) -> Segmentation {
        let mut levels = vec![0u8; (w * h) as usize];
        for block in blocks {
            for y in block.y..block.bottom() {
                for x in block.x..block.right() {
                    if x >= 0 && y >= 0 && x < w as i64 && y < h as i64 {
                        levels[(y as usize) * (w as usize) + x as usize] = 255;
                    }
                }
            }
        }
        // A 1:1 letterbox, so the test's page coordinates and mask
        // coordinates are the same thing.
        Segmentation {
            width: w,
            height: h,
            levels,
            fit: crate::detect::Letterbox { scale: 1.0, fitted_w: w, fitted_h: h, page_w: w, page_h: h },
        }
    }

    #[test]
    fn three_columns_with_gaps_read_as_vertical() {
        let seg = seg_with(
            100,
            100,
            &[Rect::new(10, 10, 10, 80), Rect::new(30, 10, 10, 80), Rect::new(50, 10, 10, 80)],
        );
        let rect = Rect::new(0, 0, 100, 100);
        assert_eq!(orientation(&seg, rect), Orientation::Vertical);
        let lines = split(&seg, rect, Orientation::Vertical);
        assert_eq!(lines.len(), 3);
        // Right to left: the rightmost column is read first.
        assert_eq!(lines[0].rect.x, 50);
        assert_eq!(lines[2].rect.x, 10);
    }

    #[test]
    fn three_rows_with_gaps_read_as_horizontal() {
        let seg = seg_with(
            100,
            100,
            &[Rect::new(10, 10, 80, 10), Rect::new(10, 30, 80, 10), Rect::new(10, 50, 80, 10)],
        );
        let rect = Rect::new(0, 0, 100, 100);
        assert_eq!(orientation(&seg, rect), Orientation::Horizontal);
        let lines = split(&seg, rect, Orientation::Horizontal);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].rect.y, 10);
        assert_eq!(lines[2].rect.y, 50);
    }

    #[test]
    fn a_column_is_trimmed_to_where_its_ink_stops() {
        // Text fills the top half of a tall column; the split gives the whole
        // region's height, and the recogniser should not be handed the rest.
        let seg = seg_with(100, 200, &[Rect::new(40, 10, 12, 60)]);
        let rect = Rect::new(0, 0, 100, 200);
        let lines = split(&seg, rect, Orientation::Vertical);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].rect.h, 200);
        let tight = tighten(&seg, &lines[0], Orientation::Vertical);
        // Trimmed to the ink, then grown by 15% of the column's width so the
        // recogniser sees the glyph rather than only its strokes.
        let margin = (12.0f32 * RECOGNITION_MARGIN).round() as i64;
        assert_eq!(tight.y, 10 - margin);
        assert_eq!(tight.h, 60 + 2 * margin as u32);
    }

    #[test]
    fn an_empty_region_yields_no_lines() {
        let seg = seg_with(50, 50, &[]);
        assert!(split(&seg, Rect::new(0, 0, 50, 50), Orientation::Vertical).is_empty());
    }
}
