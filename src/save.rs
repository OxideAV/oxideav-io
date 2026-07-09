//! The write facade: encode an [`Opened`] value back out to a
//! [`Sink`], picking the container + codec from [`SaveOptions`] or the
//! sink's file extension and dispatching through the `oxideav-core`
//! registries.
//!
//! The still-image path is the heart of the module:
//!
//! 1. Reduce the [`RgbaImage`] to the encoder's preferred pixel format
//!    ([`oxideav-pixfmt`] does the conversion).
//! 2. Build [`CodecParameters`] and pull the codec's `first_encoder`.
//! 3. Feed the single frame through `send_frame` / `flush` /
//!    `receive_packet`.
//! 4. Open the container muxer (driven by `SaveOptions.container` or the
//!    sink extension) and run `write_header` / `write_packet` /
//!    `write_trailer`.
//!
//! The whole container is assembled in an in-memory cursor so a
//! seekable muxer (PNG / JPEG rewrite their headers) works even when
//! the destination is a non-seekable [`Sink::Writer`].

use std::io::{Cursor, Seek, SeekFrom, Write};
use std::sync::{Arc, Mutex};

use oxideav_core::{
    CodecId, CodecParameters, Frame, MediaType, Packet, PixelFormat, RuntimeContext, StreamInfo,
    TimeBase, VideoFrame, VideoPlane,
};
use oxideav_pixfmt::{convert as pix_convert, ConvertOptions, FrameInfo};

use crate::error::{Error, Result};
use crate::image::RgbaImage;
use crate::open::Opened;
use crate::source::Sink;

/// Which packed pixel layout the saved image should carry. `Auto` lets
/// the facade pick whichever of RGBA / RGB24 the chosen codec accepts
/// (preferring an alpha-capable layout when the codec supports it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PixelChoice {
    /// Pick a layout the codec accepts, preferring RGBA when available.
    #[default]
    Auto,
    /// Force packed RGB24 (drops any alpha channel).
    Rgb,
    /// Force packed RGBA8888.
    Rgba,
}

/// Per-call knobs for the save facade. Both `container` and `codec` may
/// be left `None`, in which case the facade derives them from the sink's
/// file extension.
#[derive(Clone, Debug, Default)]
pub struct SaveOptions {
    /// Force a specific container/muxer name (e.g. `"png"`, `"jpeg"`).
    /// `None` ⇒ derive from the sink extension.
    pub container: Option<String>,
    /// Force a specific codec id (e.g. `"png"`, `"mjpeg"`). `None` ⇒
    /// derive a sensible default for the chosen container.
    pub codec: Option<String>,
    /// Packed pixel layout for the encoded image.
    pub pixel: PixelChoice,
    /// Advisory encode quality (0..=100) passed to codecs that read a
    /// `"quality"` option. Codecs that ignore the knob use their own
    /// default.
    pub quality: Option<u8>,
}

/// A seekable in-memory writer whose backing buffer can be reclaimed
/// after the muxer (which takes ownership of a `Box<dyn WriteSeek>`) is
/// dropped. The `Arc<Mutex<…>>` lets the facade keep a handle to the
/// bytes the boxed clone wrote.
#[derive(Clone)]
struct SharedCursor(Arc<Mutex<Cursor<Vec<u8>>>>);

impl SharedCursor {
    fn new() -> Self {
        SharedCursor(Arc::new(Mutex::new(Cursor::new(Vec::new()))))
    }

    /// Reclaim the written bytes. Call after the muxer has been dropped
    /// so this is the only remaining handle.
    fn into_bytes(self) -> Vec<u8> {
        match Arc::try_unwrap(self.0) {
            Ok(m) => m
                .into_inner()
                .unwrap_or_else(|e| e.into_inner())
                .into_inner(),
            // A clone still lives somewhere — copy the bytes out instead.
            Err(arc) => arc.lock().map(|c| c.get_ref().clone()).unwrap_or_default(),
        }
    }
}

impl Write for SharedCursor {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("SharedCursor: poisoned lock"))?
            .write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("SharedCursor: poisoned lock"))?
            .flush()
    }
}

impl Seek for SharedCursor {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("SharedCursor: poisoned lock"))?
            .seek(pos)
    }
}

/// Default codec id for a container whose muxer expects a codec whose id
/// differs from the container name (the common image case is `1:1`, but
/// JPEG's container is `"jpeg"` while its codec is `"mjpeg"`, and Y4M is
/// a raw-frame container whose payload codec is `"rawvideo"`).
fn default_codec_for_container(container: &str) -> &str {
    match container {
        "jpeg" => "mjpeg",
        "y4m" => "rawvideo",
        other => other,
    }
}

/// Save an opened value to a sink against a caller-supplied context.
///
/// Image inputs are re-encoded through the codec + container registries.
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

/// Resolve the container name to mux into, from explicit options or the
/// sink extension.
fn resolve_container(ctx: &RuntimeContext, sink: &Sink, opts: &SaveOptions) -> Result<String> {
    if let Some(c) = &opts.container {
        return Ok(c.clone());
    }
    let ext = sink.ext_hint().ok_or_else(|| {
        Error::invalid(
            "save: no container specified and the sink has no file extension to derive one from",
        )
    })?;
    ctx.containers
        .container_for_extension(&ext)
        .map(|s| s.to_string())
        .ok_or_else(|| {
            Error::unsupported(format!(
                "save: no container registered for extension '.{ext}'"
            ))
        })
}

/// Pick the packed pixel format candidates the encoder should receive,
/// in attempt order, honouring an explicit [`PixelChoice`] and otherwise
/// consulting the codec's **encoder** capability set.
///
/// An explicit choice yields exactly one candidate (the caller asked for
/// it; failing loudly beats silently re-packing). `Auto` yields a
/// preference ladder: declared accepted formats first (alpha-capable
/// preferred), then the RGBA → RGB24 fallbacks — because a capability
/// set is advisory (an empty set means "unspecified", and some encoders
/// only reject a format at `send_frame` time), the save path tries each
/// candidate in turn.
fn pixel_format_candidates(
    ctx: &RuntimeContext,
    codec_id: &CodecId,
    choice: PixelChoice,
) -> Vec<PixelFormat> {
    match choice {
        PixelChoice::Rgb => vec![PixelFormat::Rgb24],
        PixelChoice::Rgba => vec![PixelFormat::Rgba],
        PixelChoice::Auto => {
            // Only encoder implementations matter here — a decoder's
            // accepted set says nothing about what we may feed in.
            let accepted: Vec<PixelFormat> = ctx
                .codecs
                .implementations(codec_id)
                .iter()
                .filter(|i| i.make_encoder.is_some())
                .flat_map(|i| i.caps.accepted_pixel_formats.iter().copied())
                .collect();
            let mut candidates = Vec::new();
            let mut push = |f: PixelFormat| {
                if !candidates.contains(&f) {
                    candidates.push(f);
                }
            };
            // Declared formats, alpha-capable first.
            if accepted.contains(&PixelFormat::Rgba) {
                push(PixelFormat::Rgba);
            }
            if accepted.contains(&PixelFormat::Rgb24) {
                push(PixelFormat::Rgb24);
            }
            for f in accepted {
                push(f);
            }
            // Universal fallbacks for advisory/empty capability sets.
            push(PixelFormat::Rgba);
            push(PixelFormat::Rgb24);
            candidates
        }
    }
}

/// Re-pack an [`RgbaImage`] into a [`VideoFrame`] in `dst` format via
/// `oxideav-pixfmt`, returning the frame ready to feed an encoder.
fn image_to_frame(img: &RgbaImage, dst: PixelFormat) -> Result<VideoFrame> {
    let src_format = if img.is_rgb() {
        PixelFormat::Rgb24
    } else {
        PixelFormat::Rgba
    };
    let src = VideoFrame {
        pts: Some(0),
        planes: vec![VideoPlane {
            stride: img.stride,
            data: img.pixels.clone(),
        }],
    };
    if src_format == dst {
        return Ok(src);
    }
    let info = FrameInfo::new(src_format, img.width, img.height);
    let converted = pix_convert(&src, info, dst, &ConvertOptions::default())?;
    Ok(converted)
}

/// Run one encode attempt in a fixed pixel format: convert the image,
/// build the encoder, feed the single frame, and drain the packets.
/// Returns the packets plus the encoder's output parameters (which carry
/// the wire tag / codec id the muxer needs to recognise the stream).
fn encode_image(
    ctx: &RuntimeContext,
    img: &RgbaImage,
    codec_id: &CodecId,
    dst: PixelFormat,
    quality: Option<u8>,
) -> Result<(Vec<Packet>, CodecParameters)> {
    let frame = image_to_frame(img, dst)?;

    // Build the encoder parameters. Quality is advisory — codecs that
    // read a "quality" option honour it; the rest use their default.
    let mut params = CodecParameters::video(codec_id.clone());
    params.width = Some(img.width);
    params.height = Some(img.height);
    params.pixel_format = Some(dst);
    if let Some(q) = quality {
        params.options = params.options.set("quality", q.min(100).to_string());
    }

    let mut encoder = ctx
        .codecs
        .first_encoder(&params)
        .map_err(|e| Error::Decode(format!("save: no encoder for codec '{codec_id}': {e}")))?;
    encoder.send_frame(&Frame::Video(frame))?;
    encoder.flush()?;

    let mut packets = Vec::new();
    loop {
        match encoder.receive_packet() {
            Ok(pkt) => packets.push(pkt),
            Err(oxideav_core::Error::NeedMore) | Err(oxideav_core::Error::Eof) => break,
            Err(e) => return Err(e.into()),
        }
    }
    if packets.is_empty() {
        return Err(Error::Decode(
            "save: encoder produced no packets for the image".into(),
        ));
    }
    Ok((packets, encoder.output_params().clone()))
}

/// Encode + mux a still image to the sink.
fn save_image(ctx: &RuntimeContext, img: &RgbaImage, sink: Sink, opts: &SaveOptions) -> Result<()> {
    if img.width == 0 || img.height == 0 {
        return Err(Error::invalid("save: cannot encode a zero-sized image"));
    }

    let container = resolve_container(ctx, &sink, opts)?;
    let codec_name = opts
        .codec
        .clone()
        .unwrap_or_else(|| default_codec_for_container(&container).to_string());
    let codec_id = CodecId::new(codec_name);

    // Try each pixel-format candidate in preference order (exactly one
    // for an explicit PixelChoice; a ladder for Auto — capability sets
    // are advisory, so an encoder may only reject a format at
    // send_frame time and the next candidate has to step in).
    let candidates = pixel_format_candidates(ctx, &codec_id, opts.pixel);
    let mut attempt: Option<(Vec<Packet>, CodecParameters)> = None;
    let mut last_err: Option<Error> = None;
    for dst in candidates {
        match encode_image(ctx, img, &codec_id, dst, opts.quality) {
            Ok(ok) => {
                attempt = Some(ok);
                break;
            }
            Err(e) => last_err = Some(e),
        }
    }
    let (packets, mut out_params) = match attempt {
        Some(ok) => ok,
        None => {
            return Err(last_err
                .unwrap_or_else(|| Error::invalid("save: no pixel-format candidate to encode")))
        }
    };
    out_params.media_type = MediaType::Video;
    let time_base = TimeBase::new(1, 100);
    let stream = StreamInfo {
        index: 0,
        time_base,
        duration: Some(packets.len() as i64),
        start_time: Some(0),
        params: out_params,
    };

    // Assemble the container in memory (via a SharedCursor whose bytes
    // we can reclaim) so a seekable muxer works over a non-seekable
    // sink, then commit the finished buffer.
    let cursor = SharedCursor::new();
    let mut muxer = ctx
        .containers
        .open_muxer(
            &container,
            Box::new(cursor.clone()),
            std::slice::from_ref(&stream),
        )
        .map_err(|e| Error::Decode(format!("save: cannot open muxer '{container}': {e}")))?;
    muxer.write_header()?;
    for pkt in &packets {
        muxer.write_packet(pkt)?;
    }
    muxer.write_trailer()?;
    drop(muxer);

    let bytes = cursor.into_bytes();
    if bytes.is_empty() {
        return Err(Error::Decode(
            "save: muxer produced an empty container".into(),
        ));
    }
    sink.commit(bytes)
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

        // Decode it back and check dimensions survive.
        let reopened =
            open_with(&c, Source::bytes(&buf), &OpenOptions::eager()).expect("reopen PNG");
        match reopened {
            Opened::Image(out) => assert_eq!((out.width, out.height), (2, 2)),
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
        // Regression: the MJPEG encoder only accepts RGB24, but its
        // capability set is advisory — PixelChoice::Auto used to pick
        // RGBA and fail at send_frame. Auto must now walk its candidate
        // ladder and land on RGB24 by itself.
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
    fn save_y4m_derives_rawvideo_codec() {
        // Regression: the Y4M container's payload codec is "rawvideo";
        // deriving the codec id from the container name produced the
        // nonexistent codec "y4m". The registry has no rawvideo
        // *encoder* yet, so the save still fails — but it must now fail
        // asking for the *right* codec, so the moment the fleet grows a
        // rawvideo encoder this path lights up. (When it does, this
        // test should become a full save → probe roundtrip.)
        let c = ctx();
        let opened = Opened::Image(sample_image());
        let mut buf = Vec::new();
        let opts = SaveOptions {
            container: Some("y4m".into()),
            ..SaveOptions::default()
        };
        let res = save_with(&c, &opened, Sink::Buffer(&mut buf), &opts);
        match res {
            Err(Error::Decode(msg)) => assert!(
                msg.contains("'rawvideo'"),
                "the derived codec must be rawvideo, got: {msg}"
            ),
            other => panic!("expected a rawvideo encoder-not-found error, got {other:?}"),
        }
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
        let reopened = open_with(&c, Source::bytes(&buf), &OpenOptions::eager()).expect("reopen");
        match reopened {
            Opened::Image(out) => assert_eq!((out.width, out.height), (2, 2)),
            other => panic!("expected Image, got {other:?}"),
        }
    }
}
