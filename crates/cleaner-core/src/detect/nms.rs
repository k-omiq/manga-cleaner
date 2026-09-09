//! The YOLO head, decoded.
//!
//! `blk` is `[1, 64512, 7]` - the anchor grid at strides 8, 16 and 32 over a
//! 1024² input, three anchors a cell - and the seven columns are `cx cy w h`,
//! objectness, and two class scores.
//!
//! Upstream's `non_max_suppression` gates on objectness alone
//! (`xc = prediction[..., 4] > conf_thres`) and then multiplies the class
//! scores by it, which is what is reproduced here: a different gate changes
//! which boxes survive, and the thresholds were tuned against this one.

use crate::mask::Rect;

use super::{CONFIDENCE_THRESHOLD, DetBox, DetectedLanguage, NMS_THRESHOLD};

#[derive(Debug, Clone, Copy)]
struct Candidate {
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    score: f32,
    class: usize,
}

pub fn decode(
    head: &[f32],
    rows: usize,
    fit: &super::Letterbox,
    page_w: u32,
    page_h: u32,
) -> Vec<DetBox> {
    const COLUMNS: usize = 7;
    let mut candidates = Vec::new();

    for row in 0..rows {
        let at = row * COLUMNS;
        let objectness = head[at + 4];
        if objectness <= CONFIDENCE_THRESHOLD {
            continue;
        }
        let (cx, cy, w, h) = (head[at], head[at + 1], head[at + 2], head[at + 3]);
        // The best of the two class scores, scaled by objectness - upstream's
        // `x[:, 5:] *= x[:, 4:5]` followed by `.max(1)`.
        let (class, class_score) = if head[at + 5] >= head[at + 6] {
            (0, head[at + 5])
        } else {
            (1, head[at + 6])
        };
        let score = class_score * objectness;
        if score <= CONFIDENCE_THRESHOLD {
            continue;
        }
        candidates.push(Candidate {
            x1: cx - w / 2.0,
            y1: cy - h / 2.0,
            x2: cx + w / 2.0,
            y2: cy + h / 2.0,
            score,
            class,
        });
    }

    // Sorted by descending score, and ties broken by geometry rather than left
    // to the sort's stability over an input whose order is the anchor grid's.
    // Everything upstream of the models must be deterministic, and a
    // tie here would otherwise decide which of two overlapping boxes survives.
    candidates.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then(a.y1.total_cmp(&b.y1))
            .then(a.x1.total_cmp(&b.x1))
    });

    let mut kept: Vec<Candidate> = Vec::new();
    for candidate in candidates {
        // Class-agnostic, like upstream's default (`agnostic=False` adds a
        // class offset only when `multi_label` is on, which it is not here).
        if kept.iter().any(|k| iou(k, &candidate) > NMS_THRESHOLD) {
            continue;
        }
        kept.push(candidate);
        if kept.len() >= 300 {
            break; // upstream's `max_det`
        }
    }

    kept.into_iter()
        .map(|c| {
            let (x1, y1) = fit.to_page(c.x1, c.y1);
            let (x2, y2) = fit.to_page(c.x2, c.y2);
            let x1 = x1.round().clamp(0.0, page_w as f32) as i64;
            let y1 = y1.round().clamp(0.0, page_h as f32) as i64;
            let x2 = x2.round().clamp(0.0, page_w as f32) as i64;
            let y2 = y2.round().clamp(0.0, page_h as f32) as i64;
            DetBox {
                rect: Rect::new(x1, y1, (x2 - x1).max(0) as u32, (y2 - y1).max(0) as u32),
                confidence: c.score,
                language: if c.class == 0 {
                    DetectedLanguage::English
                } else {
                    DetectedLanguage::Japanese
                },
            }
        })
        .filter(|b| b.rect.w > 0 && b.rect.h > 0)
        .collect()
}

fn iou(a: &Candidate, b: &Candidate) -> f32 {
    let x1 = a.x1.max(b.x1);
    let y1 = a.y1.max(b.y1);
    let x2 = a.x2.min(b.x2);
    let y2 = a.y2.min(b.y2);
    let overlap = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    let area_a = (a.x2 - a.x1) * (a.y2 - a.y1);
    let area_b = (b.x2 - b.x1) * (b.y2 - b.y1);
    let union = area_a + area_b - overlap;
    if union <= 0.0 { 0.0 } else { overlap / union }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row of the head: centre, size, objectness, two class scores.
    fn row(cx: f32, cy: f32, w: f32, h: f32, obj: f32, eng: f32, ja: f32) -> [f32; 7] {
        [cx, cy, w, h, obj, eng, ja]
    }

    fn head(rows: &[[f32; 7]]) -> Vec<f32> {
        rows.iter().flatten().copied().collect()
    }

    fn identity_fit() -> super::super::Letterbox {
        super::super::Letterbox { scale: 1.0, fitted_w: 100, fitted_h: 100, page_w: 100, page_h: 100 }
    }

    #[test]
    fn a_low_objectness_row_never_reaches_the_output() {
        let data = head(&[row(50.0, 50.0, 20.0, 20.0, 0.2, 0.9, 0.1)]);
        assert!(decode(&data, 1, &identity_fit(), 100, 100).is_empty());
    }

    #[test]
    fn overlapping_boxes_collapse_to_the_strongest() {
        let data = head(&[
            row(50.0, 50.0, 20.0, 20.0, 0.9, 0.1, 0.95),
            row(51.0, 51.0, 20.0, 20.0, 0.9, 0.1, 0.80),
        ]);
        let boxes = decode(&data, 2, &identity_fit(), 100, 100);
        assert_eq!(boxes.len(), 1);
        assert_eq!(boxes[0].language, DetectedLanguage::Japanese);
    }

    #[test]
    fn distant_boxes_both_survive() {
        let data = head(&[
            row(20.0, 20.0, 10.0, 10.0, 0.9, 0.1, 0.95),
            row(80.0, 80.0, 10.0, 10.0, 0.9, 0.95, 0.1),
        ]);
        let boxes = decode(&data, 2, &identity_fit(), 100, 100);
        assert_eq!(boxes.len(), 2);
        assert_eq!(boxes[0].language, DetectedLanguage::Japanese);
        assert_eq!(boxes[1].language, DetectedLanguage::English);
    }

    #[test]
    fn equal_scores_resolve_by_position_rather_than_by_input_order() {
        // The anchor grid's order is not a meaningful tiebreak, and the
        // answer must be the same on every run.
        let a = row(20.0, 20.0, 10.0, 10.0, 0.9, 0.1, 0.9);
        let b = row(20.5, 20.5, 10.0, 10.0, 0.9, 0.1, 0.9);
        let forwards = decode(&head(&[a, b]), 2, &identity_fit(), 100, 100);
        let backwards = decode(&head(&[b, a]), 2, &identity_fit(), 100, 100);
        assert_eq!(forwards, backwards);
    }

    #[test]
    fn boxes_come_back_in_page_coordinates() {
        let fit = super::super::Letterbox {
            scale: 0.5,
            fitted_w: 50,
            fitted_h: 50,
            page_w: 100,
            page_h: 100,
        };
        let data = head(&[row(25.0, 25.0, 10.0, 10.0, 0.9, 0.1, 0.9)]);
        let boxes = decode(&data, 1, &fit, 100, 100);
        assert_eq!(boxes[0].rect, Rect::new(40, 40, 20, 20));
    }
}
