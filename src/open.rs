//! The read facade: discrimination + the unified and specialized
//! openers, all built on the `oxideav-core` registries.

use std::collections::VecDeque;

use oxideav_core::{Decoder, Demuxer, Frame, PixelFormat, ReadSeek, RuntimeContext, StreamInfo};

use crate::error::{Error, Result};
use crate::image::{frame_to_packed, RgbaImage};
use crate::probe::{is_mesh3d, is_pdf, peek_magic};
use crate::source::Source;

/// Per-call knobs shared by every opener. Restricts which container /
/// codec the facade is allowed to use and controls the still-image
/// collapse behaviour.
#[derive(Clone, Debug, Default)]
pub struct OpenOptions {
    /// If `Some`, only these container names may be used; anything else
    /// is rejected with [`Error::Restricted`]. `None` ⇒ any registered
    /// container.
    pub allow_containers: Option<Vec<String>>,
    /// Container names that are always rejected (takes precedence over
    /// `allow_containers`).
    pub deny_containers: Vec<String>,
    /// If `Some`, only these codec ids may be used; anything else is
    /// rejected. `None` ⇒ any registered codec.
    pub allow_codecs: Option<Vec<String>>,
    /// Codec ids that are always rejected.
    pub deny_codecs: Vec<String>,
    /// Override the extension hint used for probing (defaults to the
    /// one implied by the source address).
    pub ext_hint: Option<String>,
    /// When set, a single-frame video stream is collapsed to
    /// [`Opened::Image`] instead of being returned as lazy
    /// [`Opened::Media`]. The convenience `open()` sets this.
    pub eager_image: bool,
}

impl OpenOptions {
    /// Options with `eager_image` enabled — used by the zero-config
    /// `open()` so still images come back as [`Opened::Image`].
    pub fn eager() -> Self {
        OpenOptions {
            eager_image: true,
            ..Self::default()
        }
    }
}

/// The result of [`open`](crate::open)/[`open_with`].
///
/// Image / 3D / PDF / vector inputs are decoded eagerly; audio & video
/// stay lazy behind [`MediaReader`]. Marked `#[non_exhaustive]` and the
/// `Scene` / `Mesh` variants are feature-gated, so match arms need a
/// wildcard.
#[non_exhaustive]
pub enum Opened {
    /// A still image, decoded and packed to RGBA8888 / RGB24.
    Image(RgbaImage),
    /// A resolution-independent vector frame (SVG, vector PDF page).
    Vector(oxideav_core::VectorFrame),
    /// A multi-page document scene (PDF). Requires the `pdf` feature.
    /// Boxed because `oxideav_scene::Scene` is large relative to the other
    /// variants and would otherwise dominate every `Opened` value's size.
    #[cfg(feature = "pdf")]
    Scene(Box<oxideav_scene::Scene>),
    /// A decoded 3D model. Requires the `mesh` feature.
    #[cfg(feature = "mesh")]
    Mesh(oxideav_mesh3d::Scene3D),
    /// An audio/video stream, decoded lazily.
    Media(MediaReader),
}

impl std::fmt::Debug for Opened {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Opened::Image(_) => "Image",
            Opened::Vector(_) => "Vector",
            #[cfg(feature = "pdf")]
            Opened::Scene(_) => "Scene",
            #[cfg(feature = "mesh")]
            Opened::Mesh(_) => "Mesh",
            Opened::Media(_) => "Media",
        };
        write!(f, "Opened::{name}")
    }
}

/// A frame yielded by [`MediaReader`], tagged with its stream.
pub struct DecodedFrame {
    pub stream_index: u32,
    pub frame: Frame,
}

/// Lazy reader over an opened demuxer + its resolved decoders. Pulls
/// packets on demand and decodes them, buffering any look-ahead frames
/// produced while the facade decided image-vs-stream.
pub struct MediaReader {
    demuxer: Box<dyn Demuxer>,
    decoders: Vec<Option<Box<dyn Decoder>>>,
    streams: Vec<StreamInfo>,
    queue: VecDeque<DecodedFrame>,
    eof: bool,
}

impl MediaReader {
    /// The streams advertised by the underlying demuxer.
    pub fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }

    /// Pull the next decoded frame from any stream, or `None` at end of
    /// all streams.
    pub fn next_frame(&mut self) -> Result<Option<DecodedFrame>> {
        loop {
            if let Some(df) = self.queue.pop_front() {
                return Ok(Some(df));
            }
            if self.eof {
                return Ok(None);
            }
            self.pump()?;
        }
    }

    /// Pull the next decoded **video** frame (skipping audio/other), or
    /// `None` at end of stream.
    pub fn next_video_frame(&mut self) -> Result<Option<DecodedFrame>> {
        while let Some(df) = self.next_frame()? {
            if matches!(df.frame, Frame::Video(_)) {
                return Ok(Some(df));
            }
        }
        Ok(None)
    }

    /// Pull the next decoded **audio** frame (skipping video/other), or
    /// `None` at end of stream.
    pub fn next_audio_frame(&mut self) -> Result<Option<DecodedFrame>> {
        while let Some(df) = self.next_frame()? {
            if matches!(df.frame, Frame::Audio(_)) {
                return Ok(Some(df));
            }
        }
        Ok(None)
    }

    /// Decode one demuxer packet (or flush at EOF), appending any
    /// produced frames to the queue.
    fn pump(&mut self) -> Result<()> {
        match self.demuxer.next_packet() {
            Ok(pkt) => {
                let idx = pkt.stream_index as usize;
                if let Some(Some(dec)) = self.decoders.get_mut(idx) {
                    dec.send_packet(&pkt)?;
                    drain_decoder(dec.as_mut(), pkt.stream_index, &mut self.queue)?;
                }
                Ok(())
            }
            Err(oxideav_core::Error::Eof) => {
                self.eof = true;
                // Flush every decoder so buffered frames are surfaced.
                for (i, slot) in self.decoders.iter_mut().enumerate() {
                    if let Some(dec) = slot {
                        let _ = dec.flush();
                        drain_decoder(dec.as_mut(), i as u32, &mut self.queue)?;
                    }
                }
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Pump until the queue holds at least `n` frames or the streams
    /// end. Used by the facade to look ahead one or two frames.
    fn fill_at_least(&mut self, n: usize) -> Result<()> {
        while self.queue.len() < n && !self.eof {
            self.pump()?;
        }
        Ok(())
    }
}

/// Drain a decoder's ready frames into `queue`, stopping cleanly on the
/// "need more input" / "end" signals.
fn drain_decoder(
    dec: &mut dyn Decoder,
    stream_index: u32,
    queue: &mut VecDeque<DecodedFrame>,
) -> Result<()> {
    loop {
        match dec.receive_frame() {
            Ok(frame) => queue.push_back(DecodedFrame {
                stream_index,
                frame,
            }),
            Err(oxideav_core::Error::NeedMore) | Err(oxideav_core::Error::Eof) => return Ok(()),
            Err(e) => return Err(e.into()),
        }
    }
}

// ───────────────────────── allow/deny ─────────────────────────

fn permitted(name: &str, allow: &Option<Vec<String>>, deny: &[String]) -> bool {
    if deny.iter().any(|d| d.eq_ignore_ascii_case(name)) {
        return false;
    }
    match allow {
        Some(list) => list.iter().any(|a| a.eq_ignore_ascii_case(name)),
        None => true,
    }
}

// ───────────────────────── unified open ─────────────────────────

/// Open a source against a caller-supplied [`RuntimeContext`], returning
/// the decoded [`Opened`] variant. This is the lean entry point — the
/// caller decides what is registered in `ctx`.
pub fn open_with(ctx: &RuntimeContext, src: Source, opts: &OpenOptions) -> Result<Opened> {
    let ext = opts.ext_hint.clone().or_else(|| src.ext_hint());
    let mut reader = src.into_read_seek(ctx)?;
    let magic = peek_magic(reader.as_mut())?;

    // 1. PDF — eager Scene.
    if is_pdf(&magic, ext.as_deref()) {
        return open_pdf(reader);
    }
    // 2. 3D model — eager Mesh.
    if is_mesh3d(ext.as_deref()) {
        return open_mesh_reader(reader, ext.as_deref());
    }
    // 3. Everything else through the container+codec registry.
    let mut reader = open_registry(ctx, reader, ext.as_deref(), opts)?;

    // Look ahead up to two frames to classify image / vector / stream.
    reader.fill_at_least(2)?;
    match reader.queue.front().map(|df| &df.frame) {
        Some(Frame::Vector(_)) => {
            if let Some(DecodedFrame {
                frame: Frame::Vector(v),
                ..
            }) = reader.queue.pop_front()
            {
                return Ok(Opened::Vector(v));
            }
            unreachable!("front was Vector")
        }
        Some(Frame::Video(_)) if opts.eager_image && is_single_image(&reader) => {
            let img = collapse_front_video(&mut reader, PixelFormat::Rgba)?;
            Ok(Opened::Image(img))
        }
        _ => Ok(Opened::Media(reader)),
    }
}

/// True when the reader holds exactly one buffered frame, the streams
/// have ended, and there is no audio stream — i.e. a still image.
fn is_single_image(reader: &MediaReader) -> bool {
    reader.eof
        && reader.queue.len() == 1
        && !reader
            .queue
            .iter()
            .any(|df| matches!(df.frame, Frame::Audio(_)))
}

/// Pop the front (video) frame and pack it into the destination format.
fn collapse_front_video(reader: &mut MediaReader, dst: PixelFormat) -> Result<RgbaImage> {
    let df = reader
        .queue
        .pop_front()
        .ok_or_else(|| Error::invalid("collapse_front_video: empty queue"))?;
    pack_video(&reader.streams, df, dst)
}

/// Pack a decoded video frame into a tight RGBA/RGB24 buffer, reading
/// its dimensions/format from the owning stream's parameters.
fn pack_video(streams: &[StreamInfo], df: DecodedFrame, dst: PixelFormat) -> Result<RgbaImage> {
    let Frame::Video(vf) = df.frame else {
        return Err(Error::invalid("pack_video: frame is not video"));
    };
    let params = &streams
        .get(df.stream_index as usize)
        .ok_or_else(|| Error::invalid("pack_video: stream index out of range"))?
        .params;
    let width = params
        .width
        .ok_or_else(|| Error::invalid("image stream has no width"))?;
    let height = params
        .height
        .ok_or_else(|| Error::invalid("image stream has no height"))?;
    let src_format = params
        .pixel_format
        .ok_or_else(|| Error::invalid("image stream has no pixel format"))?;
    frame_to_packed(&vf, src_format, width, height, dst)
}

/// Build a [`MediaReader`] from a byte stream via the container+codec
/// registry, enforcing the allow/deny lists.
fn open_registry(
    ctx: &RuntimeContext,
    mut reader: Box<dyn ReadSeek>,
    ext: Option<&str>,
    opts: &OpenOptions,
) -> Result<MediaReader> {
    let cname = ctx
        .containers
        .probe_input(reader.as_mut(), ext)
        .map_err(|e| Error::probe(e.to_string()))?;
    if !permitted(&cname, &opts.allow_containers, &opts.deny_containers) {
        return Err(Error::restricted(format!(
            "container '{cname}' is not permitted by the open options"
        )));
    }
    let demuxer = ctx.containers.open_demuxer(&cname, reader, &ctx.codecs)?;
    let streams: Vec<StreamInfo> = demuxer.streams().to_vec();

    let mut decoders: Vec<Option<Box<dyn Decoder>>> = Vec::with_capacity(streams.len());
    for s in &streams {
        let codec_id = s.params.codec_id.as_str().to_string();
        if !permitted(&codec_id, &opts.allow_codecs, &opts.deny_codecs) {
            return Err(Error::restricted(format!(
                "codec '{codec_id}' is not permitted by the open options"
            )));
        }
        // A stream without a registered decoder is kept but not decoded.
        decoders.push(ctx.codecs.first_decoder(&s.params).ok());
    }

    Ok(MediaReader {
        demuxer,
        decoders,
        streams,
        queue: VecDeque::new(),
        eof: false,
    })
}

// ───────────────────────── specialized openers ─────────────────────────

/// Open a source and return its first frame packed as RGBA8888.
pub fn open_rgba_with(ctx: &RuntimeContext, src: Source, opts: &OpenOptions) -> Result<RgbaImage> {
    open_packed_with(ctx, src, opts, PixelFormat::Rgba)
}

/// Open a source and return its first frame packed as RGB24.
pub fn open_rgb_with(ctx: &RuntimeContext, src: Source, opts: &OpenOptions) -> Result<RgbaImage> {
    open_packed_with(ctx, src, opts, PixelFormat::Rgb24)
}

fn open_packed_with(
    ctx: &RuntimeContext,
    src: Source,
    opts: &OpenOptions,
    dst: PixelFormat,
) -> Result<RgbaImage> {
    let ext = opts.ext_hint.clone().or_else(|| src.ext_hint());
    let reader = src.into_read_seek(ctx)?;
    let mut reader = open_registry(ctx, reader, ext.as_deref(), opts)?;
    match reader.next_video_frame()? {
        Some(df) => pack_video(&reader.streams, df, dst),
        None => Err(Error::invalid("source produced no video frame to pack")),
    }
}

/// Open a source as a lazy [`MediaReader`], regardless of frame count.
pub fn open_media_with(
    ctx: &RuntimeContext,
    src: Source,
    opts: &OpenOptions,
) -> Result<MediaReader> {
    let ext = opts.ext_hint.clone().or_else(|| src.ext_hint());
    let reader = src.into_read_seek(ctx)?;
    open_registry(ctx, reader, ext.as_deref(), opts)
}

/// Open a PDF document as an [`oxideav_scene::Scene`] (one entry per
/// page). Errors if the source is not a PDF.
#[cfg(feature = "pdf")]
pub fn open_scene_with(
    ctx: &RuntimeContext,
    src: Source,
    _opts: &OpenOptions,
) -> Result<oxideav_scene::Scene> {
    use std::io::Read;
    let mut reader = src.into_read_seek(ctx)?;
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    oxideav_pdf::read_pdf_to_scene(&bytes).map_err(|e| Error::Decode(format!("pdf: {e}")))
}

/// Open a 3D model file as an [`oxideav_mesh3d::Scene3D`]. The decoder
/// is picked from the source's file extension.
#[cfg(feature = "mesh")]
pub fn open_mesh_with(
    ctx: &RuntimeContext,
    src: Source,
    opts: &OpenOptions,
) -> Result<oxideav_mesh3d::Scene3D> {
    let ext = opts.ext_hint.clone().or_else(|| src.ext_hint());
    let reader = src.into_read_seek(ctx)?;
    match open_mesh_reader(reader, ext.as_deref())? {
        Opened::Mesh(scene) => Ok(scene),
        _ => unreachable!("open_mesh_reader returns Opened::Mesh under the mesh feature"),
    }
}

// ───────────────────────── eager Scene / Mesh ─────────────────────────

#[cfg(feature = "pdf")]
fn open_pdf(mut reader: Box<dyn ReadSeek>) -> Result<Opened> {
    use std::io::Read;
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    let scene =
        oxideav_pdf::read_pdf_to_scene(&bytes).map_err(|e| Error::Decode(format!("pdf: {e}")))?;
    Ok(Opened::Scene(Box::new(scene)))
}

#[cfg(not(feature = "pdf"))]
fn open_pdf(_reader: Box<dyn ReadSeek>) -> Result<Opened> {
    Err(Error::unsupported(
        "PDF input requires the `pdf` feature (enabled by default via `full`)",
    ))
}

#[cfg(feature = "mesh")]
fn open_mesh_reader(mut reader: Box<dyn ReadSeek>, ext: Option<&str>) -> Result<Opened> {
    use std::io::Read;
    let ext =
        ext.ok_or_else(|| Error::invalid("3D input needs a file extension to pick a decoder"))?;
    let mut registry = oxideav_mesh3d::Mesh3DRegistry::new();
    oxideav_meta::populate_mesh3d_registry(&mut registry);
    oxideav_fbx::register(&mut registry);
    let mut decoder = registry.decoder_for_extension(ext).ok_or_else(|| {
        Error::unsupported(format!("no 3D decoder registered for extension '.{ext}'"))
    })?;
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    let scene = decoder
        .decode(&bytes)
        .map_err(|e| Error::Decode(format!("3D decode: {e}")))?;
    Ok(Opened::Mesh(scene))
}

#[cfg(not(feature = "mesh"))]
fn open_mesh_reader(_reader: Box<dyn ReadSeek>, _ext: Option<&str>) -> Result<Opened> {
    Err(Error::unsupported(
        "3D model input requires the `mesh` feature (enabled by default via `full`)",
    ))
}

#[cfg(all(test, feature = "full"))]
mod tests {
    use super::*;

    /// Minimal 2×2 binary PPM (P6, 8-bit RGB), top-down.
    /// Pixels: (0,0)=red (1,0)=white (0,1)=blue (1,1)=green.
    fn tiny_ppm() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"P6\n2 2\n255\n");
        b.extend_from_slice(&[255, 0, 0, 255, 255, 255]); // row 0
        b.extend_from_slice(&[0, 0, 255, 0, 255, 0]); // row 1
        b
    }

    fn ctx() -> RuntimeContext {
        let mut c = RuntimeContext::new();
        oxideav_meta::register_all(&mut c);
        c
    }

    #[test]
    fn open_with_collapses_still_image() {
        let c = ctx();
        let bytes = tiny_ppm();
        let opened = open_with(&c, Source::bytes(&bytes), &OpenOptions::eager()).expect("open PPM");
        match opened {
            Opened::Image(img) => assert_eq!((img.width, img.height), (2, 2)),
            other => panic!("expected Opened::Image, got {other:?}"),
        }
    }

    #[test]
    fn deny_container_is_rejected() {
        let c = ctx();
        let bytes = tiny_ppm();
        let opts = OpenOptions {
            deny_containers: vec!["pbm".to_string()],
            ..OpenOptions::default()
        };
        let res = open_with(&c, Source::bytes(&bytes), &opts);
        assert!(matches!(res, Err(Error::Restricted(_))), "got {res:?}");
    }

    #[test]
    fn allow_list_excluding_codec_is_rejected() {
        let c = ctx();
        let bytes = tiny_ppm();
        let opts = OpenOptions {
            // Allow some unrelated codec only — the real codec is excluded.
            allow_codecs: Some(vec!["definitely_not_a_real_codec".to_string()]),
            ..OpenOptions::default()
        };
        let res = open_rgba_with(&c, Source::bytes(&bytes), &opts);
        assert!(matches!(res, Err(Error::Restricted(_))), "got {res:?}");
    }
}
