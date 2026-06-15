//! The transcode facade: read a [`Source`], optionally apply a chain of
//! still-image [`Transform`]s, and write the result to a [`Sink`].
//!
//! This round wires the **still-image** path end-to-end
//! (decode → convert / resize → encode → mux). The audio/video pipeline
//! path (decode → filter graph → encode → mux, built on
//! `oxideav-pipeline`) is the documented next step — [`transcode_with`]
//! returns [`Error::Unsupported`] for a non-image input today.

use oxideav_core::{PixelFormat, RuntimeContext, VideoFrame, VideoPlane};
use oxideav_pixfmt::{convert as pix_convert, ConvertOptions, FrameInfo};

use crate::error::{Error, Result};
use crate::image::RgbaImage;
use crate::open::{open_with, OpenOptions, Opened};
use crate::save::{save_with, PixelChoice, SaveOptions};
use crate::source::{Sink, Source};

/// A single still-image transformation applied between decode and
/// encode. The chain runs in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Transform {
    /// Rescale to `width × height`. Requires the `transforms` feature
    /// (enabled by default via `full`); without it this variant errors.
    Resize { width: u32, height: u32 },
    /// Force the packed pixel layout (RGB24 / RGBA) for the in-flight
    /// image. Useful to drop or add an alpha channel before encode.
    Convert(PixelChoice),
}

/// Knobs for a transcode operation: how to open the input, the
/// transform chain, and how to save the output.
#[derive(Clone, Debug, Default)]
pub struct TranscodeOptions {
    /// Options forwarded to the read facade.
    pub open: OpenOptions,
    /// Options forwarded to the save facade.
    pub save: SaveOptions,
    /// Still-image transforms applied in order between decode and
    /// encode.
    pub transforms: Vec<Transform>,
}

/// Transcode a source to a sink against a caller-supplied context.
///
/// Still images take the full decode → transform → encode → mux path.
/// A/V inputs return [`Error::Unsupported`] — the pipeline-backed path
/// is the next step.
pub fn transcode_with(
    ctx: &RuntimeContext,
    src: Source,
    sink: Sink,
    opts: &TranscodeOptions,
) -> Result<()> {
    // Force still-image collapse so a single-frame video stream
    // (the common image case) comes back as `Opened::Image` rather than
    // a lazy `Opened::Media`.
    let mut open_opts = opts.open.clone();
    open_opts.eager_image = true;
    let opened = open_with(ctx, src, &open_opts)?;
    match opened {
        Opened::Image(mut img) => {
            for t in &opts.transforms {
                img = apply_transform(&img, t)?;
            }
            save_with(ctx, &Opened::Image(img), sink, &opts.save)
        }
        other => Err(Error::unsupported(format!(
            "transcode: only still-image inputs are supported today (got {other:?}); the a/v pipeline path is the next step"
        ))),
    }
}

/// Apply one transform to a packed image, returning the new buffer.
fn apply_transform(img: &RgbaImage, t: &Transform) -> Result<RgbaImage> {
    match t {
        Transform::Convert(choice) => convert_image(img, *choice),
        Transform::Resize { width, height } => resize_image(img, *width, *height),
    }
}

/// Re-pack an image into the requested packed layout via `oxideav-pixfmt`.
fn convert_image(img: &RgbaImage, choice: PixelChoice) -> Result<RgbaImage> {
    let dst = match choice {
        PixelChoice::Rgb => PixelFormat::Rgb24,
        // Auto on a packed buffer means "keep whatever we have".
        PixelChoice::Auto if img.is_rgb() => PixelFormat::Rgb24,
        PixelChoice::Auto | PixelChoice::Rgba => PixelFormat::Rgba,
    };
    let src_format = if img.is_rgb() {
        PixelFormat::Rgb24
    } else {
        PixelFormat::Rgba
    };
    if src_format == dst {
        return Ok(img.clone());
    }
    let frame = VideoFrame {
        pts: Some(0),
        planes: vec![VideoPlane {
            stride: img.stride,
            data: img.pixels.clone(),
        }],
    };
    let info = FrameInfo::new(src_format, img.width, img.height);
    let out = pix_convert(&frame, info, dst, &ConvertOptions::default())?;
    frame_to_image(&out, img.width, img.height, dst)
}

/// Pack a converted (possibly stride-padded) frame back into a tight
/// [`RgbaImage`].
fn frame_to_image(
    frame: &VideoFrame,
    width: u32,
    height: u32,
    dst: PixelFormat,
) -> Result<RgbaImage> {
    let bpp = match dst {
        PixelFormat::Rgba => 4usize,
        PixelFormat::Rgb24 => 3usize,
        other => {
            return Err(Error::invalid(format!(
                "transcode: unexpected packed format {other:?}"
            )))
        }
    };
    let plane = frame
        .planes
        .first()
        .ok_or_else(|| Error::invalid("transcode: converted frame has no plane"))?;
    let tight = width as usize * bpp;
    let h = height as usize;
    let mut pixels = Vec::with_capacity(tight * h);
    for row in 0..h {
        let start = row * plane.stride;
        let end = start + tight;
        if end > plane.data.len() {
            return Err(Error::invalid(
                "transcode: converted plane row out of bounds",
            ));
        }
        pixels.extend_from_slice(&plane.data[start..end]);
    }
    Ok(RgbaImage {
        width,
        height,
        pixels,
        stride: tight,
    })
}

/// Rescale an image to new dimensions via the image-filter `Resize`
/// kernel (behind the `transforms` feature).
#[cfg(feature = "transforms")]
fn resize_image(img: &RgbaImage, width: u32, height: u32) -> Result<RgbaImage> {
    use oxideav_image_filter::{ImageFilter, Resize, VideoStreamParams};
    if width == 0 || height == 0 {
        return Err(Error::invalid("transcode: resize target must be non-zero"));
    }
    let src_format = if img.is_rgb() {
        PixelFormat::Rgb24
    } else {
        PixelFormat::Rgba
    };
    let frame = VideoFrame {
        pts: Some(0),
        planes: vec![VideoPlane {
            stride: img.stride,
            data: img.pixels.clone(),
        }],
    };
    let params = VideoStreamParams {
        format: src_format,
        width: img.width,
        height: img.height,
    };
    let out = Resize::new(width, height)
        .apply(&frame, params)
        .map_err(|e| Error::Decode(format!("transcode: resize failed: {e}")))?;
    frame_to_image(&out, width, height, src_format)
}

#[cfg(not(feature = "transforms"))]
fn resize_image(_img: &RgbaImage, _width: u32, _height: u32) -> Result<RgbaImage> {
    Err(Error::unsupported(
        "transcode: Resize requires the `transforms` feature (enabled by default via `full`)",
    ))
}

#[cfg(all(test, feature = "full"))]
mod tests {
    use super::*;

    fn ctx() -> RuntimeContext {
        let mut c = RuntimeContext::new();
        oxideav_meta::register_all(&mut c);
        c
    }

    /// 4×4 RGBA test image: solid red, opaque.
    fn red_png_bytes(c: &RuntimeContext) -> Vec<u8> {
        let img = RgbaImage {
            width: 4,
            height: 4,
            pixels: [255, 0, 0, 255].repeat(16),
            stride: 16,
        };
        let mut buf = Vec::new();
        let opts = SaveOptions {
            container: Some("png".into()),
            ..SaveOptions::default()
        };
        save_with(c, &Opened::Image(img), Sink::Buffer(&mut buf), &opts).expect("encode seed PNG");
        buf
    }

    #[test]
    fn transcode_png_to_jpeg() {
        let c = ctx();
        let png = red_png_bytes(&c);
        let mut out = Vec::new();
        let opts = TranscodeOptions {
            save: SaveOptions {
                container: Some("jpeg".into()),
                pixel: PixelChoice::Rgb,
                ..SaveOptions::default()
            },
            ..TranscodeOptions::default()
        };
        transcode_with(&c, Source::bytes(&png), Sink::Buffer(&mut out), &opts)
            .expect("transcode PNG→JPEG");
        assert_eq!(&out[0..2], &[0xFF, 0xD8], "should be a JPEG");
    }

    #[test]
    fn transcode_with_resize_changes_dimensions() {
        let c = ctx();
        let png = red_png_bytes(&c);
        let mut out = Vec::new();
        let opts = TranscodeOptions {
            save: SaveOptions {
                container: Some("png".into()),
                ..SaveOptions::default()
            },
            transforms: vec![Transform::Resize {
                width: 2,
                height: 2,
            }],
            ..TranscodeOptions::default()
        };
        transcode_with(&c, Source::bytes(&png), Sink::Buffer(&mut out), &opts)
            .expect("transcode + resize");
        // Reopen and confirm new dimensions.
        let reopened = open_with(&c, Source::bytes(&out), &OpenOptions::eager()).expect("reopen");
        match reopened {
            Opened::Image(o) => assert_eq!((o.width, o.height), (2, 2)),
            other => panic!("expected Image, got {other:?}"),
        }
    }

    #[test]
    fn transcode_convert_to_rgb_then_png() {
        let c = ctx();
        let png = red_png_bytes(&c);
        let mut out = Vec::new();
        let opts = TranscodeOptions {
            save: SaveOptions {
                container: Some("png".into()),
                ..SaveOptions::default()
            },
            transforms: vec![Transform::Convert(PixelChoice::Rgb)],
            ..TranscodeOptions::default()
        };
        transcode_with(&c, Source::bytes(&png), Sink::Buffer(&mut out), &opts)
            .expect("transcode convert");
        assert_eq!(&out[0..4], &[0x89, b'P', b'N', b'G']);
    }
}
