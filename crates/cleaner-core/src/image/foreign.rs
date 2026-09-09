//! Formats this application **reads but never writes**.
//!
//! The format matrix is a list of what an export may produce, and it has two
//! entries: PNG and TIFF. Nothing here changes that. What a scan folder holds
//! is a different question, and in practice it holds JPEG and WebP - so a
//! chapter of them used to be refused file by file as
//! `input.skipReason.notAnImage`, which is a true sentence about a folder the
//! user plainly meant to open.
//!
//! So the split this module draws is between the formats that can *carry* a
//! finished page ([`super::Format`]) and the formats a page can *arrive* in
//! ([`SourceFormat`]). A source format is decoded once, at ingest, and written
//! straight back out as PNG; from that point the pipeline sees a PNG and §2's
//! bit-identical guarantee is between that PNG and the export, which is the
//! only pair of files it could ever have been about - a JPEG re-encoded
//! losslessly is not a thing that exists.
//!
//! **The original file is never touched.** [`crate::ingest::convert_to_png`]
//! writes the PNG somewhere else and records where it came from; "output never
//! overwrites input" covers the original as much as it covers a PNG source.
//!
//! Decoding is the `image` crate's, at the four codecs named in
//! `Cargo.toml` and no others. All four are pure Rust, which is the constraint
//! settled when libvips was reversed - a C decoder for AVIF or HEIC would
//! reopen that decision, so those two are still refused.

use std::io::Cursor;

use image::{DynamicImage, ImageDecoder};

use super::{BitDepth, ColorMode, ImageError, Raster};

/// A format a page can arrive in and cannot leave in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceFormat {
    Jpeg,
    Webp,
    Gif,
    Bmp,
}

impl SourceFormat {
    /// Sniffed from the leading bytes, for the reason [`super::Format::sniff`]
    /// gives: a file's name is a claim and its magic is evidence. A folder of
    /// `.jpg` files that are really PNGs is a real thing that comes out of
    /// batch converters, and it must not decide the decoder.
    pub fn sniff(bytes: &[u8]) -> Option<SourceFormat> {
        if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
            Some(SourceFormat::Jpeg)
        } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
            Some(SourceFormat::Webp)
        } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            Some(SourceFormat::Gif)
        } else if bytes.starts_with(b"BM") {
            Some(SourceFormat::Bmp)
        } else {
            None
        }
    }

    /// What the conversion notice calls this format. Not a translated string:
    /// `JPEG` is `JPEG` in every catalogue, and it is interpolated into a
    /// sentence that is translated.
    pub fn label(self) -> &'static str {
        match self {
            SourceFormat::Jpeg => "JPEG",
            SourceFormat::Webp => "WebP",
            SourceFormat::Gif => "GIF",
            SourceFormat::Bmp => "BMP",
        }
    }
}

/// Decode one of the four, ICC profile included where the file carried one.
///
/// The profile matters more here than anywhere else in the module: a JPEG from
/// a scanner very often carries one, and dropping it on the way into the PNG
/// would change how every later page looks without anything in the manifest
/// recording that it happened.
///
/// A multi-frame file - an animated GIF or WebP - yields its **first frame**.
/// A page is one image; the alternative is to refuse a file whose first frame
/// is exactly the page the user meant.
pub fn decode(bytes: &[u8]) -> Result<Raster, ImageError> {
    match SourceFormat::sniff(bytes).ok_or(ImageError::UnknownFormat)? {
        SourceFormat::Jpeg => {
            raster_of(image::codecs::jpeg::JpegDecoder::new(Cursor::new(bytes))?)
        }
        SourceFormat::Webp => {
            raster_of(image::codecs::webp::WebPDecoder::new(Cursor::new(bytes))?)
        }
        SourceFormat::Gif => raster_of(image::codecs::gif::GifDecoder::new(Cursor::new(bytes))?),
        SourceFormat::Bmp => raster_of(image::codecs::bmp::BmpDecoder::new(Cursor::new(bytes))?),
    }
}

impl From<image::ImageError> for ImageError {
    fn from(error: image::ImageError) -> ImageError {
        ImageError::Foreign(error.to_string())
    }
}

/// One decoder's output as a [`Raster`].
///
/// The profile is read **before** the pixels, because `from_decoder` consumes
/// the decoder.
fn raster_of<D: ImageDecoder>(mut decoder: D) -> Result<Raster, ImageError> {
    let icc = decoder.icc_profile().ok().flatten().filter(|bytes| !bytes.is_empty());
    let image = DynamicImage::from_decoder(decoder)?;
    let (width, height) = (image.width(), image.height());

    // Eight of the `image` crate's buffer types map onto a mode and a depth
    // this crate already has; everything else - the float variants - is taken
    // to 8-bit RGBA rather than refused, because a float page has no
    // representation in `BitDepth` at all and refusing it would report "not an
    // image" about a file that decoded perfectly.
    let (mode, depth, data) = match image {
        DynamicImage::ImageLuma8(buffer) => {
            (ColorMode::Gray, BitDepth::Eight, buffer.into_raw())
        }
        DynamicImage::ImageLumaA8(buffer) => {
            (ColorMode::GrayAlpha, BitDepth::Eight, buffer.into_raw())
        }
        DynamicImage::ImageRgb8(buffer) => (ColorMode::Rgb, BitDepth::Eight, buffer.into_raw()),
        DynamicImage::ImageRgba8(buffer) => (ColorMode::Rgba, BitDepth::Eight, buffer.into_raw()),
        DynamicImage::ImageLuma16(buffer) => {
            (ColorMode::Gray, BitDepth::Sixteen, big_endian(buffer.into_raw()))
        }
        DynamicImage::ImageLumaA16(buffer) => {
            (ColorMode::GrayAlpha, BitDepth::Sixteen, big_endian(buffer.into_raw()))
        }
        DynamicImage::ImageRgb16(buffer) => {
            (ColorMode::Rgb, BitDepth::Sixteen, big_endian(buffer.into_raw()))
        }
        DynamicImage::ImageRgba16(buffer) => {
            (ColorMode::Rgba, BitDepth::Sixteen, big_endian(buffer.into_raw()))
        }
        other => (ColorMode::Rgba, BitDepth::Eight, other.into_rgba8().into_raw()),
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

/// [`Raster::data`] is big-endian at 16 bits, because that is how PNG stores a
/// 16-bit sample and the whole buffer is "as the decoder produced it" for
/// exactly one decoder. The `image` crate's is native-endian, so this is the
/// one place a byte order is imposed rather than carried.
fn big_endian(samples: Vec<u16>) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        bytes.extend_from_slice(&sample.to_be_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::{Format, encode, fixtures};

    /// A real JPEG of one of this crate's own fixtures.
    ///
    /// Built through the `image` crate's encoder rather than checked in as a
    /// blob, so the test exercises the same library the decoder comes from and
    /// nothing has to be regenerated when a fixture changes. Note that the PNG
    /// *decoder* is deliberately not among this crate's enabled `image`
    /// features - the fixture's samples are handed over directly.
    fn jpeg_of(name: &str) -> Vec<u8> {
        let raster = &fixtures::by_name(name).raster;
        let buffer =
            image::RgbImage::from_raw(raster.width, raster.height, raster.data.clone())
                .expect("the fixture is 8-bit RGB");
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(buffer)
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Jpeg)
            .unwrap();
        bytes
    }

    #[test]
    fn the_four_magics_are_told_apart() {
        assert_eq!(SourceFormat::sniff(&jpeg_of("rgb8")), Some(SourceFormat::Jpeg));
        assert_eq!(SourceFormat::sniff(b"GIF89a...."), Some(SourceFormat::Gif));
        assert_eq!(SourceFormat::sniff(b"BM......"), Some(SourceFormat::Bmp));
        assert_eq!(SourceFormat::sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some(SourceFormat::Webp));
        // A PNG is not a *source* format: it is already one we write.
        let png = encode(&fixtures::by_name("rgb8").raster, Format::Png).unwrap();
        assert_eq!(SourceFormat::sniff(&png), None);
    }

    #[test]
    fn a_jpeg_decodes_to_a_raster_the_png_encoder_accepts() {
        let raster = decode(&jpeg_of("rgb8")).unwrap();
        assert_eq!(raster.mode, ColorMode::Rgb);
        assert_eq!(raster.depth, BitDepth::Eight);
        assert_eq!(raster.data.len(), raster.stride() * raster.height as usize);
        // The point of the whole module: what comes out is writable as PNG.
        let png = encode(&raster, Format::Png).unwrap();
        assert!(crate::image::decode(&png).unwrap().same_geometry(&raster));
    }

    #[test]
    fn a_truncated_source_is_an_error_rather_than_a_short_buffer() {
        let whole = jpeg_of("rgb8");
        assert!(decode(&whole[..whole.len() / 3]).is_err());
    }
}
