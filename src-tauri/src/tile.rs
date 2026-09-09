//! The `tile://` custom protocol: pixels, straight to the browser.
//!
//! The frontend holds patch
//! *metadata* - id, bbox, engine, visibility, order - and URLs; the bytes of a
//! page never cross the IPC boundary, because a base64 data URL inflates the
//! payload by a third and decodes on the JS main thread.
//!
//! ## Whole responses, never a range
//!
//! A maintainer benchmark recorded **~5 ms on macOS against ~200 ms on
//! Windows for 10 MB**, about 50 MB/s - because WebView2 raises
//! `WebResourceRequested` on the host UI thread and pauses page load while the
//! handler runs. **Range and streaming responses are unsupported on WebView2.**
//! That is not our measurement and it is not ours to restate as one; it is why
//! this module builds one complete `Vec<u8>` per request and never emits
//! `Accept-Ranges`, `Content-Range` or a `206`. Tauri's own asset protocol does
//! serve ranges; this one deliberately does not.
//!
//! The throughput number itself was measured in
//! Phase 2, on a Windows machine. Nothing here has been executed on one.
//!
//! ## The URL
//!
//! ```text
//! tile://localhost/<chapterId>/<pageIndex>/<variant>[/<tile>]?v=<version>
//! http://tile.localhost/<chapterId>/<pageIndex>/<variant>[/<tile>]?v=<version>
//! ```
//!
//! The second form is Windows and Android; the first is macOS, iOS and Linux.
//! The path is identical on both, which is why the frontend chooses only the
//! origin ([`src/lib/api/tile.js`](../../src/lib/api/tile.js)) and never the
//! shape. `variant` is `source` or `cleaned`.
//!
//! ## The fourth segment is a proxy tile, and it is what the interface asks for
//!
//! Rule 7: *the UI holds proxies,
//! never the strip*. With `<tile>` the response is one tile of the page's
//! **proxy** - short edge capped at 1024, long axis cut into
//! `cleaner_core::image::proxy::PROXY_TILE_LONG_EDGE` bands - and that is the
//! form both components draw. Without it the response is the page at its own
//! resolution, which rule 7 reserves for the viewport past 100% zoom and which
//! nothing requests today.
//!
//! Each tile is composited and downscaled from scratch, because the protocol
//! holds no state between requests; the whole page is decoded to produce any
//! one of them. That is affordable because the URL is immutably cacheable, so a
//! tile is fetched once per version, and because the *page* is the unit here -
//! never the strip, which is the distinction rule 1 draws.
//!
//! **`v` is read by nobody here, on purpose.** It is a cache-busting token the
//! interface derives from the page's own content hashes - the source `sha256`
//! and the `mask_sha256` of every mask on the page - so that a URL changes
//! exactly when the bytes behind it change, and the response can then be
//! cached immutably. Validating it here would mean the protocol holding a
//! second opinion about what a page's identity is, and two opinions is one too
//! many. That is the rule it follows: a
//! hash decides staleness, an mtime only explains it.
//!
//! ## What `cleaned` means
//!
//! `raw + ordered visible patches`, through `cleaner_core::composite` - the same
//! function the exporter uses, so what the user is looking at is what the file
//! will hold. A page with no patches yields the source's own bytes: that is the
//! correct answer for an uncleaned page, not a placeholder.

use std::path::{Path, PathBuf};

use cleaner_core::composite::composite;
use cleaner_core::export::{StripPatch, patches_on_page};
use cleaner_core::image::proxy::{ProxyPlan, proxy_tile};
use cleaner_core::image::{ColorMode, Format, Raster, decode, encode, png_header};
use cleaner_core::mask::Rect;
use cleaner_core::patch::Patch;
use cleaner_core::project::{Job, PatchRecord, StripMode};
use cleaner_core::strip::Strip;
use tauri::http::{Request, Response, StatusCode, header};

use crate::library;

/// The scheme, registered on the builder and named by the frontend.
pub const SCHEME: &str = "tile";

/// Which of a page's two images is wanted.
///
/// Two images rather than one image with a toggle: `src/lib/editor/PageSheet.svelte`
/// clips one over the other to make the wipe, so both must be addressable at
/// once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// The page as it arrived. Every region's text is on it.
    Source,
    /// The page as the pipeline left it.
    Cleaned,
}

impl Variant {
    fn parse(segment: &str) -> Option<Variant> {
        match segment {
            "source" => Some(Variant::Source),
            "cleaned" => Some(Variant::Cleaned),
            _ => None,
        }
    }
}

/// One parsed request. Deliberately owns its `chapter_id`: the borrow would be
/// of the request's URI, and the resolver outlives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TileRequest {
    pub chapter_id: String,
    pub page_index: usize,
    pub variant: Variant,
    /// Which tile of the page's proxy, or `None` for the page at its own
    /// resolution. See the module note on the fourth segment.
    pub tile: Option<usize>,
}

#[derive(Debug, thiserror::Error)]
pub enum TileError {
    #[error("a tile path is /<chapterId>/<pageIndex>/<variant>[/<tile>], not {0:?}")]
    Malformed(String),
    #[error("no chapter {0}")]
    NoSuchChapter(String),
    #[error("{job} has no page {page_index}")]
    NoSuchPage { job: String, page_index: usize },
    #[error("page {page_index}'s proxy has {tiles} tiles, not a tile {tile}")]
    NoSuchTile { page_index: usize, tile: usize, tiles: usize },
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error(transparent)]
    Store(#[from] cleaner_core::project::StoreError),
    #[error(transparent)]
    Image(#[from] cleaner_core::image::ImageError),
    #[error(transparent)]
    Composite(#[from] cleaner_core::composite::CompositeError),
}

impl TileError {
    /// What the webview is told.
    ///
    /// A malformed path is the caller's error and a missing chapter or page is
    /// a `404` whatever the reason for it - the *why* goes in the body, where
    /// the devtools console shows it, rather than being encoded in a status
    /// nothing branches on.
    pub fn status(&self) -> StatusCode {
        match self {
            TileError::Malformed(_) => StatusCode::BAD_REQUEST,
            TileError::NoSuchChapter(_)
            | TileError::NoSuchPage { .. }
            | TileError::NoSuchTile { .. } => StatusCode::NOT_FOUND,
            // A file that is not there is a `404` wherever the absence was
            // noticed - the manifest, or the source it names. Anything else
            // that fails to read is a fault on this side.
            TileError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound => {
                StatusCode::NOT_FOUND
            }
            TileError::Store(cleaner_core::project::StoreError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                StatusCode::NOT_FOUND
            }
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// `/<chapterId>/<pageIndex>/<variant>[/<tile>]` - the query is not read; see
/// the module note on `v`.
///
/// Takes the path alone rather than the whole URI because that is the one part
/// of the URL that is the same on every platform: the origin differs
/// (`tile://localhost` against `http://tile.localhost`) and the path does not.
pub fn parse(path: &str) -> Result<TileRequest, TileError> {
    let malformed = || TileError::Malformed(path.to_owned());
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let (chapter, index, variant, tile) = match segments.as_slice() {
        [chapter, index, variant] => (chapter, index, variant, None),
        [chapter, index, variant, tile] => {
            (chapter, index, variant, Some(tile.parse().map_err(|_| malformed())?))
        }
        _ => return Err(malformed()),
    };

    let chapter_id = percent_decode(chapter);
    if chapter_id.is_empty() {
        return Err(malformed());
    }
    Ok(TileRequest {
        chapter_id,
        page_index: index.parse().map_err(|_| malformed())?,
        variant: Variant::parse(variant).ok_or_else(malformed)?,
        tile,
    })
}

/// A chapter id is a path segment, and a path segment arrives percent-encoded.
///
/// Written out rather than taken as a dependency: the whole of what is needed
/// is `%XX`, and an invalid escape is left alone rather than replaced, so a
/// literal `%` in an id round-trips instead of becoming a replacement
/// character that would not match anything.
fn percent_decode(segment: &str) -> String {
    let bytes = segment.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(byte) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The bytes for one page, as PNG.
///
/// Split from the protocol glue so it can be tested against a real `.mtclean`
/// on disk without a webview, which is where everything that can actually be
/// wrong lives.
///
/// `page_index` is the URL's, which is `ApiPage.index`, which is a **position
/// in `strip.order`** - not an index into `sources`. The two are equal only
/// until a strip is reordered or a source is dropped, so the source index is
/// resolved through [`library::Library::resolve_page`], the one place that rule
/// is written. A position `strip.order` does not have is a `404`, the same
/// answer as a page that is not there, because that is what it is.
///
/// `tile` is the URL's fourth segment: `Some(i)` for tile `i` of the page's
/// proxy, `None` for the page at its own resolution.
pub fn render(
    job_path: &Path,
    page_index: usize,
    variant: Variant,
    tile: Option<usize>,
) -> Result<Vec<u8>, TileError> {
    let job = Job::open(job_path)?;
    let no_such_page =
        || TileError::NoSuchPage { job: job_path.display().to_string(), page_index };

    let source_idx =
        library::Library::resolve_page(&job.project, page_index).ok_or_else(no_such_page)?;
    let source_path = job.source_path(source_idx).ok_or_else(no_such_page)?;
    let source_bytes = match std::fs::read(&source_path) {
        Ok(bytes) => bytes,
        Err(source) => return Err(TileError::Io { path: source_path, source }),
    };

    let patches = match variant {
        Variant::Source => Vec::new(),
        Variant::Cleaned => visible_patches(&job, page_index, source_idx)?,
    };

    // An untouched page served as its own file: no decode, no re-encode, and
    // the browser gets the bytes the scanner produced. The same reasoning as
    // `export::export_page`'s passthrough, for the same reason - for a page
    // nothing touched, being the same file is the point.
    //
    // A proxy request keeps it only where the proxy *is* the page: tile 0 of a
    // page small enough not to be reduced and not to be cut. The header is read
    // for the dimensions rather than the whole file being decoded to find them.
    if patches.is_empty() && Format::sniff(&source_bytes) == Some(Format::Png) {
        let whole = match tile {
            None => true,
            Some(0) => png_header(&source_bytes)
                .is_ok_and(|header| ProxyPlan::for_page(header.width, header.height).is_whole_page()),
            Some(_) => false,
        };
        if whole {
            return Ok(source_bytes);
        }
    }

    let page = decode(&source_bytes)?;
    let composited = if patches.is_empty() { page } else { composite(&page, &patches)? };
    let drawn = match tile {
        None => composited,
        Some(index) => proxy_tile(&composited, index).ok_or(TileError::NoSuchTile {
            page_index,
            tile: index,
            tiles: ProxyPlan::of(&composited).tiles,
        })?,
    };
    Ok(encode_for_display(&drawn)?)
}

/// Every visible patch recorded against one page, rebuilt from the manifest.
///
/// In longstrip mode, a patch anchored on one page may span across a join onto
/// adjacent pages, so patches are resolved via intersection (`patches_on_page`).
/// For paginated chapters (`StripMode::Single`), patches are filtered strictly
/// by source index ownership.
fn visible_patches(
    job: &Job,
    page_index: usize,
    source_idx: usize,
) -> Result<Vec<Patch>, TileError> {
    if job.project.strip.mode == StripMode::Longstrip {
        let sizes: Vec<(u32, u32)> = job
            .project
            .strip
            .order
            .iter()
            .filter_map(|i| job.project.sources.get(*i).map(|s| (s.w, s.h)))
            .collect();
        let strip = Strip::of_sizes(&sizes);
        let mut patches = Vec::new();
        for (anchor, record) in intersecting_records(job, &strip, page_index) {
            if !record.visible {
                continue;
            }
            let patch = job.load_patch(record)?;
            let Some(lifted) = StripPatch::lift(&strip, anchor, &patch) else { continue };
            patches.extend(patches_on_page(&strip, page_index, std::slice::from_ref(&lifted)));
        }
        Ok(patches)
    } else {
        let mut patches = Vec::new();
        for record in &job.project.patches {
            if record.source_idx != source_idx || !record.visible {
                continue;
            }
            patches.push(job.load_patch(record)?);
        }
        Ok(patches)
    }
}

fn intersecting_records<'a>(
    job: &'a Job,
    strip: &Strip,
    position: usize,
) -> Vec<(usize, &'a PatchRecord)> {
    let Some(page) = strip.pages().get(position).map(|p| p.rect()) else { return Vec::new() };
    job.project
        .patches
        .iter()
        .filter_map(|record| {
            let anchor = job.project.strip.order.iter().position(|i| *i == record.source_idx)?;
            let (x, y) = strip.to_strip(anchor, record.bbox.x, record.bbox.y)?;
            let bbox = Rect::new(x, y, record.bbox.w, record.bbox.h);
            (bbox.x < page.right()
                && bbox.right() > page.x
                && bbox.y < page.bottom()
                && bbox.bottom() > page.y)
                .then_some((anchor, record))
        })
        .collect()
}

/// PNG, in the page's own mode wherever PNG has one.
///
/// PNG carries every mode except CMYK, so a tile is normally the page's own samples at their
/// own depth - an indexed page stays indexed, a 16-bit page stays 16-bit, and
/// nothing here is a colour decision.
///
/// CMYK is the exception and it is a **display** conversion, uncalibrated, of
/// the same kind `image::convert` is written for. It is confined to this
/// function: the export path keeps CMYK as CMYK in a TIFF, the manifest is
/// untouched, and the fidelity contract is stated over the exported file rather
/// than over what a webview drew. The profile is dropped with the mode, because
/// a CMYK profile attached to RGB samples describes the wrong thing.
fn encode_for_display(raster: &Raster) -> Result<Vec<u8>, cleaner_core::image::ImageError> {
    if raster.mode != ColorMode::Cmyk {
        return encode(raster, Format::Png);
    }
    let rgb = Raster {
        width: raster.width,
        height: raster.height,
        mode: ColorMode::Rgb,
        depth: cleaner_core::image::BitDepth::Eight,
        icc: None,
        palette: None,
        trns: None,
        srgb_intent: None,
        data: raster.to_rgb8(),
    };
    encode(&rgb, Format::Png)
}

/// The handler, over a chapter resolver.
///
/// The resolver is a parameter rather than a direct call to
/// `library::resolve_chapter` for two reasons: this module can then be tested
/// against a job on disk with no index at all, and the library and the protocol
/// are two files that would otherwise have to land together. (`render` does
/// call `library::Library::resolve_page`, which is arithmetic over a `Project`
/// already in hand and touches neither the index nor the disk - a second copy
/// of that rule is exactly what this module must not have.)
///
/// It returns `Result<PathBuf, String>` rather than the library's own error
/// because there is exactly one thing this can do with a failure - a `404` and
/// the message in the body - so a typed error would be widened here and never
/// read.
pub fn protocol<F>(
    resolve: F,
) -> impl Fn(tauri::UriSchemeContext<'_, tauri::Wry>, Request<Vec<u8>>) -> Response<Vec<u8>>
+ Send
+ Sync
+ 'static
where
    F: Fn(&tauri::AppHandle, &str) -> Result<PathBuf, String> + Send + Sync + 'static,
{
    move |ctx, request| {
        let path = request.uri().path().to_owned();
        match serve(ctx.app_handle(), &resolve, &path) {
            Ok(bytes) => ok(bytes),
            Err(error) => refuse(error),
        }
    }
}

fn serve<F>(app: &tauri::AppHandle, resolve: &F, path: &str) -> Result<Vec<u8>, TileError>
where
    F: Fn(&tauri::AppHandle, &str) -> Result<PathBuf, String>,
{
    let request = parse(path)?;
    let job_path = resolve(app, &request.chapter_id)
        .map_err(|_| TileError::NoSuchChapter(request.chapter_id.clone()))?;
    render(&job_path, request.page_index, request.variant, request.tile)
}

/// A whole-response image.
///
/// `immutable` is honest only because the URL carries the version token: the
/// interface changes `v` when the page's content hashes change, so this URL's
/// bytes really are fixed for as long as the URL exists. Without that token
/// this header would be a bug.
fn ok(bytes: Vec<u8>) -> Response<Vec<u8>> {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "image/png")
        .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(bytes)
        .expect("a response with static headers")
}

/// The message is English, and it is allowed to be: it goes to the devtools
/// console, never to a user. Nothing here reaches the seam, where
/// "no English crosses
/// the seam" applies - a broken `<img>` is what the user sees, and the
/// interface's own empty states cover it.
fn refuse(error: TileError) -> Response<Vec<u8>> {
    Response::builder()
        .status(error.status())
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(error.to_string().into_bytes())
        .expect("a response with static headers")
}

#[cfg(test)]
mod tests {
    use super::*;
    use cleaner_core::image::{fixtures, lossless_format_for};
    use cleaner_core::mask::{Mask, Rect};
    use cleaner_core::patch::{Engine, Provenance};
    use cleaner_core::project::{Project, StripMode};

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            use std::sync::atomic::{AtomicU32, Ordering};
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir()
                .join("manga-cleaner-tile")
                .join(format!("{name}-{}", NEXT.fetch_add(1, Ordering::Relaxed)));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch directory");
            Scratch(dir)
        }

        fn join(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A one-page job over a named fixture, written to disk exactly as a real
    /// one is.
    fn a_job(scratch: &Scratch, fixture: &str) -> Job {
        let page = fixtures::by_name(fixture).raster;
        let bytes = encode(&page, lossless_format_for(&page)).unwrap();
        let raws = scratch.join("raws");
        std::fs::create_dir_all(&raws).unwrap();
        let source = raws.join(format!("001.{}", match lossless_format_for(&page) {
            Format::Png => "png",
            Format::Tiff => "tif",
        }));
        std::fs::write(&source, &bytes).unwrap();
        let reference = cleaner_core::ingest::source_ref(&source, &bytes).unwrap();

        let manifest = scratch.join("out/chapter.mtclean");
        std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        let project =
            Project::new(manifest.parent().unwrap(), "0.1.0", StripMode::Single, &[reference]);
        Job::create(&manifest, project).unwrap()
    }

    /// A job over `count` distinct pages, whose `strip.order` is whatever the
    /// caller says - the case where a page index and a source index part
    /// company.
    fn a_strip(scratch: &Scratch, count: usize, order: Vec<usize>) -> Job {
        let raws = scratch.join("raws");
        std::fs::create_dir_all(&raws).unwrap();
        let mut references = Vec::new();
        for n in 0..count {
            let mut page = fixtures::by_name("l8").raster;
            // Distinct bytes per page, so a test can say which one it got.
            page.data[0] = 10 * (n as u8 + 1);
            let bytes = encode(&page, Format::Png).unwrap();
            let source = raws.join(format!("{:03}.png", n + 1));
            std::fs::write(&source, &bytes).unwrap();
            references.push(cleaner_core::ingest::source_ref(&source, &bytes).unwrap());
        }

        let manifest = scratch.join("out/chapter.mtclean");
        std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        let mut project =
            Project::new(manifest.parent().unwrap(), "0.1.0", StripMode::Single, &references);
        project.strip.order = order;
        Job::create(&manifest, project).unwrap()
    }

    /// A one-page job over a flat gray page of a stated size - the shape a
    /// webtoon segment has and no fixture does (they are all 64×48).
    fn a_big_job(scratch: &Scratch, width: u32, height: u32) -> Job {
        let page = Raster {
            width,
            height,
            mode: ColorMode::Gray,
            depth: cleaner_core::image::BitDepth::Eight,
            icc: None,
            palette: None,
            trns: None,
            srgb_intent: None,
            data: vec![40; (width as usize) * (height as usize)],
        };
        let bytes = encode(&page, Format::Png).unwrap();
        let raws = scratch.join("raws");
        std::fs::create_dir_all(&raws).unwrap();
        let source = raws.join("001.png");
        std::fs::write(&source, &bytes).unwrap();
        let reference = cleaner_core::ingest::source_ref(&source, &bytes).unwrap();

        let manifest = scratch.join("out/chapter.mtclean");
        std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        let project =
            Project::new(manifest.parent().unwrap(), "0.1.0", StripMode::Single, &[reference]);
        Job::create(&manifest, project).unwrap()
    }

    /// A patch that paints a flat block over the middle of the page, in the
    /// page's own mode.
    fn a_patch(page: &Raster, bounds: Rect, value: u16) -> Patch {
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
        for y in 0..bounds.h {
            for x in 0..bounds.w {
                for channel in 0..page.mode.samples() {
                    pixels.set_sample(x, y, channel, value);
                }
            }
        }
        Patch {
            id: "r0".into(),
            mask: Mask::filled(bounds),
            ink: Mask::filled(bounds),
            pixels,
            order: 0,
            visible: true,
            provenance: Provenance {
                engine: Engine::Fill,
                engine_version: "test".into(),
                model_sha256: None,
                execution_provider: "cpu".into(),
                params_snapshot: serde_json::json!({}),
                mask_sha256: "0".repeat(64),
                source_sha256: "1".repeat(64),
                cloud: None,
                created: 0,
            },
        }
    }

    #[test]
    fn a_url_parses_into_a_chapter_a_page_and_a_variant() {
        assert_eq!(
            parse("/ch-01/7/cleaned").unwrap(),
            TileRequest {
                chapter_id: "ch-01".into(),
                page_index: 7,
                variant: Variant::Cleaned,
                tile: None,
            }
        );
        assert_eq!(parse("/ch-01/0/source").unwrap().variant, Variant::Source);
    }

    /// The fourth segment, and the difference between "the page" and "tile 0 of
    /// the page's proxy" - which are the same picture only for a page small
    /// enough that the proxy is a copy.
    #[test]
    fn a_fourth_segment_names_a_proxy_tile() {
        assert_eq!(
            parse("/ch-01/7/cleaned/3").unwrap(),
            TileRequest {
                chapter_id: "ch-01".into(),
                page_index: 7,
                variant: Variant::Cleaned,
                tile: Some(3),
            }
        );
        assert_eq!(parse("/ch-01/7/cleaned/0").unwrap().tile, Some(0));
        assert_eq!(parse("/ch-01/7/cleaned").unwrap().tile, None);
        // Not an index, so not a tile. A guess here would serve tile 0 under a
        // URL that asked for something else.
        assert_eq!(parse("/ch-01/7/cleaned/-1").unwrap_err().status(), StatusCode::BAD_REQUEST);
        assert_eq!(parse("/ch-01/7/cleaned/first").unwrap_err().status(), StatusCode::BAD_REQUEST);
    }

    /// The query is the interface's business, not this module's: a version
    /// token it does not understand must not change what it serves.
    #[test]
    fn the_version_token_is_not_part_of_the_path_and_changes_nothing() {
        // `Request::uri().path()` has already dropped the query; the assertion
        // that matters is that a path is all `parse` is ever given, so a `?v=`
        // reaching it would be a malformed path rather than a silent match.
        assert!(parse("/ch-01/0/source?v=abc").is_err());
        assert_eq!(parse("/ch-01/0/source").unwrap().page_index, 0);
    }

    #[test]
    fn a_chapter_id_arrives_percent_decoded() {
        assert_eq!(parse("/ch%2001/0/source").unwrap().chapter_id, "ch 01");
        // A literal `%` that is not an escape survives rather than becoming a
        // replacement character, which would match no chapter at all.
        assert_eq!(parse("/100%/0/source").unwrap().chapter_id, "100%");
    }

    #[test]
    fn a_malformed_path_is_a_bad_request_rather_than_a_guess() {
        for path in ["/", "/ch-01", "/ch-01/0", "/ch-01/0/source/extra", "//0/source"] {
            let error = parse(path).expect_err(path);
            assert_eq!(error.status(), StatusCode::BAD_REQUEST, "{path}");
        }
        assert_eq!(parse("/ch-01/x/source").unwrap_err().status(), StatusCode::BAD_REQUEST);
        // A variant nobody serves is refused rather than defaulted to `source`:
        // a tile that silently shows the uncleaned page is worse than no tile.
        assert_eq!(parse("/ch-01/0/original").unwrap_err().status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn a_page_the_chapter_does_not_have_is_a_404() {
        let scratch = Scratch::new("no-page");
        let job = a_job(&scratch, "l8");
        let error = render(job.path(), 4, Variant::Source, None).unwrap_err();
        assert_eq!(error.status(), StatusCode::NOT_FOUND);
    }

    /// The URL's page index is a position in `strip.order`, not an index into
    /// `sources` - `library.rs`'s own module doc says so, and `exporting.rs`
    /// already iterates `strip.order`. They are equal only until something
    /// reorders the pages, and then this protocol serves the wrong page's
    /// pixels under a URL the interface believes is immutable.
    #[test]
    fn a_reordered_strip_serves_the_page_the_order_names() {
        let scratch = Scratch::new("reordered");
        let job = a_strip(&scratch, 3, vec![2, 0, 1]);

        for (page_index, source_idx) in [(0usize, 2usize), (1, 0), (2, 1)] {
            let on_disk = std::fs::read(job.source_path(source_idx).unwrap()).unwrap();
            assert_eq!(
                render(job.path(), page_index, Variant::Source, None).unwrap(),
                on_disk,
                "page {page_index} was not source {source_idx}"
            );
        }
    }

    /// A patch is recorded against a *source*, so a reordered strip must not
    /// move it: page 0's cleaned tile carries source 2's patch, and page 2's
    /// carries none.
    #[test]
    fn a_patch_follows_its_source_rather_than_its_position() {
        let scratch = Scratch::new("reordered-patch");
        let mut job = a_strip(&scratch, 3, vec![2, 0, 1]);
        let page = decode(&std::fs::read(job.source_path(2).unwrap()).unwrap()).unwrap();
        job.complete_region(2, &a_patch(&page, Rect::new(8, 8, 12, 10), 200), None).unwrap();

        let cleaned = decode(&render(job.path(), 0, Variant::Cleaned, None).unwrap()).unwrap();
        assert_eq!(cleaned.sample(10, 10, 0), 200, "the patch is not on the page its source is");
        assert_eq!(
            render(job.path(), 2, Variant::Cleaned, None).unwrap(),
            render(job.path(), 2, Variant::Source, None).unwrap(),
            "another page's patch reached this tile"
        );
    }

    /// A source `strip.order` does not name is not a page. `sources` still has
    /// it - that is what makes this the case a bare `source_path(page_index)`
    /// answers with pixels instead of a `404`.
    #[test]
    fn a_source_the_strip_dropped_is_not_reachable_as_a_page() {
        let scratch = Scratch::new("dropped");
        let job = a_strip(&scratch, 3, vec![1]);
        assert_eq!(job.project.sources.len(), 3);

        assert!(render(job.path(), 0, Variant::Source, None).is_ok());
        for page_index in [1, 2] {
            let error = render(job.path(), page_index, Variant::Source, None)
                .expect_err("a dropped source was served as a page");
            assert_eq!(error.status(), StatusCode::NOT_FOUND, "page {page_index}");
        }
    }

    /// The browser decodes natively, so every tile is an image format a
    /// webview has a decoder for. TIFF is not one on Windows, and the core
    /// reads TIFF sources.
    #[test]
    fn every_fixture_is_served_as_a_png_a_browser_can_decode() {
        for fixture in fixtures::all() {
            let scratch = Scratch::new("fixture");
            let job = a_job(&scratch, fixture.name);
            let bytes = render(job.path(), 0, Variant::Source, None).unwrap();
            assert_eq!(
                Format::sniff(&bytes),
                Some(Format::Png),
                "{} was served as something other than PNG",
                fixture.name
            );
            let out = decode(&bytes).unwrap();
            assert_eq!(out.width, fixture.raster.width, "{}", fixture.name);
            assert_eq!(out.height, fixture.raster.height, "{}", fixture.name);
        }
    }

    /// The mode is not a display decision. Only CMYK converts, because PNG has
    /// no colour type for it - every other fixture keeps its own mode, depth
    /// and palette, so a tile is the page rather than a rendering of it.
    #[test]
    fn a_tile_keeps_the_pages_own_mode_except_where_png_has_none() {
        for fixture in fixtures::all() {
            let scratch = Scratch::new("mode");
            let job = a_job(&scratch, fixture.name);
            let out = decode(&render(job.path(), 0, Variant::Source, None).unwrap()).unwrap();
            if fixture.raster.mode == ColorMode::Cmyk {
                assert_eq!(out.mode, ColorMode::Rgb, "{}", fixture.name);
                assert_eq!(out.icc, None, "{}: a CMYK profile followed RGB samples", fixture.name);
            } else {
                assert_eq!(out.mode, fixture.raster.mode, "{}", fixture.name);
                assert_eq!(out.depth, fixture.raster.depth, "{}", fixture.name);
                assert_eq!(out.palette, fixture.raster.palette, "{}", fixture.name);
            }
        }
    }

    /// A PNG source with nothing applied is served as its own file. Not an
    /// optimisation for its own sake: it is the same property `export_page`
    /// calls passthrough, and it is what makes the `source` variant free.
    #[test]
    fn an_untouched_png_page_is_served_as_its_own_bytes() {
        let scratch = Scratch::new("passthrough");
        let job = a_job(&scratch, "l8");
        let on_disk = std::fs::read(job.source_path(0).unwrap()).unwrap();
        assert_eq!(render(job.path(), 0, Variant::Source, None).unwrap(), on_disk);
        assert_eq!(render(job.path(), 0, Variant::Cleaned, None).unwrap(), on_disk);
    }

    /// The brief's sentence, asserted: "a page with no patches yields the
    /// source bytes; that is correct, not a stub." Including for a TIFF source,
    /// where the two are re-encoded rather than copied and must still agree.
    #[test]
    fn cleaned_and_source_are_the_same_image_until_something_is_applied() {
        let scratch = Scratch::new("no-patches");
        let job = a_job(&scratch, "cmyk8");
        assert_eq!(
            render(job.path(), 0, Variant::Source, None).unwrap(),
            render(job.path(), 0, Variant::Cleaned, None).unwrap()
        );
    }

    #[test]
    fn a_cleaned_tile_is_the_source_with_the_patches_composited_over_it() {
        let scratch = Scratch::new("cleaned");
        let mut job = a_job(&scratch, "l8");
        let page = decode(&std::fs::read(job.source_path(0).unwrap()).unwrap()).unwrap();
        let bounds = Rect::new(8, 8, 12, 10);
        job.complete_region(0, &a_patch(&page, bounds, 200), None).unwrap();

        let source = decode(&render(job.path(), 0, Variant::Source, None).unwrap()).unwrap();
        let cleaned = decode(&render(job.path(), 0, Variant::Cleaned, None).unwrap()).unwrap();

        assert_eq!(source.data, page.data, "the source variant was not the source");
        assert_eq!(cleaned.sample(10, 10, 0), 200, "the patch is not on the cleaned tile");
        assert_eq!(
            cleaned.sample(0, 0, 0),
            page.sample(0, 0, 0),
            "the cleaned tile changed a pixel outside every mask"
        );
    }

    /// `visible` is persisted per patch precisely so it can be turned off, and
    /// the compositor honours it. The tile must too, or the canvas and the
    /// export disagree about what the page is.
    #[test]
    fn a_hidden_patch_is_not_on_the_cleaned_tile() {
        let scratch = Scratch::new("hidden");
        let mut job = a_job(&scratch, "l8");
        let page = decode(&std::fs::read(job.source_path(0).unwrap()).unwrap()).unwrap();
        let mut patch = a_patch(&page, Rect::new(8, 8, 12, 10), 200);
        patch.visible = false;
        job.complete_region(0, &patch, None).unwrap();

        assert_eq!(
            render(job.path(), 0, Variant::Cleaned, None).unwrap(),
            render(job.path(), 0, Variant::Source, None).unwrap()
        );
    }

    /// A patch belongs to one page. Serving another page's edits would be
    /// invisible on a one-page fixture and wrong on every real chapter.
    #[test]
    fn a_patch_on_another_page_does_not_reach_this_ones_tile() {
        let scratch = Scratch::new("other-page");
        let mut job = a_job(&scratch, "l8");
        let page = decode(&std::fs::read(job.source_path(0).unwrap()).unwrap()).unwrap();
        let mut patch = a_patch(&page, Rect::new(8, 8, 12, 10), 200);
        patch.id = "r-elsewhere".into();
        job.complete_region(1, &patch, None).unwrap();

        assert_eq!(
            render(job.path(), 0, Variant::Cleaned, None).unwrap(),
            render(job.path(), 0, Variant::Source, None).unwrap()
        );
    }

    /// The whole point: a 20 000 px segment must never reach the webview as
    /// one image, and it must not be squeezed to a sliver either. Ten tiles,
    /// each the page's full width, together covering it.
    #[test]
    fn a_long_page_is_served_as_proxy_tiles_and_never_as_one_image() {
        let scratch = Scratch::new("proxy-tiles");
        let job = a_big_job(&scratch, 800, 5000);
        let plan = ProxyPlan::for_page(800, 5000);
        assert_eq!(plan.tiles, 3);

        let mut rows = 0;
        for index in 0..plan.tiles {
            let tile =
                decode(&render(job.path(), 0, Variant::Source, Some(index)).unwrap()).unwrap();
            assert_eq!(tile.width, 800, "tile {index} did not span the page's width");
            rows += tile.height;
        }
        assert_eq!(rows, 5000, "the tiles did not cover the page");

        // And the tile nobody has is a 404, not the last one again.
        let error = render(job.path(), 0, Variant::Source, Some(plan.tiles)).unwrap_err();
        assert_eq!(error.status(), StatusCode::NOT_FOUND);
    }

    /// The short edge is what the cap applies to. A page wider than 1024 is
    /// reduced; the aspect ratio the interface draws it at is unchanged.
    #[test]
    fn a_proxy_caps_the_short_edge_and_keeps_the_aspect_ratio() {
        let scratch = Scratch::new("proxy-cap");
        let job = a_big_job(&scratch, 2400, 3000);
        let tile = decode(&render(job.path(), 0, Variant::Source, Some(0)).unwrap()).unwrap();
        assert_eq!(tile.width, 1024);
        // 3000 × 1024/2400 = 1280, which is one tile.
        assert_eq!(tile.height, 1280);
        assert!(render(job.path(), 0, Variant::Source, Some(1)).is_err());
    }

    /// A proxy is a view of the *cleaned* page when that is what was asked for -
    /// the patches are composited at native resolution first, so a mask does
    /// not move under the downscale.
    #[test]
    fn a_cleaned_proxy_tile_carries_the_page_s_patches() {
        let scratch = Scratch::new("proxy-cleaned");
        let mut job = a_big_job(&scratch, 2400, 3000);
        let page = decode(&std::fs::read(job.source_path(0).unwrap()).unwrap()).unwrap();
        job.complete_region(0, &a_patch(&page, Rect::new(0, 0, 240, 240), 200), None).unwrap();

        let cleaned = decode(&render(job.path(), 0, Variant::Cleaned, Some(0)).unwrap()).unwrap();
        // The block covers the first tenth of the page, so the proxy's first
        // 102 pixels are the patch and everything past it is the page.
        assert_eq!(cleaned.sample(50, 50, 0), 200, "the patch is not on the proxy");
        assert_eq!(cleaned.sample(500, 500, 0), 40, "the proxy changed a pixel outside the mask");

        let source = decode(&render(job.path(), 0, Variant::Source, Some(0)).unwrap()).unwrap();
        assert_eq!(source.sample(50, 50, 0), 40, "the source proxy carried a patch");
    }

    /// Passthrough survives the proxy path, and only where it is honest: tile 0
    /// of a page the proxy would neither reduce nor cut *is* the page.
    #[test]
    fn a_page_small_enough_to_be_its_own_proxy_is_still_served_as_its_own_bytes() {
        let scratch = Scratch::new("proxy-passthrough");
        let job = a_job(&scratch, "l8");
        let on_disk = std::fs::read(job.source_path(0).unwrap()).unwrap();
        assert_eq!(render(job.path(), 0, Variant::Source, Some(0)).unwrap(), on_disk);
        assert_eq!(render(job.path(), 0, Variant::Source, None).unwrap(), on_disk);
        // And a page that would be cut is not: the bytes served for tile 0 are
        // a tile, never the whole file.
        let tall_scratch = Scratch::new("proxy-passthrough-tall");
        let tall = a_big_job(&tall_scratch, 800, 5000);
        let whole = std::fs::read(tall.source_path(0).unwrap()).unwrap();
        assert_ne!(render(tall.path(), 0, Variant::Source, Some(0)).unwrap(), whole);
    }

    /// The response is whole, and says so. WebView2 supports neither
    /// ranges nor streaming, so a handler that advertised either would be
    /// advertising something one of the two platforms cannot do.
    #[test]
    fn a_response_is_whole_and_advertises_no_range_support() {
        let response = ok(vec![1, 2, 3]);
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers().get(header::CONTENT_TYPE).unwrap(), "image/png");
        assert!(response.headers().get(header::ACCEPT_RANGES).is_none());
        assert!(response.headers().get(header::CONTENT_RANGE).is_none());
        assert_eq!(response.body(), &vec![1, 2, 3]);
    }

    /// The `immutable` on the response is only true because the URL carries a
    /// content-derived version token. If one is ever dropped the other has to
    /// go with it.
    #[test]
    fn the_response_is_cached_immutably() {
        let cache = ok(Vec::new()).headers().get(header::CACHE_CONTROL).unwrap().clone();
        assert!(cache.to_str().unwrap().contains("immutable"));
    }

    #[test]
    fn a_failure_carries_its_status_and_not_an_image_content_type() {
        let response = refuse(TileError::NoSuchChapter("ch-01".into()));
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_ne!(response.headers().get(header::CONTENT_TYPE).unwrap(), "image/png");
        assert!(String::from_utf8(response.body().clone()).unwrap().contains("ch-01"));
    }

    fn a_longstrip(scratch: &Scratch, count: usize) -> Job {
        let raws = scratch.join("raws");
        std::fs::create_dir_all(&raws).unwrap();
        let mut references = Vec::new();
        for n in 0..count {
            let mut page = fixtures::by_name("l8").raster;
            page.data[0] = 10 * (n as u8 + 1);
            let bytes = encode(&page, Format::Png).unwrap();
            let source = raws.join(format!("{:03}.png", n + 1));
            std::fs::write(&source, &bytes).unwrap();
            references.push(cleaner_core::ingest::source_ref(&source, &bytes).unwrap());
        }

        let manifest = scratch.join("out/chapter.mtclean");
        std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        let project =
            Project::new(manifest.parent().unwrap(), "0.1.0", StripMode::Longstrip, &references);
        Job::create(&manifest, project).unwrap()
    }

    /// In longstrip mode, a patch anchored on page 0 that spans past the bottom
    /// edge into page 1 must render onto page 1's cleaned tile.
    #[test]
    fn a_longstrip_patch_spanning_a_join_renders_on_both_pages() {
        let scratch = Scratch::new("longstrip-span");
        let mut job = a_longstrip(&scratch, 2);
        let page = fixtures::by_name("l8").raster;
        let bounds = Rect::new(10, 40, 20, 16);
        let mut pixels = page.clone();
        pixels.width = bounds.w;
        pixels.height = bounds.h;
        pixels.icc = None;
        pixels.data = vec![215; (bounds.w * bounds.h) as usize];
        let patch = Patch {
            id: "spanning".into(),
            mask: Mask::filled(bounds),
            ink: Mask::filled(bounds),
            pixels,
            order: 0,
            visible: true,
            provenance: Provenance {
                engine: Engine::Fill,
                engine_version: "test".into(),
                model_sha256: None,
                execution_provider: "cpu".into(),
                params_snapshot: serde_json::json!({}),
                mask_sha256: "0".repeat(64),
                source_sha256: "0".repeat(64),
                cloud: None,
                created: 0,
            },
        };
        job.complete_region(0, &patch, None).unwrap();

        // Page 0 has the top part of the patch (y: 40..48)
        let page0 = decode(&render(job.path(), 0, Variant::Cleaned, None).unwrap()).unwrap();
        assert_eq!(page0.sample(15, 42, 0), 215, "page 0 did not render patch");

        // Page 1 has the bottom part of the patch (y: 0..8)
        let page1 = decode(&render(job.path(), 1, Variant::Cleaned, None).unwrap()).unwrap();
        assert_eq!(page1.sample(15, 2, 0), 215, "page 1 did not render spanning patch");
    }
}
