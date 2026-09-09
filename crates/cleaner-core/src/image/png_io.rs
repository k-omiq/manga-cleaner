//! PNG, with `Transformations::IDENTITY` on both sides.
//!
//! The decoder default is already IDENTITY, which is the whole reason
//! libvips was dropped: indexed stays indexed and 16-bit stays 16-bit,
//! rather than being expanded to RGB8 at load.
//! It is set explicitly anyway, because a default that the fidelity contract
//! depends on should be visible at the place that depends on it.

use std::io::Cursor;

use png::{BitDepth as PngDepth, ColorType, Decoder, Encoder, Info, Transformations};

use super::{BitDepth, ColorMode, Format, Header, ImageError, Raster};

impl From<png::DecodingError> for ImageError {
    fn from(e: png::DecodingError) -> Self {
        ImageError::Png(e.to_string())
    }
}

impl From<png::EncodingError> for ImageError {
    fn from(e: png::EncodingError) -> Self {
        ImageError::Png(e.to_string())
    }
}

fn mode_of(color_type: ColorType) -> Option<ColorMode> {
    Some(match color_type {
        ColorType::Grayscale => ColorMode::Gray,
        ColorType::GrayscaleAlpha => ColorMode::GrayAlpha,
        ColorType::Rgb => ColorMode::Rgb,
        ColorType::Rgba => ColorMode::Rgba,
        ColorType::Indexed => ColorMode::Indexed,
    })
}

fn color_type_of(mode: ColorMode) -> Option<ColorType> {
    Some(match mode {
        ColorMode::Gray => ColorType::Grayscale,
        ColorMode::GrayAlpha => ColorType::GrayscaleAlpha,
        ColorMode::Rgb => ColorType::Rgb,
        ColorMode::Rgba => ColorType::Rgba,
        ColorMode::Indexed => ColorType::Indexed,
        ColorMode::Cmyk => return None,
    })
}

fn depth_of(depth: PngDepth) -> BitDepth {
    match depth {
        PngDepth::One => BitDepth::One,
        PngDepth::Two => BitDepth::Two,
        PngDepth::Four => BitDepth::Four,
        PngDepth::Eight => BitDepth::Eight,
        PngDepth::Sixteen => BitDepth::Sixteen,
    }
}

fn png_depth_of(depth: BitDepth) -> PngDepth {
    match depth {
        BitDepth::One => PngDepth::One,
        BitDepth::Two => PngDepth::Two,
        BitDepth::Four => PngDepth::Four,
        BitDepth::Eight => PngDepth::Eight,
        BitDepth::Sixteen => PngDepth::Sixteen,
    }
}

/// `IHDR` and the ancillary chunks before `IDAT`, without touching a pixel.
pub fn header(bytes: &[u8]) -> Result<Header, ImageError> {
    let mut decoder = Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(Transformations::IDENTITY);
    let reader = decoder.read_info()?;
    let info = reader.info();
    Ok(Header {
        width: info.width,
        height: info.height,
        mode: mode_of(info.color_type).ok_or(ImageError::Png("unsupported colour type".into()))?,
        depth: depth_of(info.bit_depth),
        icc: info.icc_profile.as_ref().map(|p| p.to_vec()),
    })
}

pub fn decode(bytes: &[u8]) -> Result<Raster, ImageError> {
    let mut decoder = Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(Transformations::IDENTITY);
    let mut reader = decoder.read_info()?;

    let mut data = vec![0; reader.output_buffer_size().unwrap_or(0)];
    let frame = reader.next_frame(&mut data)?;
    // `next_frame` reports the bytes it actually wrote, which is smaller than
    // the buffer for a sub-byte-depth image whose rows pad.
    data.truncate(frame.buffer_size());

    let info = reader.info();
    let mode = mode_of(info.color_type).ok_or(ImageError::Png("unsupported colour type".into()))?;

    Ok(Raster {
        width: info.width,
        height: info.height,
        mode,
        depth: depth_of(info.bit_depth),
        icc: info.icc_profile.as_ref().map(|p| p.to_vec()),
        palette: info.palette.as_ref().map(|p| p.to_vec()),
        trns: info.trns.as_ref().map(|t| t.to_vec()),
        srgb_intent: info.srgb.map(|intent| intent as u8),
        data,
    })
}

pub fn encode(raster: &Raster) -> Result<Vec<u8>, ImageError> {
    let color_type = color_type_of(raster.mode).ok_or(ImageError::Unrepresentable {
        format: Format::Png,
        mode: raster.mode,
        depth: raster.depth,
    })?;

    let mut info = Info::with_size(raster.width, raster.height);
    info.color_type = color_type;
    info.bit_depth = png_depth_of(raster.depth);
    info.palette = raster.palette.clone().map(Into::into);
    info.trns = raster.trns.clone().map(Into::into);
    info.icc_profile = raster.icc.clone().map(Into::into);

    // Deliberately **not** set on the Info: the encoder writes `iCCP` only in
    // the `else` branch of `if info.srgb.is_some()`, so setting both here drops
    // the profile without a word. The chunk is written by hand below instead,
    // which keeps a source that carried both.
    let srgb_intent = raster.srgb_intent;

    let mut out = Vec::new();
    {
        let encoder = Encoder::with_info(&mut out, info)?;
        let mut writer = encoder.write_header()?;
        if let Some(intent) = srgb_intent {
            writer.write_chunk(png::chunk::sRGB, &[intent])?;
        }
        writer.write_image_data(&raster.data)?;
        writer.finish()?;
    }
    Ok(out)
}
