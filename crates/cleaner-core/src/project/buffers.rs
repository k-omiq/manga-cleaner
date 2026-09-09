//! The `<job>.mtclean.d/` sidecar: masks and patch pixels on disk.
//!
//! They are put beside the manifest rather than inside it, and rule 6 says
//! what they are:
//!
//! > Patches are **raw planar buffers with a small header**, not PNG - PNG has
//! > no CMYK mode and no 16-bit palette, and CMYK TIFF is a required fixture.
//!
//! So this is deliberately the dullest possible container. Every field is
//! little-endian, the sample data is copied verbatim out of the [`Raster`] -
//! including the sub-byte row padding, which is the layout the decoder produced
//! and the one the exporter needs back - and nothing is compressed. A format
//! that re-encodes is a format that can lose something, and the fidelity
//! contract is about not losing things.

use std::path::Path;

use crate::image::{BitDepth, ColorMode, Raster};
use crate::mask::{Mask, Rect};

use super::StoreError;

const MASK_MAGIC: &[u8; 8] = b"MTCLEANM";
const PATCH_MAGIC: &[u8; 8] = b"MTCLEANP";
const FORMAT: u8 = 1;

/// Encode a mask: its rectangle, then one byte per pixel.
///
/// A byte rather than a bit, matching [`Mask`]'s own representation. The
/// packing would cost an eighth of a patch buffer that sits beside it at up to
/// eight bytes per pixel, and a file whose layout differs from memory is a file
/// that needs a converter with its own bugs.
pub fn encode_mask(mask: &Mask) -> Vec<u8> {
    let mut out = Vec::with_capacity(mask.bits.len() + 32);
    out.extend_from_slice(MASK_MAGIC);
    out.push(FORMAT);
    out.extend_from_slice(&mask.bounds.x.to_le_bytes());
    out.extend_from_slice(&mask.bounds.y.to_le_bytes());
    out.extend_from_slice(&mask.bounds.w.to_le_bytes());
    out.extend_from_slice(&mask.bounds.h.to_le_bytes());
    out.extend_from_slice(&mask.bits);
    out
}

pub fn decode_mask(bytes: &[u8]) -> Result<Mask, StoreError> {
    let mut reader = Reader::new(bytes, MASK_MAGIC)?;
    let x = reader.i64()?;
    let y = reader.i64()?;
    let w = reader.u32()?;
    let h = reader.u32()?;
    let bits = reader.take(w as usize * h as usize)?.to_vec();
    Ok(Mask { bounds: Rect { x, y, w, h }, bits })
}

/// Encode a patch's pixels, with everything the compositor and the exporter
/// need to put them back.
///
/// The palette and `tRNS` travel with the buffer even though the page carries
/// them too: a patch is refused at composite time unless its mode and depth
/// match the page's, and an indexed patch that came back without its palette
/// would pass that check while naming different colours.
pub fn encode_patch(raster: &Raster) -> Vec<u8> {
    let palette = raster.palette.as_deref().unwrap_or(&[]);
    let trns = raster.trns.as_deref().unwrap_or(&[]);

    let mut out = Vec::with_capacity(raster.data.len() + palette.len() + trns.len() + 64);
    out.extend_from_slice(PATCH_MAGIC);
    out.push(FORMAT);
    out.push(mode_code(raster.mode));
    out.push(raster.depth.bits());
    // Present-or-absent is not the same as empty: a zero-entry palette and no
    // palette at all round-trip differently through the encoder.
    out.push(u8::from(raster.palette.is_some()) | (u8::from(raster.trns.is_some()) << 1));
    out.extend_from_slice(&raster.width.to_le_bytes());
    out.extend_from_slice(&raster.height.to_le_bytes());
    out.extend_from_slice(&(palette.len() as u32).to_le_bytes());
    out.extend_from_slice(&(trns.len() as u32).to_le_bytes());
    out.extend_from_slice(&(raster.data.len() as u64).to_le_bytes());
    out.extend_from_slice(palette);
    out.extend_from_slice(trns);
    out.extend_from_slice(&raster.data);
    out
}

pub fn decode_patch(bytes: &[u8]) -> Result<Raster, StoreError> {
    let mut reader = Reader::new(bytes, PATCH_MAGIC)?;
    let mode = mode_of(reader.u8()?)?;
    let depth = BitDepth::from_bits(reader.u8()?)
        .ok_or_else(|| StoreError::Malformed("unknown bit depth in a patch buffer".into()))?;
    let flags = reader.u8()?;
    let width = reader.u32()?;
    let height = reader.u32()?;
    let palette_len = reader.u32()? as usize;
    let trns_len = reader.u32()? as usize;
    let data_len = reader.u64()? as usize;

    let palette = reader.take(palette_len)?.to_vec();
    let trns = reader.take(trns_len)?.to_vec();
    let data = reader.take(data_len)?.to_vec();

    let raster = Raster {
        width,
        height,
        mode,
        depth,
        icc: None,
        palette: (flags & 1 != 0).then_some(palette),
        trns: (flags & 2 != 0).then_some(trns),
        srgb_intent: None,
        data,
    };
    // The one invariant worth checking on the way in: a buffer whose length
    // disagrees with its own dimensions would panic inside `sample` at
    // composite time, a long way from the file that caused it.
    let expected = raster.stride() * height as usize;
    if raster.data.len() != expected {
        return Err(StoreError::Malformed(format!(
            "a patch buffer is {} bytes and its {width}×{height} {mode:?}/{depth:?} header needs {expected}",
            raster.data.len()
        )));
    }
    Ok(raster)
}

/// Write a file the same way the manifest is written: temp, fsync, rename.
///
/// The sidecar gets the same treatment as the manifest because §6's flush is
/// only atomic end to end if it is atomic at both ends - a manifest naming a
/// half-written buffer is exactly the crash the temp-and-rename exists to
/// survive.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    use std::io::Write;

    let temp = temp_path(path);
    {
        let mut file = std::fs::File::create(&temp).map_err(|e| StoreError::io(&temp, e))?;
        file.write_all(bytes).map_err(|e| StoreError::io(&temp, e))?;
        file.sync_all().map_err(|e| StoreError::io(&temp, e))?;
    }
    std::fs::rename(&temp, path).map_err(|e| StoreError::io(path, e))
}

/// `<name>.<pid>-<n>.tmp`, in the same directory - `rename` is only atomic
/// within one filesystem, and a temp directory is not guaranteed to be on the
/// same one.
///
/// The suffix is unique per call, and that is the whole point of it. A fixed
/// `<name>.tmp` is one file two writers of the same path both create and both
/// write into: the bytes interleave, and whichever `rename` runs second
/// publishes the mixture as if it were a whole file. The counter separates two
/// writers inside one process and the process id separates two app instances,
/// so each writer has a temp file nobody else is holding and `rename` decides
/// which whole file wins.
///
/// This is not the single-instance job lock. Two instances writing the same
/// manifest still race, and the loser's completed regions are still lost -
/// that lock belongs to Phase 6.
/// What this rules out is the third outcome, where neither writer's bytes are
/// what ends up on disk.
fn temp_path(path: &Path) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut name = path.as_os_str().to_owned();
    name.push(format!(
        ".{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::path::PathBuf::from(name)
}

fn mode_code(mode: ColorMode) -> u8 {
    match mode {
        ColorMode::Gray => 0,
        ColorMode::GrayAlpha => 1,
        ColorMode::Rgb => 2,
        ColorMode::Rgba => 3,
        ColorMode::Indexed => 4,
        ColorMode::Cmyk => 5,
    }
}

fn mode_of(code: u8) -> Result<ColorMode, StoreError> {
    Ok(match code {
        0 => ColorMode::Gray,
        1 => ColorMode::GrayAlpha,
        2 => ColorMode::Rgb,
        3 => ColorMode::Rgba,
        4 => ColorMode::Indexed,
        5 => ColorMode::Cmyk,
        other => return Err(StoreError::Malformed(format!("unknown colour mode {other}"))),
    })
}

/// A cursor that refuses to read past the end, so a truncated sidecar is an
/// error naming the file rather than a panic naming a slice index.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8], magic: &[u8; 8]) -> Result<Reader<'a>, StoreError> {
        if bytes.len() < 9 || &bytes[..8] != magic {
            return Err(StoreError::Malformed("not a buffer this reads".into()));
        }
        if bytes[8] != FORMAT {
            return Err(StoreError::Malformed(format!(
                "buffer format {}, and this build writes {FORMAT}",
                bytes[8]
            )));
        }
        Ok(Reader { bytes, at: 9 })
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], StoreError> {
        let end = self.at.checked_add(n).ok_or_else(|| StoreError::Malformed("length overflow".into()))?;
        if end > self.bytes.len() {
            return Err(StoreError::Malformed("truncated buffer".into()));
        }
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, StoreError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, StoreError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("4 bytes")))
    }

    fn u64(&mut self) -> Result<u64, StoreError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().expect("8 bytes")))
    }

    fn i64(&mut self) -> Result<i64, StoreError> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().expect("8 bytes")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::fixtures;

    #[test]
    fn every_fixture_round_trips_through_a_patch_buffer() {
        for fixture in fixtures::all() {
            let decoded = decode_patch(&encode_patch(&fixture.raster)).expect(fixture.name);
            assert_eq!(decoded.mode, fixture.raster.mode, "{}: mode", fixture.name);
            assert_eq!(decoded.depth, fixture.raster.depth, "{}: depth", fixture.name);
            assert_eq!(decoded.width, fixture.raster.width, "{}", fixture.name);
            assert_eq!(decoded.height, fixture.raster.height, "{}", fixture.name);
            assert_eq!(decoded.palette, fixture.raster.palette, "{}: palette", fixture.name);
            assert_eq!(decoded.trns, fixture.raster.trns, "{}: trns", fixture.name);
            assert_eq!(decoded.data, fixture.raster.data, "{}: samples", fixture.name);
        }
    }

    #[test]
    fn a_mask_round_trips_with_its_rectangle() {
        let mut mask = Mask::empty(Rect::new(-4, 17, 9, 5));
        mask.set(0, 18, true);
        mask.set(4, 20, true);
        assert_eq!(decode_mask(&encode_mask(&mask)).unwrap(), mask);
    }

    #[test]
    fn a_truncated_buffer_is_an_error_rather_than_a_panic() {
        let whole = encode_patch(&fixtures::by_name("rgb16").raster);
        assert!(matches!(decode_patch(&whole[..whole.len() / 2]), Err(StoreError::Malformed(_))));
        assert!(matches!(decode_patch(b"nothing"), Err(StoreError::Malformed(_))));
    }

    /// The sub-byte case, singled out because it is the one where "the data is
    /// `width × height × samples`" is wrong and the padding is load-bearing.
    #[test]
    fn a_bitonal_buffer_keeps_its_row_padding() {
        let source = fixtures::by_name("bitonal").raster;
        let decoded = decode_patch(&encode_patch(&source)).unwrap();
        assert_eq!(decoded.stride(), source.stride());
        assert_eq!(decoded.data.len(), source.data.len());
    }

    /// Two writers of the same path must not be handed the same temp file. A
    /// fixed `<name>.tmp` gives both of them one file to write into, and the
    /// second `rename` then publishes an interleaving of two writes as though
    /// it were one whole file.
    #[test]
    fn two_writes_to_one_path_do_not_share_a_temp_file() {
        let path = std::path::Path::new("/tmp/library/index.json");
        let first = temp_path(path);
        let second = temp_path(path);
        assert_ne!(first, second, "two writers were given the same temp path");
        // Still beside the file it will be renamed onto: `rename` is atomic
        // only within one filesystem.
        assert_eq!(first.parent(), path.parent());
        assert!(first.to_string_lossy().ends_with(".tmp"), "{first:?}");
        assert!(first.to_string_lossy().starts_with("/tmp/library/index.json."), "{first:?}");
    }

    /// The property the unique name buys, end to end: whatever lands on disk is
    /// one writer's bytes entire, never a mixture of two.
    #[test]
    fn concurrent_writers_leave_one_whole_file_rather_than_a_mixture() {
        let dir = std::env::temp_dir().join("manga-cleaner-atomic-write");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("index.json");

        // Different lengths as well as different bytes, so an interleaving is
        // detectable however the two writes land on each other.
        let payloads: Vec<Vec<u8>> = (0..8u8).map(|n| vec![n; 64 * 1024 * (n as usize + 1)]).collect();
        std::thread::scope(|scope| {
            for payload in &payloads {
                let path = path.clone();
                scope.spawn(move || write_atomic(&path, payload).unwrap());
            }
        });

        let written = std::fs::read(&path).unwrap();
        assert!(payloads.contains(&written), "the published file is not any one writer's bytes");
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name())
            .filter(|name| name.to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp files were left behind: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
