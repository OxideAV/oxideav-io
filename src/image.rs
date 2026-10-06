//! Packed pixel buffer + the bridge to the gateway's native
//! [`oxideav_image::Image`].
//!
//! [`RgbaImage`] is the flattened handoff shape this facade has always
//! returned: an owned, tightly-packed RGBA8888 (or RGB24) buffer with
//! explicit dimensions, the pixel layout inferred from `stride / width`
//! (4 ⇒ RGBA, 3 ⇒ RGB24). Since round 473 every conversion into or out
//! of it goes through `oxideav-image` — [`RgbaImage::from_image`] packs
//! a native [`Image`] (any layout the registry decodes, with the
//! stream's colour signal honoured), [`RgbaImage::to_image`] wraps the
//! buffer back into an `Image` for the gateway's encoders.

use oxideav_core::PixelFormat;
use oxideav_image::Image;

use crate::error::{Error, Result};

/// Owned, tightly-packed RGBA8888 / RGB24 image with explicit
/// dimensions. `stride == width * 4` ⇒ RGBA; `stride == width * 3` ⇒
/// RGB24.
///
/// This is the flattened view; the native picture (any layout, palette,
/// colour signal, timing) is [`oxideav_image::Image`], reachable through
/// [`open_image_with`](crate::open_image_with) and the conversions
/// below.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    /// Tightly packed pixel bytes, `height * stride` long.
    pub pixels: Vec<u8>,
    /// Bytes per row. `width * 4` (RGBA) or `width * 3` (RGB24).
    pub stride: usize,
}

impl RgbaImage {
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

    /// Flatten a native gateway [`Image`] to packed RGBA8888 (alpha
    /// opaque when the source has none). The conversion runs through
    /// `oxideav-image` → `oxideav-pixfmt` with the picture's colour
    /// signal.
    pub fn from_image(img: &Image) -> Result<RgbaImage> {
        Self::from_image_in(img, PixelFormat::Rgba)
    }

    /// Flatten a native gateway [`Image`] to packed RGB24 (alpha dropped).
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
        })
    }

    /// Wrap this buffer as a native gateway [`Image`] in its own packed
    /// layout (no conversion). Fails with [`Error::Invalid`] when
    /// `pixels.len()` disagrees with `width × height × bytes-per-pixel`.
    pub fn to_image(&self) -> Result<Image> {
        Ok(Image::from_raw(
            self.width,
            self.height,
            self.pixel_format(),
            self.pixels.clone(),
        )?)
    }

    /// [`to_image`](Self::to_image), consuming the buffer.
    pub fn into_image(self) -> Result<Image> {
        Ok(Image::from_raw(
            self.width,
            self.height,
            self.pixel_format(),
            self.pixels,
        )?)
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
        let rgba = RgbaImage {
            width: 4,
            height: 2,
            pixels: vec![0; 32],
            stride: 16,
        };
        assert!(!rgba.is_rgb());
        assert_eq!(rgba.bytes_per_pixel(), 4);
        assert_eq!(rgba.pixel_format(), PixelFormat::Rgba);
        let rgb = RgbaImage {
            width: 4,
            height: 2,
            pixels: vec![0; 24],
            stride: 12,
        };
        assert!(rgb.is_rgb());
        assert_eq!(rgb.bytes_per_pixel(), 3);
        assert_eq!(rgb.pixel_format(), PixelFormat::Rgb24);
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
        // And the RGB24 flattening is the identity.
        let rgb = RgbaImage::from_image_rgb(&img).unwrap();
        assert!(rgb.is_rgb());
        assert_eq!(&rgb.pixels[..], img.as_packed().unwrap());
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
        let src = RgbaImage {
            width: 2,
            height: 1,
            pixels: vec![1, 2, 3, 4, 5, 6, 7, 8],
            stride: 8,
        };
        let img: Image = (&src).try_into().unwrap();
        assert_eq!(img.format(), PixelFormat::Rgba);
        assert_eq!((img.width(), img.height()), (2, 1));
        let back: RgbaImage = img.try_into().unwrap();
        assert_eq!(back, src);
    }

    #[test]
    fn inconsistent_stride_is_invalid() {
        // 2 wide, stride says RGBA, but only 6 bytes of pixels.
        let bad = RgbaImage {
            width: 2,
            height: 1,
            pixels: vec![0; 6],
            stride: 8,
        };
        assert!(matches!(bad.to_image(), Err(Error::Invalid(_))));
    }
}
