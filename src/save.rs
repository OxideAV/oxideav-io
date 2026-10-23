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
//! encoders whose option schema declares it, hands the picture's blobs
//! (and [`SaveOptions::blobs`]) to the encoder at both levels, and
//! assembles the whole file in memory — so a seekable muxer (PNG / JPEG rewrite their
//! headers) works even when the destination is a non-seekable
//! [`Sink::Writer`]. The finished bytes are committed to the sink here.

use oxideav_core::{BlobKind, MetadataBlob, PixelFormat, RuntimeContext};

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
///
/// `#[non_exhaustive]`: build with [`SaveOptions::new`] and the
/// `with_*` builders; the fields stay readable.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
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
    /// Blobs written on top of the picture's own: a blob here replaces
    /// every source blob of the same kind (set an ICC profile, swap the
    /// XMP packet); other kinds pass through untouched. Written even
    /// when `strip_metadata` withholds the source's.
    pub blobs: Vec<MetadataBlob>,
    /// Hand none of the picture's blobs to the encoder (`blobs` above
    /// are still written). Default `false`.
    pub strip_metadata: bool,
    /// Source blob kinds withheld from the encoder when
    /// `strip_metadata` is off.
    pub strip_blobs: Vec<BlobKind>,
    /// Extra encoder options as `(name, value)` pairs, forwarded
    /// verbatim; an encoder rejects names its declared schema lacks.
    pub options: Vec<(String, String)>,
}

impl SaveOptions {
    /// Defaults everywhere: container and codec from the sink, the
    /// gateway's pixel ladder, the encoder's own quality, the picture's
    /// blobs written as they are.
    pub fn new() -> Self {
        Self::default()
    }

    /// Force the container / format name or extension (see the field).
    pub fn with_container(mut self, name: impl Into<String>) -> Self {
        self.container = Some(name.into());
        self
    }

    /// Force the codec id (see the field).
    pub fn with_codec(mut self, codec: impl Into<String>) -> Self {
        self.codec = Some(codec.into());
        self
    }

    /// Choose the packed layout (see the field).
    pub fn with_pixel(mut self, pixel: PixelChoice) -> Self {
        self.pixel = pixel;
        self
    }

    /// Advisory quality `0..=100` (see the field).
    pub fn with_quality(mut self, q: u8) -> Self {
        self.quality = Some(q.min(100));
        self
    }

    /// Write this blob, replacing the source's of its kind (see the
    /// field).
    pub fn with_blob(mut self, blob: MetadataBlob) -> Self {
        self.blobs.push(blob);
        self
    }

    /// Replace the written blobs (see the field).
    pub fn with_blobs(mut self, blobs: Vec<MetadataBlob>) -> Self {
        self.blobs = blobs;
        self
    }

    /// Keep (`true`, the default) or withhold every source blob.
    pub fn with_metadata(mut self, keep: bool) -> Self {
        self.strip_metadata = !keep;
        self
    }

    /// Withhold every source blob from the encoder.
    pub fn without_metadata(self) -> Self {
        self.with_metadata(false)
    }

    /// Withhold these source blob kinds from the encoder.
    pub fn with_stripped_blobs<I, K>(mut self, kinds: I) -> Self
    where
        I: IntoIterator<Item = K>,
        K: Into<BlobKind>,
    {
        self.strip_blobs = kinds.into_iter().map(Into::into).collect();
        self
    }

    /// Add one encoder option (see the field).
    pub fn with_option(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.options.push((name.into(), value.into()));
        self
    }

    /// The gateway options this facade's knobs map onto, one for one
    /// (`container` is resolved by the facade and is not among them).
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
        if !self.blobs.is_empty() {
            g = g.with_blobs(self.blobs.clone());
        }
        g = g.with_metadata(!self.strip_metadata);
        if !self.strip_blobs.is_empty() {
            g = g.with_stripped_blobs(self.strip_blobs.iter().cloned());
        }
        for (k, v) in &self.options {
            g = g.with_option(k.clone(), v.clone());
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
///
/// A kind named in [`SaveOptions::blobs`] is removed from the picture
/// at both levels (frame and stream) before the encode, so the
/// replacement holds whichever level the encoder reads — the gateway
/// replaces at the stream level only, and a per-picture blob of the
/// same kind would otherwise win on reopen.
pub fn save_image_with(
    ctx: &RuntimeContext,
    image: &oxideav_image::Image,
    sink: Sink,
    opts: &SaveOptions,
) -> Result<()> {
    let format = resolve_format(&sink, opts)?;
    let replaced: Vec<&BlobKind> = opts.blobs.iter().map(|b| &b.kind).collect();
    let stripped;
    let image = if replaced.is_empty() || !image.has_metadata() {
        image
    } else {
        let mut i = image.clone();
        i.retain_blobs(|b| !replaced.contains(&&b.kind));
        stripped = i;
        &stripped
    };
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

/// Encode a flattened [`RgbaImage`] to a sink: the buffer is wrapped
/// as a native picture with its `blobs` attached ([`RgbaImage::to_image`])
/// and handed to [`save_image_with`], so its metadata is written unless
/// `opts` strips or replaces it. What `save_with` does for
/// `Opened::Image`.
pub fn save_rgba_with(
    ctx: &RuntimeContext,
    img: &RgbaImage,
    sink: Sink,
    opts: &SaveOptions,
) -> Result<()> {
    if img.width == 0 || img.height == 0 {
        return Err(Error::invalid("save: cannot encode a zero-sized image"));
    }
    let image = img.to_image()?;
    save_image_with(ctx, &image, sink, opts)
}

/// Encode + mux a still image to the sink.
fn save_image(ctx: &RuntimeContext, img: &RgbaImage, sink: Sink, opts: &SaveOptions) -> Result<()> {
    save_rgba_with(ctx, img, sink, opts)
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
        let icc = MetadataBlob::new(BlobKind::ICC, vec![1, 2, 3]);
        let o = SaveOptions::new()
            .with_container("jpeg")
            .with_codec("mjpeg")
            .with_pixel(PixelChoice::Rgb)
            .with_quality(250)
            .with_blob(icc.clone())
            .with_stripped_blobs([BlobKind::EXIF])
            .with_option("k", "v");
        assert_eq!(o.container.as_deref(), Some("jpeg"));
        let g = o.to_image_options();
        assert_eq!(g.codec.as_deref(), Some("mjpeg"));
        assert_eq!(g.pixel_format, Some(PixelFormat::Rgb24));
        // Quality is clamped to 100 on the way in.
        assert_eq!(g.quality, Some(100));
        assert_eq!(g.blobs, vec![icc]);
        assert!(!g.strip_metadata);
        assert_eq!(g.strip_blobs, vec![BlobKind::EXIF]);
        assert_eq!(g.options, vec![("k".to_string(), "v".to_string())]);
        let g = SaveOptions::new().without_metadata().to_image_options();
        assert!(g.strip_metadata && g.blobs.is_empty() && g.options.is_empty());
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
    use crate::open::{OpenOptions, open_with};
    use crate::source::Source;

    fn ctx() -> RuntimeContext {
        let mut c = RuntimeContext::new();
        oxideav_meta::register_all(&mut c);
        c
    }

    fn sample_image() -> RgbaImage {
        // Tight RGBA, 4 distinct pixels: red, green, blue,
        // semi-transparent white.
        RgbaImage::rgba(
            2,
            2,
            vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 128,
            ],
        )
        .unwrap()
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
            blobs: Vec::new(),
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

/// Save behaviour on the lean `registry` slice, driven by the in-crate
/// test format (`crate::testfmt`).
#[cfg(test)]
mod registry_tests {
    use super::*;
    use crate::open::{OpenOptions, open_image_file_with, open_with};
    use crate::source::Source;
    use crate::testfmt;

    fn sample() -> RgbaImage {
        RgbaImage::rgba(2, 1, vec![1, 2, 3, 255, 4, 5, 6, 0]).unwrap()
    }

    #[test]
    fn flattened_save_round_trips_and_forwards_quality() {
        let ctx = testfmt::context();
        let mut buf = Vec::new();
        let opts = SaveOptions {
            container: Some(testfmt::EXT.into()),
            quality: Some(77),
            ..SaveOptions::default()
        };
        save_with(
            &ctx,
            &Opened::Image(sample()),
            Sink::Buffer(&mut buf),
            &opts,
        )
        .unwrap();
        let file =
            open_image_file_with(&ctx, Source::bytes(&buf), &OpenOptions::default()).unwrap();
        assert_eq!(file.container(), testfmt::NAME);
        assert_eq!(
            file.metadata()[0],
            ("quality".to_string(), "77".to_string())
        );
        match open_with(&ctx, Source::bytes(&buf), &OpenOptions::eager()).unwrap() {
            Opened::Image(img) => assert_eq!(img, sample()),
            other => panic!("expected Image, got {other:?}"),
        }
    }

    #[test]
    fn flattened_save_writes_the_pictures_blobs() {
        use oxideav_core::{BlobKind, MetadataBlob};
        let ctx = testfmt::context();
        let img = sample()
            .with_blob(MetadataBlob::new(BlobKind::ICC, vec![3u8; 8]))
            .with_blob(MetadataBlob::new(BlobKind::XMP, b"<x/>".to_vec()));
        let mut buf = Vec::new();
        let opts = SaveOptions {
            container: Some(testfmt::NAME.into()),
            ..SaveOptions::default()
        };
        save_with(
            &ctx,
            &Opened::Image(img.clone()),
            Sink::Buffer(&mut buf),
            &opts,
        )
        .unwrap();
        let native =
            crate::open::open_image_with(&ctx, Source::bytes(&buf), &OpenOptions::default())
                .unwrap();
        // A single-picture encode promotes the picture's blobs to the
        // stream (the gateway's ruling), and they come back resolved.
        assert_eq!(native.icc(), Some(&[3u8; 8][..]));
        assert_eq!(native.xmp(), Some(&b"<x/>"[..]));
        let Opened::Image(back) =
            open_with(&ctx, Source::bytes(&buf), &OpenOptions::eager()).unwrap()
        else {
            panic!("expected Image")
        };
        assert_eq!(back, img);
    }

    #[test]
    fn save_options_replace_strip_and_forward_encoder_options() {
        use oxideav_core::{BlobKind, MetadataBlob};
        let ctx = testfmt::context();
        let img = sample()
            .with_blob(MetadataBlob::new(BlobKind::ICC, vec![3u8; 8]))
            .with_blob(MetadataBlob::new(BlobKind::EXIF, vec![4u8; 4]))
            .with_blob(MetadataBlob::new(BlobKind::XMP, b"<x/>".to_vec()));
        let reopen = |buf: &[u8]| {
            crate::open::open_image_with(&ctx, Source::bytes(buf), &OpenOptions::default()).unwrap()
        };
        // A blob in the options replaces the source's of its kind;
        // the other kinds pass through.
        let mut buf = Vec::new();
        let opts = SaveOptions::new()
            .with_container(testfmt::NAME)
            .with_blob(MetadataBlob::new(BlobKind::ICC, vec![9u8; 2]));
        save_rgba_with(&ctx, &img, Sink::Buffer(&mut buf), &opts).unwrap();
        let back = reopen(&buf);
        assert_eq!(back.icc(), Some(&[9u8; 2][..]));
        assert_eq!(back.exif(), Some(&[4u8; 4][..]));
        assert_eq!(back.xmp(), Some(&b"<x/>"[..]));
        // strip_metadata withholds the source's, still writes the
        // options' own.
        let mut buf = Vec::new();
        let opts = SaveOptions::new()
            .with_container(testfmt::NAME)
            .without_metadata()
            .with_blob(MetadataBlob::new(BlobKind::IPTC, vec![5u8]));
        save_rgba_with(&ctx, &img, Sink::Buffer(&mut buf), &opts).unwrap();
        let back = reopen(&buf);
        assert!(back.icc().is_none() && back.exif().is_none() && back.xmp().is_none());
        assert_eq!(back.iptc(), Some(&[5u8][..]));
        // Kinds can be withheld one by one.
        let mut buf = Vec::new();
        let opts = SaveOptions::new()
            .with_container(testfmt::NAME)
            .with_stripped_blobs([BlobKind::EXIF]);
        save_rgba_with(&ctx, &img, Sink::Buffer(&mut buf), &opts).unwrap();
        let back = reopen(&buf);
        assert!(back.exif().is_none());
        assert!(back.icc().is_some() && back.xmp().is_some());
        // A declared encoder option is forwarded verbatim; an unknown
        // one is the encoder's refusal, not a silent drop.
        let mut buf = Vec::new();
        let opts = SaveOptions::new()
            .with_container(testfmt::NAME)
            .with_option("quality", "33");
        save_rgba_with(&ctx, &img, Sink::Buffer(&mut buf), &opts).unwrap();
        let file =
            open_image_file_with(&ctx, Source::bytes(&buf), &OpenOptions::default()).unwrap();
        assert_eq!(file.metadata()[0].1, "33");
        let mut buf = Vec::new();
        let opts = SaveOptions::new()
            .with_container(testfmt::NAME)
            .with_option("no-such-option", "1");
        assert!(save_rgba_with(&ctx, &img, Sink::Buffer(&mut buf), &opts).is_err());
        assert!(buf.is_empty());
    }

    #[test]
    fn writer_sink_receives_the_whole_file() {
        let ctx = testfmt::context();
        let shared = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        struct W(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
        impl std::io::Write for W {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let opts = SaveOptions {
            container: Some(testfmt::NAME.into()),
            ..SaveOptions::default()
        };
        save_with(
            &ctx,
            &Opened::Image(sample()),
            Sink::writer(W(shared.clone())),
            &opts,
        )
        .unwrap();
        let written = shared.lock().unwrap().clone();
        let img =
            crate::open::open_image_with(&ctx, Source::bytes(&written), &OpenOptions::default())
                .unwrap();
        assert_eq!(img.to_rgba8().unwrap(), sample().pixels);
    }

    #[test]
    fn forced_rgb_drops_alpha_and_unknown_format_is_unsupported() {
        let ctx = testfmt::context();
        let mut buf = Vec::new();
        let opts = SaveOptions {
            container: Some(testfmt::NAME.into()),
            pixel: PixelChoice::Rgb,
            ..SaveOptions::default()
        };
        save_with(
            &ctx,
            &Opened::Image(sample()),
            Sink::Buffer(&mut buf),
            &opts,
        )
        .unwrap();
        let img = crate::open::open_image_with(&ctx, Source::bytes(&buf), &OpenOptions::default())
            .unwrap();
        assert_eq!(img.format(), PixelFormat::Rgb24);
        assert_eq!(img.to_rgb8().unwrap(), vec![1, 2, 3, 4, 5, 6]);
        let mut buf = Vec::new();
        let opts = SaveOptions {
            container: Some("nope".into()),
            ..SaveOptions::default()
        };
        let r = save_with(
            &ctx,
            &Opened::Image(sample()),
            Sink::Buffer(&mut buf),
            &opts,
        );
        assert!(matches!(r, Err(Error::Unsupported(_))), "{r:?}");
        assert!(buf.is_empty());
    }
}
