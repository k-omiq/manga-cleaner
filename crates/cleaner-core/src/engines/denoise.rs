//! Rung 1 - denoise.
//!
//! > Mask dilated 5 px, denoised crop composited through the whole dilated mask
//! > (interior included; it is not a ring), hard radius-1 feather. Native
//! > resolution. Skipped for bitonal sources.
//!
//! The rung exists for the case rung 0 makes *worse*: a tight mask on a grainy
//! or JPEG-blocked scan. Rung 0 pastes a perfectly smooth plane, and a
//! perfectly smooth rectangle inside grain is more visible than the text was.
//! So rung 1 runs rung 0 first and then smooths the fill **together with the
//! band of real paper around it**, which is why the composite covers the whole
//! dilated mask rather than a ring: the seam is the thing being removed.
//!
//! Three decisions the documents leave open, each in its constant below: what
//! "denoised" means, how hard, and how far.

use crate::constants::{DENOISE_DILATION, DENOISE_FEATHER};
use crate::engines::fill;
use crate::fit::Fitted;
use crate::image::{BitDepth, ColorMode, Raster};
use crate::mask::Mask;

/// The spatial radius of the filter kernel, in native pixels - a 5×5 window.
///
/// Grain and JPEG ringing are per-pixel; averaging up to 25 samples cuts a
/// standard deviation by up to 5×, which is more than the trigger asks for. A
/// wider window costs quadratically and starts averaging structure rather than
/// noise, and this filter runs inside a 5 px band where structure is exactly
/// what must survive.
const SPATIAL_RADIUS: i64 = 2;

/// The spatial σ, in pixels. Half the radius, so the kernel has decayed to
/// `e^-2` at its own edge rather than being truncated while still heavy.
const SPATIAL_SIGMA: f64 = SPATIAL_RADIUS as f64 / 2.0;

/// The blend weight of the feather ring: the midpoint of a linear ramp that a
/// radius of 1 gives exactly one step of.
const FEATHER_ALPHA: f64 = 0.5;

/// Whether rung 1 can run on this page at all.
///
/// Skipped for bitonal sources, and the reason generalises: a
/// denoiser averages *levels*, and a source whose samples are not levels has
/// nothing to average. One bit has two of them, and an averaged result is
/// neither. An indexed sample is not a level at all - it is a palette index,
/// and the mean of two indices names an unrelated colour.
///
/// Only rungs 0 and 1 run for indexed sources; that is right about the model
/// rungs and wrong about this one. Mapping a denoised value back to the
/// nearest palette entry is not a way out: it is the nearest-entry snapping
/// that is already refused, applied per pixel.
pub fn applies(page: &Raster) -> bool {
    page.depth != BitDepth::One && page.mode != ColorMode::Indexed
}

/// The mask rung 1 writes through: the fitted mask dilated by the denoise
/// dilation **plus** the feather, in one dilation.
///
/// One dilation by the sum, and not the dilation composed with the feather,
/// which is what the prose describes and what a reader writes first. Composing
/// them escapes the margin the constant is derived to cover: `Mask::dilated`
/// takes a disc at radius 5 and a square at radius 1, and disc(5) ⊕ square(1)
/// reaches √37 ≈ 6.08 px at the corners while `dilate(mask, EDIT_MARGIN)` is a
/// disc of exactly 6. A rung-1 region would then violate the write-bound
/// margin by a fraction of a pixel at four points - the same class of error an
/// earlier revision made by a whole one. With the single dilation this is an equality, not slack:
/// [`write_bound`] returns the same set.
pub fn applied_mask(fitted: &Fitted, page_w: u32, page_h: u32) -> Mask {
    fitted.mask.dilated(DENOISE_DILATION + DENOISE_FEATHER, page_w, page_h)
}

/// How far from the fitted mask rung 1 may write, which is what `EDIT_MARGIN`
/// is derived to cover.
pub fn write_bound(fitted: &Fitted, page_w: u32, page_h: u32) -> Mask {
    fitted.mask.dilated(crate::constants::EDIT_MARGIN, page_w, page_h)
}

/// Rung 0's fill, then rung 1's denoise, as **one** patch.
///
/// One rather than two layers because the region's edit is one thing: a user
/// deleting it wants the text back, not a half-smoothed fill, and re-running it
/// re-runs both. The patch's engine is therefore `Denoise`, and the mask it
/// carries is the mask it actually wrote through - [`applied_mask`].
///
/// `noise_sigma` is the page's own floor, in 16-bit luma. The filter's range σ
/// is that floor, for the same reason the trigger is relative to it:
/// "noise" is not an absolute quantity, and a fixed σ either leaves a JPEG raw
/// untouched or erases a clean scan's screentone.
pub fn render(page: &Raster, fitted: &Fitted, noise_sigma: f32) -> (Mask, Raster) {
    let core = fitted.mask.dilated(DENOISE_DILATION, page.width, page.height);
    let applied = applied_mask(fitted, page.width, page.height);
    let bounds = applied.bounds;
    let samples = page.mode.samples();
    let alpha_channel = page.mode.alpha_channel();

    // Rung 0's output, read through wherever the fitted mask covers. The filter
    // sees the filled page rather than the raw one, so what it smooths is the
    // fill against its surroundings and not the text that is being removed.
    let filled = fill::render(page, fitted);
    let source = |px: i64, py: i64, channel: usize| -> u16 {
        let cx = px.clamp(0, page.width as i64 - 1);
        let cy = py.clamp(0, page.height as i64 - 1);
        if fitted.mask.contains(cx, cy) {
            let bounds = fitted.mask.bounds;
            filled.sample((cx - bounds.x) as u32, (cy - bounds.y) as u32, channel)
        } else {
            page.sample(cx as u32, cy as u32, channel)
        }
    };

    let scale = scale_for(page.depth);
    let ceiling = ceiling_for(page.depth);
    // Floored at half a level so a synthetic page with no measurable noise
    // yields a degenerate kernel rather than a division by zero. On such a page
    // the filter is very nearly the identity, which is the honest answer.
    let sigma_range = (noise_sigma as f64).max(scale * 0.5);
    let spatial = spatial_weights();

    let mut patch = Raster {
        width: bounds.w,
        height: bounds.h,
        mode: page.mode,
        depth: page.depth,
        icc: None,
        palette: page.palette.clone(),
        trns: page.trns.clone(),
        srgb_intent: None,
        data: vec![0; {
            let bits = bounds.w as usize * samples * page.depth.bits() as usize;
            bits.div_ceil(8) * bounds.h as usize
        }],
    };

    for y in 0..bounds.h {
        for x in 0..bounds.w {
            let (px, py) = (bounds.x + x as i64, bounds.y + y as i64);
            let alpha = if core.contains(px, py) {
                1.0
            } else if applied.contains(px, py) {
                FEATHER_ALPHA
            } else {
                0.0
            };

            for channel in 0..samples {
                let original = source(px, py, channel);
                // Alpha is copied, never filtered: it is kept
                // out of every statistic, and smoothing
                // it would make the edge of a transparent region translucent.
                let value = if alpha == 0.0 || alpha_channel == Some(channel) {
                    original
                } else {
                    let filtered = bilateral(&source, px, py, channel, scale, sigma_range, &spatial);
                    let blended = alpha * filtered + (1.0 - alpha) * (original as f64 * scale);
                    (blended / scale).round().clamp(0.0, ceiling) as u16
                };
                patch.set_sample(x, y, channel, value);
            }
        }
    }

    (applied, patch)
}

/// One output sample, in 16-bit-scaled units.
///
/// A bilateral filter rather than the non-local means upstream tools reach for.
/// Both are edge-preserving; this one is a fixed 25-sample kernel where NLM is a
/// search, it has no parameter this project cannot state a reason for, and its
/// range σ is the page's measured noise floor rather than a tuned constant.
/// Which of the two reconstructs manga screentone better is a question for a
/// real corpus, not for the synthetic fixtures.
fn bilateral(
    source: &impl Fn(i64, i64, usize) -> u16,
    x: i64,
    y: i64,
    channel: usize,
    scale: f64,
    sigma_range: f64,
    spatial: &[f64],
) -> f64 {
    let centre = source(x, y, channel) as f64 * scale;
    let denominator = 2.0 * sigma_range * sigma_range;
    let mut weighted = 0.0;
    let mut total = 0.0;
    let mut index = 0;
    for dy in -SPATIAL_RADIUS..=SPATIAL_RADIUS {
        for dx in -SPATIAL_RADIUS..=SPATIAL_RADIUS {
            let value = source(x + dx, y + dy, channel) as f64 * scale;
            let difference = value - centre;
            let weight = spatial[index] * (-(difference * difference) / denominator).exp();
            weighted += weight * value;
            total += weight;
            index += 1;
        }
    }
    // The centre's own weight is 1 × 1, so this is never zero.
    weighted / total
}

fn spatial_weights() -> Vec<f64> {
    let denominator = 2.0 * SPATIAL_SIGMA * SPATIAL_SIGMA;
    let mut weights = Vec::with_capacity(((SPATIAL_RADIUS * 2 + 1) * (SPATIAL_RADIUS * 2 + 1)) as usize);
    for dy in -SPATIAL_RADIUS..=SPATIAL_RADIUS {
        for dx in -SPATIAL_RADIUS..=SPATIAL_RADIUS {
            weights.push((-((dx * dx + dy * dy) as f64) / denominator).exp());
        }
    }
    weights
}

/// What one stored sample is worth in 16-bit units. Filtering in a common scale
/// keeps the range σ - which is measured in 16-bit luma - comparable across
/// depths, and dividing back at the end is exact for an unchanged pixel.
fn scale_for(depth: BitDepth) -> f64 {
    match depth {
        BitDepth::Sixteen => 1.0,
        BitDepth::Eight => 257.0,
        other => 65535.0 / (((1u32 << other.bits()) - 1) as f64),
    }
}

fn ceiling_for(depth: BitDepth) -> f64 {
    match depth {
        BitDepth::Sixteen => u16::MAX as f64,
        other => ((1u32 << other.bits()) - 1) as f64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composite::{changed_pixels, composite, permitted_region};
    use crate::fit::{self, Route};
    use crate::image::fixtures;
    use crate::mask::Rect;
    use crate::patch::{Engine, Patch, Provenance};

    fn gray_page(w: u32, h: u32, f: impl Fn(u32, u32) -> u8) -> Raster {
        let mut data = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                data.push(f(x, y));
            }
        }
        Raster {
            width: w,
            height: h,
            mode: ColorMode::Gray,
            depth: BitDepth::Eight,
            icc: None,
            palette: None,
            trns: None,
            srgb_intent: None,
            data,
        }
    }

    /// Deterministic pseudo-grain: a scan's noise, without a dependency on a
    /// random number generator whose sequence a test cannot pin.
    ///
    /// Two properties the fixture needs and a naive one lacks. The mixing has to
    /// be good, or the lattice a cheap hash of `x` and `y` leaves is found by
    /// the ring's periodicity check and the region routes to the ladder. And the
    /// distribution has to be roughly normal: a *uniform* draw over a handful of
    /// levels is bimodal by Otsu's test - its two halves are 5 levels apart and
    /// only 1.5 wide - so the ring is refused as multimodal. Real grain is not
    /// uniform, so three draws are summed.
    fn grain(x: u32, y: u32) -> i32 {
        let draw = |salt: u64| -> i32 {
            let mut h = (((y as u64) << 32) | x as u64) ^ salt.wrapping_mul(0x2545_F491_4F6C_DD1D);
            h = h.wrapping_mul(0x9E37_79B9_7F4A_7C15);
            h ^= h >> 29;
            h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
            h ^= h >> 32;
            (h % 5) as i32
        };
        draw(1) + draw(2) + draw(3) - 6
    }

    fn fitted_over(page: &Raster, rect: Rect) -> Fitted {
        let seed = Mask::filled(rect);
        fit::fit(page, &seed, 1.0, 0.0, &fit::EdgeMap::none(page.width, page.height), true)
    }

    fn patch_of(mask: Mask, pixels: Raster) -> Patch {
        Patch {
            id: "d1".into(),
            ink: mask.clone(),
            mask,
            pixels,
            order: 0,
            visible: true,
            provenance: Provenance {
                engine: Engine::Denoise,
                engine_version: "test".into(),
                model_sha256: None,
                execution_provider: "cpu".into(),
                params_snapshot: "{}".into(),
                mask_sha256: "0".repeat(64),
                source_sha256: "0".repeat(64),
                cloud: None,
                created: 0,
            },
        }
    }

    #[test]
    fn a_bitonal_or_indexed_source_is_held_back_from_rung_one() {
        assert!(!applies(&fixtures::by_name("bitonal").raster));
        assert!(!applies(&fixtures::by_name("indexed-p").raster));
        assert!(applies(&fixtures::by_name("l8").raster));
        assert!(applies(&fixtures::by_name("rgb16").raster));
    }

    /// The routing consequence of the rule above: a source rung 1 cannot run on
    /// is routed to rung 0 alone, not to a rung that will be skipped later.
    #[test]
    fn a_source_rung_one_cannot_run_on_never_routes_there() {
        let noisy = gray_page(96, 96, |x, y| (200 + grain(x, y)) as u8);
        let sigma = fit::page_noise_sigma(&noisy);
        let edges = fit::EdgeMap::none(96, 96);
        let seed = Mask::filled(Rect::new(36, 36, 24, 24));
        assert_eq!(
            fit::fit(&noisy, &seed, 1.0, sigma, &edges, true).route,
            Route::FillAndDenoise,
            "the grain fixture is supposed to trigger rung 1"
        );

        let mut bitonal = fixtures::by_name("bitonal").raster;
        bitonal.width = bitonal.width.min(64);
        let seed = Mask::filled(Rect::new(8, 8, 8, 8));
        let route = fit::fit(
            &bitonal,
            &seed,
            1.0,
            sigma,
            &fit::EdgeMap::none(bitonal.width, bitonal.height),
            true,
        )
        .route;
        assert_ne!(route, Route::FillAndDenoise);
    }

    /// The constant `EDIT_MARGIN` is derived as `denoise_dilation + feather`
    /// precisely so this holds. Revision 1 wrote down the
    /// isolation radius alone, after which every rung-1 region violated the
    /// contract by one pixel.
    #[test]
    fn nothing_is_written_further_than_the_edit_margin_from_the_fitted_mask() {
        let page = gray_page(96, 96, |x, y| (200 + grain(x, y)) as u8);
        let sigma = fit::page_noise_sigma(&page);
        let fitted = fitted_over(&page, Rect::new(36, 36, 24, 24));
        let (mask, pixels) = render(&page, &fitted, sigma);

        let bound = write_bound(&fitted, page.width, page.height);
        let out = composite(&page, &[patch_of(mask, pixels)]).unwrap();
        let changed = changed_pixels(&page, &out);
        assert!(!changed.is_empty(), "rung 1 wrote nothing at all");
        for (x, y) in &changed {
            assert!(
                bound.contains(*x as i64, *y as i64),
                "({x}, {y}) is further than edit_margin from the fitted mask"
            );
        }
    }

    /// A σ=1 Gaussian spreads non-zero alpha 2–3 px; the feather is one ring and
    /// then nothing, which is not a Gaussian.
    #[test]
    fn the_feather_is_one_ring_and_the_contract_still_holds() {
        let page = gray_page(96, 96, |x, y| (200 + grain(x, y)) as u8);
        let sigma = fit::page_noise_sigma(&page);
        let fitted = fitted_over(&page, Rect::new(36, 36, 24, 24));
        let (mask, pixels) = render(&page, &fitted, sigma);

        let bound = write_bound(&fitted, page.width, page.height);
        assert_eq!(mask, bound, "the applied mask is not exactly the margin");

        let patch = patch_of(mask, pixels);
        let out = composite(&page, std::slice::from_ref(&patch)).unwrap();
        let permitted = permitted_region(std::slice::from_ref(&patch), page.width, page.height);
        for (x, y) in changed_pixels(&page, &out) {
            assert!(permitted.contains(x as i64, y as i64), "({x}, {y}) escaped the contract");
        }
    }

    #[test]
    fn the_band_around_the_fill_comes_out_smoother_than_it_went_in() {
        let page = gray_page(96, 96, |x, y| (200 + grain(x, y)) as u8);
        let sigma = fit::page_noise_sigma(&page);
        let fitted = fitted_over(&page, Rect::new(36, 36, 24, 24));
        let (mask, pixels) = render(&page, &fitted, sigma);

        // The band only: inside the fitted mask rung 0 already made it flat, so
        // measuring there would prove nothing about the denoiser.
        let deviation = |read: &dyn Fn(i64, i64) -> f64| {
            let mut values = Vec::new();
            for y in mask.bounds.y..mask.bounds.bottom() {
                for x in mask.bounds.x..mask.bounds.right() {
                    if mask.contains(x, y) && !fitted.mask.contains(x, y) {
                        values.push(read(x, y));
                    }
                }
            }
            let mean = values.iter().sum::<f64>() / values.len() as f64;
            (values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64).sqrt()
        };

        let before = deviation(&|x, y| page.sample(x as u32, y as u32, 0) as f64);
        let after = deviation(&|x, y| {
            pixels.sample((x - mask.bounds.x) as u32, (y - mask.bounds.y) as u32, 0) as f64
        });
        assert!(after < before * 0.8, "grain survived: {before:.2} → {after:.2}");
    }

    /// The 5 px band can reach a balloon stroke even when the fitted mask could
    /// not - the edge gate constrains the candidate search, not the dilation.
    /// A plain blur would drag the stroke's black into the paper beside it.
    #[test]
    fn an_edge_inside_the_band_keeps_its_contrast() {
        let page = gray_page(96, 96, |x, y| {
            if (58..62).contains(&x) { 10 } else { (235 + grain(x, y)) as u8 }
        });
        let sigma = fit::page_noise_sigma(&page);
        let fitted = fitted_over(&page, Rect::new(40, 36, 16, 24));
        let (mask, pixels) = render(&page, &fitted, sigma);

        let read = |x: i64, y: i64| -> i32 {
            if mask.contains(x, y) {
                pixels.sample((x - mask.bounds.x) as u32, (y - mask.bounds.y) as u32, 0) as i32
            } else {
                page.sample(x as u32, y as u32, 0) as i32
            }
        };
        let contrast = read(56, 48) - read(59, 48);
        assert!(contrast > 180, "the stroke was smeared into the paper: {contrast}");
    }

    #[test]
    fn an_alpha_channel_is_copied_rather_than_denoised() {
        let page = fixtures::by_name("rgba8").raster;
        let fitted = fitted_over(&page, Rect::new(20, 16, 12, 10));
        let (mask, pixels) = render(&page, &fitted, fit::page_noise_sigma(&page));
        for y in mask.bounds.y..mask.bounds.bottom() {
            for x in mask.bounds.x..mask.bounds.right() {
                assert_eq!(
                    pixels.sample((x - mask.bounds.x) as u32, (y - mask.bounds.y) as u32, 3),
                    page.sample(x as u32, y as u32, 3),
                    "alpha changed at {x},{y}"
                );
            }
        }
    }

    #[test]
    fn a_page_with_no_noise_at_all_is_left_where_it_is() {
        // σ floors rather than dividing by zero, and the honest result on a page
        // with nothing to denoise is the page.
        let page = gray_page(96, 96, |_, _| 246);
        let fitted = fitted_over(&page, Rect::new(36, 36, 24, 24));
        let (mask, pixels) = render(&page, &fitted, 0.0);
        for y in mask.bounds.y..mask.bounds.bottom() {
            for x in mask.bounds.x..mask.bounds.right() {
                let value = pixels.sample((x - mask.bounds.x) as u32, (y - mask.bounds.y) as u32, 0);
                assert_eq!(value, 246, "({x}, {y}) moved off flat paper");
            }
        }
    }

    #[test]
    fn a_sixteen_bit_page_is_denoised_at_sixteen_bits() {
        let page = fixtures::by_name("l16").raster;
        let fitted = fitted_over(&page, Rect::new(20, 16, 12, 10));
        let (mask, pixels) = render(&page, &fitted, fit::page_noise_sigma(&page));
        assert_eq!(pixels.depth, BitDepth::Sixteen);
        assert_eq!(pixels.mode, page.mode);
        // A patch that had been filtered at 8 bits and promoted back would have
        // every sample a multiple of 257.
        let mut off_the_eight_bit_grid = 0;
        for y in 0..mask.bounds.h {
            for x in 0..mask.bounds.w {
                if pixels.sample(x, y, 0) % 257 != 0 {
                    off_the_eight_bit_grid += 1;
                }
            }
        }
        assert!(off_the_eight_bit_grid > 0, "the patch quantised to 8 bits");
    }
}
