//! The page as the detector wants it, and the way back.
//!
//! Phase 0 spike 6 pinned the input as `images float32[1, 3, 1024, 1024]` -
//! **fixed**, not dynamic. So every page is letterboxed into that square,
//! and the transform has to be invertible because every box that comes back
//! is in letterbox coordinates and every consumer works in page coordinates.
//!
//! Three details are reproduced from upstream rather than chosen, because the
//! model was trained through exactly this preprocessing and a "better" version
//! is a different distribution:
//!
//! - **Padding goes bottom and right only**, not centred:
//!   `copyMakeBorder(im, 0, dh, 0, dw)`.
//! - **Small pages are scaled up.** `letterbox` takes `scaleup=True` by
//!   default and upstream does not override it, so a page smaller than the
//!   square in both dimensions - a 400×600 thumbnail, a short webtoon strip
//!   segment - is *enlarged* before detection rather than left alone. An
//!   800×1200 page is still reduced, to 683×1024; only both dimensions being
//!   under 1024 triggers the enlargement.
//! - **The planes arrive in BGR order.** `preprocess_img` converts BGR→RGB and
//!   then reverses the channel axis again with `transpose((2,0,1))[::-1]`, so
//!   the tensor's first plane is blue. The comment beside it says the opposite.
//!   Manga is mostly achromatic and the difference is usually invisible, which
//!   is exactly why it would survive unnoticed on a colour page.

use crate::constants::DETECTOR_INPUT;
use crate::image::Raster;

/// How a page was fitted into the square, and how to get back out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Letterbox {
    /// Multiply a page coordinate by this to reach letterbox space.
    pub scale: f32,
    /// The page's size inside the square, before padding.
    pub fitted_w: u32,
    pub fitted_h: u32,
    pub page_w: u32,
    pub page_h: u32,
}

impl Letterbox {
    pub fn fit(page_w: u32, page_h: u32) -> Letterbox {
        let target = DETECTOR_INPUT as f32;
        let scale = (target / page_w as f32).min(target / page_h as f32);
        Letterbox {
            scale,
            fitted_w: (page_w as f32 * scale).round() as u32,
            fitted_h: (page_h as f32 * scale).round() as u32,
            page_w,
            page_h,
        }
    }

    /// Padding, in letterbox pixels, on the right and bottom edges.
    pub fn padding(&self) -> (u32, u32) {
        (DETECTOR_INPUT - self.fitted_w, DETECTOR_INPUT - self.fitted_h)
    }

    /// Letterbox coordinate back to page coordinate.
    ///
    /// Upstream divides by `(input − dw)` rather than by the scale, which is
    /// the same number written differently; this way round is one operation and
    /// cannot drift from `fit`.
    pub fn to_page(&self, x: f32, y: f32) -> (f32, f32) {
        (x / self.scale, y / self.scale)
    }
}

/// `[1, 3, 1024, 1024]` in BGR plane order, normalised to 0..1.
///
/// Resampling is bilinear, matching upstream's `cv2.INTER_LINEAR`. Area
/// averaging would be better for a downscale and is *not* used, for the same
/// reason the channel order is not corrected: the weights were fitted through
/// this exact resample.
pub fn to_tensor(page: &Raster) -> (Vec<f32>, Letterbox) {
    let fit = Letterbox::fit(page.width, page.height);
    let side = DETECTOR_INPUT as usize;
    // Zero everywhere, so the padding is the black upstream fills with.
    let mut tensor = vec![0f32; 3 * side * side];

    for y in 0..fit.fitted_h as usize {
        // Sample position in page space, at the centre of the destination
        // pixel, which is what INTER_LINEAR does.
        let src_y = ((y as f32 + 0.5) / fit.scale) - 0.5;
        for x in 0..fit.fitted_w as usize {
            let src_x = ((x as f32 + 0.5) / fit.scale) - 0.5;
            let [r, g, b] = bilinear_rgb(page, src_x, src_y);
            // Plane order is blue, green, red - see the module note.
            tensor[y * side + x] = b / 255.0;
            tensor[side * side + y * side + x] = g / 255.0;
            tensor[2 * side * side + y * side + x] = r / 255.0;
        }
    }
    (tensor, fit)
}

fn bilinear_rgb(page: &Raster, x: f32, y: f32) -> [f32; 3] {
    let x0 = x.floor();
    let y0 = y.floor();
    let fx = x - x0;
    let fy = y - y0;
    let clamp = |v: f32, max: u32| v.clamp(0.0, (max - 1) as f32) as u32;
    let (x0i, y0i) = (clamp(x0, page.width), clamp(y0, page.height));
    let (x1i, y1i) = (clamp(x0 + 1.0, page.width), clamp(y0 + 1.0, page.height));

    let mut out = [0f32; 3];
    for (channel, slot) in out.iter_mut().enumerate() {
        let at = |px: u32, py: u32| rgb_channel(page, px, py, channel) as f32;
        let top = at(x0i, y0i) * (1.0 - fx) + at(x1i, y0i) * fx;
        let bottom = at(x0i, y1i) * (1.0 - fx) + at(x1i, y1i) * fx;
        *slot = top * (1.0 - fy) + bottom * fy;
    }
    out
}

fn rgb_channel(page: &Raster, x: u32, y: u32, channel: usize) -> u8 {
    // `to_rgb8` for one pixel. Kept here rather than materialising the whole
    // page as RGB8 first: a 30 000 px longstrip segment would be 270 MB of
    // throwaway buffer, and the memory budget does not have it.
    let rgb = page.rgb8_pixel(x, y);
    rgb[channel]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::fixtures;

    #[test]
    fn a_tall_page_fits_by_its_height_and_pads_to_the_right() {
        let fit = Letterbox::fit(1600, 2400);
        assert_eq!(fit.fitted_h, 1024);
        assert_eq!(fit.fitted_w, 683);
        let (dw, dh) = fit.padding();
        assert_eq!(dh, 0);
        assert_eq!(dw, 1024 - 683);
    }

    #[test]
    fn a_page_smaller_than_the_square_is_scaled_up_rather_than_left_alone() {
        // Upstream's `scaleup` defaults to true and is not overridden. Worth a
        // test because it is the opposite of what "downscaled proxy" suggests.
        let fit = Letterbox::fit(400, 600);
        assert!(fit.scale > 1.0, "scale was {}", fit.scale);
        assert_eq!(fit.fitted_h, 1024);

        // And the common case is still a reduction, so neither reading is safe
        // to assume.
        assert!(Letterbox::fit(800, 1200).scale < 1.0);
    }

    #[test]
    fn a_box_round_trips_through_the_transform() {
        let fit = Letterbox::fit(1600, 2400);
        let (x, y) = fit.to_page(683.0, 1024.0);
        assert!((x - 1600.0).abs() < 2.0, "x was {x}");
        assert!((y - 2400.0).abs() < 2.0, "y was {y}");
    }

    #[test]
    fn the_tensor_is_bgr_and_the_padding_is_black() {
        // A fixture whose channels differ, so a channel-order mistake shows.
        let page = fixtures::by_name("rgb8").raster;
        let (tensor, fit) = to_tensor(&page);
        let side = DETECTOR_INPUT as usize;
        assert_eq!(tensor.len(), 3 * side * side);

        let blue_plane = tensor[0];
        let red_plane = tensor[2 * side * side];
        let [r, _, b] = page.rgb8_pixel(0, 0);
        assert!((blue_plane - b as f32 / 255.0).abs() < 0.02, "first plane is not blue");
        assert!((red_plane - r as f32 / 255.0).abs() < 0.02, "third plane is not red");

        let (dw, _) = fit.padding();
        if dw > 0 {
            let padded_x = fit.fitted_w as usize;
            assert_eq!(tensor[padded_x], 0.0, "padding is not black");
        }
    }
}
