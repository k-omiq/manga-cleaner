//! Bounded upload planning in source page coordinates.
use crate::cloud_analysis_wire::{TileRect, MAX_TILE_SIDE};

pub const MAX_TILES: usize = 256;
pub const MAX_TOTAL_PIXELS: u64 = 64 * 1024 * 1024;
pub const MAX_ESTIMATED_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct UploadExtent {
    pub tiles: Vec<TileRect>,
    pub total_pixels: u64,
    pub encoded_byte_estimate: u64,
    pub pages: u32,
}

fn intersect(a: TileRect, b: TileRect) -> Option<TileRect> {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let right = a.x.saturating_add(a.width).min(b.x.saturating_add(b.width));
    let bottom = a.y.saturating_add(a.height).min(b.y.saturating_add(b.height));
    if right > x && bottom > y {
        Some(TileRect { x, y, width: right - x, height: bottom - y })
    } else {
        None
    }
}

pub fn plan(page_width: u32, page_height: u32, regions: &[TileRect]) -> Result<UploadExtent, String> {
    if page_width == 0 || page_height == 0 { return Err("empty analysis page".into()); }
    let page = TileRect { x: 0, y: 0, width: page_width, height: page_height };
    let requests = if regions.is_empty() { vec![page] } else { regions.to_vec() };
    for region in &requests {
        if region.width == 0 || region.height == 0 || region.x.checked_add(region.width).is_none_or(|v| v > page_width)
            || region.y.checked_add(region.height).is_none_or(|v| v > page_height)
        { return Err("analysis region outside page".into()); }
    }
    let mut tiles = Vec::new();
    let across = u64::from(page_width).div_ceil(u64::from(MAX_TILE_SIDE));
    let down = u64::from(page_height).div_ceil(u64::from(MAX_TILE_SIDE));
    if across.checked_mul(down).is_none_or(|v| v > 1_000_000) { return Err("analysis page grid too large".into()); }
    for row in 0..down {
        for column in 0..across {
            let x = (column * u64::from(MAX_TILE_SIDE)) as u32;
            let y = (row * u64::from(MAX_TILE_SIDE)) as u32;
            let tile = TileRect { x, y, width: MAX_TILE_SIDE.min(page_width - x), height: MAX_TILE_SIDE.min(page_height - y) };
            if requests.iter().any(|r| intersect(tile, *r).is_some()) {
                tiles.push(tile);
                if tiles.len() > MAX_TILES { return Err("analysis tile count exceeded".into()); }
            }
        }
    }
    let total_pixels = tiles.iter().map(|r| u64::from(r.width) * u64::from(r.height)).sum::<u64>();
    let encoded_byte_estimate = total_pixels.checked_mul(4).ok_or("analysis estimate overflow")?;
    if total_pixels > MAX_TOTAL_PIXELS || encoded_byte_estimate > MAX_ESTIMATED_BYTES {
        return Err("analysis upload extent exceeded".into());
    }
    Ok(UploadExtent { tiles, total_pixels, encoded_byte_estimate, pages: 1 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_tiles_long_strip_and_regions() {
        let region = TileRect { x: 71, y: 9000, width: 62, height: 1300 };
        let extent = plan(800, 20000, &[region]).unwrap();
        assert!(extent.tiles.iter().all(|t| t.x + t.width <= 800 && t.y + t.height <= 20000));
        for y in region.y..region.y + region.height {
            assert!(extent.tiles.iter().any(|t| t.y <= y && y < t.y + t.height && t.x <= region.x && region.x + region.width <= t.x + t.width));
        }
        assert_eq!(extent.total_pixels, extent.tiles.iter().map(|t| u64::from(t.width) * u64::from(t.height)).sum::<u64>());
        assert_eq!(extent.encoded_byte_estimate, extent.total_pixels * 4);
        assert_eq!(extent.pages, 1);
        let whole = plan(800, 20000, &[]).unwrap();
        assert_eq!(whole.total_pixels, 16_000_000);
        assert_eq!(whole.tiles.len(), 20);
        let crossing = TileRect { x: 1000, y: 1000, width: 50, height: 50 };
        let crossing_plan = plan(2048, 2048, &[crossing]).unwrap();
        assert_eq!(crossing_plan.tiles.len(), 4);
        for y in crossing.y..crossing.y + crossing.height {
            for x in crossing.x..crossing.x + crossing.width {
                assert!(crossing_plan.tiles.iter().any(|t| t.x <= x && x < t.x + t.width && t.y <= y && y < t.y + t.height));
            }
        }
        assert!(plan(800, 400_000, &[]).is_err());
    }
}
