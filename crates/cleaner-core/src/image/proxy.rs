//! Preview proxies: the only picture the interface is allowed to hold.
//!
//! Rule 7 - *the UI holds proxies, never the strip*:
//!
//! > Previews cap the **short** edge at 1024 and tile the long axis - capping
//! > the long edge would render an 800×20000 page as a 40×1024 sliver.
//!
//! Both halves matter, and the second is the one that gets dropped. Capping the
//! short edge alone still leaves an 800×20000 webtoon segment whole: 16 MB over
//! the protocol and about 64 MB of decoded bitmap sitting in the webview for one
//! page, which the budget does not have and which a column of them
//! multiplies. So the long axis is cut into
//! tiles, each one an independent response the browser fetches, decodes and
//! evicts on its own.
//!
//! ## A cap is a cap, never an enlargement
//!
//! `detect::letterbox` scales a small page **up**, because the detector's input
//! is a fixed square and upstream trained it that way. This does the
//! opposite and the difference is deliberate: a preview of a 400×600 page
//! is 400×600. Enlarging
//! it would spend bytes inventing pixels the scan does not have.
//!
//! ## What a proxy is not
//!
//! It is not the page. A proxy is 8-bit, in a mode the browser can draw, and
//! that is a **display** conversion of exactly the kind
//! [`super::convert`] exists for - the same standing this module's sibling
//! `tile::encode_for_display` has when it flattens CMYK. A 16-bit page's proxy
//! is 8-bit and its 16-bit samples are untouched on disk; an indexed page's
//! proxy is RGB and the palette is untouched on disk; the fidelity contract
//! is stated over the exported file, never over what a webview drew.
//! Nothing here is ever written to an output file.
//!
//! Alpha is carried, and averaged **premultiplied**, because a plain average of
//! colour across a transparent edge drags the fringe toward whatever happens to
//! be stored under `a = 0`.

use super::{BitDepth, ColorMode, Raster};

/// The short edge's ceiling, from rule 7. The rule's number,
/// not a measurement: it is the resolution the interface draws at, and the
/// point of the rule is which edge it applies to.
pub const PROXY_SHORT_EDGE: u32 = 1024;

/// How much of the long axis one tile covers, in **proxy** pixels.
///
/// **Provisional.** The number that decides this
/// is how many bytes WebView2's UI thread can absorb per response: it raises
/// `WebResourceRequested` on that thread and pauses page load while the handler
/// runs, and the maintainer benchmark cited is ~5 ms on macOS against
/// **~200 ms on Windows for 10 MB**. That measurement
/// has never been taken on a Windows machine here - this is open, in this
/// phase, for exactly that, and a one-machine number written down as a
/// general one is exactly the mistake to avoid. So this
/// is not tuned; it is *bounded*: at the 1024 short edge a tile is at most
/// 1024×2048, which is the next power of two up from the cap and keeps one
/// response to a few megabytes of RGB8 before compression. Moving it is a
/// one-constant change on this side and on `src/lib/api/tile.js`'s, and the
/// URL's version token folds both constants in so a change cannot leave a
/// differently-cut tile in an immutable cache.
pub const PROXY_TILE_LONG_EDGE: u32 = 2048;

/// One page's proxy: its size, and how the long axis is cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProxyPlan {
    pub page_w: u32,
    pub page_h: u32,
    pub proxy_w: u32,
    pub proxy_h: u32,
    /// Whether the long axis - the one that is tiled - is `y`.
    pub vertical: bool,
    /// How many tiles the long axis is cut into. Never zero.
    pub tiles: usize,
}

/// One tile's rectangle, in **proxy** pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl ProxyPlan {
    /// The plan for a page of these dimensions.
    ///
    /// Integer arithmetic throughout, so the frontend's own copy of this
    /// (`src/lib/api/tile.js#proxyPlan`) can agree exactly rather than nearly:
    /// the two are separate implementations of one rule, and a tile count that
    /// disagreed by one would be a `404` on the last tile of a page.
    pub fn for_page(page_w: u32, page_h: u32) -> ProxyPlan {
        let page_w = page_w.max(1);
        let page_h = page_h.max(1);
        let short = page_w.min(page_h);
        let long = page_w.max(page_h);

        let proxy_short = short.min(PROXY_SHORT_EDGE);
        let proxy_long = if proxy_short == short {
            long
        } else {
            let scaled = (long as u64 * proxy_short as u64 + short as u64 / 2) / short as u64;
            (scaled.max(1) as u32).max(proxy_short)
        };

        let vertical = page_h >= page_w;
        let (proxy_w, proxy_h) =
            if vertical { (proxy_short, proxy_long) } else { (proxy_long, proxy_short) };
        let tiles = (proxy_long.div_ceil(PROXY_TILE_LONG_EDGE) as usize).max(1);

        ProxyPlan { page_w, page_h, proxy_w, proxy_h, vertical, tiles }
    }

    /// The plan for this raster.
    pub fn of(page: &Raster) -> ProxyPlan {
        ProxyPlan::for_page(page.width, page.height)
    }

    /// Whether the proxy is the page at 1:1 in one piece - the case where a
    /// tile is a copy and the caller can serve the source file instead.
    pub fn is_whole_page(&self) -> bool {
        self.tiles == 1 && self.proxy_w == self.page_w && self.proxy_h == self.page_h
    }

    /// Tile `index`, or `None` past the end. The last tile along the axis is
    /// short whenever the axis does not divide evenly.
    pub fn tile(&self, index: usize) -> Option<TileRect> {
        if index >= self.tiles {
            return None;
        }
        let start = index as u32 * PROXY_TILE_LONG_EDGE;
        Some(if self.vertical {
            TileRect {
                x: 0,
                y: start,
                w: self.proxy_w,
                h: PROXY_TILE_LONG_EDGE.min(self.proxy_h - start),
            }
        } else {
            TileRect {
                x: start,
                y: 0,
                w: PROXY_TILE_LONG_EDGE.min(self.proxy_w - start),
                h: self.proxy_h,
            }
        })
    }
}

/// One tile of `page`'s proxy, or `None` for a tile the plan does not have -
/// which includes every tile of a raster with no pixels in it.
///
/// Only the tile's own source rows are read, so a 20 000 px segment's proxy
/// costs one tile's worth of output buffer rather than a whole second copy of
/// the page. (The page itself is already decoded by the caller - the *page* is
/// the unit here, not the strip, which is the distinction rule 1 draws.)
///
/// A zero dimension is refused here rather than clamped, because
/// [`ProxyPlan::for_page`] has already clamped it: the plan of a 0×0 raster is
/// a 1×1 proxy, and sampling that plan's one pixel reads row 0 of a raster that
/// has no row 0. Nothing [`super::decode`] produces has a zero dimension -
/// neither PNG nor TIFF can express one - but this takes any `&Raster`, and a
/// caller that builds one is entitled to an answer rather than a panic.
pub fn proxy_tile(page: &Raster, index: usize) -> Option<Raster> {
    if page.width == 0 || page.height == 0 {
        return None;
    }
    let plan = ProxyPlan::of(page);
    let rect = plan.tile(index)?;
    let mode = proxy_mode(page);
    let samples = mode.samples();
    let has_alpha = mode.alpha_channel().is_some();

    let mut out = Raster {
        width: rect.w,
        height: rect.h,
        mode,
        depth: BitDepth::Eight,
        // A profile describes the samples, and the samples are still the
        // page's colours - only their resolution changed. CMYK is the one mode
        // whose samples do not survive, so its profile does not either.
        icc: if page.mode == ColorMode::Cmyk { None } else { page.icc.clone() },
        palette: None,
        trns: None,
        srgb_intent: if page.mode == ColorMode::Cmyk { None } else { page.srgb_intent },
        data: vec![0; rect.w as usize * rect.h as usize * samples],
    };

    for ty in 0..rect.h {
        let (y0, y1) = span(rect.y + ty, plan.proxy_h, plan.page_h);
        for tx in 0..rect.w {
            let (x0, x1) = span(rect.x + tx, plan.proxy_w, plan.page_w);

            // Premultiplied accumulation: colour weighted by coverage, and the
            // coverage kept so it can be divided back out.
            let mut colour = [0u64; 3];
            let mut alpha = 0u64;
            let mut count = 0u64;
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let rgb = page.rgb8_pixel(sx, sy);
                    let a = alpha8(page, sx, sy) as u64;
                    for (slot, value) in colour.iter_mut().zip(rgb) {
                        *slot += value as u64 * a;
                    }
                    alpha += a;
                    count += 1;
                }
            }

            // Fully transparent covers nothing, so there is no colour to
            // recover and black is as good an answer as any other.
            let value =
                |channel: usize| -> u16 { colour[channel].checked_div(alpha).unwrap_or(0) as u16 };
            match mode {
                ColorMode::Gray => out.set_sample(tx, ty, 0, value(0)),
                ColorMode::GrayAlpha => {
                    out.set_sample(tx, ty, 0, value(0));
                    out.set_sample(tx, ty, 1, (alpha / count) as u16);
                }
                _ => {
                    for channel in 0..3 {
                        out.set_sample(tx, ty, channel, value(channel));
                    }
                    if has_alpha {
                        out.set_sample(tx, ty, 3, (alpha / count) as u16);
                    }
                }
            }
        }
    }
    Some(out)
}

/// What a page of this mode is drawn as. Gray stays gray - a manga page tripled
/// into RGB is three times the bytes over the protocol whose throughput is
/// the whole constraint - and everything else that a browser cannot draw
/// directly becomes RGB.
fn proxy_mode(page: &Raster) -> ColorMode {
    match page.mode {
        ColorMode::Gray => ColorMode::Gray,
        ColorMode::GrayAlpha => ColorMode::GrayAlpha,
        ColorMode::Rgba => ColorMode::Rgba,
        // A palette with a `tRNS` chunk carries transparency that would
        // silently become opaque in RGB.
        ColorMode::Indexed if page.trns.is_some() => ColorMode::Rgba,
        ColorMode::Rgb | ColorMode::Indexed | ColorMode::Cmyk => ColorMode::Rgb,
    }
}

/// One pixel's alpha as 0..=255. Opaque is the answer for every mode that
/// carries none, which is what makes the averaging one code path.
fn alpha8(page: &Raster, x: u32, y: u32) -> u8 {
    match page.mode {
        ColorMode::GrayAlpha => page.sample8(x, y, 1),
        ColorMode::Rgba => page.sample8(x, y, 3),
        ColorMode::Indexed => match &page.trns {
            Some(trns) => trns.get(page.sample(x, y, 0) as usize).copied().unwrap_or(255),
            None => 255,
        },
        _ => 255,
    }
}

/// The source pixels one proxy pixel covers along one axis. Never empty: a
/// proxy at 1:1 maps pixel `i` to exactly `[i, i+1)`.
fn span(at: u32, proxy: u32, page: u32) -> (u32, u32) {
    let start = (at as u64 * page as u64 / proxy as u64) as u32;
    let end = ((at as u64 + 1) * page as u64 / proxy as u64) as u32;
    (start.min(page - 1), end.max(start + 1).min(page))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::fixtures;

    /// A page of a stated size, mid-gray with a black block, so an average has
    /// something to be wrong about.
    fn a_page(width: u32, height: u32) -> Raster {
        let mut page = Raster {
            width,
            height,
            mode: ColorMode::Gray,
            depth: BitDepth::Eight,
            icc: None,
            palette: None,
            trns: None,
            srgb_intent: None,
            data: vec![128; (width * height) as usize],
        };
        for y in 0..height.min(64) {
            for x in 0..width.min(64) {
                page.set_sample(x, y, 0, 0);
            }
        }
        page
    }

    /// Rule 7's own sentence: capping the long edge is what a naive reading
    /// does, and it renders this page as a 40px sliver.
    #[test]
    fn a_webtoon_segment_keeps_its_width_and_is_not_reduced_to_a_sliver() {
        let plan = ProxyPlan::for_page(800, 20_000);
        assert_eq!((plan.proxy_w, plan.proxy_h), (800, 20_000));
        assert_ne!(plan.proxy_w, 40, "the long edge was capped instead of the short one");
    }

    #[test]
    fn a_wide_page_is_reduced_by_its_short_edge() {
        let plan = ProxyPlan::for_page(2400, 3600);
        assert_eq!(plan.proxy_w, PROXY_SHORT_EDGE);
        assert_eq!(plan.proxy_h, 1536);
        assert!(plan.vertical);
    }

    /// The same rule with the axes swapped. A page is not assumed portrait.
    #[test]
    fn a_landscape_page_is_tiled_along_x() {
        let plan = ProxyPlan::for_page(6000, 1200);
        assert!(!plan.vertical);
        assert_eq!(plan.proxy_h, PROXY_SHORT_EDGE);
        assert_eq!(plan.proxy_w, 5120);
        assert_eq!(plan.tiles, 3);
        assert_eq!(plan.tile(0).unwrap(), TileRect { x: 0, y: 0, w: 2048, h: 1024 });
        assert_eq!(plan.tile(2).unwrap(), TileRect { x: 4096, y: 0, w: 1024, h: 1024 });
    }

    /// Unlike the detector's letterbox, which scales up because the model's
    /// input is a fixed square.
    #[test]
    fn a_page_smaller_than_the_cap_is_left_alone() {
        let plan = ProxyPlan::for_page(400, 600);
        assert_eq!((plan.proxy_w, plan.proxy_h), (400, 600));
        assert_eq!(plan.tiles, 1);
        assert!(plan.is_whole_page());
    }

    #[test]
    fn the_long_axis_is_cut_into_whole_tiles_and_one_short_one() {
        let plan = ProxyPlan::for_page(800, 20_000);
        assert_eq!(plan.tiles, 10);

        let mut covered = 0;
        for index in 0..plan.tiles {
            let rect = plan.tile(index).unwrap();
            assert_eq!(rect.w, 800, "a tile did not span the short edge");
            assert_eq!(rect.y, covered);
            covered += rect.h;
        }
        assert_eq!(covered, plan.proxy_h, "the tiles did not cover the page");
        assert!(plan.tile(plan.tiles).is_none());
        assert!(!plan.is_whole_page());
    }

    #[test]
    fn a_tile_is_the_part_of_the_page_it_names() {
        let page = a_page(800, 5000);
        let plan = ProxyPlan::of(&page);
        assert_eq!(plan.tiles, 3);

        let first = proxy_tile(&page, 0).unwrap();
        assert_eq!((first.width, first.height), (800, 2048));
        // The black block is in the first tile's top-left corner and nowhere
        // else, which is what says the tile is not merely the right size.
        assert_eq!(first.sample(10, 10, 0), 0);
        assert_eq!(first.sample(10, 100, 0), 128);

        let last = proxy_tile(&page, 2).unwrap();
        assert_eq!(last.height, 5000 - 2 * 2048);
        assert_eq!(last.sample(10, 10, 0), 128);
        assert!(proxy_tile(&page, 3).is_none());
    }

    /// At 1:1 a proxy is a copy, sample for sample. This is the property that
    /// makes the small-page case free rather than merely cheap.
    #[test]
    fn a_proxy_at_one_to_one_changes_no_sample() {
        let page = a_page(600, 900);
        let tile = proxy_tile(&page, 0).unwrap();
        assert_eq!((tile.width, tile.height), (600, 900));
        for y in [0, 63, 64, 899] {
            for x in [0, 63, 64, 599] {
                assert_eq!(tile.sample(x, y, 0), page.sample(x, y, 0), "at {x},{y}");
            }
        }
    }

    /// A downscale averages the box it covers. A 2:1 reduction of a page that
    /// is half black and half mid-gray along a boundary must produce the mean
    /// on the boundary row and neither of the two extremes.
    #[test]
    fn a_downscale_averages_rather_than_drops_pixels() {
        let mut page = a_page(2048, 2048);
        for y in 0..2048 {
            for x in 0..2048 {
                page.set_sample(x, y, 0, if y % 2 == 0 { 0 } else { 200 });
            }
        }
        let plan = ProxyPlan::of(&page);
        assert_eq!((plan.proxy_w, plan.proxy_h), (1024, 1024));

        let tile = proxy_tile(&page, 0).unwrap();
        // Every proxy pixel covers one black row and one 200 row.
        for y in [0, 1, 500, 1023] {
            assert_eq!(tile.sample(7, y, 0), 100, "row {y} dropped a source row");
        }
    }

    /// Every fixture, through the proxy path, in a mode a browser can draw and
    /// with nothing left of a palette or a sub-byte depth.
    #[test]
    fn every_fixture_becomes_something_the_browser_can_draw() {
        for fixture in fixtures::all() {
            let name = fixture.name;
            let tile = proxy_tile(&fixture.raster, 0).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(tile.depth, BitDepth::Eight, "{name}");
            assert_eq!(tile.palette, None, "{name}");
            assert!(
                matches!(
                    tile.mode,
                    ColorMode::Gray | ColorMode::GrayAlpha | ColorMode::Rgb | ColorMode::Rgba
                ),
                "{name} became {:?}",
                tile.mode
            );
            assert_eq!((tile.width, tile.height), (fixture.raster.width, fixture.raster.height));
        }
    }

    /// A gray page stays gray. Tripling it into RGB is three times the bytes
    /// over the one path whose throughput is the open question.
    #[test]
    fn a_gray_page_is_not_promoted_to_colour() {
        for name in ["l8", "l16", "bitonal"] {
            let page = fixtures::by_name(name).raster;
            assert_eq!(proxy_tile(&page, 0).unwrap().mode, ColorMode::Gray, "{name}");
        }
        assert_eq!(
            proxy_tile(&fixtures::by_name("la8").raster, 0).unwrap().mode,
            ColorMode::GrayAlpha
        );
    }

    /// CMYK is the mode whose samples do not survive the conversion, so its
    /// profile must not follow them - the same rule `tile::encode_for_display`
    /// already keeps for a whole page.
    #[test]
    fn a_cmyk_profile_does_not_follow_rgb_samples() {
        let mut page = fixtures::by_name("cmyk8").raster;
        page.icc = Some(vec![1, 2, 3]);
        let tile = proxy_tile(&page, 0).unwrap();
        assert_eq!(tile.mode, ColorMode::Rgb);
        assert_eq!(tile.icc, None);
    }

    /// And every other mode's profile does, because the samples are still the
    /// page's colours at a lower resolution.
    #[test]
    fn a_profile_survives_a_mode_that_keeps_its_samples() {
        let page = fixtures::by_name("rgb8-icc").raster;
        assert!(page.icc.is_some());
        assert_eq!(proxy_tile(&page, 0).unwrap().icc, page.icc);
    }

    /// An indexed page with `tRNS` has transparency the palette carries; RGB
    /// would drop it silently.
    #[test]
    fn an_indexed_page_with_transparency_keeps_it() {
        let mut page = fixtures::by_name("indexed-p").raster;
        page.trns = Some(vec![0, 255, 255, 255]);
        let tile = proxy_tile(&page, 0).unwrap();
        assert_eq!(tile.mode, ColorMode::Rgba);

        page.trns = None;
        assert_eq!(proxy_tile(&page, 0).unwrap().mode, ColorMode::Rgb);
    }

    /// A fully transparent pixel must not drag its neighbours' colour: the
    /// average is premultiplied, so what is under `a = 0` contributes nothing.
    #[test]
    fn a_transparent_edge_does_not_bleed_into_the_average() {
        let mut page = Raster {
            width: 2048,
            height: 2048,
            mode: ColorMode::Rgba,
            depth: BitDepth::Eight,
            icc: None,
            palette: None,
            trns: None,
            srgb_intent: None,
            data: vec![0; 2048 * 2048 * 4],
        };
        for y in 0..2048 {
            for x in 0..2048 {
                let opaque = x % 2 == 0;
                // The transparent half stores black; a plain average would
                // halve the red of every proxy pixel.
                page.set_sample(x, y, 0, if opaque { 240 } else { 0 });
                page.set_sample(x, y, 3, if opaque { 255 } else { 0 });
            }
        }

        let tile = proxy_tile(&page, 0).unwrap();
        assert_eq!(tile.sample(5, 5, 0), 240, "the transparent half bled into the colour");
        assert_eq!(tile.sample(5, 5, 3), 127, "coverage was not averaged");
    }

    /// A raster with no pixels in it. The plan clamps a zero dimension to 1 so
    /// its own arithmetic is defined, which leaves one proxy pixel covering a
    /// source row that does not exist - read, that is an index past the end of
    /// an empty `data`. Nothing `decode` produces looks like this, and
    /// `proxy_tile` takes any `&Raster`.
    #[test]
    fn a_raster_with_a_zero_dimension_has_no_tiles_rather_than_panicking() {
        let empty = |width, height| Raster {
            width,
            height,
            mode: ColorMode::Gray,
            depth: BitDepth::Eight,
            icc: None,
            palette: None,
            trns: None,
            srgb_intent: None,
            data: Vec::new(),
        };
        assert!(proxy_tile(&empty(0, 400), 0).is_none());
        assert!(proxy_tile(&empty(400, 0), 0).is_none());
        assert!(proxy_tile(&empty(0, 0), 0).is_none());
    }
}
