//! Rung 0 - planar fill.
//!
//! > Rung 0 not because it is cheap but because it is *more faithful* than any
//! > model for the case it covers: it fills with the measured local paper,
//! > fitted as a plane. A model asked to reconstruct flat paper produces
//! > approximately-flat paper.
//!
//! The whole engine is "paste the plane through the chosen mask", and every
//! interesting decision was already made by [`crate::fit`]: which mask, what
//! tone, whether a plane is warranted at all. What remains here is doing it in
//! the page's own sample space, which is the difference between a fill that
//! preserves color fidelity and one that quietly promotes a grayscale page to
//! RGB.

use crate::fit::{Fitted, ring};
use crate::image::{BitDepth, ColorMode, Raster};

/// The pixels a fill produces, covering `mask.bounds`.
///
/// Everything outside the mask is copied from the page unchanged, so the patch
/// composites without a seam and the compositor's own mask test is a second
/// line of defence rather than the only one.
pub fn render(page: &Raster, fitted: &Fitted) -> Raster {
    let bounds = fitted.mask.bounds;
    let samples = page.mode.samples();

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

    // An indexed page cannot carry a gradient: its samples are palette indices
    // and interpolating between two of them is meaningless. Indexed sources
    // get rungs 0 and 1 only, and this is what rung 0 does there -
    // the single nearest tone, which the palette either already holds or gains
    // one entry for.
    // Both are measured over `fitted.paper` - the ring the route was decided
    // from - rather than over a fresh annulus. See [`crate::fit::Fitted::paper`]:
    // a ring that reaches across a bubble stroke tilts a least-squares plane as
    // readily as it inflates the deviation beside it, and a fill measured from a
    // different set than the routing is a fill nobody checked.
    let flat = page.mode == ColorMode::Indexed;
    let planes = if flat { Vec::new() } else { ring::fill_planes(page, &fitted.paper) };
    let medians = ring::fill_medians(page, &fitted.paper);
    let ceiling = match page.depth {
        crate::image::BitDepth::Sixteen => u16::MAX as f64,
        other => ((1u32 << other.bits()) - 1) as f64,
    };

    for y in 0..bounds.h {
        for x in 0..bounds.w {
            let (px, py) = (bounds.x + x as i64, bounds.y + y as i64);
            let inside = fitted.mask.contains(px, py);
            for channel in 0..samples {
                let value = if !inside {
                    page.sample(px as u32, py as u32, channel)
                } else if flat || planes.is_empty() {
                    medians[channel]
                } else {
                    // Alpha is copied through unmodified: §2's third
                    // clarification says it never enters a ring statistic, and
                    // filling it would make a transparent region opaque.
                    if page.mode.alpha_channel() == Some(channel) {
                        page.sample(px as u32, py as u32, channel)
                    } else {
                        planes[channel].at(px, py).round().clamp(0.0, ceiling) as u16
                    }
                };
                patch.set_sample(x, y, channel, value);
            }
        }
    }
    patch
}

/// The pixels a flat/solid fill produces, covering `mask.bounds`.
///
/// Unlike planar fill ([`render`]), this paints the masked area with one flat
/// colour sampled from the median of the mask's immediate surroundings
/// (`fitted.paper`).
pub fn render_solid(page: &Raster, fitted: &Fitted) -> Raster {
    let bounds = fitted.mask.bounds;
    let samples = page.mode.samples();

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

    let medians = ring::fill_medians(page, &fitted.paper);

    for y in 0..bounds.h {
        for x in 0..bounds.w {
            let (px, py) = (bounds.x + x as i64, bounds.y + y as i64);
            let inside = fitted.mask.contains(px, py);
            for (channel, &median) in medians.iter().enumerate() {
                // Outside the mask, or on the alpha channel: keep the source
                // pixel. Alpha is not part of what median fill replaces -
                // only the colour channels get the ring's median.
                let value = if !inside || page.mode.alpha_channel() == Some(channel) {
                    page.sample(px as u32, py as u32, channel)
                } else {
                    median
                };
                patch.set_sample(x, y, channel, value);
            }
        }
    }
    patch
}

/// The pixels a solid color fill produces, covering `mask.bounds`.
///
/// Unlike [`render_solid`] (which samples median tone from surroundings),
/// this paints the masked area with a specified RGB color (`[r, g, b]`),
/// adapted to the raster's color mode and bit depth while preserving
/// unchanged outside regions and alpha channels.
pub fn render_solid_color(page: &Raster, fitted: &Fitted, color: [u8; 3]) -> Raster {
    let bounds = fitted.mask.bounds;
    let samples = page.mode.samples();

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

    let scale_to_depth = |val: u8| -> u16 {
        match page.depth {
            BitDepth::Sixteen => (val as u16) * 257,
            BitDepth::Eight => val as u16,
            other => {
                let max = ((1u32 << other.bits()) - 1) as u16;
                ((val as u32 * max as u32) / 255) as u16
            }
        }
    };

    let [r, g, b] = color;

    let fill_samples: Vec<u16> = match page.mode {
        ColorMode::Gray => {
            let luma = ((r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000) as u8;
            vec![scale_to_depth(luma)]
        }
        ColorMode::GrayAlpha => {
            let luma = ((r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000) as u8;
            vec![scale_to_depth(luma), 0]
        }
        ColorMode::Rgb => {
            vec![scale_to_depth(r), scale_to_depth(g), scale_to_depth(b)]
        }
        ColorMode::Rgba => {
            vec![scale_to_depth(r), scale_to_depth(g), scale_to_depth(b), 0]
        }
        ColorMode::Indexed => {
            let mut best_index = 0u16;
            if let Some(palette) = &page.palette {
                let entries = palette.len() / 3;
                let mut min_dist = i64::MAX;
                for i in 0..entries {
                    let pr = palette[i * 3] as i64;
                    let pg = palette[i * 3 + 1] as i64;
                    let pb = palette[i * 3 + 2] as i64;
                    let dr = pr - r as i64;
                    let dg = pg - g as i64;
                    let db = pb - b as i64;
                    let dist = dr * dr + dg * dg + db * db;
                    if dist < min_dist {
                        min_dist = dist;
                        best_index = i as u16;
                    }
                }
            }
            vec![best_index]
        }
        ColorMode::Cmyk => {
            let max_rgb = r.max(g).max(b);
            let k = 255 - max_rgb;
            let (c, m, y) = if k == 255 {
                (0, 0, 0)
            } else {
                let denom = (255 - k) as u32;
                (
                    ((255 - r as u32 - k as u32) * 255 / denom) as u8,
                    ((255 - g as u32 - k as u32) * 255 / denom) as u8,
                    ((255 - b as u32 - k as u32) * 255 / denom) as u8,
                )
            };
            vec![
                scale_to_depth(c),
                scale_to_depth(m),
                scale_to_depth(y),
                scale_to_depth(k),
            ]
        }
    };

    for y in 0..bounds.h {
        for x in 0..bounds.w {
            let (px, py) = (bounds.x + x as i64, bounds.y + y as i64);
            let inside = fitted.mask.contains(px, py);
            for (channel, &sample_val) in fill_samples.iter().enumerate() {
                let value = if !inside || page.mode.alpha_channel() == Some(channel) {
                    page.sample(px as u32, py as u32, channel)
                } else {
                    sample_val
                };
                patch.set_sample(x, y, channel, value);
            }
        }
    }
    patch
}

/// The tone a fill would use, for the review row and for the cluster
/// reconciliation. Luma, so two regions on different colour pages still
/// compare.
pub fn tone(fitted: &Fitted) -> u16 {
    fitted.ring.median
}

/// Whether two regions' tones are close enough to pool.
///
/// §7: "Pool only across regions whose ring medians agree within ~3 levels; a
/// page with a white panel and a grey-toned panel keeps two tones." Three
/// 8-bit levels, in 16-bit luma.
pub fn tones_agree(a: u16, b: u16) -> bool {
    (a as i32 - b as i32).abs() <= 3 * 257
}

/// Group regions into tone clusters, so a fill tone is reconciled across a
/// panel rather than pooled across a page.
pub fn cluster(tones: &[u16]) -> Vec<usize> {
    // Single-link over a sorted order, which for a one-dimensional quantity is
    // the whole of the clustering: two tones join if they agree, and a chain of
    // agreements is one cluster.
    let mut order: Vec<usize> = (0..tones.len()).collect();
    order.sort_by_key(|i| tones[*i]);

    let mut assignment = vec![0usize; tones.len()];
    let mut cluster = 0usize;
    for (rank, &index) in order.iter().enumerate() {
        if rank > 0 {
            let previous = order[rank - 1];
            if !tones_agree(tones[previous], tones[index]) {
                cluster += 1;
            }
        }
        assignment[index] = cluster;
    }
    assignment
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fit::{self, Route};
    use crate::image::{BitDepth, fixtures};
    use crate::mask::{Mask, Rect};

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

    fn fitted_over(page: &Raster, rect: Rect) -> Fitted {
        let seed = Mask::filled(rect);
        fit::fit(page, &seed, 1.0, 0.0, &fit::EdgeMap::none(page.width, page.height), true)
    }

    #[test]
    fn a_fill_on_cream_paper_paints_cream_rather_than_white() {
        // The near-white snap that is refused would paint 255 here.
        let page = gray_page(80, 80, |_, _| 246);
        let fitted = fitted_over(&page, Rect::new(30, 30, 20, 20));
        let patch = render(&page, &fitted);
        assert_eq!(patch.sample(10, 10, 0), 246);
    }

    #[test]
    fn a_gradient_is_filled_with_the_gradient() {
        // A flat fill here reads as a Mach band, which is the whole reason §4
        // fits a plane.
        let page = gray_page(80, 80, |x, _| (150 + x) as u8);
        let fitted = fitted_over(&page, Rect::new(30, 30, 20, 20));
        let patch = render(&page, &fitted);
        let left = patch.sample(2, 10, 0) as i32;
        let right = patch.sample(17, 10, 0) as i32;
        assert!(right - left > 8, "the fill was flat: {left} → {right}");
    }

    #[test]
    fn pixels_outside_the_mask_are_the_page_untouched() {
        let page = gray_page(80, 80, |x, y| ((x * 3 + y * 5) % 251) as u8);
        let mut fitted = fitted_over(&page, Rect::new(30, 30, 20, 20));
        // Punch a hole so part of the bbox is outside the mask.
        fitted.mask.set(35, 35, false);
        let patch = render(&page, &fitted);
        assert_eq!(patch.sample(5, 5, 0), page.sample(35, 35, 0));
    }

    #[test]
    fn an_alpha_channel_is_copied_rather_than_filled() {
        let page = fixtures::by_name("rgba8").raster;
        let fitted = fitted_over(&page, Rect::new(20, 16, 12, 10));
        let patch = render(&page, &fitted);
        for y in 0..patch.height {
            for x in 0..patch.width {
                assert_eq!(
                    patch.sample(x, y, 3),
                    page.sample(20 + x, 16 + y, 3),
                    "alpha changed at {x},{y}"
                );
            }
        }
    }

    #[test]
    fn an_indexed_fill_stays_on_a_single_palette_index() {
        let page = fixtures::by_name("indexed-p").raster;
        let fitted = fitted_over(&page, Rect::new(20, 16, 12, 10));
        let patch = render(&page, &fitted);
        let entries = page.palette.as_ref().unwrap().len() / 3;
        let mut inside: Vec<u16> = Vec::new();
        for y in 0..patch.height {
            for x in 0..patch.width {
                let (px, py) = (20 + x as i64, 16 + y as i64);
                if fitted.mask.contains(px, py) {
                    inside.push(patch.sample(x, y, 0));
                }
            }
        }
        assert!(!inside.is_empty());
        assert!(inside.iter().all(|v| *v == inside[0]), "a gradient was written into indices");
        assert!((inside[0] as usize) < entries, "the index is outside the palette");
    }

    #[test]
    fn a_white_panel_and_a_grey_panel_keep_two_tones() {
        // Pooled per page this would be one average tone on both.
        let tones = [246 * 257, 247 * 257, 180 * 257, 181 * 257];
        let clusters = cluster(&tones);
        assert_eq!(clusters[0], clusters[1]);
        assert_eq!(clusters[2], clusters[3]);
        assert_ne!(clusters[0], clusters[2]);
    }

    #[test]
    fn a_flat_region_routes_to_the_fill_and_a_toned_one_does_not() {
        let flat = gray_page(80, 80, |_, _| 246);
        assert_eq!(fitted_over(&flat, Rect::new(30, 30, 20, 20)).route, Route::Fill);

        let toned = gray_page(80, 80, |x, y| if x % 8 < 3 && y % 8 < 3 { 40 } else { 250 });
        assert_eq!(fitted_over(&toned, Rect::new(30, 30, 20, 20)).route, Route::Inpaint);
    }

    #[test]
    fn a_solid_fill_paints_flat_surround_median() {
        let mut page = gray_page(80, 80, |_, _| 255);
        for y in 35..45 {
            for x in 35..45 {
                page.data[(y * 80 + x) as usize] = 0;
            }
        }
        let fitted = fitted_over(&page, Rect::new(30, 30, 20, 20));
        let patch = render_solid(&page, &fitted);
        for y in 0..patch.height {
            for x in 0..patch.width {
                let (px, py) = (30 + x as i64, 30 + y as i64);
                if fitted.mask.contains(px, py) {
                    assert_eq!(patch.sample(x, y, 0), 255, "masked pixel was not flat white at {x},{y}");
                }
            }
        }
    }

    #[test]
    fn a_solid_color_fill_paints_chosen_color_in_gray_and_rgb() {
        let gray = gray_page(80, 80, |_, _| 100);
        let fitted = fitted_over(&gray, Rect::new(30, 30, 20, 20));
        // Pure white [255, 255, 255]
        let patch = render_solid_color(&gray, &fitted, [255, 255, 255]);
        assert_eq!(patch.sample(10, 10, 0), 255);

        // RGB page
        let rgb_page = fixtures::by_name("rgb8").raster;
        let fitted_rgb = fitted_over(&rgb_page, Rect::new(10, 10, 20, 20));
        let patch_rgb = render_solid_color(&rgb_page, &fitted_rgb, [255, 128, 64]);
        assert_eq!(patch_rgb.sample(5, 5, 0), 255);
        assert_eq!(patch_rgb.sample(5, 5, 1), 128);
        assert_eq!(patch_rgb.sample(5, 5, 2), 64);
    }

    #[test]
    fn a_solid_color_fill_preserves_alpha_and_scales_depth() {
        let rgba = fixtures::by_name("rgba8").raster;
        let fitted = fitted_over(&rgba, Rect::new(20, 16, 12, 10));
        let patch = render_solid_color(&rgba, &fitted, [255, 255, 255]);
        assert_eq!(patch.sample(5, 5, 0), 255);
        assert_eq!(patch.sample(5, 5, 1), 255);
        assert_eq!(patch.sample(5, 5, 2), 255);
        // Alpha preserved from page
        assert_eq!(patch.sample(5, 5, 3), rgba.sample(25, 21, 3));

        // 16-bit gray page
        let l16 = fixtures::by_name("l16").raster;
        let fitted_16 = fitted_over(&l16, Rect::new(10, 10, 20, 20));
        let patch_16 = render_solid_color(&l16, &fitted_16, [255, 255, 255]);
        assert_eq!(patch_16.sample(5, 5, 0), 65535);
    }

    #[test]
    fn a_solid_color_fill_keeps_indexed_and_cmyk_pages_in_their_native_modes() {
        let indexed = fixtures::by_name("indexed-p").raster;
        let indexed_fit = fitted_over(&indexed, Rect::new(20, 16, 12, 10));
        let indexed_patch = render_solid_color(&indexed, &indexed_fit, [250, 20, 30]);
        let entries = indexed.palette.as_ref().unwrap().len() / 3;
        assert_eq!(indexed_patch.mode, ColorMode::Indexed);
        assert!((indexed_patch.sample(5, 5, 0) as usize) < entries);

        let cmyk = fixtures::by_name("cmyk8").raster;
        let cmyk_fit = fitted_over(&cmyk, Rect::new(20, 16, 12, 10));
        let cmyk_patch = render_solid_color(&cmyk, &cmyk_fit, [255, 0, 0]);
        assert_eq!(cmyk_patch.mode, ColorMode::Cmyk);
        assert_eq!(
            (0..4).map(|channel| cmyk_patch.sample(5, 5, channel)).collect::<Vec<_>>(),
            vec![0, 255, 255, 0],
        );
    }
}
