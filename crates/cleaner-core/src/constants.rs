//! The pipeline's tuned constants, in one place.
//!
//! They are gathered here rather than left at their use sites for one
//! reason: several of them are *related* - `edit_margin` is defined in terms of
//! `isolation_radius` and the denoise dilation, and revision 1 got the
//! relationship wrong and every rung-1 region violated the contract by one
//! pixel. A derived constant should be derived where its inputs are visible.

/// First growth step of the mask candidate series, in proxy pixels.
pub const MIN_MASK_THICKNESS: u32 = 4;

/// Each subsequent step, in proxy pixels. Growth is cumulative - each step
/// dilates the previous result, not a fresh dilation by `4 + 2n`.
pub const MASK_GROWTH_STEP: u32 = 2;

/// How many growth steps follow the first.
pub const MASK_GROWTH_STEPS: u32 = 11;

/// The ring is the native annulus between `dilate(mask, k)` and
/// `dilate(mask, k + ANNULUS_WIDTH)` - never the upscaled mask's own contour,
/// which at k ≥ 2 lands on the anti-aliasing fringe.
pub const ANNULUS_WIDTH: u32 = 4;

/// Below this, the candidate search stops ratcheting and takes the largest
/// candidate still under the floor. Takes precedence over the monotone gate.
pub const FLAT_FLOOR: f32 = 0.5;

/// Deviation above which a fitted-but-poor box still goes to the inpainter.
pub const INPAINT_MIN_STD: f32 = 15.0;

/// Also the chosen-thickness ceiling for that second inpaint population.
pub const MIN_INPAINTING_RADIUS: u32 = 5;

/// The post-engine cut: an engine's output is trimmed back to
/// `mask ⊕ ISOLATION_RADIUS`.
pub const ISOLATION_RADIUS: u32 = 5;

/// How far past the lettering's fringe a model rung's hole reaches, in native
/// pixels, where no strong edge stops it ([`crate::fit::Fitted::ink`]).
///
/// Equal to [`ISOLATION_RADIUS`] because that is the number the retry button
/// was already using: a re-run takes the stored applied mask - the first pass's
/// `ink ⊕ ISOLATION_RADIUS` - as its authoritative hole, and on real scans that
/// second pass came back clean where the first left glyph-shaped marks in every
/// balloon (`spikes/gate-probe --lama`, ten regions of one page, before/first/
/// retry crops side by side). The stroke-tight hole measured best on synthetic
/// fixtures ([`crate::engines::lama::model_hole`]); on a scanned page the
/// model reads a hole with a glyph's silhouette and draws a glyph into it.
pub const MODEL_HOLE_MARGIN: u32 = ISOLATION_RADIUS;

/// Rung 1 dilates the mask by this before compositing the denoised crop through
/// the whole dilated mask.
pub const DENOISE_DILATION: u32 = 5;

/// A hard radius-truncated feather, never a Gaussian: a σ=1 Gaussian spreads
/// non-zero alpha 2–3 px and would put rung 1 outside `EDIT_MARGIN`.
pub const DENOISE_FEATHER: u32 = 1;

/// The fidelity contract's margin:
/// `max(isolation_radius, denoise_dilation + feather)`. Derived, not written
/// down as 6, because revision 1 wrote down the isolation radius alone and
/// every rung-1 region then violated the contract by one pixel.
pub const EDIT_MARGIN: u32 = if ISOLATION_RADIUS > DENOISE_DILATION + DENOISE_FEATHER {
    ISOLATION_RADIUS
} else {
    DENOISE_DILATION + DENOISE_FEATHER
};

/// The detector's fixed input, and therefore what "proxy resolution" means:
/// the page is letterboxed into this square.
pub const DETECTOR_INPUT: u32 = 1024;

/// `intra_op_num_threads` for the LaMa session. Measured, not guessed: one
/// thread costs 42% and ten cost 55% against this.
pub const LAMA_INTRA_THREADS: usize = 2;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_edit_margin_covers_the_widest_write_any_rung_makes() {
        assert_eq!(EDIT_MARGIN, 6);
        const { assert!(EDIT_MARGIN >= ISOLATION_RADIUS) };
        const { assert!(EDIT_MARGIN >= DENOISE_DILATION + DENOISE_FEATHER) };
    }
}
