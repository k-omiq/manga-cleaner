//! **Vendored, not written here.** Copied verbatim from the `freeditor`
//! repository (`rust/image-kernels/src/kernels/clone_heal.rs`, commit `36f37dc`),
//! with two mechanical edits and no algorithmic ones: the
//! `#[cfg(target_arch = "wasm32")]` ABI exports are dropped - this crate links
//! the kernels natively and `wasm32-unknown-unknown` is not even installed,
//! and `crate::kernels::primitives` becomes `super::primitives`.
//! The `#[cfg(test)]` TS-oracle parity tests are kept as they stood, because
//! they are resolution-independent and they are what keeps the kernel verified.
//!

// The vendored code keeps freeditor's own shape, including the wide kernel
// signatures and its explicit clamp ladders. Linting it into a different shape
// would break the one property that makes it auditable: it is a copy.
#![allow(clippy::too_many_arguments)]

use super::primitives::{
    blend_channel, collect_brush_pixels, in_bounds, pixel_offset, read_raster_pixel,
    sample_average_3x3_rgba, selection_mask_weight, tool_strength,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HealMode {
    Spot,
    Soft,
    Object,
}

fn heal_source_mix(mode: HealMode) -> f64 {
    match mode {
        HealMode::Soft => 0.42,
        HealMode::Object => 0.88,
        HealMode::Spot => 0.6,
    }
}

fn blend_pixel(out: &mut [u8], offset: usize, target: [u8; 4], source: [u8; 4], amount: f64) {
    out[offset] = blend_channel(source[0], target[0], amount);
    out[offset + 1] = blend_channel(source[1], target[1], amount);
    out[offset + 2] = blend_channel(source[2], target[2], amount);
    out[offset + 3] = blend_channel(source[3], target[3], amount);
}

pub fn apply_clone_stamp(
    image: &[u8],
    width: usize,
    height: usize,
    source_image: &[u8],
    source_width: usize,
    source_height: usize,
    source_center: (f64, f64),
    target_center: (f64, f64),
    size: f64,
    opacity: f64,
    flow: f64,
    selection_mask: Option<&[u8]>,
    out: &mut [u8],
) {
    out.copy_from_slice(image);

    let amount_base = tool_strength(opacity, flow);
    if amount_base <= 0.0 || width == 0 || height == 0 {
        return;
    }

    let source_base_x = source_center.0.round() as i32;
    let source_base_y = source_center.1.round() as i32;
    let target_base_x = target_center.0.round() as i32;
    let target_base_y = target_center.1.round() as i32;

    for visit in collect_brush_pixels(width, height, target_center.0, target_center.1, size) {
        let x = visit.x as i32;
        let y = visit.y as i32;
        let selection_weight = selection_mask_weight(selection_mask, width, height, x, y);
        if selection_weight <= 0.0 {
            continue;
        }

        let source_x = source_base_x + (x - target_base_x);
        let source_y = source_base_y + (y - target_base_y);
        if !in_bounds(source_x, source_y, source_width, source_height) {
            continue;
        }

        let source_offset = pixel_offset(source_x as usize, source_y as usize, source_width);
        let offset = visit.offset as usize;
        let source = read_raster_pixel(source_image, source_offset);
        let target = read_raster_pixel(image, offset);
        blend_pixel(
            out,
            offset,
            target,
            source,
            amount_base * visit.weight * selection_weight,
        );
    }
}

pub fn apply_heal_stamp(
    image: &[u8],
    width: usize,
    height: usize,
    source_image: &[u8],
    source_width: usize,
    source_height: usize,
    source_center: (f64, f64),
    target_center: (f64, f64),
    mode: HealMode,
    size: f64,
    opacity: f64,
    flow: f64,
    selection_mask: Option<&[u8]>,
    out: &mut [u8],
) {
    out.copy_from_slice(image);

    let amount_base = tool_strength(opacity, flow);
    if amount_base <= 0.0 || width == 0 || height == 0 {
        return;
    }

    let source_base_x = source_center.0.round() as i32;
    let source_base_y = source_center.1.round() as i32;
    let target_base_x = target_center.0.round() as i32;
    let target_base_y = target_center.1.round() as i32;
    let source_mix = heal_source_mix(mode);
    let target_mix = 1.0 - source_mix;

    for visit in collect_brush_pixels(width, height, target_center.0, target_center.1, size) {
        let x = visit.x as i32;
        let y = visit.y as i32;
        let selection_weight = selection_mask_weight(selection_mask, width, height, x, y);
        if selection_weight <= 0.0 {
            continue;
        }

        let source_x = source_base_x + (x - target_base_x);
        let source_y = source_base_y + (y - target_base_y);
        if !in_bounds(source_x, source_y, source_width, source_height) {
            continue;
        }

        let source_offset = pixel_offset(source_x as usize, source_y as usize, source_width);
        let offset = visit.offset as usize;
        let source = read_raster_pixel(source_image, source_offset);
        let target = read_raster_pixel(image, offset);
        let blurred_source = sample_average_3x3_rgba(
            source_image,
            source_width,
            source_height,
            source_x,
            source_y,
        );
        let blurred_target = sample_average_3x3_rgba(image, width, height, x, y);
        let healed_base = if mode == HealMode::Soft {
            blurred_source
        } else {
            source
        };
        let healed_target = if mode == HealMode::Object {
            blurred_target
        } else {
            target
        };
        let healed = [
            (healed_base[0] as f64 * source_mix + healed_target[0] as f64 * target_mix).round()
                as u8,
            (healed_base[1] as f64 * source_mix + healed_target[1] as f64 * target_mix).round()
                as u8,
            (healed_base[2] as f64 * source_mix + healed_target[2] as f64 * target_mix).round()
                as u8,
            target[3],
        ];

        blend_pixel(
            out,
            offset,
            target,
            healed,
            amount_base * visit.weight * selection_weight,
        );
    }
}

fn center_at(centers: &[f64], index: usize) -> (f64, f64) {
    (centers[index * 2], centers[index * 2 + 1])
}

#[derive(Clone, Copy)]
enum ActiveStrokeBuffer {
    Original,
    Scratch,
    Out,
}

#[allow(clippy::too_many_arguments)]
pub fn apply_clone_stroke(
    image: &[u8],
    width: usize,
    height: usize,
    source_image: &[u8],
    source_width: usize,
    source_height: usize,
    target_centers: &[f64],
    source_centers: &[f64],
    size: f64,
    opacity: f64,
    flow: f64,
    selection_mask: Option<&[u8]>,
    scratch: &mut [u8],
    out: &mut [u8],
) {
    if target_centers.is_empty() {
        out.copy_from_slice(image);
        return;
    }

    let mut active = ActiveStrokeBuffer::Original;

    for index in 0..(target_centers.len() / 2) {
        match active {
            ActiveStrokeBuffer::Original | ActiveStrokeBuffer::Out => {
                let source = match active {
                    ActiveStrokeBuffer::Original => image,
                    ActiveStrokeBuffer::Out => &*out,
                    ActiveStrokeBuffer::Scratch => unreachable!(),
                };
                apply_clone_stamp(
                    source,
                    width,
                    height,
                    source_image,
                    source_width,
                    source_height,
                    center_at(source_centers, index),
                    center_at(target_centers, index),
                    size,
                    opacity,
                    flow,
                    selection_mask,
                    scratch,
                );
                active = ActiveStrokeBuffer::Scratch;
            }
            ActiveStrokeBuffer::Scratch => {
                apply_clone_stamp(
                    &*scratch,
                    width,
                    height,
                    source_image,
                    source_width,
                    source_height,
                    center_at(source_centers, index),
                    center_at(target_centers, index),
                    size,
                    opacity,
                    flow,
                    selection_mask,
                    out,
                );
                active = ActiveStrokeBuffer::Out;
            }
        }
    }

    if let ActiveStrokeBuffer::Scratch = active {
        out.copy_from_slice(scratch);
    }
}

#[allow(clippy::too_many_arguments)]
pub fn apply_heal_stroke(
    image: &[u8],
    width: usize,
    height: usize,
    source_image: &[u8],
    source_width: usize,
    source_height: usize,
    target_centers: &[f64],
    source_centers: &[f64],
    mode: HealMode,
    size: f64,
    opacity: f64,
    flow: f64,
    selection_mask: Option<&[u8]>,
    scratch: &mut [u8],
    out: &mut [u8],
) {
    if target_centers.is_empty() {
        out.copy_from_slice(image);
        return;
    }

    let mut active = ActiveStrokeBuffer::Original;

    for index in 0..(target_centers.len() / 2) {
        match active {
            ActiveStrokeBuffer::Original | ActiveStrokeBuffer::Out => {
                let source = match active {
                    ActiveStrokeBuffer::Original => image,
                    ActiveStrokeBuffer::Out => &*out,
                    ActiveStrokeBuffer::Scratch => unreachable!(),
                };
                apply_heal_stamp(
                    source,
                    width,
                    height,
                    source_image,
                    source_width,
                    source_height,
                    center_at(source_centers, index),
                    center_at(target_centers, index),
                    mode,
                    size,
                    opacity,
                    flow,
                    selection_mask,
                    scratch,
                );
                active = ActiveStrokeBuffer::Scratch;
            }
            ActiveStrokeBuffer::Scratch => {
                apply_heal_stamp(
                    &*scratch,
                    width,
                    height,
                    source_image,
                    source_width,
                    source_height,
                    center_at(source_centers, index),
                    center_at(target_centers, index),
                    mode,
                    size,
                    opacity,
                    flow,
                    selection_mask,
                    out,
                );
                active = ActiveStrokeBuffer::Out;
            }
        }
    }

    if let ActiveStrokeBuffer::Scratch = active {
        out.copy_from_slice(scratch);
    }
}

#[cfg(test)]
mod tests {
    use super::{apply_clone_stroke, apply_heal_stroke, HealMode};

    fn decode_hex(hex: &str) -> Vec<u8> {
        hex.as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let text = core::str::from_utf8(pair).expect("hex fixtures must be utf8");
                u8::from_str_radix(text, 16).expect("hex fixtures must decode")
            })
            .collect()
    }

    #[test]
    fn clone_heal_strokes_match_ts_large_fixture_bytes() {
        let source = decode_hex(
            "0b1d2fff305276e05587bdc07abc04a09ff14b80c4269260e95bd9ff0e9020e033c567c058faaea07d2ff580a2643c60c79983ffeccecae0110311c0363858a05b6d9f8080a2e660a5d72dffca0c74e0ef41bbc0147602a039ab49805ee090608315d7ffa84a1ee0cd7f65c0f2b4aca017e9f3803c1e3a60615381ff8688c8e0abbd0fc0d0f256a0f5279d801a5ce4603f912bff64c672e089fbb9c0ae3000a0d3654780f89a8e601dcfd5ff42041ce0673963c08c6eaaa0b1a3f180d6d83860",
        );
        let mask = decode_hex(
            "002e5c8ab891bfed1b4922507eacdab3e10f3d6b4472a0cefcd503315f8d6694c2f01ef7255381af88b6e41240194775",
        );
        let mut scratch = vec![0; source.len()];
        let mut out = vec![0; source.len()];
        let targets = [5.0, 4.0];
        let clone_sources = [1.0, 1.0];

        apply_clone_stroke(
            &source,
            8,
            6,
            &source,
            8,
            6,
            &targets,
            &clone_sources,
            5.0,
            75.0,
            80.0,
            Some(&mask),
            &mut scratch,
            &mut out,
        );
        assert_eq!(
            out,
            decode_hex(
                "0b1d2fff305276e05587bdc07abc04a09ff14b80c4269260e95bd9ff0e9020e033c567c058faaea07d2ff580a2643c60c79983ffeccecae0110311c0363858a05b6d9f8080a2e660a5d72dffca0c74e0ef41bbc0147602a039ab49805ee090608315d7ffa84a1ee0cd7f65c0f2b4aca016d5e08c3a284679605887f8868ac1deabbd0fc0d0f256a0f5279d801a5ce4603e942efc62d07ed487d6c4b4ad34059bd3654780f89a8e601dcfd5ff42041ce0663c67bc8c70ac9eb0a7e389d6d23a64",
            )
        );

        let mut scratch = vec![0; source.len()];
        let mut out = vec![0; source.len()];
        let heal_sources = [2.0, 1.0];
        apply_heal_stroke(
            &source,
            8,
            6,
            &source,
            8,
            6,
            &targets,
            &heal_sources,
            HealMode::Object,
            5.0,
            70.0,
            75.0,
            Some(&mask),
            &mut scratch,
            &mut out,
        );
        assert_eq!(
            out,
            decode_hex(
                "0b1d2fff305276e05587bdc07abc04a09ff14b80c4269260e95bd9ff0e9020e033c567c058faaea07d2ff580a2643c60c79983ffeccecae0110311c0363858a05b6d9f8080a2e660a5d72dffca0c74e0ef41bbc0147602a039ab49805ee090608315d7ffa84a1ee0cd7f65c0f0b3aba01adde88040304f60635c77ff878bc4e0abbd0fc0d0f256a0f5279d801d66d860409531ff68ae86e08de4a6c0af3809a0d3654780f89a8e601dcfd5ff42041ce0683f6ac08d71a7a0b29be980d6d43b60",
            )
        );
    }
}
