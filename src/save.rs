//! The write facade: encode an [`Opened`] value back out to a
//! [`Sink`], picking the container + codec from [`SaveOptions`] or the
//! sink's file extension.
//!
//! The still-image path rides the `oxideav-image` gateway: the
//! [`RgbaImage`] is wrapped as a native [`oxideav_image::Image`] and
//! handed to [`oxideav_image::encode`], which resolves the muxer by
//! format name or extension, picks the container's default codec (or
//! [`SaveOptions::codec`]), walks its pixel-format ladder when an
//! encoder **or** muxer refuses a layout, forwards `quality` only to
//! encoders whose option schema declares it, and assembles the whole
//! file in memory — so a seekable muxer (PNG / JPEG rewrite their
//! headers) works even when the destination is a non-seekable
//! [`Sink::Writer`]. The finished bytes are committed to the sink here.

use oxideav_core::{PixelFormat, RuntimeContext};

use crate::error::{Error, Result};
use crate::image::RgbaImage;
use crate::open::Opened;
use crate::source::Sink;

/// Which packed pixel layout the saved image should carry. `Auto` lets
/// the gateway's ladder pick: the image's own layout first, then the
/// layouts `oxideav-pixfmt` can reach from it (`Rgba`, `Rgb24`,
/// `Gray8`, `Yuv444P`, `Yuv420P`, `Rgba64Le`) until the encoder and the
/// muxer both accept one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PixelChoice {
    /// Let the gateway's ladder pick a layout the codec + container
    /// accept, starting from the image's own.
    #[default]
    Auto,
    /// Force packed RGB24 (drops any alpha channel).
    Rgb,
    /// Force packed RGBA8888.
    Rgba,
}

impl PixelChoice {
    /// The gateway's `SaveOptions::pixel_format` equivalent: an explicit
    /// layout replaces the ladder; `Auto` leaves it to the gateway.
    pub fn pixel_format(self) -> Option<PixelFormat> {
        match self {
            PixelChoice::Auto => None,
            PixelChoice::Rgb => Some(PixelFormat::Rgb24),
            PixelChoice::Rgba => Some(PixelFormat::Rgba),
        }
    }
}

/// Per-call knobs for the save facade. Both `container` and `codec` may
/// be left `None`, in which case the facade derives them from the sink's
/// file extension.
#[derive(Clone, Debug, Default)]
pub struct SaveOptions {
    /// Force a specific container / format name (e.g. `"png"`,
    /// `"jpeg"`, or a file extension such as `"jpg"` / `"heic"`).
    /// `None` ⇒ derive from the sink extension.
    pub container: Option<String>,
    /// Force a specific codec id (e.g. `"png"`, `"mjpeg"`). `None` ⇒
    /// the container's default codec, as the gateway resolves it.
    pub codec: Option<String>,
    /// Packed pixel layout for the encoded image.
    pub pixel: PixelChoice,
    /// Advisory encode quality (0..=100), forwarded to the encoder as
    /// its `"quality"` option **only when its declared option schema
    /// has one** (encoders parse options strictly). Codecs without the
    /// knob use their own default.
    pub quality: Option<u8>,
}

impl SaveOptions {
    /// The gateway options this facade's knobs map onto.
    pub fn to_image_options(&self) -> oxideav_image::SaveOptions {
        let mut g = oxideav_image::SaveOptions::new();
        if let Some(q) = self.quality {
            g = g.with_quality(q);
        }
        if let Some(f) = self.pixel.pixel_format() {
            g = g.with_pixel_format(f);
        }
        if let Some(c) = &self.codec {
            g = g.with_codec(c.clone());
        }
        g
    }
}

/// Save an opened value to a sink against a caller-supplied context.
///
/// Image inputs are re-encoded through the `oxideav-image` gateway.
/// 3D meshes are re-encoded through the mesh registry when the `mesh`
/// feature is on and the sink names a 3D extension. PDF scene writing is
/// out of scope for the facade today.
pub fn save_with(
    ctx: &RuntimeContext,
    opened: &Opened,
    sink: Sink,
    opts: &SaveOptions,
) -> Result<()> {
    match opened {
        Opened::Image(img) => save_image(ctx, img, sink, opts),
        #[cfg(feature = "mesh")]
        Opened::Mesh(scene) => save_mesh(scene, sink),
        #[cfg(feature = "pdf")]
        Opened::Scene(_) => Err(Error::unsupported(
            "saving a PDF/document Scene is not supported by oxideav-io (read-only for now)",
        )),
        Opened::Vector(_) => Err(Error::unsupported(
            "saving a vector frame is not yet supported by oxideav-io",
        )),
        Opened::Media(_) => Err(Error::unsupported(
            "saving a lazy a/v MediaReader is not yet supported by oxideav-io (use transcode for the a/v path)",
        )),
        #[allow(unreachable_patterns)]
        _ => Err(Error::unsupported(
            "saving this opened kind is not supported by oxideav-io",
        )),
    }
}

/// Encode a native gateway picture to a sink. The format is
/// [`SaveOptions::container`] or the sink's extension; everything else
/// is the gateway's resolution (see the module docs).
pub fn save_image_with(
    ctx: &RuntimeContext,
    image: &oxideav_image::Image,
    sink: Sink,
    opts: &SaveOptions,
) -> Result<()> {
    let format = resolve_format(&sink, opts)?;
    let bytes =
        oxideav_image::encode(ctx, image, &format, &opts.to_image_options()).map_err(save_error)?;
    if bytes.is_empty() {
        return Err(Error::Decode(
            "save: muxer produced an empty container".into(),
        ));
    }
    sink.commit(bytes)
}

/// The format name handed to the gateway: explicit, else the sink
/// extension (the gateway accepts both container names and extensions).
fn resolve_format(sink: &Sink, opts: &SaveOptions) -> Result<String> {
    if let Some(c) = &opts.container {
        return Ok(c.clone());
    }
    sink.ext_hint().ok_or_else(|| {
        Error::invalid(
            "save: no container specified and the sink has no file extension to derive one from",
        )
    })
}

/// On the write side an unknown format name / extension is a request
/// this registry cannot serve, not a detection failure.
fn save_error(e: oxideav_image::ImageError) -> Error {
    match e {
        oxideav_image::ImageError::UnknownFormat(m) => Error::Unsupported(format!("save: {m}")),
        other => other.into(),
    }
}

/// Encode + mux a still image to the sink.
fn save_image(ctx: &RuntimeContext, img: &RgbaImage, sink: Sink, opts: &SaveOptions) -> Result<()> {
    if img.width == 0 || img.height == 0 {
        return Err(Error::invalid("save: cannot encode a zero-sized image"));
    }
    let image = img.to_image()?;
    save_image_with(ctx, &image, sink, opts)
}

/// Encode + mux a 3D mesh scene to the sink via the mesh registry. The
/// output format is chosen from the sink's file extension.
#[cfg(feature = "mesh")]
fn save_mesh(scene: &oxideav_mesh3d::Scene3D, sink: Sink) -> Result<()> {
    let ext = sink.ext_hint().ok_or_else(|| {
        Error::invalid("save: 3D output needs a file extension to pick an encoder")
    })?;
    let mut registry = oxideav_mesh3d::Mesh3DRegistry::new();
    oxideav_meta::populate_mesh3d_registry(&mut registry);
    oxideav_fbx::register(&mut registry);
    let mut encoder = registry.encoder_for_extension(&ext).ok_or_else(|| {
        Error::unsupported(format!(
            "save: no 3D encoder registered for extension '.{ext}'"
        ))
    })?;
    let bytes = encoder
        .encode(scene)
        .map_err(|e| Error::Decode(format!("3D encode: {e}")))?;
    sink.commit(bytes)
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn pixel_choice_maps_onto_gateway_options() {
        assert_eq!(PixelChoice::Auto.pixel_format(), None);
        assert_eq!(PixelChoice::Rgb.pixel_format(), Some(PixelFormat::Rgb24));
        assert_eq!(PixelChoice::Rgba.pixel_format(), Some(PixelFormat::Rgba));
        let o = SaveOptions {
            container: Some("jpeg".into()),
            codec: Some("mjpeg".into()),
            pixel: PixelChoice::Rgb,
            quality: Some(250),
        }
        .to_image_options();
        assert_eq!(o.codec.as_deref(), Some("mjpeg"));
        assert_eq!(o.pixel_format, Some(PixelFormat::Rgb24));
        // The gateway clamps quality to 100.
        assert_eq!(o.quality, Some(100));
    }

    #[test]
    fn save_without_container_or_extension_is_invalid() {
        let mut buf = Vec::new();
        let res = resolve_format(&Sink::Buffer(&mut buf), &SaveOptions::default());
        assert!(matches!(res, Err(Error::Invalid(_))), "got {res:?}");
        let path = std::path::Path::new("out.JPG");
        let res = resolve_format(&Sink::Path(path), &SaveOptions::default()).unwrap();
        assert_eq!(res, "jpg");
    }
}

#[cfg(all(test, feature = "full"))]
mod tests {
    use super::*;
    use crate::open::{open_with, OpenOptions};
    use crate::source::Source;

    fn ctx() -> RuntimeContext {
        let mut c = RuntimeContext::new();
        oxideav_meta::register_all(&mut c);
        c
    }

    fn sample_image() -> RgbaImage {
        RgbaImage {
            width: 2,
            height: 2,
            // Tight RGBA, 4 distinct pixels.
            pixels: vec![
                255, 0, 0, 255, // red
                0, 255, 0, 255, // green
                0, 0, 255, 255, // blue
                255, 255, 255, 128, // semi-transparent white
            ],
            stride: 8,
        }
    }

    #[test]
    fn save_png_buffer_roundtrips_through_open() {
        let c = ctx();
        let img = sample_image();
        let opened = Opened::Image(img.clone());
        let mut buf = Vec::new();
        let opts = SaveOptions {
            container: Some("png".into()),
            ..SaveOptions::default()
        };
        save_with(&c, &opened, Sink::Buffer(&mut buf), &opts).expect("save PNG");
        assert!(!buf.is_empty(), "PNG buffer should be non-empty");
        assert_eq!(
            &buf[0..8],
            &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]
        );

        // Decode it back: dimensions AND pixels survive (PNG is
        // lossless and the ladder keeps the image's own RGBA layout).
        let reopened =
            open_with(&c, Source::bytes(&buf), &OpenOptions::eager()).expect("reopen PNG");
        match reopened {
            Opened::Image(out) => assert_eq!(out, img),
            other => panic!("expected Image, got {other:?}"),
        }
    }

    #[test]
    fn save_derives_container_from_path_extension() {
        let c = ctx();
        let dir = std::env::temp_dir();
        let path = dir.join(format!("oxideav-io-save-test-{}.png", std::process::id()));
        let opened = Opened::Image(sample_image());
        save_with(&c, &opened, Sink::Path(&path), &SaveOptions::default()).expect("save by path");
        let bytes = std::fs::read(&path).expect("read back");
        let _ = std::fs::remove_file(&path);
        assert_eq!(&bytes[0..4], &[0x89, b'P', b'N', b'G']);
    }

    #[test]
    fn save_jpeg_via_extension_picks_mjpeg_codec() {
        let c = ctx();
        let opened = Opened::Image(sample_image());
        let mut buf = Vec::new();
        let opts = SaveOptions {
            container: Some("jpeg".into()),
            pixel: PixelChoice::Rgb,
            quality: Some(80),
            ..SaveOptions::default()
        };
        save_with(&c, &opened, Sink::Buffer(&mut buf), &opts).expect("save JPEG");
        // SOI marker.
        assert_eq!(&buf[0..2], &[0xFF, 0xD8], "JPEG should start with SOI");
    }

    #[test]
    fn save_jpeg_with_auto_pixel_choice_falls_back_to_rgb() {
        // The MJPEG encoder only accepts RGB24; PixelChoice::Auto must
        // walk the gateway's ladder (Rgba first, then Rgb24) and land on
        // RGB24 by itself.
        let c = ctx();
        let opened = Opened::Image(sample_image());
        let mut buf = Vec::new();
        let opts = SaveOptions {
            container: Some("jpeg".into()),
            ..SaveOptions::default() // pixel: PixelChoice::Auto
        };
        save_with(&c, &opened, Sink::Buffer(&mut buf), &opts).expect("save JPEG with Auto pixel");
        assert_eq!(&buf[0..2], &[0xFF, 0xD8], "JPEG should start with SOI");
    }

    #[test]
    fn save_by_extension_name_resolves_the_container() {
        // The gateway accepts an extension where this facade used to
        // insist on a registered container name.
        let c = ctx();
        let opened = Opened::Image(sample_image());
        let mut buf = Vec::new();
        let opts = SaveOptions {
            container: Some("jpg".into()),
            ..SaveOptions::default()
        };
        save_with(&c, &opened, Sink::Buffer(&mut buf), &opts).expect("save by .jpg");
        assert_eq!(&buf[0..2], &[0xFF, 0xD8]);
    }

    #[test]
    fn save_y4m_round_trips_through_rawvideo() {
        // The Y4M container's payload codec is "rawvideo" (the gateway's
        // default-codec table). Y4M cannot carry RGBA / RGB24, so the
        // gateway's ladder steps past the packed layouts the rawvideo
        // encoder accepts to a colorspace the muxer can express (round
        // 473: the ladder is the gateway's — Rgba, Rgb24, Gray8, Yuv444P,
        // Yuv420P — so the first rung Y4M takes is `Cmono`).
        let c = ctx();
        let opened = Opened::Image(sample_image());
        let mut buf = Vec::new();
        let opts = SaveOptions {
            container: Some("y4m".into()),
            ..SaveOptions::default()
        };
        save_with(&c, &opened, Sink::Buffer(&mut buf), &opts).expect("save Y4M");
        assert!(
            buf.starts_with(b"YUV4MPEG2 "),
            "Y4M stream must start with its signature"
        );
        let header_end = buf.iter().position(|&b| b == b'\n').expect("header line");
        let header = std::str::from_utf8(&buf[..header_end]).unwrap();
        assert!(
            header.contains(" W2 ") && header.contains(" H2 "),
            "header carries the 2x2 geometry: {header}"
        );
        assert!(
            header.contains(" C444") || header.contains(" C420") || header.contains(" Cmono"),
            "header names a colorspace the muxer can express: {header}"
        );
        assert!(
            buf[header_end + 1..].starts_with(b"FRAME"),
            "one FRAME follows the header"
        );
    }

    #[test]
    fn explicit_pixel_choice_does_not_fall_back() {
        // An explicit choice the encoder can't take must fail loudly,
        // not silently re-pack: MJPEG + forced RGBA is an error.
        let c = ctx();
        let opened = Opened::Image(sample_image());
        let mut buf = Vec::new();
        let opts = SaveOptions {
            container: Some("jpeg".into()),
            pixel: PixelChoice::Rgba,
            ..SaveOptions::default()
        };
        let res = save_with(&c, &opened, Sink::Buffer(&mut buf), &opts);
        assert!(res.is_err(), "forced RGBA into MJPEG must error: {res:?}");
        assert!(buf.is_empty(), "nothing is committed on failure");
    }

    #[test]
    fn save_rejects_zero_sized_image() {
        let c = ctx();
        let opened = Opened::Image(RgbaImage {
            width: 0,
            height: 0,
            pixels: Vec::new(),
            stride: 0,
        });
        let mut buf = Vec::new();
        let opts = SaveOptions {
            container: Some("png".into()),
            ..SaveOptions::default()
        };
        let res = save_with(&c, &opened, Sink::Buffer(&mut buf), &opts);
        assert!(matches!(res, Err(Error::Invalid(_))), "got {res:?}");
    }

    #[test]
    fn save_without_container_or_extension_errors() {
        let c = ctx();
        let opened = Opened::Image(sample_image());
        let mut buf = Vec::new();
        let res = save_with(&c, &opened, Sink::Buffer(&mut buf), &SaveOptions::default());
        assert!(matches!(res, Err(Error::Invalid(_))), "got {res:?}");
    }

    #[test]
    fn save_unknown_format_is_unsupported() {
        let c = ctx();
        let opened = Opened::Image(sample_image());
        let mut buf = Vec::new();
        let opts = SaveOptions {
            container: Some("definitely-not-a-format".into()),
            ..SaveOptions::default()
        };
        let res = save_with(&c, &opened, Sink::Buffer(&mut buf), &opts);
        assert!(matches!(res, Err(Error::Unsupported(_))), "got {res:?}");
    }

    #[test]
    fn pixel_choice_rgb_drops_alpha_in_saved_png() {
        let c = ctx();
        let opened = Opened::Image(sample_image());
        let mut buf = Vec::new();
        let opts = SaveOptions {
            container: Some("png".into()),
            pixel: PixelChoice::Rgb,
            ..SaveOptions::default()
        };
        save_with(&c, &opened, Sink::Buffer(&mut buf), &opts).expect("save RGB PNG");
        // Reopen natively: the file carries RGB24, the alpha is gone.
        let native = crate::open::open_image_with(&c, Source::bytes(&buf), &OpenOptions::default())
            .expect("reopen native");
        assert_eq!(native.format(), PixelFormat::Rgb24);
        assert_eq!((native.width(), native.height()), (2, 2));
        assert_eq!(&native.to_rgb8().unwrap()[9..12], &[255, 255, 255]);
    }

    #[test]
    fn save_native_image_directly() {
        let c = ctx();
        let img = oxideav_image::Image::from_rgb8(1, 1, vec![9, 8, 7]).unwrap();
        let mut buf = Vec::new();
        let opts = SaveOptions {
            container: Some("png".into()),
            ..SaveOptions::default()
        };
        save_image_with(&c, &img, Sink::Buffer(&mut buf), &opts).expect("save native");
        assert_eq!(&buf[0..4], &[0x89, b'P', b'N', b'G']);
    }
}
