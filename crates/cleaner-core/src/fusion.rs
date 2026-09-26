//! Source-coordinate review evidence. Detector regions and COO polygons
//! associate with SAM pixels; neither can create an erase pixel. Every SAM
//! component survives, including components with no detector support.
use crate::balloon::{BalloonBox, BalloonClass};
use crate::mask::Rect;
use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Component {
    pub id: String,
    pub bounds: Rect,
    pub pixels: u32,
    pub rt_text_ids: Vec<String>,
    pub rt_bubble_ids: Vec<String>,
    pub coo_ids: Vec<String>,
    /// Review is required before an editor turns any part into write support.
    pub review_required: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Region {
    pub id: String,
    pub kind: &'static str,
    pub bounds: Rect,
    pub score: f32,
    pub component_ids: Vec<String>,
    pub detector_only: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Link {
    pub component_id: String,
    pub region_id: String,
    pub shared_pixels: u32,
    pub fraction_of_component: f32,
    pub fraction_of_region: f32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewEvidence {
    pub width: u32,
    pub height: u32,
    pub components: Vec<Component>,
    pub regions: Vec<Region>,
    pub links: Vec<Link>,
    pub grouping_suggestions: Vec<Vec<String>>,
    /// The exact input mask is deliberately outside this structure. A caller
    /// retains it byte-for-byte for preview and explicit approval.
    pub mask_pixels: u64,
}

#[derive(Debug, Clone)]
pub struct CooProposal {
    pub polygon: Vec<(i64, i64)>,
    pub score: f32,
}

fn polygon_contains(polygon: &[(i64, i64)], x: i64, y: i64) -> bool {
    if polygon.len() < 3 {
        return false;
    }
    let (x, y) = (x as f64 + 0.5, y as f64 + 0.5);
    let mut inside = false;
    for i in 0..polygon.len() {
        let (ax, ay) = polygon[i];
        let (bx, by) = polygon[(i + 1) % polygon.len()];
        if (ay as f64 > y) != (by as f64 > y) {
            let cross = ax as f64 + (y - ay as f64) * (bx - ax) as f64 / (by - ay) as f64;
            if x < cross {
                inside = !inside;
            }
        }
    }
    inside
}

fn polygon_bounds(polygon: &[(i64, i64)]) -> Rect {
    let left = polygon.iter().map(|p| p.0).min().unwrap_or(0);
    let right = polygon.iter().map(|p| p.0).max().unwrap_or(left);
    let top = polygon.iter().map(|p| p.1).min().unwrap_or(0);
    let bottom = polygon.iter().map(|p| p.1).max().unwrap_or(top);
    Rect::new(
        left,
        top,
        (right - left + 1).max(0) as u32,
        (bottom - top + 1).max(0) as u32,
    )
}

/// Join any subset of the three capabilities. `mask = None` makes a regions
/// only result; `boxes = []` makes a mask-only result. COO is caller supplied
/// and has no download or model dependency here.
pub fn fuse(
    width: u32,
    height: u32,
    mask: Option<&[u8]>,
    boxes: &[BalloonBox],
    coo: &[CooProposal],
) -> Result<ReviewEvidence, String> {
    let length = width as usize * height as usize;
    if width == 0 || height == 0 || length > 100_000_000 {
        return Err("fusion page dimensions are invalid or too large".into());
    }
    if let Some(mask) = mask {
        if mask.len() != length || mask.iter().any(|pixel| *pixel != 0 && *pixel != 255) {
            return Err("SAM mask dimensions or binary values are invalid".into());
        }
    }
    let mut labels = vec![0u32; length];
    let mut components = Vec::new();
    if let Some(mask) = mask {
        for start in 0..length {
            if mask[start] == 0 || labels[start] != 0 {
                continue;
            }
            let number = components.len() as u32 + 1;
            labels[start] = number;
            let mut queue = VecDeque::from([start]);
            let (mut left, mut top) = (width, height);
            let (mut right, mut bottom) = (0u32, 0u32);
            let mut pixels = 0u32;
            while let Some(at) = queue.pop_front() {
                let x = (at % width as usize) as u32;
                let y = (at / width as usize) as u32;
                pixels += 1;
                left = left.min(x);
                top = top.min(y);
                right = right.max(x + 1);
                bottom = bottom.max(y + 1);
                for ny in y.saturating_sub(1)..=(y + 1).min(height - 1) {
                    for nx in x.saturating_sub(1)..=(x + 1).min(width - 1) {
                        let next = ny as usize * width as usize + nx as usize;
                        if mask[next] != 0 && labels[next] == 0 {
                            labels[next] = number;
                            queue.push_back(next);
                        }
                    }
                }
            }
            components.push(Component {
                id: format!("sam-{number:05}"),
                bounds: Rect::new(left as i64, top as i64, right - left, bottom - top),
                pixels,
                rt_text_ids: Vec::new(),
                rt_bubble_ids: Vec::new(),
                coo_ids: Vec::new(),
                review_required: true,
            });
        }
    }

    let mut regions = Vec::new();
    let mut links = Vec::new();
    for (i, box_) in boxes.iter().enumerate() {
        let id = format!("rt-{i:04}");
        let kind = match box_.class {
            BalloonClass::Bubble => "bubble_context",
            BalloonClass::TextInBubble => "text_bubble",
            BalloonClass::TextFree => "text_free",
        };
        let coverage = |x: i64, y: i64| box_.rect.contains(x, y);
        let (component_ids, matches) =
            associate(width, height, box_.rect, &labels, &components, &coverage);
        for (component, shared, region_pixels) in matches {
            let target = &mut components[component];
            if kind == "bubble_context" {
                target.rt_bubble_ids.push(id.clone());
            } else {
                target.rt_text_ids.push(id.clone());
            }
            links.push(Link {
                component_id: target.id.clone(),
                region_id: id.clone(),
                shared_pixels: shared,
                fraction_of_component: shared as f32 / target.pixels as f32,
                fraction_of_region: shared as f32 / region_pixels as f32,
            });
        }
        regions.push(Region {
            id,
            kind,
            bounds: box_.rect,
            score: box_.score,
            detector_only: component_ids.is_empty() && kind != "bubble_context",
            component_ids,
        });
    }
    let mut grouping_suggestions = Vec::new();
    for (i, proposal) in coo.iter().enumerate() {
        if proposal.polygon.len() < 3 || !proposal.score.is_finite() {
            continue;
        }
        let id = format!("coo-{i:04}");
        let coverage = |x: i64, y: i64| polygon_contains(&proposal.polygon, x, y);
        let bounds = polygon_bounds(&proposal.polygon);
        let (component_ids, matches) =
            associate(width, height, bounds, &labels, &components, &coverage);
        for (component, shared, region_pixels) in matches {
            let target = &mut components[component];
            target.coo_ids.push(id.clone());
            links.push(Link {
                component_id: target.id.clone(),
                region_id: id.clone(),
                shared_pixels: shared,
                fraction_of_component: shared as f32 / target.pixels as f32,
                fraction_of_region: shared as f32 / region_pixels as f32,
            });
        }
        if component_ids.len() > 1 {
            grouping_suggestions.push(component_ids.clone());
        }
        regions.push(Region {
            id,
            kind: "sfx_group",
            bounds,
            score: proposal.score,
            detector_only: component_ids.is_empty(),
            component_ids,
        });
    }
    Ok(ReviewEvidence {
        width,
        height,
        mask_pixels: mask.map_or(0, |m| m.iter().filter(|x| **x != 0).count() as u64),
        components,
        regions,
        links,
        grouping_suggestions,
    })
}

fn associate(
    width: u32,
    height: u32,
    bounds: Rect,
    labels: &[u32],
    components: &[Component],
    contains: &impl Fn(i64, i64) -> bool,
) -> (Vec<String>, Vec<(usize, u32, u32)>) {
    if components.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let mut shared = BTreeMap::<usize, u32>::new();
    let mut region_pixels = 0u32;
    let left = bounds.x.max(0).min(width as i64) as u32;
    let top = bounds.y.max(0).min(height as i64) as u32;
    let right = bounds.right().max(0).min(width as i64) as u32;
    let bottom = bounds.bottom().max(0).min(height as i64) as u32;
    for y in top..bottom {
        for x in left..right {
            if contains(x as i64, y as i64) {
                region_pixels += 1;
                let label = labels[y as usize * width as usize + x as usize];
                if label != 0 {
                    *shared.entry(label as usize - 1).or_default() += 1;
                }
            }
        }
    }
    let ids = shared
        .keys()
        .map(|index| components[*index].id.clone())
        .collect();
    let matches = shared
        .into_iter()
        .map(|(index, pixels)| (index, pixels, region_pixels))
        .collect();
    (ids, matches)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_capabilities_keep_every_component_and_detector_only() {
        let mut mask = vec![0; 8 * 5];
        mask[1] = 255;
        mask[10] = 255; // 8-connected one component
        mask[6] = 255; // unsupported component
        let boxes = [
            BalloonBox {
                rect: Rect::new(0, 0, 3, 3),
                class: BalloonClass::TextFree,
                score: 0.9,
            },
            BalloonBox {
                rect: Rect::new(4, 3, 2, 2),
                class: BalloonClass::TextInBubble,
                score: 0.8,
            },
        ];
        let mask_only = fuse(8, 5, Some(&mask), &[], &[]).unwrap();
        assert_eq!(mask_only.components.len(), 2);
        assert!(mask_only.components.iter().all(|c| c.review_required));
        let regions_only = fuse(8, 5, None, &boxes, &[]).unwrap();
        assert_eq!(
            regions_only
                .regions
                .iter()
                .filter(|r| r.detector_only)
                .count(),
            2
        );
        let fused = fuse(8, 5, Some(&mask), &boxes, &[]).unwrap();
        assert_eq!(fused.components.len(), 2);
        assert_eq!(fused.regions.iter().filter(|r| r.detector_only).count(), 1);
        assert_eq!(fused.mask_pixels, 3);
    }

    #[test]
    fn coo_only_suggests_grouping_and_never_adds_mask_pixels() {
        let mut mask = vec![0; 10 * 10];
        mask[2 * 10 + 2] = 255;
        mask[2 * 10 + 6] = 255;
        let proposal = CooProposal {
            polygon: vec![(1, 1), (8, 1), (8, 4), (1, 4)],
            score: 0.6,
        };
        let original = mask.clone();
        let fused = fuse(10, 10, Some(&mask), &[], &[proposal]).unwrap();
        assert_eq!(fused.components.len(), 2);
        assert_eq!(
            fused.grouping_suggestions,
            vec![vec!["sam-00001".to_owned(), "sam-00002".to_owned()]]
        );
        assert_eq!(fused.mask_pixels, 2);
        assert_eq!(mask, original);
        assert!(!fused.regions[0].detector_only);
    }
}
