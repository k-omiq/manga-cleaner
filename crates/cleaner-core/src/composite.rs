//! `raw + ordered visible patches`, and nothing else.
//!
//! This function is where the fidelity contract stops being a promise and
//! becomes a property: it writes a sample only where a patch's applied mask
//! says so, so the set of changed pixels is a subset of `union(masks)` **by
//! construction**, whatever an engine returned.
//!
//! This is also the limit of what the fidelity test can prove - the subset
//! property holds no matter how bad the model output was, and interior
//! quality belongs to the drift gate and the reviewer.

use crate::image::{ColorMode, Raster};
use crate::mask::{Mask, Rect};
use crate::patch::Patch;

#[derive(Debug, thiserror::Error)]
pub enum CompositeError {
    #[error("patch {id} is {pw}×{ph} but its mask bounds are {mw}×{mh}")]
    Malformed { id: String, pw: u32, ph: u32, mw: u32, mh: u32 },
    #[error("patch {id} is {patch:?}/{patch_depth:?} but the page is {page:?}/{page_depth:?}")]
    ModeMismatch {
        id: String,
        patch: ColorMode,
        patch_depth: crate::image::BitDepth,
        page: ColorMode,
        page_depth: crate::image::BitDepth,
    },
}

/// The displayed and exported page.
///
/// Alpha is copied through unmodified - mask fitting and fills operate on
/// colour channels only, and §2's third clarification says alpha never enters a
/// ring statistic. So an `LA`/`RGBA` patch carries the source's alpha in its
/// buffer and this copies it like any other sample; nothing composites
/// *through* alpha, because a patch is a replacement, not a layer blend.
pub fn composite(page: &Raster, patches: &[Patch]) -> Result<Raster, CompositeError> {
    let mut out = page.clone();
    let whole = Rect::new(0, 0, page.width, page.height);
    for patch in ordered_visible(patches, None) {
        stamp(&mut out, page, patch, whole)?;
    }
    Ok(out)
}

/// The same page, over one rectangle of it.
///
/// **A Phase 1 deliverable and not an optimisation.** A brush
/// stroke needs the surface *under* it to blend its soft edge against, and
/// [`composite`] clones and rebuilds the whole page: on a 4000×6000 scan that is
/// unusable once per stroke. `Raster::rows` is not an answer either - it crops
/// row ranges, because a row range is a contiguous slice at every depth, and a
/// stroke's box is not a row range.
///
/// `rect` is clamped to the page, and the answer is a raster of the clamped
/// rectangle in the page's own mode and depth - never a promotion, so the
/// result can be handed back to `set_sample` as-is.
///
/// `below` is an **exclusive** order ceiling: `Some(n)` composites the patches
/// whose `order < n`, which is what "the surface under this stroke" means for a
/// patch that is about to take order `n`. `None` composites all of them.
///
/// The ordering rule is [`composite`]'s, shared rather than restated, so the two
/// cannot drift apart into two different pages.
pub fn composite_region(
    page: &Raster,
    patches: &[Patch],
    rect: Rect,
    below: Option<u32>,
) -> Result<Raster, CompositeError> {
    let window = clamp_to(rect, page);
    let mut out = crop(page, window);
    for patch in ordered_visible(patches, below) {
        stamp(&mut out, page, patch, window)?;
    }
    Ok(out)
}

/// The visible patches, in the order they composite, optionally cut off above
/// an exclusive `order`.
pub(crate) fn ordered_visible(patches: &[Patch], below: Option<u32>) -> Vec<&Patch> {
    let mut ordered: Vec<&Patch> = patches
        .iter()
        .filter(|p| p.visible)
        .filter(|p| below.is_none_or(|ceiling| p.order < ceiling))
        .collect();
    // Stable by `order`, then by id, so two patches with the same order
    // composite the same way on every run - the region order must be
    // fixed, and "fixed" has to survive a tie.
    ordered.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.id.cmp(&b.id)));
    ordered
}

/// A rectangle, inside the page. A window may be asked for off the edge; a crop
/// may not be taken there.
fn clamp_to(rect: Rect, page: &Raster) -> Rect {
    let x0 = rect.x.clamp(0, page.width as i64);
    let y0 = rect.y.clamp(0, page.height as i64);
    let x1 = rect.right().clamp(x0, page.width as i64);
    let y1 = rect.bottom().clamp(y0, page.height as i64);
    Rect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32)
}

/// One rectangle of a raster, as a raster of its own.
///
/// Sample by sample rather than by slice, because an arbitrary `x` lands
/// mid-byte at every sub-byte depth - the exact reason `Raster::rows` exists
/// and stops at rows. Everything but the size travels unchanged, so the crop's
/// mode, depth, palette and `tRNS` are the page's.
fn crop(page: &Raster, rect: Rect) -> Raster {
    let mut out = Raster {
        width: rect.w,
        height: rect.h,
        mode: page.mode,
        depth: page.depth,
        icc: page.icc.clone(),
        palette: page.palette.clone(),
        trns: page.trns.clone(),
        srgb_intent: page.srgb_intent,
        data: vec![0; blank_len(rect.w, rect.h, page)],
    };
    let samples = page.mode.samples();
    for y in 0..rect.h {
        for x in 0..rect.w {
            for channel in 0..samples {
                let value = page.sample(rect.x as u32 + x, rect.y as u32 + y, channel);
                out.set_sample(x, y, channel, value);
            }
        }
    }
    out
}

/// The byte length of a `w × h` buffer in a page's mode and depth, padded to a
/// byte boundary per row exactly as `Raster::stride` reads it.
pub(crate) fn blank_len(w: u32, h: u32, like: &Raster) -> usize {
    let bits = w as usize * like.mode.samples() * like.depth.bits() as usize;
    bits.div_ceil(8) * h as usize
}

/// Write one patch into `out`, where `out` covers `window` of the page.
fn stamp(
    out: &mut Raster,
    page: &Raster,
    patch: &Patch,
    window: Rect,
) -> Result<(), CompositeError> {
    if !patch.is_well_formed() {
        return Err(CompositeError::Malformed {
            id: patch.id.clone(),
            pw: patch.pixels.width,
            ph: patch.pixels.height,
            mw: patch.mask.bounds.w,
            mh: patch.mask.bounds.h,
        });
    }
    if patch.pixels.mode != page.mode || patch.pixels.depth != page.depth {
        return Err(CompositeError::ModeMismatch {
            id: patch.id.clone(),
            patch: patch.pixels.mode,
            patch_depth: patch.pixels.depth,
            page: page.mode,
            page_depth: page.depth,
        });
    }

    let bounds = patch.mask.bounds;
    let samples = page.mode.samples();
    for y in bounds.y.max(window.y)..bounds.bottom().min(window.bottom()) {
        for x in bounds.x.max(window.x)..bounds.right().min(window.right()) {
            if !patch.mask.contains(x, y) {
                continue;
            }
            if x < 0 || y < 0 || x >= page.width as i64 || y >= page.height as i64 {
                continue;
            }
            let (lx, ly) = ((x - bounds.x) as u32, (y - bounds.y) as u32);
            let (ox, oy) = ((x - window.x) as u32, (y - window.y) as u32);
            for channel in 0..samples {
                out.set_sample(ox, oy, channel, patch.pixels.sample(lx, ly, channel));
            }
        }
    }
    Ok(())
}

/// Every pixel where two rasters of the same geometry differ.
///
/// The fidelity test's left-hand side. Returned as coordinates rather than a
/// count so a failure names the pixel it failed on - a count tells you the test
/// broke and nothing about where.
pub fn changed_pixels(before: &Raster, after: &Raster) -> Vec<(u32, u32)> {
    if !before.same_geometry(after) {
        // A geometry change is a total change; reporting it as "every pixel"
        // keeps the caller's assertion honest rather than silently empty.
        return (0..after.height).flat_map(|y| (0..after.width).map(move |x| (x, y))).collect();
    }
    let samples = before.mode.samples();
    let mut changed = Vec::new();
    for y in 0..before.height {
        for x in 0..before.width {
            if (0..samples).any(|c| before.sample(x, y, c) != after.sample(x, y, c)) {
                changed.push((x, y));
            }
        }
    }
    changed
}

/// `dilate(union(applied masks), edit_margin)` - the right-hand side of the
/// contract. Built here so the test and the exporter agree on what it means.
pub fn permitted_region(patches: &[Patch], page_w: u32, page_h: u32) -> Mask {
    let masks: Vec<&Mask> = patches.iter().filter(|p| p.visible).map(|p| &p.mask).collect();
    if masks.is_empty() {
        return Mask::empty(Rect::new(0, 0, 0, 0));
    }
    Mask::union(&masks, page_w, page_h).dilated(crate::constants::EDIT_MARGIN, page_w, page_h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::{BitDepth, fixtures};
    use crate::patch::{Engine, Provenance};

    fn provenance() -> Provenance {
        Provenance {
            engine: Engine::Fill,
            engine_version: "test".into(),
            model_sha256: None,
            execution_provider: "cpu".into(),
            params_snapshot: "{}".into(),
            mask_sha256: "0".repeat(64),
            source_sha256: "0".repeat(64),
            cloud: None,
            created: 0,
        }
    }

    /// A patch whose pixel buffer differs from the page **everywhere in its
    /// bbox**, but whose mask covers only the middle. If the compositor wrote
    /// the buffer rather than the masked part of it, the subset assertion
    /// fails - which is the only reason to build the fixture this way.
    fn synthetic_patch(page: &Raster, bounds: Rect) -> Patch {
        let mut pixels = Raster {
            width: bounds.w,
            height: bounds.h,
            mode: page.mode,
            depth: page.depth,
            icc: None,
            palette: None,
            trns: None,
            srgb_intent: None,
            data: vec![0; {
                let bits = bounds.w as usize * page.mode.samples() * page.depth.bits() as usize;
                bits.div_ceil(8) * bounds.h as usize
            }],
        };
        let ceiling = match page.depth {
            BitDepth::Sixteen => u16::MAX,
            other => (1u16 << other.bits()) - 1,
        };
        for y in 0..bounds.h {
            for x in 0..bounds.w {
                for channel in 0..page.mode.samples() {
                    let source = page.sample((bounds.x as u32) + x, (bounds.y as u32) + y, channel);
                    // Guaranteed different, and inside the depth's range for
                    // indexed sources too - where a value past the palette
                    // would not survive a round-trip.
                    let value = if page.mode == ColorMode::Indexed {
                        let entries = (page.palette.as_ref().map_or(1, |p| p.len() / 3)) as u16;
                        (source + 1) % entries
                    } else {
                        ceiling - source
                    };
                    pixels.set_sample(x, y, channel, value);
                }
            }
        }

        let mut mask = Mask::empty(bounds);
        for y in bounds.y + 1..bounds.bottom() - 1 {
            for x in bounds.x + 1..bounds.right() - 1 {
                mask.set(x, y, true);
            }
        }

        Patch { id: "p1".into(), ink: mask.clone(), mask, pixels, order: 0, visible: true, provenance: provenance() }
    }

    #[test]
    fn nothing_changes_under_an_empty_patch_set() {
        for fixture in fixtures::all() {
            let out = composite(&fixture.raster, &[]).unwrap();
            assert!(changed_pixels(&fixture.raster, &out).is_empty(), "{}", fixture.name);
        }
    }

    /// The half of the fidelity test that passthrough cannot satisfy with
    /// `cp`, on every fixture: changed pixels is a subset of
    /// dilate(union(masks), edit_margin).
    #[test]
    fn a_non_empty_patch_set_changes_only_what_it_is_allowed_to() {
        for fixture in fixtures::all() {
            let page = &fixture.raster;
            let bounds = Rect::new(8, 8, 24, 20);
            let patch = synthetic_patch(page, bounds);
            let out = composite(page, std::slice::from_ref(&patch)).unwrap();

            let changed = changed_pixels(page, &out);
            assert!(!changed.is_empty(), "{}: the patch changed nothing", fixture.name);

            let permitted = permitted_region(std::slice::from_ref(&patch), page.width, page.height);
            for (x, y) in &changed {
                assert!(
                    permitted.contains(*x as i64, *y as i64),
                    "{}: ({x}, {y}) changed outside the permitted region",
                    fixture.name
                );
            }

            // Tighter than the contract, and true of this compositor: it writes
            // only inside the mask itself, never into the margin. If this ever
            // fails while the assertion above passes, an engine's output has
            // started leaking and the margin is absorbing it.
            for (x, y) in &changed {
                assert!(
                    patch.mask.contains(*x as i64, *y as i64),
                    "{}: ({x}, {y}) is in the margin but outside the mask",
                    fixture.name
                );
            }
        }
    }

    #[test]
    fn an_invisible_patch_writes_nothing() {
        let page = fixtures::by_name("rgb8").raster;
        let mut patch = synthetic_patch(&page, Rect::new(8, 8, 24, 20));
        patch.visible = false;
        let out = composite(&page, &[patch]).unwrap();
        assert!(changed_pixels(&page, &out).is_empty());
    }

    #[test]
    fn later_patches_win_and_ties_break_deterministically() {
        let page = fixtures::by_name("l8").raster;
        let bounds = Rect::new(8, 8, 8, 8);

        let mut first = synthetic_patch(&page, bounds);
        first.id = "a".into();
        let mut second = synthetic_patch(&page, bounds);
        second.id = "b".into();
        for y in 0..bounds.h {
            for x in 0..bounds.w {
                second.pixels.set_sample(x, y, 0, 200);
            }
        }

        let forwards = composite(&page, &[first.clone(), second.clone()]).unwrap();
        let backwards = composite(&page, &[second, first]).unwrap();
        assert_eq!(forwards.data, backwards.data, "argument order changed the result");
        assert_eq!(forwards.sample(10, 10, 0), 200, "the later id did not win the tie");
    }

    /* -- the windowed composite ---------------------------------------- */

    /// **The window is the page, cropped, and nothing else.** Stated over every
    /// fixture mode and depth, including the sub-byte ones where an arbitrary
    /// `x` lands mid-byte - which is the exact reason `Raster::rows` stops at
    /// rows and this had to be written rather than reached for.
    ///
    /// A separate implementation that merely *usually* agrees with `composite`
    /// is two pages, and the fidelity guarantee is stated over one.
    #[test]
    fn a_window_equals_the_full_composite_cropped_to_it() {
        for fixture in fixtures::all() {
            let page = &fixture.raster;
            let patch = synthetic_patch(page, Rect::new(8, 8, 24, 20));
            let patches = std::slice::from_ref(&patch);
            let whole = composite(page, patches).unwrap();

            // One window inside the patch, one straddling its corner, one
            // entirely off it, and one that is the whole page.
            for rect in [
                Rect::new(12, 12, 6, 5),
                Rect::new(4, 4, 12, 12),
                Rect::new(40, 4, 10, 10),
                Rect::new(0, 0, page.width, page.height),
            ] {
                let window = composite_region(page, patches, rect, None).unwrap();
                assert_eq!((window.width, window.height), (rect.w, rect.h), "{}", fixture.name);
                assert_eq!(window.mode, page.mode, "{}", fixture.name);
                assert_eq!(window.depth, page.depth, "{}", fixture.name);
                for y in 0..rect.h {
                    for x in 0..rect.w {
                        for c in 0..page.mode.samples() {
                            assert_eq!(
                                window.sample(x, y, c),
                                whole.sample(rect.x as u32 + x, rect.y as u32 + y, c),
                                "{} at ({x}, {y}) channel {c} of {rect:?}",
                                fixture.name
                            );
                        }
                    }
                }
            }
        }
    }

    /// A window off the edge is clamped rather than refused: a brush stroke
    /// that runs past the page is an ordinary gesture, and the answer to it is
    /// the part of the page it actually crossed.
    #[test]
    fn a_window_off_the_page_is_clamped_to_what_is_there() {
        let page = fixtures::by_name("rgb8").raster;
        let clipped = composite_region(&page, &[], Rect::new(-10, -10, 20, 20), None).unwrap();
        assert_eq!((clipped.width, clipped.height), (10, 10));
        assert_eq!(clipped.sample(0, 0, 0), page.sample(0, 0, 0));

        let outside = composite_region(&page, &[], Rect::new(500, 500, 8, 8), None).unwrap();
        assert_eq!((outside.width, outside.height), (0, 0));
    }

    /// **The order ceiling is exclusive, and it is what "the surface under this
    /// stroke" means.** A patch about to take order `n` blends over the patches
    /// below it and not over itself - otherwise re-running a stroke would paint
    /// over its own previous commit and compound every time.
    #[test]
    fn an_order_ceiling_composites_only_what_is_under_it() {
        let page = fixtures::by_name("l8").raster;
        let bounds = Rect::new(8, 8, 8, 8);
        let flat = |id: &str, order: u32, value: u16| {
            let mut patch = synthetic_patch(&page, bounds);
            patch.id = id.into();
            patch.order = order;
            for y in 0..bounds.h {
                for x in 0..bounds.w {
                    patch.pixels.set_sample(x, y, 0, value);
                }
            }
            patch
        };
        let patches = [flat("under", 0, 30), flat("over", 1, 200)];
        let rect = Rect::new(9, 9, 4, 4);

        let all = composite_region(&page, &patches, rect, None).unwrap();
        assert_eq!(all.sample(1, 1, 0), 200);

        let below = composite_region(&page, &patches, rect, Some(1)).unwrap();
        assert_eq!(below.sample(1, 1, 0), 30, "the ceiling let the patch at its own order in");

        let none = composite_region(&page, &patches, rect, Some(0)).unwrap();
        assert_eq!(none.sample(1, 1, 0), page.sample(10, 10, 0), "a zero ceiling is the raw page");
    }

    #[test]
    fn a_patch_in_the_wrong_mode_is_refused_rather_than_converted() {
        let page = fixtures::by_name("rgb8").raster;
        let mut patch = synthetic_patch(&page, Rect::new(8, 8, 8, 8));
        patch.pixels = fixtures::by_name("l8").raster;
        patch.pixels.width = 8;
        patch.pixels.height = 8;
        assert!(matches!(
            composite(&page, &[patch]),
            Err(CompositeError::ModeMismatch { .. })
        ));
    }
}
