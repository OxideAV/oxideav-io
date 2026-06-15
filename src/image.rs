//! Packed pixel buffer + the collapse from a decoded [`VideoFrame`].
//!
//! [`RgbaImage`] mirrors the proven handoff shape used elsewhere in the
//! workspace: an owned, tightly-packed RGBA8888 (or RGB24) buffer with
//! explicit dimensions. The pixel layout is inferred from `stride /
//! width` (4 ⇒ RGBA, 3 ⇒ RGB24) so the struct stays format-tag-free.

use oxideav_core::{PixelFormat, VideoFrame};
use oxideav_pixfmt::{convert as pix_convert, ConvertOptions, FrameInfo};

use crate::error::{Error, Result};

/// Owned, tightly-packed RGBA8888 / RGB24 image with explicit
/// dimensions. `stride == width * 4` ⇒ RGBA; `stride == width * 3` ⇒
/// RGB24.
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
}

/// Collapse a decoded [`VideoFrame`] (in its native pixel format,
/// described by `src_format`/`width`/`height`) into a tightly-packed
/// [`RgbaImage`] in the requested `dst` format.
///
/// `dst` must be [`PixelFormat::Rgba`] or [`PixelFormat::Rgb24`]; any
/// other target is rejected. Conversion is delegated to
/// `oxideav-pixfmt`, then the (possibly stride-padded) result plane is
/// repacked to a tight `width * bpp` stride.
pub(crate) fn frame_to_packed(
    frame: &VideoFrame,
    src_format: PixelFormat,
    width: u32,
    height: u32,
    dst: PixelFormat,
) -> Result<RgbaImage> {
    let bpp = match dst {
        PixelFormat::Rgba => 4usize,
        PixelFormat::Rgb24 => 3usize,
        other => {
            return Err(Error::invalid(format!(
                "frame_to_packed: destination must be Rgba or Rgb24, got {other:?}"
            )))
        }
    };
    if width == 0 || height == 0 {
        return Err(Error::invalid("frame_to_packed: zero-sized frame"));
    }

    let info = FrameInfo::new(src_format, width, height);
    let converted = pix_convert(frame, info, dst, &ConvertOptions::default())?;
    let plane = converted
        .planes
        .first()
        .ok_or_else(|| Error::invalid("frame_to_packed: converted frame has no plane"))?;

    let tight = (width as usize) * bpp;
    let h = height as usize;
    let mut pixels = Vec::with_capacity(tight * h);
    if plane.stride == tight {
        // Already tight — but defend against a short final row.
        let needed = tight * h;
        if plane.data.len() < needed {
            return Err(Error::invalid(
                "frame_to_packed: converted plane shorter than width*height*bpp",
            ));
        }
        pixels.extend_from_slice(&plane.data[..needed]);
    } else {
        // Strip per-row padding.
        for row in 0..h {
            let start = row * plane.stride;
            let end = start + tight;
            if end > plane.data.len() {
                return Err(Error::invalid(
                    "frame_to_packed: converted plane row out of bounds",
                ));
            }
            pixels.extend_from_slice(&plane.data[start..end]);
        }
    }

    Ok(RgbaImage {
        width,
        height,
        pixels,
        stride: tight,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxideav_core::frame::VideoPlane;

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
        let rgb = RgbaImage {
            width: 4,
            height: 2,
            pixels: vec![0; 24],
            stride: 12,
        };
        assert!(rgb.is_rgb());
        assert_eq!(rgb.bytes_per_pixel(), 3);
    }

    #[test]
    fn rgb24_collapses_to_rgba_with_opaque_alpha() {
        // 2×2 Rgb24, tight stride 6.
        let frame = VideoFrame {
            pts: None,
            planes: vec![VideoPlane {
                stride: 6,
                data: vec![
                    10, 20, 30, 40, 50, 60, // row 0: two pixels
                    70, 80, 90, 100, 110, 120, // row 1
                ],
            }],
        };
        let out = frame_to_packed(&frame, PixelFormat::Rgb24, 2, 2, PixelFormat::Rgba).unwrap();
        assert_eq!(out.width, 2);
        assert_eq!(out.height, 2);
        assert_eq!(out.stride, 8);
        assert_eq!(out.pixels.len(), 16);
        // First pixel R,G,B preserved, alpha opaque.
        assert_eq!(&out.pixels[0..4], &[10, 20, 30, 255]);
    }

    #[test]
    fn rejects_non_rgb_destination() {
        let frame = VideoFrame {
            pts: None,
            planes: vec![VideoPlane {
                stride: 6,
                data: vec![0; 12],
            }],
        };
        assert!(frame_to_packed(&frame, PixelFormat::Rgb24, 2, 2, PixelFormat::Yuv420P).is_err());
    }
}
