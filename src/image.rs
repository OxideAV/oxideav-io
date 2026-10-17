//! Packed pixel buffer + the bridge to the gateway's native
//! [`oxideav_image::Image`].
//!
//! [`RgbaImage`] is the flattened handoff shape this facade has always
//! returned: an owned, tightly-packed RGBA8888 (or RGB24) buffer with
//! explicit dimensions, the pixel layout inferred from `stride / width`
//! (4 ⇒ RGBA, 3 ⇒ RGB24), plus — since round 477 — the metadata blobs
//! (ICC / Exif / XMP / IPTC / custom) that rode with the picture. Every
//! conversion into or out of it goes through `oxideav-image` —
//! [`RgbaImage::from_image`] packs a native [`Image`] (any layout the
//! registry decodes, with the stream's colour signal honoured and an
//! indexed picture's palette alpha composited into the alpha channel)
//! and lifts its blobs, [`RgbaImage::to_image`] wraps the buffer back
//! into an `Image` with the blobs re-attached for the gateway's
//! encoders.
//!
//! **Ruling (metadata on the flattened shape):** the blobs live on
//! `RgbaImage` itself, not on the surrounding [`crate::Opened`] — the
//! buffer is the unit that is saved, transformed and handed around, and
//! `Opened::Image` has no second slot. `RgbaImage::blobs` holds the
//! native picture's resolved list ([`Image::blobs`]: the frame's blobs
//! first, then the stream blobs of kinds the frame does not carry), so
//! flattening never loses a kind; the frame / stream distinction is
//! only reachable on the native `Image`.

use oxideav_core::{BlobKind, MetadataBlob, PixelFormat};
use oxideav_image::Image;

use crate::error::{Error, Result};

/// Owned, tightly-packed RGBA8888 / RGB24 image with explicit
/// dimensions. `stride == width * 4` ⇒ RGBA; `stride == width * 3` ⇒
/// RGB24.
///
/// This is the flattened view; the native picture (any layout, palette,
/// colour signal, timing, the stream / frame blob levels) is
/// [`oxideav_image::Image`], reachable through
/// [`open_image_with`](crate::open_image_with) and the conversions
/// below.
///
/// `#[non_exhaustive]`: build one with [`rgba`](Self::rgba) /
/// [`rgb`](Self::rgb) / [`new`](Self::new) (or
/// [`from_image`](Self::from_image)) and read the fields directly.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    /// Tightly packed pixel bytes, `height * stride` long.
    pub pixels: Vec<u8>,
    /// Bytes per row. `width * 4` (RGBA) or `width * 3` (RGB24).
    pub stride: usize,
    /// Metadata blobs flattened with the picture (ICC profile, Exif,
    /// XMP, IPTC, custom kinds), in the native picture's resolved order
    /// (see the module docs). Written back by the save path; empty when
    /// the source carried none.
    pub blobs: Vec<MetadataBlob>,
}

impl RgbaImage {
    /// A packed buffer of `width × height` pixels with `stride` bytes
    /// per row, no blobs. `stride` must be `width * 4` (RGBA) or
    /// `width * 3` (RGB24), both sides non-zero, and `pixels` exactly
    /// `height * stride` bytes — else [`Error::Invalid`].
    pub fn new(width: u32, height: u32, pixels: Vec<u8>, stride: usize) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(Error::invalid("RgbaImage: zero-sized image"));
        }
        let w = width as usize;
        if stride != w * 4 && stride != w * 3 {
            return Err(Error::invalid(format!(
                "RgbaImage: stride {stride} is neither {} (RGBA) nor {} (RGB24) for width {width}",
                w * 4,
                w * 3
            )));
        }
        let expected = stride * height as usize;
        if pixels.len() != expected {
            return Err(Error::invalid(format!(
                "RgbaImage: {} pixel bytes for {width}x{height} with stride {stride} (expected {expected})",
                pixels.len()
            )));
        }
        Ok(RgbaImage {
            width,
            height,
            pixels,
            stride,
            blobs: Vec::new(),
        })
    }

    /// A packed RGBA8888 buffer (`width × height × 4` bytes).
    pub fn rgba(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self> {
        Self::new(width, height, pixels, width as usize * 4)
    }

    /// A packed RGB24 buffer (`width × height × 3` bytes).
    pub fn rgb(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self> {
        Self::new(width, height, pixels, width as usize * 3)
    }

    /// True when the buffer is packed RGB24 (3 bytes/pixel) rather than
    /// RGBA (4 bytes/pixel).
    pub fn is_rgb(&self) -> bool {
        self.stride == (self.width as usize) * 3
    }

    /// Number of bytes per pixel implied by `stride / width`.
    pub fn bytes_per_pixel(&self) -> usize {
        if self.width == 0 {
            0
        } else {
            self.stride / (self.width as usize)
        }
    }

    /// The packed layout this buffer carries: [`PixelFormat::Rgb24`]
    /// when [`is_rgb`](Self::is_rgb), else [`PixelFormat::Rgba`].
    pub fn pixel_format(&self) -> PixelFormat {
        if self.is_rgb() {
            PixelFormat::Rgb24
        } else {
            PixelFormat::Rgba
        }
    }

    // ───────────────────────── metadata ─────────────────────────

    /// Builder: replace the blobs.
    pub fn with_blobs(mut self, blobs: Vec<MetadataBlob>) -> Self {
        self.blobs = blobs;
        self
    }

    /// Builder: append one blob.
    pub fn with_blob(mut self, blob: MetadataBlob) -> Self {
        self.blobs.push(blob);
        self
    }

    /// Append one blob.
    pub fn push_blob(&mut self, blob: MetadataBlob) {
        self.blobs.push(blob);
    }

    /// The first blob of `kind`, if any.
    pub fn blob(&self, kind: &BlobKind) -> Option<&MetadataBlob> {
        self.blobs.iter().find(|b| b.is(kind))
    }

    /// Every blob of `kind`, in order.
    pub fn blobs_of(&self, kind: &BlobKind) -> Vec<&MetadataBlob> {
        self.blobs.iter().filter(|b| b.is(kind)).collect()
    }

    /// The ICC profile bytes, if a [`BlobKind::ICC`] blob rode along.
    pub fn icc(&self) -> Option<&[u8]> {
        self.blob(&BlobKind::ICC).map(|b| b.data.as_slice())
    }

    /// The Exif block, if any ([`BlobKind::EXIF`]).
    pub fn exif(&self) -> Option<&[u8]> {
        self.blob(&BlobKind::EXIF).map(|b| b.data.as_slice())
    }

    /// The XMP packet, if any ([`BlobKind::XMP`]).
    pub fn xmp(&self) -> Option<&[u8]> {
        self.blob(&BlobKind::XMP).map(|b| b.data.as_slice())
    }

    /// The IPTC datasets, if any ([`BlobKind::IPTC`]).
    pub fn iptc(&self) -> Option<&[u8]> {
        self.blob(&BlobKind::IPTC).map(|b| b.data.as_slice())
    }

    /// True when at least one blob rode along.
    pub fn has_metadata(&self) -> bool {
        !self.blobs.is_empty()
    }

    /// Keep only the blobs `keep` accepts.
    pub fn retain_blobs(&mut self, keep: impl FnMut(&MetadataBlob) -> bool) {
        self.blobs.retain(keep);
    }

    /// Drop every blob.
    pub fn strip_metadata(&mut self) {
        self.blobs.clear();
    }

    /// Builder form of [`strip_metadata`](Self::strip_metadata).
    pub fn without_metadata(mut self) -> Self {
        self.strip_metadata();
        self
    }

    // ───────────────────────── gateway bridge ─────────────────────────

    /// Flatten a native gateway [`Image`] to packed RGBA8888 (alpha
    /// opaque when the source has none; an indexed picture's palette
    /// alpha composited into the channel). The conversion runs through
    /// `oxideav-image` → `oxideav-pixfmt` with the picture's colour
    /// signal; the picture's resolved blobs ([`Image::blobs`]) are
    /// lifted into [`blobs`](Self::blobs).
    pub fn from_image(img: &Image) -> Result<RgbaImage> {
        Self::from_image_in(img, PixelFormat::Rgba)
    }

    /// Flatten a native gateway [`Image`] to packed RGB24 (alpha —
    /// channel or palette — dropped, not composited). Blobs are lifted
    /// as in [`from_image`](Self::from_image).
    pub fn from_image_rgb(img: &Image) -> Result<RgbaImage> {
        Self::from_image_in(img, PixelFormat::Rgb24)
    }

    /// Flatten to `dst`, which must be `Rgba` or `Rgb24` — the two
    /// layouts [`RgbaImage`] can express.
    pub(crate) fn from_image_in(img: &Image, dst: PixelFormat) -> Result<RgbaImage> {
        let bpp = match dst {
            PixelFormat::Rgba => 4usize,
            PixelFormat::Rgb24 => 3usize,
            other => {
                return Err(Error::invalid(format!(
                    "RgbaImage: destination must be Rgba or Rgb24, got {other:?}"
                )))
            }
        };
        let (width, height) = (img.width(), img.height());
        if width == 0 || height == 0 {
            return Err(Error::invalid("RgbaImage: zero-sized frame"));
        }
        let pixels = img.to_packed(dst)?;
        Ok(RgbaImage {
            width,
            height,
            pixels,
            stride: (width as usize) * bpp,
            blobs: img.blobs().into_iter().cloned().collect(),
        })
    }

    /// Wrap this buffer as a native gateway [`Image`] in its own packed
    /// layout (no conversion), the blobs attached as the picture's own
    /// (`Image::with_blobs` — a single-picture encode promotes them to
    /// the stream level, so a still-format encoder sees them). Fails
    /// with [`Error::Invalid`] when `pixels.len()` disagrees with
    /// `width × height × bytes-per-pixel`.
    pub fn to_image(&self) -> Result<Image> {
        Ok(Image::from_raw(
            self.width,
            self.height,
            self.pixel_format(),
            self.pixels.clone(),
        )?
        .with_blobs(self.blobs.clone()))
    }

    /// [`to_image`](Self::to_image), consuming the buffer.
    pub fn into_image(self) -> Result<Image> {
        let format = self.pixel_format();
        Ok(Image::from_raw(self.width, self.height, format, self.pixels)?.with_blobs(self.blobs))
    }
}

impl TryFrom<Image> for RgbaImage {
    type Error = Error;
    /// Packed RGBA8888 (see [`RgbaImage::from_image`]).
    fn try_from(img: Image) -> Result<Self> {
        RgbaImage::from_image(&img)
    }
}

impl TryFrom<&Image> for RgbaImage {
    type Error = Error;
    /// Packed RGBA8888 (see [`RgbaImage::from_image`]).
    fn try_from(img: &Image) -> Result<Self> {
        RgbaImage::from_image(img)
    }
}

impl TryFrom<RgbaImage> for Image {
    type Error = Error;
    fn try_from(img: RgbaImage) -> Result<Self> {
        img.into_image()
    }
}

impl TryFrom<&RgbaImage> for Image {
    type Error = Error;
    fn try_from(img: &RgbaImage) -> Result<Self> {
        img.to_image()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_rgb_and_bpp() {
        let rgba = RgbaImage::rgba(4, 2, vec![0; 32]).unwrap();
        assert!(!rgba.is_rgb());
        assert_eq!(rgba.bytes_per_pixel(), 4);
        assert_eq!(rgba.pixel_format(), PixelFormat::Rgba);
        assert_eq!(rgba.stride, 16);
        let rgb = RgbaImage::rgb(4, 2, vec![0; 24]).unwrap();
        assert!(rgb.is_rgb());
        assert_eq!(rgb.bytes_per_pixel(), 3);
        assert_eq!(rgb.pixel_format(), PixelFormat::Rgb24);
        assert_eq!(rgb.stride, 12);
    }

    #[test]
    fn constructors_validate_the_geometry() {
        assert!(matches!(
            RgbaImage::rgba(0, 2, vec![]),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            RgbaImage::rgba(2, 1, vec![0; 7]),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            RgbaImage::new(2, 1, vec![0; 4], 2),
            Err(Error::Invalid(_))
        ));
        assert!(RgbaImage::new(2, 1, vec![0; 6], 6).unwrap().is_rgb());
    }

    #[test]
    fn rgb24_image_flattens_to_rgba_with_opaque_alpha() {
        // 2×2 Rgb24 native picture.
        let img = Image::from_rgb8(
            2,
            2,
            vec![
                10, 20, 30, 40, 50, 60, // row 0: two pixels
                70, 80, 90, 100, 110, 120, // row 1
            ],
        )
        .unwrap();
        let out = RgbaImage::from_image(&img).unwrap();
        assert_eq!((out.width, out.height, out.stride), (2, 2, 8));
        assert_eq!(out.pixels.len(), 16);
        assert_eq!(&out.pixels[0..4], &[10, 20, 30, 255]);
        assert!(!out.has_metadata());
        // And the RGB24 flattening is the identity.
        let rgb = RgbaImage::from_image_rgb(&img).unwrap();
        assert!(rgb.is_rgb());
        assert_eq!(&rgb.pixels[..], img.as_packed().unwrap());
    }

    #[test]
    fn palette_alpha_is_composited_into_rgba_and_dropped_by_rgb() {
        let img =
            Image::from_indexed(2, 1, vec![0, 1], &[[255, 0, 0, 255], [0, 0, 255, 0]]).unwrap();
        assert!(img.has_palette_alpha());
        let rgba = RgbaImage::from_image(&img).unwrap();
        assert_eq!(rgba.pixels, vec![255, 0, 0, 255, 0, 0, 255, 0]);
        let rgb = RgbaImage::from_image_rgb(&img).unwrap();
        assert_eq!(rgb.pixels, vec![255, 0, 0, 0, 0, 255]);
    }

    #[test]
    fn blobs_are_lifted_resolved_and_reattached() {
        // Stream ICC + XMP, frame XMP: the frame's XMP wins, the ICC
        // comes from the stream.
        let img = Image::from_rgb8(1, 1, vec![1, 2, 3])
            .unwrap()
            .with_stream_blobs(vec![
                MetadataBlob::new(BlobKind::ICC, vec![9u8; 4]),
                MetadataBlob::new(BlobKind::XMP, b"stream".to_vec()),
            ])
            .with_blobs(vec![MetadataBlob::new(BlobKind::XMP, b"frame".to_vec())]);
        let flat = RgbaImage::from_image(&img).unwrap();
        assert_eq!(flat.blobs.len(), 2);
        assert_eq!(flat.xmp(), Some(&b"frame"[..]));
        assert_eq!(flat.icc(), Some(&[9u8; 4][..]));
        assert!(flat.exif().is_none() && flat.iptc().is_none());
        assert!(flat.has_metadata());
        assert_eq!(flat.blobs_of(&BlobKind::XMP).len(), 1);
        // Back to a native picture: the blobs are the picture's own,
        // so a single-picture encode promotes them to the stream.
        let back = flat.to_image().unwrap();
        assert_eq!(back.frame_blobs().len(), 2);
        assert_eq!(back.xmp(), Some(&b"frame"[..]));
        let back: Image = flat.clone().try_into().unwrap();
        assert_eq!(back.icc(), Some(&[9u8; 4][..]));
        // Stripping.
        let mut stripped = flat.clone();
        stripped.retain_blobs(|b| b.is(&BlobKind::ICC));
        assert_eq!(stripped.blobs.len(), 1);
        assert!(!flat.without_metadata().has_metadata());
        let built = RgbaImage::rgb(1, 1, vec![1, 2, 3])
            .unwrap()
            .with_blob(MetadataBlob::new(BlobKind::IPTC, vec![7]));
        assert_eq!(built.iptc(), Some(&[7u8][..]));
    }

    #[test]
    fn rejects_non_rgb_destination() {
        let img = Image::from_rgb8(2, 2, vec![0; 12]).unwrap();
        assert!(matches!(
            RgbaImage::from_image_in(&img, PixelFormat::Yuv420P),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn round_trips_through_the_native_image() {
        let src = RgbaImage::rgba(2, 1, vec![1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        let img: Image = (&src).try_into().unwrap();
        assert_eq!(img.format(), PixelFormat::Rgba);
        assert_eq!((img.width(), img.height()), (2, 1));
        let back: RgbaImage = img.try_into().unwrap();
        assert_eq!(back, src);
    }

    #[test]
    fn inconsistent_stride_is_invalid() {
        // 2 wide, stride says RGBA, but only 6 bytes of pixels (a
        // literal built in-crate; the constructors refuse this shape).
        let bad = RgbaImage {
            width: 2,
            height: 1,
            pixels: vec![0; 6],
            stride: 8,
            blobs: Vec::new(),
        };
        assert!(matches!(bad.to_image(), Err(Error::Invalid(_))));
    }
}
