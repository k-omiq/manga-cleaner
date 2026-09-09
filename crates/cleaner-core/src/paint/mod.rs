//! Painting: a stroke, and the patch it becomes.
//!
//! **No new patch kind and no new fields.** A
//! painted stroke leaves the same thing every other rung leaves: an opaque
//! [`Raster`] and a binary [`Mask`]. The soft edge is resolved *here*, before
//! the patch exists - the surface under the stroke is composited, the coverage
//! is blended over it in f32, and what comes out is an ordinary replacement
//! patch. So `composite`, the exporter, `tile.rs`, `history.rs` and the
//! fidelity contract all keep working with nothing changed.
//!
//! **What that costs is written down rather than hidden.** Because the blend
//! happens at commit, an anti-aliased paint patch B over an earlier patch A
//! carries A's pixels inside B's semi-transparent rim. Undoing B is exact;
//! undoing **A** leaves a one-to-two-pixel ghost of A's pixels inside B's rim,
//! and only where the two masks overlap. That is the price of keeping
//! `composite` a replacement and the manifest schema unchanged;
//! true parametric independence would need a recompute pass over the patches
//! whose `order` sits above a deleted one.
//!
//! ## Modes
//!
//! Phase 1 serves `Gray8`, `GrayAlpha8`, `Rgb8` and `Rgba8`. Everything else is
//! [`PaintError::UnsupportedMode`], which the shell reports as
//! `decline.reason.paintUnsupportedMode` rather than promoting the page - no
//! helpful promotion. 16-bit, indexed, bitonal and CMYK are Phase 2.
//!
//! ## Gray
//!
//! A gray page's samples are widened to `(g, g, g)`, blended as RGB, and folded
//! back with **Rec. 709 coefficients applied to the encoded samples** - no sRGB
//! transfer decode, because this crate does not parse the page's profile on the
//! fidelity path. Two properties follow and both are wanted. The fold is
//! *linear in the encoded channels and
//! its weights sum to one*, so it commutes with the source-over blend: a pixel
//! the stroke did not cover comes back bit-identical, and covering a gray page
//! with a neutral colour is exact at every alpha. And a *chromatic* colour on a
//! gray page lands at its 709 luma, which is a defensible answer and not a
//! colour-managed one - the colour-managed path is the eyedropper of Phase 2.
//!
//! Alpha is copied through untouched on `GrayAlpha`/`Rgba`, never produced.

pub mod brush;
pub mod clone_heal;
pub mod plan;
pub mod primitives;

use crate::composite::{composite_region, CompositeError};
use crate::image::{BitDepth, ColorMode, Raster};
use crate::mask::{Mask, Rect};
use crate::patch::Patch;

use brush::{AccumulationView, RasterizableDab, StampOptions, TipShape};
pub use clone_heal::HealMode;
pub use plan::{BrushSpec, StrokePoint};

/// The angle every Phase 1 dab is stamped at: none.
///
/// The kernel takes `cos(-angle)`/`sin(-angle)` precomputed rather than doing
/// trigonometry itself - that is its cross-runtime determinism contract - so a
/// round tip at zero rotation is these two constants and no `cos` call.
const COS_ZERO: f64 = 1.0;
const SIN_ZERO: f64 = -0.0;

/// The slack around a stroke's box, in page pixels. The outermost dab's soft
/// rim has to have surface under it to blend against.
const EDGE_SLACK: f64 = 2.0;

#[derive(Debug, thiserror::Error)]
pub enum PaintError {
    /// The page is not one of Phase 1's four modes. Carries no detail on
    /// purpose: the shell answers one `decline.reason.*` key, and a key is the
    /// whole of what crosses the seam.
    #[error("painting is not supported on a {mode:?}/{depth:?} page")]
    UnsupportedMode { mode: ColorMode, depth: BitDepth },
    /// The stroke covered no pixel of the page - every point off the edge, or a
    /// brush whose dabs all fell outside it. Not an error the user caused;
    /// the caller declines rather than committing an empty patch.
    #[error("the stroke covered nothing")]
    Empty,
    #[error(transparent)]
    Composite(#[from] CompositeError),
}

/// What one painted stroke produced: the applied mask, and the pixels covering
/// its bounds in the page's own mode and depth.
pub struct Painted {
    pub mask: Mask,
    pub pixels: Raster,
    /// How many dabs the planner emitted. Provenance, not geometry - it is the
    /// one number that says how much of the stroke the commit actually walked.
    pub dabs: usize,
}

/// Whether this page can be painted on at all.
pub fn supports(page: &Raster) -> bool {
    page.depth == BitDepth::Eight
        && matches!(
            page.mode,
            ColorMode::Gray | ColorMode::GrayAlpha | ColorMode::Rgb | ColorMode::Rgba
        )
}

fn require_supported(page: &Raster) -> Result<(), PaintError> {
    if supports(page) {
        Ok(())
    } else {
        Err(PaintError::UnsupportedMode { mode: page.mode, depth: page.depth })
    }
}

/* ------------------------------------------------------------------ */
/* The brush                                                           */
/* ------------------------------------------------------------------ */

/// Paint one stroke and answer the patch it makes.
///
/// `patches` are the page's patches; `below` is the exclusive `order` ceiling
/// that says which of them are *under* this stroke. `color` is straight sRGB
/// bytes, as the `#rrggbb` the seam carries.
pub fn render_brush(
    page: &Raster,
    patches: &[Patch],
    below: Option<u32>,
    points: &[StrokePoint],
    spec: &BrushSpec,
    color: [u8; 3],
) -> Result<Painted, PaintError> {
    require_supported(page)?;

    let dabs = plan::plan_stroke(points, spec);
    let Some(window) = window_of(&dabs, page) else { return Err(PaintError::Empty) };

    let surface = composite_region(page, patches, window, below)?;
    let (w, h) = (window.w as usize, window.h as usize);
    let rgba = to_rgba8(&surface);

    let mut acc = vec![0f32; w * h * 4];
    let mut coverage = vec![0f32; w * h];
    let mut view = AccumulationView { color: &mut acc, stroke_mask: &mut coverage, width: w, height: h };
    let options = StampOptions {
        opacity: (spec.opacity / 100.0).clamp(0.0, 1.0),
        hardness: (spec.hardness / 100.0).clamp(0.0, 1.0),
        seed: spec.seed,
        ..StampOptions::default()
    };
    let dab_color = [color[0] as f64 / 255.0, color[1] as f64 / 255.0, color[2] as f64 / 255.0];
    for dab in &dabs {
        let local = RasterizableDab {
            x: dab.x - window.x as f64,
            y: dab.y - window.y as f64,
            ..*dab
        };
        brush::stamp_dab_subpixel(
            &mut view,
            &local,
            TipShape::Analytic,
            dab_color,
            &options,
            COS_ZERO,
            SIN_ZERO,
        );
    }

    // Source-over, straight alpha, over an opaque surface. The accumulation's
    // RGB is premultiplied, so the blend is `src + dst × (1 − a)` with no
    // un-premultiply in between - going through `accumulation_to_image_data`
    // first would quantise the colour to 8 bits before the blend rather than
    // after it.
    let mut blended = rgba.clone();
    for i in 0..(w * h) {
        let a = (acc[i * 4 + 3] as f64).clamp(0.0, 1.0);
        if a <= 0.0 {
            continue;
        }
        for c in 0..3 {
            let src = (acc[i * 4 + c] as f64).clamp(0.0, 1.0) * 255.0;
            let dst = rgba[i * 4 + c] as f64;
            blended[i * 4 + c] = (src + dst * (1.0 - a)).round().clamp(0.0, 255.0) as u8;
        }
    }

    finish(page, window, &blended, &coverage, dabs.len())
}

/* ------------------------------------------------------------------ */
/* Clone and heal                                                      */
/* ------------------------------------------------------------------ */

/// What the clone tool was asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloneMode {
    /// Copy the source pixels straight.
    Clone,
    /// Blend the source's texture into the target's tone.
    Heal(HealMode),
}

/// Clone or heal one stroke.
///
/// `offset` is `target − source` in **native page pixels**, so a dab centred at
/// `t` reads from `t − offset`. **The seam spells the same displacement the
/// other way round** - `gesture.js#cloneOffset` sends `source − strokeStart` and
/// rebuilds the source as `strokeStart + offset` - and the negation happens once,
/// at the parse site (`region::clone_offset`). This function only ever sees the
/// internal direction, so the sign question has exactly one place to be wrong in.
///
/// The window is the union of the target's box and the source's, so the kernel
/// sees both without a second composite: it takes `image` and `source_image` as
/// one buffer.
pub fn render_clone(
    page: &Raster,
    patches: &[Patch],
    below: Option<u32>,
    points: &[StrokePoint],
    spec: &BrushSpec,
    offset: (f64, f64),
    mode: CloneMode,
) -> Result<Painted, PaintError> {
    require_supported(page)?;

    let dabs = plan::plan_stroke(points, spec);
    let Some(target) = window_of(&dabs, page) else { return Err(PaintError::Empty) };
    // The source's box is the target's, moved. Unioned rather than composited
    // twice, because the kernel indexes one buffer for both.
    let source = Rect::new(
        target.x - offset.0.round() as i64,
        target.y - offset.1.round() as i64,
        target.w,
        target.h,
    );
    let window = union_clamped(target, source, page);
    if window.w == 0 || window.h == 0 {
        return Err(PaintError::Empty);
    }

    let surface = composite_region(page, patches, window, below)?;
    let (w, h) = (window.w as usize, window.h as usize);
    let rgba = to_rgba8(&surface);

    // Centres in the window's coordinates, as the flat `[x, y, ...]` pairs the
    // kernel's stroke entry points take.
    let mut targets = Vec::with_capacity(dabs.len() * 2);
    let mut sources = Vec::with_capacity(dabs.len() * 2);
    for dab in &dabs {
        let (tx, ty) = (dab.x - window.x as f64, dab.y - window.y as f64);
        targets.push(tx);
        targets.push(ty);
        sources.push(tx - offset.0);
        sources.push(ty - offset.1);
    }

    let mut scratch = vec![0u8; rgba.len()];
    let mut out = vec![0u8; rgba.len()];
    let size = spec.size.max(1.0);
    let opacity = spec.opacity.clamp(0.0, 100.0);
    let flow = spec.flow.clamp(0.0, 100.0);
    match mode {
        CloneMode::Clone => clone_heal::apply_clone_stroke(
            &rgba, w, h, &rgba, w, h, &targets, &sources, size, opacity, flow, None,
            &mut scratch, &mut out,
        ),
        CloneMode::Heal(heal) => clone_heal::apply_heal_stroke(
            &rgba, w, h, &rgba, w, h, &targets, &sources, heal, size, opacity, flow, None,
            &mut scratch, &mut out,
        ),
    }

    // **The stamp's own footprint, not the pixels that moved.** A clone over a
    // flat area changes nothing measurable and would otherwise leave a mask with
    // holes in it - or no mask at all - which is not what the user drew. The
    // kernel's own pixel walk is the definition, so the mask is exactly the set
    // of pixels the kernel could have written.
    let mut coverage = vec![0f32; w * h];
    for pair in targets.chunks_exact(2) {
        for visit in primitives::collect_brush_pixels(w, h, pair[0], pair[1], size) {
            let at = visit.y as usize * w + visit.x as usize;
            coverage[at] = coverage[at].max(visit.weight as f32);
        }
    }

    finish(page, window, &out, &coverage, dabs.len())
}

/* ------------------------------------------------------------------ */
/* Shared: the window, and the way back to the page's own samples      */
/* ------------------------------------------------------------------ */

/// The stroke's box, grown by the dabs' reach and clamped to the page.
fn window_of(dabs: &[RasterizableDab], page: &Raster) -> Option<Rect> {
    let (x0, y0, x1, y1) = plan::bounds_of(dabs, EDGE_SLACK)?;
    let x0 = x0.floor().clamp(0.0, page.width as f64) as i64;
    let y0 = y0.floor().clamp(0.0, page.height as f64) as i64;
    let x1 = x1.ceil().clamp(x0 as f64, page.width as f64) as i64;
    let y1 = y1.ceil().clamp(y0 as f64, page.height as f64) as i64;
    (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32))
}

fn union_clamped(a: Rect, b: Rect, page: &Raster) -> Rect {
    let x0 = a.x.min(b.x).clamp(0, page.width as i64);
    let y0 = a.y.min(b.y).clamp(0, page.height as i64);
    let x1 = a.right().max(b.right()).clamp(x0, page.width as i64);
    let y1 = a.bottom().max(b.bottom()).clamp(y0, page.height as i64);
    Rect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32)
}

/// The mask is the support of the coverage, and the patch is the blended RGBA
/// cropped to it and folded back into the page's own mode and depth.
fn finish(
    page: &Raster,
    window: Rect,
    blended: &[u8],
    coverage: &[f32],
    dabs: usize,
) -> Result<Painted, PaintError> {
    let (w, h) = (window.w as usize, window.h as usize);
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for y in 0..h {
        for x in 0..w {
            if coverage[y * w + x] > 0.0 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    if x0 == usize::MAX {
        return Err(PaintError::Empty);
    }

    let bounds = Rect::new(
        window.x + x0 as i64,
        window.y + y0 as i64,
        (x1 - x0) as u32,
        (y1 - y0) as u32,
    );
    let mut mask = Mask::empty(bounds);
    let mut pixels = Raster {
        width: bounds.w,
        height: bounds.h,
        mode: page.mode,
        depth: page.depth,
        icc: page.icc.clone(),
        palette: page.palette.clone(),
        trns: page.trns.clone(),
        srgb_intent: page.srgb_intent,
        data: vec![0; crate::composite::blank_len(bounds.w, bounds.h, page)],
    };
    for y in y0..y1 {
        for x in x0..x1 {
            let at = y * w + x;
            let (lx, ly) = ((x - x0) as u32, (y - y0) as u32);
            if coverage[at] > 0.0 {
                mask.set(bounds.x + lx as i64, bounds.y + ly as i64, true);
            }
            write_sample(&mut pixels, lx, ly, &blended[at * 4..at * 4 + 4]);
        }
    }

    Ok(Painted { mask, pixels, dabs })
}

/// A raster in one of Phase 1's four modes, as tightly packed RGBA8 - the only
/// layout the vendored kernels read.
fn to_rgba8(raster: &Raster) -> Vec<u8> {
    let (w, h) = (raster.width as usize, raster.height as usize);
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let at = (y * w + x) * 4;
            let (px, py) = (x as u32, y as u32);
            let s = |c: usize| raster.sample(px, py, c) as u8;
            let (rgb, alpha) = match raster.mode {
                ColorMode::Gray => ([s(0), s(0), s(0)], 255),
                ColorMode::GrayAlpha => ([s(0), s(0), s(0)], s(1)),
                ColorMode::Rgb => ([s(0), s(1), s(2)], 255),
                // Unreachable: `require_supported` ran first. Written as the
                // same shape rather than a panic, so a fourth mode arriving
                // here degrades to RGB rather than killing the edit.
                _ => ([s(0), s(1), s(2)], s(3)),
            };
            out[at] = rgb[0];
            out[at + 1] = rgb[1];
            out[at + 2] = rgb[2];
            out[at + 3] = alpha;
        }
    }
    out
}

/// Rec. 709 luma over the **encoded** samples. Linear in its inputs with
/// weights summing to one, which is what makes the gray round trip exact - see
/// the module documentation.
fn luma709(rgba: &[u8]) -> u16 {
    let y = 0.2126 * rgba[0] as f64 + 0.7152 * rgba[1] as f64 + 0.0722 * rgba[2] as f64;
    y.round().clamp(0.0, 255.0) as u16
}

fn write_sample(pixels: &mut Raster, x: u32, y: u32, rgba: &[u8]) {
    match pixels.mode {
        ColorMode::Gray => pixels.set_sample(x, y, 0, luma709(rgba)),
        ColorMode::GrayAlpha => {
            pixels.set_sample(x, y, 0, luma709(rgba));
            pixels.set_sample(x, y, 1, rgba[3] as u16);
        }
        ColorMode::Rgb => {
            for (channel, value) in rgba.iter().take(3).enumerate() {
                pixels.set_sample(x, y, channel, *value as u16);
            }
        }
        _ => {
            for (channel, value) in rgba.iter().take(4).enumerate() {
                pixels.set_sample(x, y, channel, *value as u16);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::fixtures;

    fn stroke(points: &[(f64, f64)]) -> Vec<StrokePoint> {
        points.iter().map(|(x, y)| StrokePoint { x: *x, y: *y, p: 1.0 }).collect()
    }

    fn hard_brush(size: f64) -> BrushSpec {
        BrushSpec {
            size,
            hardness: 100.0,
            flow: 100.0,
            opacity: 100.0,
            spacing: 10.0,
            pressure_size: false,
            pressure_opacity: false,
            seed: 7,
        }
    }

    /// The four Phase 1 modes are served and every other one is refused by
    /// name rather than promoted. This is the whole of `decline.reason
    /// .paintUnsupportedMode`'s population, computed from the fixtures rather
    /// than typed out, so a new fixture mode lands on one side or the other
    /// without this test being edited.
    #[test]
    fn phase_one_serves_four_modes_and_declines_the_rest() {
        for fixture in fixtures::all() {
            let page = &fixture.raster;
            let expected = page.depth == BitDepth::Eight
                && matches!(
                    page.mode,
                    ColorMode::Gray | ColorMode::GrayAlpha | ColorMode::Rgb | ColorMode::Rgba
                );
            assert_eq!(supports(page), expected, "{}", fixture.name);

            let painted = render_brush(
                page,
                &[],
                None,
                &stroke(&[(4.0, 4.0), (10.0, 6.0)]),
                &hard_brush(5.0),
                [255, 0, 0],
            );
            match painted {
                Err(PaintError::UnsupportedMode { .. }) => assert!(!expected, "{}", fixture.name),
                other => assert!(expected && other.is_ok(), "{}", fixture.name),
            }
        }
    }

    /// **The patch is a replacement and the mask is its support.** Both halves
    /// matter: pixels covering `mask.bounds`, so `Patch::is_well_formed` holds
    /// and the compositor can place it, and a mask that is not empty, so
    /// something actually changes.
    #[test]
    fn a_stroke_becomes_a_well_formed_patch_in_the_pages_own_mode() {
        for name in ["l8", "rgb8"] {
            let page = fixtures::by_name(name).raster;
            let painted = render_brush(
                &page,
                &[],
                None,
                &stroke(&[(3.0, 3.0), (12.0, 9.0)]),
                &hard_brush(4.0),
                [12, 200, 40],
            )
            .unwrap();
            assert_eq!(painted.pixels.width, painted.mask.bounds.w, "{name}");
            assert_eq!(painted.pixels.height, painted.mask.bounds.h, "{name}");
            assert_eq!(painted.pixels.mode, page.mode, "{name}");
            assert_eq!(painted.pixels.depth, page.depth, "{name}");
            assert!(!painted.mask.is_empty(), "{name}");
            assert!(painted.dabs >= 2, "{name}");
        }
    }

    /// A fully opaque hard dab lands the colour it was given, and on a gray
    /// page it lands that colour's 709 luma. The centre pixel is the one the
    /// blend cannot have softened.
    #[test]
    fn an_opaque_hard_dab_writes_the_colour_it_was_asked_for() {
        let page = fixtures::by_name("rgb8").raster;
        let painted =
            render_brush(&page, &[], None, &stroke(&[(8.0, 8.0)]), &hard_brush(6.0), [10, 20, 30])
                .unwrap();
        let (lx, ly) = ((8 - painted.mask.bounds.x) as u32, (8 - painted.mask.bounds.y) as u32);
        assert_eq!(
            [
                painted.pixels.sample(lx, ly, 0),
                painted.pixels.sample(lx, ly, 1),
                painted.pixels.sample(lx, ly, 2)
            ],
            [10, 20, 30]
        );

        let gray = fixtures::by_name("l8").raster;
        let painted =
            render_brush(&gray, &[], None, &stroke(&[(8.0, 8.0)]), &hard_brush(6.0), [10, 20, 30])
                .unwrap();
        let (lx, ly) = ((8 - painted.mask.bounds.x) as u32, (8 - painted.mask.bounds.y) as u32);
        assert_eq!(painted.pixels.sample(lx, ly, 0), luma709(&[10, 20, 30, 255]));
    }

    /// **The gray fold commutes with the blend.** A neutral colour painted at
    /// any coverage onto a gray page must not tint anything, and a pixel the
    /// stroke did not cover must come back the sample it went in as. This is
    /// the property the module's Rec. 709-on-encoded-samples choice buys, and
    /// the reason the choice is not simply "whatever luma".
    #[test]
    fn a_neutral_colour_on_a_gray_page_leaves_uncovered_samples_alone() {
        let page = fixtures::by_name("l8").raster;
        let soft = BrushSpec { hardness: 0.0, ..hard_brush(7.0) };
        let painted =
            render_brush(&page, &[], None, &stroke(&[(9.0, 9.0)]), &soft, [128, 128, 128]).unwrap();
        let bounds = painted.mask.bounds;
        for y in 0..bounds.h {
            for x in 0..bounds.w {
                let (px, py) = (bounds.x as u32 + x, bounds.y as u32 + y);
                if !painted.mask.contains(px as i64, py as i64) {
                    assert_eq!(
                        painted.pixels.sample(x, y, 0),
                        page.sample(px, py, 0),
                        "({px},{py}) outside the mask moved"
                    );
                }
            }
        }
    }

    /// Alpha is copied, never produced. A stroke over an `RGBA` page carries
    /// the source's alpha into the patch unchanged, whatever the brush's own
    /// opacity was.
    #[test]
    fn alpha_is_carried_through_rather_than_painted() {
        let page = fixtures::by_name("rgba8").raster;
        let spec = BrushSpec { opacity: 60.0, ..hard_brush(6.0) };
        let painted =
            render_brush(&page, &[], None, &stroke(&[(6.0, 6.0), (11.0, 11.0)]), &spec, [0, 0, 0])
                .unwrap();
        let bounds = painted.mask.bounds;
        for y in 0..bounds.h {
            for x in 0..bounds.w {
                assert_eq!(
                    painted.pixels.sample(x, y, 3),
                    page.sample(bounds.x as u32 + x, bounds.y as u32 + y, 3)
                );
            }
        }
    }

    /// An empty stroke is `Empty` rather than a patch with nothing in it, and a
    /// stroke entirely off the page is the same answer - a caller that
    /// committed either would put a zero-area record in the manifest.
    #[test]
    fn a_stroke_that_covers_nothing_is_refused_rather_than_committed() {
        let page = fixtures::by_name("l8").raster;
        assert!(matches!(
            render_brush(&page, &[], None, &[], &hard_brush(5.0), [0, 0, 0]),
            Err(PaintError::Empty)
        ));
        assert!(matches!(
            render_brush(&page, &[], None, &stroke(&[(-900.0, -900.0)]), &hard_brush(5.0), [0, 0, 0]),
            Err(PaintError::Empty)
        ));
    }

    /// Clone copies real pixels from the offset source, and the mask is the
    /// stamp's own footprint rather than the pixels that happened to move - a
    /// clone onto identical tone must still leave the patch the user drew.
    #[test]
    fn clone_reads_the_offset_source_and_masks_the_stamps_footprint() {
        let page = fixtures::by_name("rgb8").raster;
        let painted = render_clone(
            &page,
            &[],
            None,
            &stroke(&[(10.0, 10.0), (13.0, 10.0)]),
            &hard_brush(5.0),
            (6.0, 0.0),
            CloneMode::Clone,
        )
        .unwrap();
        assert!(!painted.mask.is_empty());
        assert_eq!(painted.pixels.width, painted.mask.bounds.w);
        assert_eq!(painted.pixels.mode, page.mode);

        // A source outside the page is skipped by the kernel rather than read
        // out of bounds, so the whole stroke is still a patch.
        let far = render_clone(
            &page,
            &[],
            None,
            &stroke(&[(2.0, 2.0)]),
            &hard_brush(5.0),
            (40.0, 40.0),
            CloneMode::Clone,
        )
        .unwrap();
        assert!(!far.mask.is_empty());
    }

    /// Heal takes the soft mode the seam's `heal` word maps to, and answers a
    /// patch in the page's mode like every other rung.
    #[test]
    fn heal_runs_in_soft_mode_and_answers_the_pages_own_samples() {
        let page = fixtures::by_name("l8").raster;
        let painted = render_clone(
            &page,
            &[],
            None,
            &stroke(&[(9.0, 9.0), (12.0, 12.0)]),
            &hard_brush(4.0),
            (-5.0, -5.0),
            CloneMode::Heal(HealMode::Soft),
        )
        .unwrap();
        assert_eq!(painted.pixels.mode, ColorMode::Gray);
        assert_eq!(painted.pixels.depth, BitDepth::Eight);
        assert!(!painted.mask.is_empty());
    }
}
