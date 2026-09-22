//! Pure diffusion crop preparation and host-side compositing contracts.
//!
//! This module decouples the raster preparation (tight crop extraction, context padding,
//! edge replication, and hint mask extraction) and host-side composition (isolation
//! ramp, alpha blending, dithering, channel reconstruction, and bit-exact preservation)
//! from any specific diffusion runner (in-process, sidecar, or remote cloud execution).
//!
//! ## Invariants
//!
//! 1. **Bit-Identical Outside Applied Mask:** Pixels outside the applied blend mask
//!    ([`applied_mask`]) are bit-identical to the source raster.
//! 2. **Alpha Channel Preservation:** Alpha channels (in `LA` and `RGBA` modes) are copied
//!    directly from the source page and never overwritten or synthesized by diffusion.
//! 3. **Proportional, Latent-Snapped Context:** The crop is sized tightly to the region's
//!    bounding box with context clamped between [`CONTEXT_MIN`] and [`CONTEXT_MAX`], snapped
//!    up to multiples of [`LATENT_STRIDE`].
//! 4. **Edge Replication Without Reflection:** When a crop extends beyond the source page
//!    boundaries, missing pixels are edge-replicated. Missing padding is recorded via [`EdgePad`].
//! 5. **Explicit Checked Output Validation:** Model outputs are verified against expected
//!    geometry and buffer length using checked arithmetic before compositing.
//! 6. **Immutable Prepared State Binding:** Preparation captures the target patch, alpha ramp,
//!    and source mode/depth. Composition consumes this immutable snapshot alongside the
//!    [`GeneratedCrop`], preventing source desynchronization by construction.

use serde::{Deserialize, Serialize};

use crate::engines::model::{
    self, applied_mask, ceiling_for, dither_at, page_crop, source_channel, AlphaRamp, Decline,
    Error, Rendered, MAX_BOX,
};
use crate::fit::Fitted;
use crate::image::{BitDepth, ColorMode, Raster};
use crate::mask::{Mask, Rect};
use crate::strip::window::EdgePad;

/// Supported cloud inference providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CloudProvider {
    Beam,
    Modal,
}

impl CloudProvider {
    pub fn as_str(&self) -> &'static str {
        match self {
            CloudProvider::Beam => "beam",
            CloudProvider::Modal => "modal",
        }
    }
}

/// Where a rendering operation should execute.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum ExecutionTarget {
    #[default]
    Local,
    #[serde(rename = "beam")]
    Beam {
        profile_id: String,
    },
    #[serde(rename = "modal")]
    Modal {
        profile_id: String,
    },
}

impl ExecutionTarget {
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local)
    }

    pub fn provider(&self) -> Option<CloudProvider> {
        match self {
            Self::Local => None,
            Self::Beam { .. } => Some(CloudProvider::Beam),
            Self::Modal { .. } => Some(CloudProvider::Modal),
        }
    }

    pub fn profile_id(&self) -> Option<&str> {
        match self {
            Self::Local => None,
            Self::Beam { profile_id } | Self::Modal { profile_id } => Some(profile_id),
        }
    }
}

/// Model and recipe identity contract for diffusion rendering.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RenderRecipe {
    pub recipe_id: String,
    pub preprocessing_version: String,
    pub model_id: String,
    pub model_revision: String,
    pub native_mask_conditioning: bool,
}

impl RenderRecipe {
    pub fn new(
        recipe_id: impl Into<String>,
        preprocessing_version: impl Into<String>,
        model_id: impl Into<String>,
        model_revision: impl Into<String>,
        native_mask_conditioning: bool,
    ) -> Self {
        Self {
            recipe_id: recipe_id.into(),
            preprocessing_version: preprocessing_version.into(),
            model_id: model_id.into(),
            model_revision: model_revision.into(),
            native_mask_conditioning,
        }
    }
}

/// The crop's dimensions are snapped up to a multiple of this.
pub const LATENT_STRIDE: u32 = 16;

/// The least context a crop carries, per side.
pub const CONTEXT_MIN: u32 = 24;

/// The most context a crop carries, per side.
pub const CONTEXT_MAX: u32 = 80;

/// The largest region accepted by model engines.
pub const MAX_REGION: u32 = MAX_BOX;

/// How much real page the crop carries around the region, per side: half the
/// region's long side, clamped between [`CONTEXT_MIN`] and [`CONTEXT_MAX`].
pub fn context_for(bounds: Rect) -> u32 {
    (bounds.w.max(bounds.h) / 2).clamp(CONTEXT_MIN, CONTEXT_MAX)
}

/// Snap an extent up to the next multiple of [`LATENT_STRIDE`].
pub(crate) fn snap(extent: u32) -> u32 {
    extent.div_ceil(LATENT_STRIDE) * LATENT_STRIDE
}

/// The crop the model is shown, in page coordinates.
pub fn crop_for(bounds: Rect) -> Rect {
    let context = context_for(bounds);
    let side = |extent: u32| snap(extent.saturating_add(2 * context));
    let (w, h) = (side(bounds.w), side(bounds.h));
    Rect::new(
        bounds.x + bounds.w as i64 / 2 - w as i64 / 2,
        bounds.y + bounds.h as i64 / 2 - h as i64 / 2,
        w,
        h,
    )
}

/// Whether model diffusion engines can run on this raster.
pub fn applies(page: &Raster) -> bool {
    model::applies(page) && page.depth != BitDepth::Sixteen
}

/// Every reason a region would be declined before crop preparation or execution.
pub fn declines(page: &Raster, fitted: &Fitted) -> Option<Decline> {
    if let Some(decline) = model::declines(page, fitted) {
        return Some(decline);
    }
    (page.depth == BitDepth::Sixteen).then_some(Decline::DepthBeyondEngine(page.depth))
}

/// The crop as RGB8 samples, edge-replicated where it runs off the page.
pub(crate) fn crop_samples(page: &Raster, crop: Rect, ceiling: f64) -> (EdgePad, Vec<u8>) {
    let mut pad = EdgePad::None;
    let total_samples = (crop.w as usize)
        .checked_mul(crop.h as usize)
        .and_then(|px| px.checked_mul(3))
        .expect("crop dimensions overflow byte length");
    let mut out = vec![0u8; total_samples];
    for y in 0..crop.h as i64 {
        for x in 0..crop.w as i64 {
            let (px, py) = (crop.x + x, crop.y + y);
            let cx = px.clamp(0, page.width as i64 - 1);
            let cy = py.clamp(0, page.height as i64 - 1);
            if (cx, cy) != (px, py) {
                pad = EdgePad::Replicate;
            }
            let at = (y * crop.w as i64 + x) as usize * 3;
            for c in 0..3 {
                let channel = source_channel(page.mode, c);
                let sample = page.sample(cx as u32, cy as u32, channel) as f64;
                out[at + c] = (sample / ceiling * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    (pad, out)
}

/// The hint mask samples: 255 where ink is present, 0 elsewhere.
pub(crate) fn hint_samples(fitted: &Fitted, crop: Rect) -> Vec<u8> {
    let total_samples = (crop.w as usize)
        .checked_mul(crop.h as usize)
        .expect("crop dimensions overflow byte length");
    let mut out = vec![0u8; total_samples];
    for y in 0..crop.h as i64 {
        for x in 0..crop.w as i64 {
            if fitted.ink.contains(crop.x + x, crop.y + y) {
                out[(y * crop.w as i64 + x) as usize] = 255;
            }
        }
    }
    out
}

/// One returned sample reduced back to the page's color mode.
pub(crate) fn returned_sample(edited: &[u8], mode: ColorMode, channel: usize, at: usize) -> f64 {
    match mode {
        ColorMode::Gray | ColorMode::GrayAlpha => {
            (edited[at] as f64 + edited[at + 1] as f64 + edited[at + 2] as f64) / 3.0 / 255.0
        }
        _ => edited[at + channel.min(2)] as f64 / 255.0,
    }
}

/// Prepared diffusion inputs containing isolated raster crops, hint masks, bounds,
/// and the captured source state needed for immutable host composition.
#[derive(Debug, Clone)]
pub struct PreparedRender {
    applied: Mask,
    bounds: Rect,
    crop: Rect,
    pad: EdgePad,
    image_rgb8: Vec<u8>,
    hint_gray8: Vec<u8>,
    ramp: AlphaRamp,
    patch: Raster,
    mode: ColorMode,
    depth: BitDepth,
    ceiling: f64,
}

/// A generated diffusion crop returned by an inpainter or inference engine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeneratedCrop<'a> {
    width: u32,
    height: u32,
    rgb8: &'a [u8],
}

impl<'a> GeneratedCrop<'a> {
    pub fn new(width: u32, height: u32, rgb8: &'a [u8]) -> Self {
        Self {
            width,
            height,
            rgb8,
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn rgb8(&self) -> &'a [u8] {
        self.rgb8
    }
}

impl PreparedRender {
    /// Prepare the render inputs for a page region.
    ///
    /// Validates source raster geometry, enforces model engine eligibility,
    /// extracts the snapped crop and hint samples, and captures the original
    /// unedited patch for subsequent composition.
    pub fn prepare(page: &Raster, fitted: &Fitted) -> Result<Self, Error> {
        if page.width == 0 || page.height == 0 {
            return Err(Error::Run(
                "cannot prepare render for zero-dimension page".to_owned(),
            ));
        }
        if let Some(decline) = declines(page, fitted) {
            return Err(decline.into());
        }
        let applied = applied_mask(fitted, page.width, page.height);
        let bounds = applied.bounds;
        if bounds.w == 0 || bounds.h == 0 {
            return Err(Error::Run(
                "cannot prepare render for empty region bounds".to_owned(),
            ));
        }
        let crop = crop_for(bounds);
        if crop.w == 0 || crop.h == 0 {
            return Err(Error::Run(
                "cannot prepare render for empty crop bounds".to_owned(),
            ));
        }
        let ceiling = ceiling_for(page.depth);
        let (pad, image_rgb8) = crop_samples(page, crop, ceiling);
        let hint_gray8 = hint_samples(fitted, crop);
        let ramp = AlphaRamp::new(fitted, page.width, page.height);
        let patch = page_crop(page, bounds);

        Ok(PreparedRender {
            applied,
            bounds,
            crop,
            pad,
            image_rgb8,
            hint_gray8,
            ramp,
            patch,
            mode: page.mode,
            depth: page.depth,
            ceiling,
        })
    }

    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    pub fn crop(&self) -> Rect {
        self.crop
    }

    pub fn pad(&self) -> EdgePad {
        self.pad
    }

    pub fn image_rgb8(&self) -> &[u8] {
        &self.image_rgb8
    }

    pub fn hint_gray8(&self) -> &[u8] {
        &self.hint_gray8
    }

    pub fn applied(&self) -> &Mask {
        &self.applied
    }

    /// Validate the dimensions and buffer length of the generated crop.
    pub fn validate_generated(&self, generated: &GeneratedCrop<'_>) -> Result<(), Error> {
        if generated.width != self.crop.w || generated.height != self.crop.h {
            return Err(Error::Run(format!(
                "asked for {}×{} and got {}×{}",
                self.crop.w, self.crop.h, generated.width, generated.height
            )));
        }
        let expected_len = (self.crop.w as usize)
            .checked_mul(self.crop.h as usize)
            .and_then(|px| px.checked_mul(3))
            .ok_or_else(|| {
                Error::Run("crop dimensions overflow byte length calculation".to_owned())
            })?;
        if generated.rgb8.len() != expected_len {
            return Err(Error::Run(format!(
                "the model output byte length ({}) does not match expected length ({})",
                generated.rgb8.len(),
                expected_len
            )));
        }
        Ok(())
    }

    /// Composite the generated crop back onto the captured source patch.
    ///
    /// Clones the captured bounded patch and evaluates the captured AlphaRamp without requiring
    /// the source page to be passed again, preventing caller desynchronization by construction.
    pub fn composite(&self, generated: &GeneratedCrop<'_>) -> Result<Rendered, Error> {
        self.validate_generated(generated)?;
        let mut patch = self.patch.clone();
        let samples = self.mode.samples();
        let alpha_channel = self.mode.alpha_channel();

        for y in self.bounds.y..self.bounds.bottom() {
            for x in self.bounds.x..self.bounds.right() {
                if !self.applied.contains(x, y) {
                    continue;
                }
                let alpha = self.ramp.at(x, y);
                if alpha == 0.0 {
                    continue;
                }
                let at = ((y - self.crop.y) * self.crop.w as i64 + (x - self.crop.x)) as usize * 3;
                let (lx, ly) = ((x - self.bounds.x) as u32, (y - self.bounds.y) as u32);
                let dither = dither_at(self.depth, alpha, x, y, self.ceiling);
                for channel in 0..samples {
                    // Alpha is copied, never produced.
                    if alpha_channel == Some(channel) {
                        continue;
                    }
                    let model = returned_sample(generated.rgb8, self.mode, channel, at);
                    let original = patch.sample(lx, ly, channel) as f64 / self.ceiling;
                    let blended = alpha * model + (1.0 - alpha) * original;
                    let value = (blended * self.ceiling + dither)
                        .round()
                        .clamp(0.0, self.ceiling) as u16;
                    patch.set_sample(lx, ly, channel, value);
                }
            }
        }

        Ok(Rendered {
            mask: self.applied.clone(),
            pixels: patch,
            pad: self.pad,
            tiles: 1,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engines::model::write_bound;
    use crate::fit;

    fn make_page(
        w: u32,
        h: u32,
        mode: ColorMode,
        depth: BitDepth,
        fill_fn: impl Fn(u32, u32, usize) -> u16,
    ) -> Raster {
        let samples = mode.samples();
        let bits = depth.bits() as usize;
        let stride = (w as usize * samples * bits).div_ceil(8);
        let mut raster = Raster {
            width: w,
            height: h,
            mode,
            depth,
            icc: None,
            palette: None,
            trns: None,
            srgb_intent: None,
            data: vec![0u8; stride * h as usize],
        };
        for y in 0..h {
            for x in 0..w {
                for c in 0..samples {
                    raster.set_sample(x, y, c, fill_fn(x, y, c));
                }
            }
        }
        raster
    }

    fn fitted_over(page: &Raster, rect: Rect) -> Fitted {
        let seed = Mask::filled(rect);
        fit::fit(
            page,
            &seed,
            1.0,
            0.0,
            &fit::EdgeMap::none(page.width, page.height),
            true,
        )
    }

    #[test]
    fn pure_crop_prep_and_dimensions_geometry() {
        let page = make_page(500, 600, ColorMode::Rgb, BitDepth::Eight, |x, y, c| {
            ((x * 7 + y * 13 + c as u32 * 31) % 256) as u16
        });
        let rect = Rect::new(100, 150, 50, 70);
        let fitted = fitted_over(&page, rect);

        let prepared = PreparedRender::prepare(&page, &fitted).expect("prepare should succeed");
        assert_eq!(prepared.crop().w % LATENT_STRIDE, 0);
        assert_eq!(prepared.crop().h % LATENT_STRIDE, 0);
        assert_eq!(
            prepared.image_rgb8().len(),
            (prepared.crop().w * prepared.crop().h * 3) as usize
        );
        assert_eq!(
            prepared.hint_gray8().len(),
            (prepared.crop().w * prepared.crop().h) as usize
        );
        assert_eq!(prepared.pad(), EdgePad::None);
    }

    #[test]
    fn outside_applied_mask_samples_bit_identical_and_inside_blended() {
        let page = make_page(400, 400, ColorMode::Rgb, BitDepth::Eight, |x, y, c| {
            ((x * 11 + y * 17 + c as u32 * 43) % 256) as u16
        });
        let rect = Rect::new(120, 120, 40, 40);
        let fitted = fitted_over(&page, rect);
        let prepared = PreparedRender::prepare(&page, &fitted).unwrap();

        // Synthetic generated crop that is black everywhere (0)
        let generated_pixels = vec![0u8; (prepared.crop().w * prepared.crop().h * 3) as usize];
        let generated = GeneratedCrop::new(prepared.crop().w, prepared.crop().h, &generated_pixels);

        let rendered = prepared.composite(&generated).unwrap();
        let bounds = rendered.mask.bounds;
        let mut inside_changed = 0usize;

        for y in 0..rendered.pixels.height {
            for x in 0..rendered.pixels.width {
                let (px, py) = (bounds.x + x as i64, bounds.y + y as i64);
                let inside = rendered.mask.contains(px, py);
                for c in 0..3 {
                    let actual = rendered.pixels.sample(x, y, c);
                    let orig = page.sample(px as u32, py as u32, c);
                    if inside {
                        if actual != orig {
                            inside_changed += 1;
                        }
                    } else {
                        assert_eq!(
                            actual, orig,
                            "Sample outside mask changed at {px},{py} channel {c}"
                        );
                    }
                }
            }
        }
        assert!(inside_changed > 0, "Expected changes inside the mask");
    }

    #[test]
    fn alpha_channel_preserved_on_rgba_and_gray_alpha() {
        for mode in [ColorMode::Rgba, ColorMode::GrayAlpha] {
            let page = make_page(300, 300, mode, BitDepth::Eight, |x, y, c| {
                if mode.alpha_channel() == Some(c) {
                    ((x * 3 + y * 5) % 200 + 55) as u16 // Specific varied alpha
                } else {
                    ((x + y * 2 + c as u32) % 256) as u16
                }
            });
            let rect = Rect::new(80, 80, 50, 50);
            let fitted = fitted_over(&page, rect);
            let prepared = PreparedRender::prepare(&page, &fitted).unwrap();

            // Generated crop is pure white (255)
            let generated_pixels =
                vec![255u8; (prepared.crop().w * prepared.crop().h * 3) as usize];
            let generated =
                GeneratedCrop::new(prepared.crop().w, prepared.crop().h, &generated_pixels);

            let rendered = prepared.composite(&generated).unwrap();
            let bounds = rendered.mask.bounds;
            let alpha_ch = mode.alpha_channel().unwrap();

            for y in 0..rendered.pixels.height {
                for x in 0..rendered.pixels.width {
                    let (px, py) = (bounds.x + x as i64, bounds.y + y as i64);
                    let orig_alpha = page.sample(px as u32, py as u32, alpha_ch);
                    let rendered_alpha = rendered.pixels.sample(x, y, alpha_ch);
                    assert_eq!(
                        rendered_alpha, orig_alpha,
                        "Alpha channel modified at {px},{py} in {mode:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn edge_crop_mapping_and_replicated_padding_recorded() {
        let page = make_page(200, 200, ColorMode::Gray, BitDepth::Eight, |x, y, _| {
            if x == 0 || y == 0 {
                42
            } else {
                180
            }
        });
        // Region tightly abutting (2, 2), causing the context crop to extend into negative coords
        let rect = Rect::new(2, 2, 20, 20);
        let fitted = fitted_over(&page, rect);
        let prepared = PreparedRender::prepare(&page, &fitted).unwrap();

        assert_eq!(
            prepared.pad(),
            EdgePad::Replicate,
            "Should detect edge replication"
        );
        assert!(
            prepared.crop().x < 0 || prepared.crop().y < 0,
            "Crop should extend past page origin"
        );

        // The edge-replicated samples in the negative coordinate portion should match (0, 0)
        let sample_at_0_0 = prepared.image_rgb8()[0];
        assert_eq!(
            sample_at_0_0, 42,
            "Out-of-bounds sample should be replicated from (0,0)"
        );
    }

    #[test]
    fn mismatched_generated_dimensions_and_lengths_rejected() {
        let page = make_page(300, 300, ColorMode::Rgb, BitDepth::Eight, |_, _, _| 128);
        let rect = Rect::new(50, 50, 40, 40);
        let fitted = fitted_over(&page, rect);
        let prepared = PreparedRender::prepare(&page, &fitted).unwrap();

        // Wrong width
        let wrong_w_bytes = vec![0u8; ((prepared.crop().w + 16) * prepared.crop().h * 3) as usize];
        let gen_wrong_w =
            GeneratedCrop::new(prepared.crop().w + 16, prepared.crop().h, &wrong_w_bytes);
        match prepared.composite(&gen_wrong_w) {
            Err(Error::Run(err)) => assert!(err.contains("asked for"), "{err}"),
            other => panic!("Expected Error::Run for wrong width, got {other:?}"),
        }

        // Wrong height
        let wrong_h_bytes = vec![0u8; (prepared.crop().w * (prepared.crop().h + 16) * 3) as usize];
        let gen_wrong_h =
            GeneratedCrop::new(prepared.crop().w, prepared.crop().h + 16, &wrong_h_bytes);
        match prepared.composite(&gen_wrong_h) {
            Err(Error::Run(err)) => assert!(err.contains("asked for"), "{err}"),
            other => panic!("Expected Error::Run for wrong height, got {other:?}"),
        }

        // Correct dimensions but short byte length
        let short_bytes = vec![0u8; (prepared.crop().w * prepared.crop().h * 3 - 5) as usize];
        let gen_short_bytes =
            GeneratedCrop::new(prepared.crop().w, prepared.crop().h, &short_bytes);
        match prepared.composite(&gen_short_bytes) {
            Err(Error::Run(err)) => assert!(err.contains("byte length"), "{err}"),
            other => panic!("Expected Error::Run for short buffer, got {other:?}"),
        }
    }

    #[test]
    fn unsupported_source_modes_and_depths_refused() {
        // 16-bit depth
        let page_16bit = make_page(200, 200, ColorMode::Gray, BitDepth::Sixteen, |_, _, _| {
            30000
        });
        let fitted = fitted_over(&page_16bit, Rect::new(20, 20, 30, 30));
        assert!(!applies(&page_16bit));
        assert_eq!(
            declines(&page_16bit, &fitted),
            Some(Decline::DepthBeyondEngine(BitDepth::Sixteen))
        );
        assert!(matches!(
            PreparedRender::prepare(&page_16bit, &fitted),
            Err(Error::Declined(Decline::DepthBeyondEngine(
                BitDepth::Sixteen
            )))
        ));

        // 1-bit depth
        let page_1bit = make_page(200, 200, ColorMode::Gray, BitDepth::One, |_, _, _| 1);
        let fitted = fitted_over(&page_1bit, Rect::new(20, 20, 30, 30));
        assert!(!applies(&page_1bit));
        assert_eq!(
            declines(&page_1bit, &fitted),
            Some(Decline::Depth(BitDepth::One))
        );

        // Indexed mode
        let page_indexed = make_page(200, 200, ColorMode::Indexed, BitDepth::Eight, |_, _, _| 0);
        let fitted = fitted_over(&page_indexed, Rect::new(20, 20, 30, 30));
        assert!(!applies(&page_indexed));
        assert_eq!(
            declines(&page_indexed, &fitted),
            Some(Decline::Mode(ColorMode::Indexed))
        );

        // CMYK mode
        let page_cmyk = make_page(200, 200, ColorMode::Cmyk, BitDepth::Eight, |_, _, _| 0);
        let fitted = fitted_over(&page_cmyk, Rect::new(20, 20, 30, 30));
        assert!(!applies(&page_cmyk));
        assert_eq!(
            declines(&page_cmyk, &fitted),
            Some(Decline::Mode(ColorMode::Cmyk))
        );

        // Oversized region
        let page_large = make_page(3000, 3000, ColorMode::Gray, BitDepth::Eight, |_, _, _| 128);
        let fitted_large = fitted_over(&page_large, Rect::new(0, 0, MAX_REGION + 1, 10));
        assert!(matches!(
            declines(&page_large, &fitted_large),
            Some(Decline::TooLarge { .. })
        ));
    }

    #[test]
    fn edit_stays_strictly_inside_write_bound() {
        let page = make_page(500, 500, ColorMode::Gray, BitDepth::Eight, |_, _, _| 200);
        let fitted = fitted_over(&page, Rect::new(150, 150, 60, 40));
        let prepared = PreparedRender::prepare(&page, &fitted).unwrap();

        let gen_pixels = vec![0u8; (prepared.crop().w * prepared.crop().h * 3) as usize];
        let generated = GeneratedCrop::new(prepared.crop().w, prepared.crop().h, &gen_pixels);
        let rendered = prepared.composite(&generated).unwrap();

        let permitted = write_bound(&fitted, page.width, page.height);
        let bounds = rendered.mask.bounds;
        for y in 0..rendered.pixels.height {
            for x in 0..rendered.pixels.width {
                if rendered.pixels.sample(x, y, 0) == 200 {
                    continue;
                }
                let (px, py) = (bounds.x + x as i64, bounds.y + y as i64);
                assert!(
                    permitted.contains(px, py),
                    "Wrote outside write bound at {px},{py}"
                );
            }
        }
    }

    #[test]
    fn prepared_snapshot_is_stable_even_if_original_page_mutates() {
        let mut page = make_page(400, 400, ColorMode::Rgb, BitDepth::Eight, |_, _, _| 200);
        let fitted = fitted_over(&page, Rect::new(100, 100, 40, 40));
        let prepared = PreparedRender::prepare(&page, &fitted).unwrap();

        // Mutate original page samples drastically after prepare
        for b in page.data.iter_mut() {
            *b = 0;
        }

        let gen_pixels = vec![50u8; (prepared.crop().w * prepared.crop().h * 3) as usize];
        let generated = GeneratedCrop::new(prepared.crop().w, prepared.crop().h, &gen_pixels);
        let rendered = prepared.composite(&generated).unwrap();

        // Pixels outside the mask must match the captured page state (200), not the mutated page (0)
        let bounds = rendered.mask.bounds;
        for y in 0..rendered.pixels.height {
            for x in 0..rendered.pixels.width {
                let (px, py) = (bounds.x + x as i64, bounds.y + y as i64);
                if !rendered.mask.contains(px, py) {
                    assert_eq!(
                        rendered.pixels.sample(x, y, 0),
                        200,
                        "Outside-mask sample leaked mutated page state at {px},{py}"
                    );
                }
            }
        }
    }

    #[test]
    fn zero_dimension_page_and_empty_bounds_refused() {
        let zero_w_page = Raster {
            width: 0,
            height: 100,
            mode: ColorMode::Gray,
            depth: BitDepth::Eight,
            icc: None,
            palette: None,
            trns: None,
            srgb_intent: None,
            data: Vec::new(),
        };
        let fitted = fitted_over(
            &make_page(100, 100, ColorMode::Gray, BitDepth::Eight, |_, _, _| 0),
            Rect::new(10, 10, 20, 20),
        );
        match PreparedRender::prepare(&zero_w_page, &fitted) {
            Err(Error::Run(err)) => assert!(err.contains("zero-dimension"), "{err}"),
            other => panic!("Expected Error::Run for zero width, got {other:?}"),
        }

        let zero_h_page = Raster {
            width: 100,
            height: 0,
            mode: ColorMode::Gray,
            depth: BitDepth::Eight,
            icc: None,
            palette: None,
            trns: None,
            srgb_intent: None,
            data: Vec::new(),
        };
        match PreparedRender::prepare(&zero_h_page, &fitted) {
            Err(Error::Run(err)) => assert!(err.contains("zero-dimension"), "{err}"),
            other => panic!("Expected Error::Run for zero height, got {other:?}"),
        }
    }

    #[test]
    fn execution_target_and_cloud_provider_serde() {
        let local = ExecutionTarget::Local;
        let local_json = serde_json::to_string(&local).unwrap();
        assert_eq!(local_json, r#"{"type":"local"}"#);
        let local_de: ExecutionTarget = serde_json::from_str(&local_json).unwrap();
        assert_eq!(local_de, ExecutionTarget::Local);
        assert!(local_de.is_local());
        assert_eq!(local_de.provider(), None);
        assert_eq!(local_de.profile_id(), None);

        let beam = ExecutionTarget::Beam {
            profile_id: "profile-1".to_string(),
        };
        let beam_json = serde_json::to_string(&beam).unwrap();
        assert_eq!(beam_json, r#"{"type":"beam","profile_id":"profile-1"}"#);
        let beam_de: ExecutionTarget = serde_json::from_str(&beam_json).unwrap();
        assert_eq!(beam_de, beam);
        assert!(!beam_de.is_local());
        assert_eq!(beam_de.provider(), Some(CloudProvider::Beam));
        assert_eq!(beam_de.profile_id(), Some("profile-1"));

        let modal = ExecutionTarget::Modal {
            profile_id: "profile-2".to_string(),
        };
        let modal_json = serde_json::to_string(&modal).unwrap();
        assert_eq!(modal_json, r#"{"type":"modal","profile_id":"profile-2"}"#);
        let modal_de: ExecutionTarget = serde_json::from_str(&modal_json).unwrap();
        assert_eq!(modal_de, modal);
        assert_eq!(modal_de.provider(), Some(CloudProvider::Modal));
        assert_eq!(modal_de.profile_id(), Some("profile-2"));

        assert_eq!(CloudProvider::Beam.as_str(), "beam");
        assert_eq!(CloudProvider::Modal.as_str(), "modal");
        assert_eq!(
            serde_json::to_string(&CloudProvider::Beam).unwrap(),
            r#""beam""#
        );
        assert_eq!(
            serde_json::to_string(&CloudProvider::Modal).unwrap(),
            r#""modal""#
        );
    }

    #[test]
    fn render_recipe_identity_contract() {
        let recipe = RenderRecipe::new(
            "flux-sdnq-klein-v1",
            "cleaner-crop-v1",
            "FLUX.2-klein-4b",
            "pinned-rev-1",
            false,
        );
        assert_eq!(recipe.recipe_id, "flux-sdnq-klein-v1");
        assert_eq!(recipe.preprocessing_version, "cleaner-crop-v1");
        assert_eq!(recipe.model_id, "FLUX.2-klein-4b");
        assert_eq!(recipe.model_revision, "pinned-rev-1");
        assert!(!recipe.native_mask_conditioning);

        let json = serde_json::to_string(&recipe).unwrap();
        let decoded: RenderRecipe = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, recipe);
    }
}
