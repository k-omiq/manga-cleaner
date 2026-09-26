//! Bounded, ordered input for an edit. The raw source digest remains a separate
//! identity; this digest describes the pixels a renderer actually reads.

use cleaner_core::composite::composite_region;
use cleaner_core::engines::render::PreparedRender;
use cleaner_core::fit::{self, EdgeMap};
use cleaner_core::image::Raster;
use cleaner_core::mask::{Mask, Rect};
use cleaner_core::patch::Patch;
use cleaner_core::project::Job;
use cleaner_core::project::PatchRecord;
use cleaner_core::strip::Strip;
use cleaner_core::strip::{DecodeWindow, EdgePad, JoinState, Joins};
use sha2::{Digest, Sha256};
use std::borrow::Cow;

#[cfg(test)]
thread_local! {
    static PATCH_LOADS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub(crate) const READ_MARGIN: i64 = 768;
const MAX_READ_PIXELS: u64 = 4096 * 4096;

pub(crate) struct Input {
    pub image: Raster,
    /// Bounds within `raw`, before translation to the strip or source page.
    pub window: Rect,
    pub digest: String,
    /// Hash of complete visible predecessor records and patch bytes that
    /// intersect the read window, including order and visibility.
    pub predecessors: String,
}

#[derive(Clone, Copy)]
pub(crate) struct StripSpace<'a> {
    pub strip: &'a Strip,
    pub origin: (i64, i64),
}

fn intersection(a: Rect, b: Rect) -> Option<Rect> {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let r = a.right().min(b.right());
    let bottom = a.bottom().min(b.bottom());
    (r > x && bottom > y).then(|| Rect::new(x, y, (r - x) as u32, (bottom - y) as u32))
}

pub(crate) fn window_for(bounds: Rect, width: u32, height: u32) -> Result<Rect, String> {
    if bounds.w == 0 || bounds.h == 0 {
        return Err("empty edit bounds".into());
    }
    let x = bounds.x.saturating_sub(READ_MARGIN).max(0);
    let y = bounds.y.saturating_sub(READ_MARGIN).max(0);
    let right = bounds.right().saturating_add(READ_MARGIN).min(width as i64);
    let bottom = bounds
        .bottom()
        .saturating_add(READ_MARGIN)
        .min(height as i64);
    if right <= x || bottom <= y {
        return Err("edit bounds outside source".into());
    }
    let window = Rect::new(x, y, (right - x) as u32, (bottom - y) as u32);
    if u64::from(window.w) * u64::from(window.h) > MAX_READ_PIXELS {
        return Err("edit context exceeds bounded read budget".into());
    }
    Ok(window)
}

pub(crate) fn read(
    job: &Job,
    source_idx: usize,
    raw: &Raster,
    bounds: Rect,
    below: u32,
    strip: Option<StripSpace<'_>>,
) -> Result<Input, String> {
    let window = window_for(bounds, raw.width, raw.height)?;
    read_exact(job, source_idx, raw, window, below, strip)
}

pub(crate) fn read_exact(
    job: &Job,
    source_idx: usize,
    raw: &Raster,
    window: Rect,
    below: u32,
    strip: Option<StripSpace<'_>>,
) -> Result<Input, String> {
    if window.w == 0
        || window.h == 0
        || window.x < 0
        || window.y < 0
        || window.right() > raw.width as i64
        || window.bottom() > raw.height as i64
        || u64::from(window.w) * u64::from(window.h) > MAX_READ_PIXELS
    {
        return Err("invalid bounded read window".into());
    }
    let mut patches = Vec::new();
    let mut identities = Sha256::new();
    identities.update(b"visible_predecessors_v1\0");
    for record in &job.project.patches {
        if !record.visible || record.order >= below {
            continue;
        }
        let translation = if let Some(space) = strip {
            let Some((_, placed)) = space
                .strip
                .pages()
                .iter()
                .enumerate()
                .find(|(position, _)| {
                    job.project.strip.order.get(*position) == Some(&record.source_idx)
                })
            else {
                continue;
            };
            (
                placed.x_offset - space.origin.0,
                placed.y_offset - space.origin.1,
            )
        } else {
            if record.source_idx != source_idx {
                continue;
            }
            (0, 0)
        };
        let translated = Rect::new(
            record.bbox.x + translation.0,
            record.bbox.y + translation.1,
            record.bbox.w,
            record.bbox.h,
        );
        if intersection(translated, window).is_none() {
            continue;
        }
        #[cfg(test)]
        PATCH_LOADS.with(|count| count.set(count.get() + 1));
        let mut patch = job.load_patch(record).map_err(|e| e.to_string())?;
        patch.mask.bounds.x += translation.0;
        patch.mask.bounds.y += translation.1;
        patch.ink.bounds.x += translation.0;
        patch.ink.bounds.y += translation.1;
        identities.update(serde_json::to_vec(record).map_err(|e| e.to_string())?);
        identities.update(&patch.mask.bits);
        identities.update(&patch.ink.bits);
        identities.update(&patch.pixels.data);
        patches.push(patch);
    }
    let image = composite_region(raw, &patches, window, Some(below)).map_err(|e| e.to_string())?;
    if image.width != window.w || image.height != window.h {
        return Err("compositor returned an undersized input window".into());
    }
    let mut digest = Sha256::new();
    digest.update(b"edit_input_v1\0");
    let (source_x, source_y) = if let Some(space) = strip {
        let (_, anchor) = space
            .strip
            .pages()
            .iter()
            .enumerate()
            .find(|(position, _)| job.project.strip.order.get(*position) == Some(&source_idx))
            .ok_or("source absent from strip during input digest")?;
        (
            window.x + space.origin.0 - anchor.x_offset,
            window.y + space.origin.1 - anchor.y_offset,
        )
    } else {
        (window.x, window.y)
    };
    digest.update(source_x.to_le_bytes());
    digest.update(source_y.to_le_bytes());
    digest.update(window.w.to_le_bytes());
    digest.update(window.h.to_le_bytes());
    digest.update(serde_json::to_vec(&(image.mode, image.depth)).map_err(|e| e.to_string())?);
    digest.update(&image.data);
    Ok(Input {
        image,
        window,
        digest: format!("{:x}", digest.finalize()),
        predecessors: format!("{:x}", identities.finalize()),
    })
}

fn read_record_footprint(
    job: &Job,
    record: &PatchRecord,
    footprint: Rect,
    strip: &Strip,
    decoded_source_idx: usize,
    decoded_source: &Raster,
) -> Result<Input, String> {
    if job.project.strip.mode != cleaner_core::project::StripMode::Longstrip {
        if record.source_idx != decoded_source_idx {
            return Err("dependency source changed".into());
        }
        return read_exact(
            job,
            record.source_idx,
            decoded_source,
            footprint,
            record.order,
            None,
        );
    }
    let (_, anchor) = strip
        .pages()
        .iter()
        .enumerate()
        .find(|(position, _)| job.project.strip.order.get(*position) == Some(&record.source_idx))
        .ok_or("dependency anchor missing from strip")?;
    let global = Rect::new(
        footprint.x + anchor.x_offset,
        footprint.y + anchor.y_offset,
        footprint.w,
        footprint.h,
    );
    if global.w == 0
        || global.h == 0
        || global.x < 0
        || global.y < 0
        || global.right() > strip.width() as i64
        || global.bottom() > strip.height() as i64
        || u64::from(global.w) * u64::from(global.h) > MAX_READ_PIXELS
    {
        return Err("invalid strip dependency footprint".into());
    }
    let mut joins = Joins::unchecked(strip.joins());
    for join in 0..strip.joins() {
        joins.set(join, JoinState::Verified);
    }
    let rect = strip.clamp(global, &joins);
    let decoded = cleaner_core::strip::read_window_borrowing(
        strip,
        DecodeWindow {
            rect,
            requested: global,
            pad: EdgePad::None,
        },
        |position| {
            let source_idx = *job
                .project
                .strip
                .order
                .get(position)
                .ok_or("dependency strip source missing")?;
            if source_idx == decoded_source_idx {
                return Ok(Cow::Borrowed(decoded_source));
            }
            let path = job
                .source_path(source_idx)
                .ok_or("dependency source file missing")?;
            let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
            cleaner_core::image::decode(&bytes)
                .map(Cow::Owned)
                .map_err(|e| e.to_string())
        },
    )?;
    read_exact(
        job,
        record.source_idx,
        &decoded.raster,
        Rect::new(0, 0, rect.w, rect.h),
        record.order,
        Some(StripSpace {
            strip,
            origin: decoded.origin,
        }),
    )
}

/// Called under the job lock after an earlier layer is changed. Existing
/// output remains committed; only the review state changes. Old records have
/// no read provenance and are marked unknown only if their likely context
/// intersects the changed patch.
pub(crate) fn refresh_dependencies(job: &mut Job, changed_id: &str) -> Result<(), String> {
    let Some(changed) = job
        .project
        .patches
        .iter()
        .find(|p| p.id == changed_id)
        .cloned()
    else {
        return Ok(());
    };
    let strip = crate::run::strip_of(&job.project);
    let changed_place = strip
        .pages()
        .iter()
        .enumerate()
        .find(|(position, _)| job.project.strip.order.get(*position) == Some(&changed.source_idx))
        .map(|(_, p)| *p);
    let later = job
        .project
        .patches
        .iter()
        .enumerate()
        .filter(|(_, p)| {
            p.visible
                && p.order > changed.order
                && (p.source_idx == changed.source_idx
                    || job.project.strip.mode == cleaner_core::project::StripMode::Longstrip)
        })
        .map(|(i, p)| (i, p.clone()))
        .collect::<Vec<_>>();
    if later.is_empty() {
        return Ok(());
    }
    let source = job
        .source_path(changed.source_idx)
        .ok_or("source missing for dependency review")?;
    let bytes = std::fs::read(source).map_err(|e| e.to_string())?;
    let raw = cleaner_core::image::decode(&bytes).map_err(|e| e.to_string())?;
    let mut dirty = false;
    for (index, record) in later {
        let changed_box = if record.source_idx == changed.source_idx {
            changed.bbox
        } else {
            let Some(from) = changed_place else { continue };
            let Some((_, to)) = strip.pages().iter().enumerate().find(|(position, _)| {
                job.project.strip.order.get(*position) == Some(&record.source_idx)
            }) else {
                continue;
            };
            Rect::new(
                changed.bbox.x + from.x_offset - to.x_offset,
                changed.bbox.y + from.y_offset - to.y_offset,
                changed.bbox.w,
                changed.bbox.h,
            )
        };
        let provenance = record.provenance.params_snapshot.get("input_provenance");
        let footprint = provenance
            .and_then(|p| p.get("read_footprint"))
            .and_then(|v| serde_json::from_value::<Rect>(v.clone()).ok())
            .unwrap_or_else(|| {
                window_for(
                    record.bbox,
                    job.project.sources[record.source_idx].w,
                    job.project.sources[record.source_idx].h,
                )
                .unwrap_or(record.bbox)
            });
        if intersection(footprint, changed_box).is_none() {
            continue;
        }
        let next = if let Some(expected) = provenance
            .and_then(|p| p.get("input_sha256"))
            .and_then(|v| v.as_str())
        {
            match read_record_footprint(job, &record, footprint, &strip, changed.source_idx, &raw) {
                Ok(input) if input.digest == expected => None,
                Ok(_) => Some("review.reason.inputChanged".to_owned()),
                Err(_) => Some("review.reason.inputUnknown".to_owned()),
            }
        } else {
            Some("review.reason.inputUnknown".to_owned())
        };
        let current = &mut job.project.patches[index];
        let dependency = |state: Option<&str>| {
            matches!(
                state,
                Some("review.reason.inputChanged" | "review.reason.inputUnknown")
            )
        };
        if next.is_none() && !dependency(current.review_state.as_deref()) {
            continue;
        }
        if current.review_state != next {
            if !dependency(current.review_state.as_deref()) && dependency(next.as_deref()) {
                current.provenance.params_snapshot["review_before_input_change"] =
                    serde_json::json!(current.review_state);
            }
            current.review_state = next.or_else(|| {
                current
                    .provenance
                    .params_snapshot
                    .get("review_before_input_change")
                    .and_then(|value| value.as_str())
                    .map(str::to_owned)
            });
            dirty = true;
        }
    }
    if dirty {
        job.flush().map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub(crate) fn local_mask(mask: &Mask, window: Rect) -> Mask {
    let mut result = mask.clone();
    result.bounds.x -= window.x;
    result.bounds.y -= window.y;
    result
}

pub(crate) fn page_rect(local: Rect, window: Rect) -> Rect {
    Rect::new(local.x + window.x, local.y + window.y, local.w, local.h)
}

/// A renderer may replicate a true page edge, but it cannot mistake an
/// insufficient composited window edge for the page edge.
pub(crate) fn require_footprint(
    input: &Input,
    footprint: Rect,
    raw: &Raster,
) -> Result<(), String> {
    let page = Rect::new(0, 0, raw.width, raw.height);
    if let Some(needed) = intersection(page_rect(footprint, input.window), page) {
        let covered = intersection(needed, input.window);
        if covered != Some(needed) {
            return Err("model read footprint exceeds composited window".into());
        }
    }
    Ok(())
}

pub(crate) struct CloudInput {
    pub input: Input,
    pub prepared: PreparedRender,
    pub crop_bounds: Rect,
    /// Map coordinates in the decoded strip window back to the anchor source.
    pub source_shift: (i64, i64),
}

pub(crate) fn prepare_cloud(
    job: &Job,
    source_idx: usize,
    raw: &Raster,
    record: &PatchRecord,
    patch: &Patch,
) -> Result<CloudInput, String> {
    if !record.visible {
        return Err("cloud target is hidden or deleted".into());
    }
    if patch.mask.is_empty() {
        return Err("seed mask is empty".into());
    }
    let strip = crate::run::strip_of(&job.project);
    let strip_read = if job.project.strip.mode == cleaner_core::project::StripMode::Longstrip {
        let (position, anchor) = strip
            .pages()
            .iter()
            .enumerate()
            .find(|(position, _)| job.project.strip.order.get(*position) == Some(&source_idx))
            .ok_or("anchor page is missing from strip")?;
        let global = Rect::new(
            patch.mask.bounds.x + anchor.x_offset,
            patch.mask.bounds.y + anchor.y_offset,
            patch.mask.bounds.w,
            patch.mask.bounds.h,
        );
        let requested = Rect::new(
            global.x.saturating_sub(1024).max(0),
            global.y.saturating_sub(1024).max(0),
            0,
            0,
        );
        let right = global
            .right()
            .saturating_add(1024)
            .min(strip.width() as i64);
        let bottom = global
            .bottom()
            .saturating_add(1024)
            .min(strip.height() as i64);
        if right <= requested.x || bottom <= requested.y {
            return Err("cloud target outside strip".into());
        }
        let requested = Rect::new(
            requested.x,
            requested.y,
            (right - requested.x) as u32,
            (bottom - requested.y) as u32,
        );
        let mut joins = Joins::unchecked(strip.joins());
        for join in 0..strip.joins() {
            joins.set(join, JoinState::Verified);
        }
        let rect = strip.clamp(requested, &joins);
        if u64::from(rect.w) * u64::from(rect.h) > MAX_READ_PIXELS {
            return Err("cloud strip context exceeds bounded read budget".into());
        }
        let window = DecodeWindow {
            rect,
            requested,
            pad: EdgePad::None,
        };
        let decoded = cleaner_core::strip::read_window_borrowing(&strip, window, |at| {
            if at == position {
                return Ok(Cow::Borrowed(raw));
            }
            let other_source = *job
                .project
                .strip
                .order
                .get(at)
                .ok_or_else(|| format!("strip source {at} missing"))?;
            let path = job
                .source_path(other_source)
                .ok_or_else(|| format!("strip source {at} has no file"))?;
            let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
            cleaner_core::image::decode(&bytes)
                .map(Cow::Owned)
                .map_err(|e| e.to_string())
        })?;
        let source_shift = (
            decoded.origin.0 - anchor.x_offset,
            decoded.origin.1 - anchor.y_offset,
        );
        Some((decoded, source_shift))
    } else {
        None
    };
    let (read_page, source_shift) = strip_read
        .as_ref()
        .map_or((raw, (0, 0)), |(decoded, shift)| (&decoded.raster, *shift));
    let local_bounds = Rect::new(
        patch.mask.bounds.x - source_shift.0,
        patch.mask.bounds.y - source_shift.1,
        patch.mask.bounds.w,
        patch.mask.bounds.h,
    );
    let strip_space = strip_read.as_ref().map(|(decoded, _)| StripSpace {
        strip: &strip,
        origin: decoded.origin,
    });
    let input = read(
        job,
        source_idx,
        read_page,
        local_bounds,
        record.order,
        strip_space,
    )?;
    let mut seed = patch.mask.clone();
    seed.bounds.x -= source_shift.0;
    seed.bounds.y -= source_shift.1;
    let seed = local_mask(&seed, input.window);
    let noise = fit::page_noise_sigma(&input.image);
    let edges = EdgeMap::sobel(&input.image);
    let fitted = fit::fit(&input.image, &seed, 1.0, noise, &edges, true);
    let applied =
        cleaner_core::engines::model::applied_mask(&fitted, input.image.width, input.image.height);
    let crop = cleaner_core::engines::render::crop_for(applied.bounds);
    require_footprint(&input, crop, read_page)?;
    let prepared = PreparedRender::prepare(&input.image, &fitted).map_err(|e| e.to_string())?;
    let crop_bounds = page_rect(
        prepared.crop(),
        Rect::new(
            input.window.x + source_shift.0,
            input.window.y + source_shift.1,
            0,
            0,
        ),
    );
    Ok(CloudInput {
        input,
        prepared,
        crop_bounds,
        source_shift,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cleaner_core::image::{encode, BitDepth, ColorMode, Format};
    use cleaner_core::ingest::source_ref;
    use cleaner_core::patch::{Engine, Provenance};
    use cleaner_core::project::{Project, StripMode};

    #[test]
    fn dependency_footprint_clamps_across_narrow_strip_join() {
        let root = std::env::temp_dir().join(format!(
            "mc-underlay-narrow-join-{}", std::process::id()
        ));
        let raws = root.join("raws");
        std::fs::create_dir_all(&raws).unwrap();
        let gray = |width: u32| Raster {
            width, height: 10, mode: ColorMode::Gray, depth: BitDepth::Eight,
            icc: None, palette: None, trns: None, srgb_intent: None,
            data: vec![255; width as usize * 10],
        };
        let sources = [gray(10), gray(8)].into_iter().enumerate().map(|(index, raw)| {
            let path = raws.join(format!("{index}.png"));
            let bytes = encode(&raw, Format::Png).unwrap();
            std::fs::write(&path, &bytes).unwrap();
            source_ref(&path, &bytes).unwrap()
        }).collect::<Vec<_>>();
        let manifest = root.join("job/chapter.mtclean");
        std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        let job = Job::create(&manifest, Project::new(
            manifest.parent().unwrap(), "fixture", StripMode::Longstrip, &sources,
        )).unwrap();
        let bounds = Rect::new(0, 8, 10, 2);
        let mask = Mask::filled(bounds);
        let patch = Patch {
            id: "dependent".into(), mask: mask.clone(), ink: mask,
            pixels: Raster { height: 2, data: vec![255; 20], ..gray(10) },
            order: 0, visible: true,
            provenance: Provenance {
                engine: Engine::Fill, engine_version: "fixture".into(),
                model_sha256: None, execution_provider: "cpu".into(),
                params_snapshot: serde_json::json!({}), mask_sha256: String::new(),
                source_sha256: String::new(), cloud: None, created: 0,
            },
        };
        let record = PatchRecord::of(0, &patch, None);
        let input = read_record_footprint(
            &job, &record, Rect::new(0, 8, 10, 4),
            &crate::run::strip_of(&job.project), 0, &gray(10),
        ).unwrap();
        assert_eq!((input.image.width, input.image.height), (8, 4));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn insufficient_model_context_is_refused() {
        let raw = Raster {
            width: 100,
            height: 100,
            mode: ColorMode::Gray,
            depth: BitDepth::Eight,
            icc: None,
            palette: None,
            trns: None,
            srgb_intent: None,
            data: vec![255; 100 * 100],
        };
        let input = Input {
            image: Raster {
                width: 20,
                height: 20,
                data: vec![255; 20 * 20],
                ..raw.clone()
            },
            window: Rect::new(10, 10, 20, 20),
            digest: String::new(),
            predecessors: String::new(),
        };
        assert!(require_footprint(&input, Rect::new(0, 0, 30, 30), &raw).is_err());
        assert!(window_for(Rect::new(10, 10, 4000, 4000), 6000, 6000).is_err());
    }

    /// Run explicitly with `--ignored --nocapture`; reports measured process
    /// peak as well as the bounded composited buffer on this host.
    #[test]
    #[ignore]
    fn bounded_4000x6000_dense_history_benchmark() {
        for (mode, channels) in [(ColorMode::Gray, 1), (ColorMode::Rgb, 3)] {
            let started = std::time::Instant::now();
            let root = std::env::temp_dir().join(format!("mc-underlay-bench-{}-{channels}", std::process::id()));
            std::fs::create_dir_all(&root).unwrap();
            let raw = Raster {
                width: 4000,
                height: 6000,
                mode,
                depth: BitDepth::Eight,
                icc: None,
                palette: None,
                trns: None,
                srgb_intent: None,
                data: vec![255; 4000 * 6000 * channels],
            };
            let bytes = encode(&raw, Format::Png).unwrap();
            let source_path = root.join("page.png");
            std::fs::write(&source_path, &bytes).unwrap();
            let source = source_ref(&source_path, &bytes).unwrap();
            let manifest = root.join("job/chapter.mtclean");
            std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
            let mut job = Job::create(
                &manifest,
                Project::new(
                    manifest.parent().unwrap(),
                    "bench",
                    StripMode::Single,
                    &[source],
                ),
            )
            .unwrap();
            for order in 0..128 {
                let bounds = if order < 96 {
                    Rect::new(1940 + (order % 8) as i64 * 3,
                        2940 + (order % 8) as i64 * 3, 64, 64)
                } else {
                    Rect::new(100 + (order % 8) as i64 * 3, 100, 64, 64)
                };
                let mask = Mask::filled(bounds);
                let patch = Patch {
                    id: format!("p{order}"),
                    mask: mask.clone(),
                    ink: mask,
                    pixels: Raster {
                        width: 64,
                        height: 64,
                        mode,
                        depth: BitDepth::Eight,
                        icc: None,
                        palette: None,
                        trns: None,
                        srgb_intent: None,
                        data: vec![order as u8; 64 * 64 * channels],
                    },
                    order,
                    visible: true,
                    provenance: Provenance {
                        engine: Engine::Fill,
                        engine_version: "bench".into(),
                        model_sha256: None,
                        execution_provider: "cpu".into(),
                        params_snapshot: serde_json::json!({}),
                        mask_sha256: String::new(),
                        source_sha256: String::new(),
                        cloud: None,
                        created: 0,
                    },
                };
                job.complete_region(0, &patch, None).unwrap();
            }
            let outside: Vec<_> = job.project.patches.iter()
                .filter(|record| record.order >= 96)
                .map(|record| cleaner_core::project::sidecar_dir(&manifest).join(&record.buffer_ref))
                .collect();
            assert_eq!(outside.len(), 32);
            for path in &outside {
                std::fs::remove_file(path).unwrap();
            }
            PATCH_LOADS.with(|count| count.set(0));
            let read_started = std::time::Instant::now();
            let input = read(&job, 0, &raw, Rect::new(1970, 2970, 80, 80), 128, None).unwrap();
            let loaded = PATCH_LOADS.with(std::cell::Cell::get);
            assert_eq!(loaded, 96);
            let read_ms = read_started.elapsed().as_millis();
            assert!(input.image.width <= 80 + READ_MARGIN as u32 * 2);
            assert!(input.image.height <= 80 + READ_MARGIN as u32 * 2);
            assert!(input.image.data.len() < raw.data.len() / 4);
            let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
            let peak = if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } == 0 {
                unsafe { usage.assume_init().ru_maxrss }
            } else {
                0
            };
            println!(
                "mode={mode:?} raw={} window={} patches_loaded={} patches_outside_unloaded={} peak_rss={} read_ms={} total_ms={}",
                raw.data.len(),
                input.image.data.len(),
                loaded,
                outside.len(),
                peak,
                read_ms,
                started.elapsed().as_millis()
            );
            std::fs::remove_dir_all(root).ok();
        }
    }
}
