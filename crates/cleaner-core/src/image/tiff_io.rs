//! TIFF, which is here for the one thing PNG has no colour type for: CMYK.
//!
//! It is also the right choice for stitched longstrip output, where a
//! single file runs past what PNG is comfortable with.
//!
//! One conversion happens at this boundary and nowhere else. A [`Raster`] holds
//! 16-bit samples as **big-endian bytes**, because that is what PNG produces and
//! what TIFF's own `MM` byte order uses; the `tiff` crate works in native-order
//! `u16`, so this module swaps on the way in and out. Everything above sees one
//! layout.

use std::io::{Cursor, Seek, Write};

use tiff::decoder::{Decoder, DecodingResult};
use tiff::encoder::{TiffEncoder, colortype};
use tiff::tags::Tag;

use super::{BitDepth, ColorMode, Format, Header, ImageError, Raster};

impl From<tiff::TiffError> for ImageError {
    fn from(e: tiff::TiffError) -> Self {
        ImageError::Tiff(e.to_string())
    }
}

/// TIFF tag 34675, `ICC Profile`. Not in the crate's `Tag` enum.
const TAG_ICC_PROFILE: Tag = Tag::Unknown(34675);

/// The first IFD, without reading a strip.
pub fn header(reader: Cursor<&[u8]>) -> Result<Header, ImageError> {
    let mut decoder = Decoder::new(reader)?;
    let (width, height) = decoder.dimensions()?;
    let (mode, depth) = mode_and_depth(decoder.colortype()?)?;
    let icc = decoder.find_tag(TAG_ICC_PROFILE)?.map(|value| value.into_u8_vec()).transpose()?;
    Ok(Header { width, height, mode, depth, icc })
}

/// The one place TIFF's colour type is mapped, so the header read and the full
/// decode cannot disagree about what a file is.
fn mode_and_depth(color: tiff::ColorType) -> Result<(ColorMode, BitDepth), ImageError> {
    Ok(match color {
        tiff::ColorType::Gray(8) => (ColorMode::Gray, BitDepth::Eight),
        tiff::ColorType::Gray(16) => (ColorMode::Gray, BitDepth::Sixteen),
        tiff::ColorType::GrayA(8) => (ColorMode::GrayAlpha, BitDepth::Eight),
        tiff::ColorType::GrayA(16) => (ColorMode::GrayAlpha, BitDepth::Sixteen),
        tiff::ColorType::RGB(8) => (ColorMode::Rgb, BitDepth::Eight),
        tiff::ColorType::RGB(16) => (ColorMode::Rgb, BitDepth::Sixteen),
        tiff::ColorType::RGBA(8) => (ColorMode::Rgba, BitDepth::Eight),
        tiff::ColorType::RGBA(16) => (ColorMode::Rgba, BitDepth::Sixteen),
        tiff::ColorType::CMYK(8) => (ColorMode::Cmyk, BitDepth::Eight),
        tiff::ColorType::CMYK(16) => (ColorMode::Cmyk, BitDepth::Sixteen),
        other => return Err(ImageError::Tiff(format!("unsupported colour type {other:?}"))),
    })
}

pub fn decode(reader: Cursor<&[u8]>) -> Result<Raster, ImageError> {
    let mut decoder = Decoder::new(reader)?;
    let (width, height) = decoder.dimensions()?;
    let color = decoder.colortype()?;

    let (mode, depth) = mode_and_depth(color)?;

    let icc = decoder
        .find_tag(TAG_ICC_PROFILE)?
        .map(|value| value.into_u8_vec())
        .transpose()?;

    let data = match decoder.read_image()? {
        DecodingResult::U8(samples) => samples,
        DecodingResult::U16(samples) => {
            let mut bytes = Vec::with_capacity(samples.len() * 2);
            for sample in samples {
                bytes.extend_from_slice(&sample.to_be_bytes());
            }
            bytes
        }
        other => {
            return Err(ImageError::Tiff(format!(
                "unsupported sample type {}",
                match other {
                    DecodingResult::U32(_) => "u32",
                    DecodingResult::U64(_) => "u64",
                    DecodingResult::F32(_) => "f32",
                    DecodingResult::F64(_) => "f64",
                    _ => "unknown",
                }
            )));
        }
    };

    Ok(Raster {
        width,
        height,
        mode,
        depth,
        icc,
        palette: None,
        trns: None,
        srgb_intent: None,
        data,
    })
}

pub fn encode(raster: &Raster) -> Result<Vec<u8>, ImageError> {
    let mut out = Cursor::new(Vec::new());
    {
        let mut encoder = TiffEncoder::new(&mut out)?;
        match (raster.mode, raster.depth) {
            (ColorMode::Gray, BitDepth::Eight) => write::<colortype::Gray8, _>(&mut encoder, raster)?,
            (ColorMode::Gray, BitDepth::Sixteen) => write::<colortype::Gray16, _>(&mut encoder, raster)?,
            (ColorMode::Rgb, BitDepth::Eight) => write::<colortype::RGB8, _>(&mut encoder, raster)?,
            (ColorMode::Rgb, BitDepth::Sixteen) => write::<colortype::RGB16, _>(&mut encoder, raster)?,
            (ColorMode::Rgba, BitDepth::Eight) => write::<colortype::RGBA8, _>(&mut encoder, raster)?,
            (ColorMode::Rgba, BitDepth::Sixteen) => write::<colortype::RGBA16, _>(&mut encoder, raster)?,
            (ColorMode::Cmyk, BitDepth::Eight) => write::<colortype::CMYK8, _>(&mut encoder, raster)?,
            (ColorMode::Cmyk, BitDepth::Sixteen) => write::<colortype::CMYK16, _>(&mut encoder, raster)?,
            // Sub-byte depths, palettes and gray+alpha: the crate has no
            // `RGBPalette` and no `GrayA` *encoder* colour type, and all bit
            // depths must be equal. Declared unrepresentable rather than
            // silently promoted. Nothing routes here in practice,
            // because `lossless_format_for` sends everything but CMYK to PNG.
            (mode, depth) => return Err(ImageError::Unrepresentable { format: Format::Tiff, mode, depth }),
        }
    }
    Ok(out.into_inner())
}

/// One generic body for every colour type, so the ICC tag is written in exactly
/// one place and cannot be forgotten for a mode added later.
fn write<C, W>(encoder: &mut TiffEncoder<W>, raster: &Raster) -> Result<(), ImageError>
where
    C: colortype::ColorType,
    W: Write + Seek,
    [C::Inner]: tiff::encoder::TiffValue,
    C::Inner: SampleFromBytes,
{
    let mut image = encoder.new_image::<C>(raster.width, raster.height)?;
    if let Some(icc) = &raster.icc {
        image.encoder().write_tag(TAG_ICC_PROFILE, icc.as_slice())?;
    }
    let samples = C::Inner::from_be_bytes(&raster.data);
    image.write_data(&samples)?;
    Ok(())
}

/// Turning the raster's big-endian byte buffer back into the crate's native
/// sample type. Only `u8` and `u16` are reachable from [`encode`]'s match.
pub trait SampleFromBytes: Sized {
    fn from_be_bytes(bytes: &[u8]) -> Vec<Self>;
}

impl SampleFromBytes for u8 {
    fn from_be_bytes(bytes: &[u8]) -> Vec<u8> {
        bytes.to_vec()
    }
}

impl SampleFromBytes for u16 {
    fn from_be_bytes(bytes: &[u8]) -> Vec<u16> {
        bytes.chunks_exact(2).map(|pair| u16::from_be_bytes([pair[0], pair[1]])).collect()
    }
}
