//! **Vendored, not written here.** Copied verbatim from the `freeditor`
//! repository (`rust/image-kernels/src/kernels/primitives.rs`, commit `36f37dc`),
//! with two mechanical edits and no algorithmic ones: the
//! `#[cfg(target_arch = "wasm32")]` ABI exports are dropped - this crate links
//! the kernels natively and `wasm32-unknown-unknown` is not even installed,
//! and the two `crate::types` constants are re-declared below.
//! The `#[cfg(test)]` TS-oracle parity tests are kept as they stood, because
//! they are resolution-independent and they are what keeps the kernel verified.
//!
/// From `freeditor`'s `crate::types`: bytes per pixel in the canonical RGBA layout.
pub const RGBA_CHANNELS: usize = 4;
/// From `freeditor`'s `crate::types`: channels emitted by the RGBA sample helpers.
pub const RGBA_SAMPLE_CHANNELS: usize = 4;


#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrushPixelVisit {
    pub x: u32,
    pub y: u32,
    pub offset: u32,
    pub weight: f64,
}

pub fn clamp01(value: f64) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    if value <= 0.0 {
        return 0.0;
    }
    if value >= 1.0 {
        return 1.0;
    }
    value
}

pub fn clamp_byte(value: f64) -> u8 {
    if !value.is_finite() {
        return 0;
    }
    if value <= 0.0 {
        return 0;
    }
    if value >= 255.0 {
        return 255;
    }
    value.round() as u8
}

pub fn clamp_size(size: f64) -> u32 {
    if !size.is_finite() {
        return 1;
    }
    size.round().max(1.0) as u32
}

pub fn in_bounds(x: i32, y: i32, width: usize, height: usize) -> bool {
    x >= 0 && y >= 0 && x < width as i32 && y < height as i32
}

pub fn pixel_offset(x: usize, y: usize, width: usize) -> usize {
    (y * width + x) * RGBA_CHANNELS
}

pub fn blend_channel(source: u8, target: u8, amount: f64) -> u8 {
    clamp_byte(target as f64 + (source as f64 - target as f64) * clamp01(amount))
}

pub fn tool_strength(opacity_percent: f64, flow_percent: f64) -> f64 {
    clamp01(clamp01(opacity_percent / 100.0) * clamp01(flow_percent / 100.0))
}

pub fn selection_mask_weight(
    selection_mask: Option<&[u8]>,
    width: usize,
    height: usize,
    x: i32,
    y: i32,
) -> f64 {
    let Some(mask) = selection_mask else {
        return 1.0;
    };
    if !in_bounds(x, y, width, height) {
        return 0.0;
    }
    clamp01(mask[y as usize * width + x as usize] as f64 / 255.0)
}

pub fn read_raster_pixel(source: &[u8], offset: usize) -> [u8; 4] {
    [
        source[offset],
        source[offset + 1],
        source[offset + 2],
        source[offset + 3],
    ]
}

pub fn sample_average_3x3_rgba(
    image: &[u8],
    width: usize,
    height: usize,
    x: i32,
    y: i32,
) -> [u8; 4] {
    let mut totals = [0u32; 4];
    let mut count = 0u32;

    for sample_y in (y - 1)..=(y + 1) {
        for sample_x in (x - 1)..=(x + 1) {
            if !in_bounds(sample_x, sample_y, width, height) {
                continue;
            }

            let offset = pixel_offset(sample_x as usize, sample_y as usize, width);
            totals[0] += image[offset] as u32;
            totals[1] += image[offset + 1] as u32;
            totals[2] += image[offset + 2] as u32;
            totals[3] += image[offset + 3] as u32;
            count += 1;
        }
    }

    if count == 0 {
        return [0, 0, 0, 0];
    }

    [
        (totals[0] as f64 / count as f64).round() as u8,
        (totals[1] as f64 / count as f64).round() as u8,
        (totals[2] as f64 / count as f64).round() as u8,
        (totals[3] as f64 / count as f64).round() as u8,
    ]
}

fn clamp(value: f64, min: f64, max: f64) -> f64 {
    if max < min {
        return min;
    }
    value.max(min).min(max)
}

fn read_pixel_clamped(data: &[u8], width: usize, height: usize, x: f64, y: f64) -> [u8; 4] {
    let safe_x = clamp(x.round(), 0.0, (width.saturating_sub(1)) as f64) as usize;
    let safe_y = clamp(y.round(), 0.0, (height.saturating_sub(1)) as f64) as usize;
    read_raster_pixel(data, pixel_offset(safe_x, safe_y, width))
}

fn lerp(start: f64, end: f64, amount: f64) -> f64 {
    start + (end - start) * amount
}

pub fn sample_rgba_bilinear(data: &[u8], width: usize, height: usize, x: f64, y: f64) -> [f64; 4] {
    if width == 0 || height == 0 {
        return [0.0, 0.0, 0.0, 0.0];
    }

    let clamped_x = clamp(x, 0.0, (width - 1) as f64);
    let clamped_y = clamp(y, 0.0, (height - 1) as f64);
    let x0 = clamped_x.floor() as usize;
    let y0 = clamped_y.floor() as usize;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let tx = clamped_x - x0 as f64;
    let ty = clamped_y - y0 as f64;

    let top_left = read_pixel_clamped(data, width, height, x0 as f64, y0 as f64);
    let top_right = read_pixel_clamped(data, width, height, x1 as f64, y0 as f64);
    let bottom_left = read_pixel_clamped(data, width, height, x0 as f64, y1 as f64);
    let bottom_right = read_pixel_clamped(data, width, height, x1 as f64, y1 as f64);

    let mut sample = [0.0; 4];
    for channel in 0..RGBA_SAMPLE_CHANNELS {
        let top = lerp(top_left[channel] as f64, top_right[channel] as f64, tx);
        let bottom = lerp(
            bottom_left[channel] as f64,
            bottom_right[channel] as f64,
            tx,
        );
        sample[channel] = lerp(top, bottom, ty);
    }

    sample
}

pub fn color_distance_squared(sample: [f64; 4], reference: [f64; 4]) -> f64 {
    let mut distance_squared = 0.0;
    for channel in 0..RGBA_SAMPLE_CHANNELS {
        let delta = sample[channel] - reference[channel];
        distance_squared += delta * delta;
    }
    distance_squared
}

pub fn score_tolerance_coverage(
    distance_squared: f64,
    tolerance_squared: f64,
    softness_squared: f64,
) -> f64 {
    if distance_squared <= tolerance_squared {
        return 1.0;
    }

    let safe_softness = softness_squared.max(1.0);
    let max_distance_squared = tolerance_squared + safe_softness;
    if distance_squared >= max_distance_squared {
        return 0.0;
    }

    1.0 - (distance_squared - tolerance_squared) / safe_softness
}

pub fn collect_brush_pixels(
    width: usize,
    height: usize,
    center_x: f64,
    center_y: f64,
    size: f64,
) -> Vec<BrushPixelVisit> {
    if width == 0 || height == 0 {
        return Vec::new();
    }

    let diameter = clamp_size(size) as f64;
    let radius = diameter / 2.0;
    let center_x = center_x.round();
    let center_y = center_y.round();
    let min_x = (center_x - radius).floor().max(0.0) as usize;
    let max_x = (center_x + radius).ceil().min((width - 1) as f64) as usize;
    let min_y = (center_y - radius).floor().max(0.0) as usize;
    let max_y = (center_y + radius).ceil().min((height - 1) as f64) as usize;
    let mut visits = Vec::new();

    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let distance = ((x as f64 - center_x).powi(2) + (y as f64 - center_y).powi(2)).sqrt();
            if distance > radius {
                continue;
            }

            let weight = if radius <= 0.5 {
                1.0
            } else {
                clamp01(1.0 - distance / radius)
            };
            if weight <= 0.0 {
                continue;
            }

            visits.push(BrushPixelVisit {
                x: x as u32,
                y: y as u32,
                offset: pixel_offset(x, y, width) as u32,
                weight,
            });
        }
    }

    visits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canonical_medium_rgba() -> Vec<u8> {
        vec![
            255, 0, 0, 255, 200, 30, 30, 255, 20, 40, 220, 255, 0, 0, 0, 255, 255, 255, 255, 255,
            30, 200, 80, 200, 160, 120, 40, 255, 220, 180, 20, 255, 40, 60, 80, 255, 10, 10, 10,
            255, 0, 120, 255, 255, 255, 80, 0, 180, 90, 30, 160, 255, 140, 200, 60, 255, 60, 20,
            200, 255, 255, 0, 255, 128, 80, 160, 240, 255, 15, 25, 35, 255, 220, 220, 80, 255, 5,
            200, 150, 255,
        ]
    }

    fn canonical_medium_mask() -> Vec<u8> {
        vec![
            255, 96, 0, 192, 48, 224, 255, 160, 64, 0, 0, 144, 255, 96, 192, 32, 0, 128, 255, 224,
        ]
    }

    #[test]
    fn clamps_match_ts_contract() {
        assert_eq!(clamp01(f64::NAN), 0.0);
        assert_eq!(clamp01(-0.5), 0.0);
        assert_eq!(clamp01(0.25), 0.25);
        assert_eq!(clamp01(2.0), 1.0);

        assert_eq!(clamp_byte(f64::NAN), 0);
        assert_eq!(clamp_byte(-1.0), 0);
        assert_eq!(clamp_byte(12.4), 12);
        assert_eq!(clamp_byte(12.5), 13);
        assert_eq!(clamp_byte(400.0), 255);

        assert_eq!(clamp_size(f64::NAN), 1);
        assert_eq!(clamp_size(0.0), 1);
        assert_eq!(clamp_size(1.2), 1);
        assert_eq!(clamp_size(2.6), 3);
    }

    #[test]
    fn low_level_pixel_math_matches_ts_contract() {
        assert!(in_bounds(2, 1, 5, 4));
        assert!(!in_bounds(-1, 1, 5, 4));
        assert_eq!(pixel_offset(2, 1, 5), 28);
        assert_eq!(blend_channel(255, 0, 0.5), 128);
        assert_eq!(blend_channel(0, 255, 0.25), 191);
    }

    #[test]
    fn selection_mask_weight_matches_ts_contract() {
        let mask = canonical_medium_mask();
        assert_eq!(selection_mask_weight(None, 5, 4, 2, 1), 1.0);
        assert_eq!(
            selection_mask_weight(Some(mask.as_slice()), 5, 4, 1, 0),
            96.0 / 255.0
        );
        assert_eq!(
            selection_mask_weight(Some(mask.as_slice()), 5, 4, -1, 0),
            0.0
        );
    }

    #[test]
    fn average_sampling_matches_ts_contract() {
        let image = canonical_medium_rgba();
        assert_eq!(
            sample_average_3x3_rgba(image.as_slice(), 5, 4, 2, 1),
            [125, 82, 68, 247]
        );
        assert_eq!(
            sample_average_3x3_rgba(image.as_slice(), 5, 4, 0, 0),
            [161, 88, 38, 241]
        );
    }

    #[test]
    fn bilinear_and_tolerance_helpers_match_ts_contract() {
        let image = vec![
            0, 0, 0, 255, 255, 0, 0, 255, 0, 255, 0, 255, 255, 255, 255, 255,
        ];
        assert_eq!(
            sample_rgba_bilinear(image.as_slice(), 2, 2, 0.5, 0.5),
            [127.5, 127.5, 63.75, 255.0]
        );
        assert_eq!(
            color_distance_squared([10.0, 20.0, 30.0, 40.0], [13.0, 24.0, 30.0, 35.0]),
            50.0
        );
        assert_eq!(score_tolerance_coverage(1000.0, 1000.0, 4096.0), 1.0);
        assert_eq!(score_tolerance_coverage(3000.0, 1000.0, 4000.0), 0.5);
        assert_eq!(score_tolerance_coverage(5000.0, 1000.0, 4000.0), 0.0);
    }

    #[test]
    fn brush_walk_matches_ts_contract() {
        let visits = collect_brush_pixels(5, 4, 2.0, 1.0, 4.0);
        let encoded: Vec<[u32; 4]> = visits
            .into_iter()
            .map(|visit| {
                [
                    visit.x,
                    visit.y,
                    visit.offset,
                    clamp_byte(visit.weight * 255.0) as u32,
                ]
            })
            .collect();

        assert_eq!(
            encoded,
            vec![
                [1, 0, 4, 75],
                [2, 0, 8, 128],
                [3, 0, 12, 75],
                [1, 1, 24, 128],
                [2, 1, 28, 255],
                [3, 1, 32, 128],
                [1, 2, 44, 75],
                [2, 2, 48, 128],
                [3, 2, 52, 75],
            ]
        );
    }
}
