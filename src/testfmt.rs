//! A tiny self-contained still-image container + codec, registered
//! only in tests, so the lean `registry` slice (what the crate's own CI
//! builds — no `oxideav-meta`, no format crates) can drive the whole
//! facade pipeline through the gateway: probe → demux → decode →
//! flatten, and encode → mux, with every record the facade is
//! responsible for carrying — stream and per-picture metadata blobs,
//! an indexed picture's palette and palette alpha, and the forwarded
//! encode quality.
//!
//! The byte layout (`IOT1` magic, little-endian everywhere):
//!
//! ```text
//! "IOT1"  u8 layout (0 Pal8 · 1 Rgba · 2 Rgb24)  u8 quality (255 = unset)
//! u16 w   u16 h
//! u16 palette_len   palette bytes (packed RGB)
//! u16 alpha_len     palette-alpha bytes
//! u32 pixels_len    tightly packed pixel rows
//! u32 sblobs_len    stream-level blobs (core wire form)
//! u32 fblobs_len    per-picture blobs (core wire form)
//! ```
//!
//! The demuxer exposes the stream blobs on `CodecParameters::blobs`
//! and the written quality as container metadata (`quality = n`); the
//! decoder attaches the palette, the alpha record and the frame blobs
//! to the picture; the encoder accepts exactly the three layouts above
//! (anything else is refused at construction, so the gateway's ladder
//! steps) and writes the blobs it is handed at both levels.

use std::io::{Read, Write};

use oxideav_core::blob::{decode_blobs, encode_blobs};

use oxideav_core::{
    CodecCapabilities, CodecId, CodecInfo, CodecOptionsStruct, CodecParameters, CodecResolver,
    Decoder, Demuxer, Encoder, Error, Frame, MetadataBlob, Muxer, OptionField, OptionKind,
    OptionValue, Packet, PixelFormat, ProbeData, ProbeScore, ReadSeek, Result, RuntimeContext,
    StreamInfo, TimeBase, VideoFrame, VideoPlane, WriteSeek,
};

/// Registered container, codec and extension name.
pub(crate) const NAME: &str = "iotest";
/// File extension the container claims.
pub(crate) const EXT: &str = "iot";
const MAGIC: &[u8; 4] = b"IOT1";

/// Register the format in `ctx` (idempotent per registry contract).
pub(crate) fn register(ctx: &mut RuntimeContext) {
    let mut caps = CodecCapabilities::video("oxideav-io test codec");
    caps.decode = true;
    caps.encode = true;
    caps.lossless = true;
    caps.accepted_pixel_formats = vec![PixelFormat::Pal8, PixelFormat::Rgba, PixelFormat::Rgb24];
    ctx.codecs.register(
        CodecInfo::new(CodecId::new(NAME))
            .capabilities(caps)
            .decoder(open_decoder)
            .encoder(open_encoder)
            .encoder_options::<IotestOptions>(),
    );
    ctx.containers.register_demuxer(NAME, open_demuxer);
    ctx.containers.register_muxer(NAME, open_muxer);
    ctx.containers.register_extension(EXT, NAME);
    ctx.containers.register_probe(NAME, probe);
}

/// A context holding only this format.
pub(crate) fn context() -> RuntimeContext {
    let mut ctx = RuntimeContext::new();
    register(&mut ctx);
    ctx
}

fn probe(p: &ProbeData) -> ProbeScore {
    if p.buf.starts_with(MAGIC) { 100 } else { 0 }
}

// ───────────────────────── wire form ─────────────────────────

/// One parsed file.
struct Payload {
    layout: PixelFormat,
    quality: Option<u8>,
    width: u32,
    height: u32,
    palette: Vec<u8>,
    alpha: Vec<u8>,
    pixels: Vec<u8>,
    stream_blobs: Vec<MetadataBlob>,
    frame_blobs: Vec<MetadataBlob>,
}

fn layout_tag(f: PixelFormat) -> Option<u8> {
    match f {
        PixelFormat::Pal8 => Some(0),
        PixelFormat::Rgba => Some(1),
        PixelFormat::Rgb24 => Some(2),
        _ => None,
    }
}

fn bytes_per_pixel(f: PixelFormat) -> usize {
    match f {
        PixelFormat::Pal8 => 1,
        PixelFormat::Rgba => 4,
        _ => 3,
    }
}

impl Payload {
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.push(layout_tag(self.layout).expect("encodable layout"));
        out.push(self.quality.unwrap_or(255));
        out.extend_from_slice(&(self.width as u16).to_le_bytes());
        out.extend_from_slice(&(self.height as u16).to_le_bytes());
        out.extend_from_slice(&(self.palette.len() as u16).to_le_bytes());
        out.extend_from_slice(&self.palette);
        out.extend_from_slice(&(self.alpha.len() as u16).to_le_bytes());
        out.extend_from_slice(&self.alpha);
        out.extend_from_slice(&(self.pixels.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.pixels);
        let sb = encode_blobs(&self.stream_blobs);
        out.extend_from_slice(&(sb.len() as u32).to_le_bytes());
        out.extend_from_slice(&sb);
        let fb = encode_blobs(&self.frame_blobs);
        out.extend_from_slice(&(fb.len() as u32).to_le_bytes());
        out.extend_from_slice(&fb);
        out
    }

    fn decode(bytes: &[u8]) -> Result<Payload> {
        struct Cur<'a>(&'a [u8]);
        impl Cur<'_> {
            fn take(&mut self, n: usize) -> Result<&[u8]> {
                if self.0.len() < n {
                    return Err(Error::invalid("iotest: truncated"));
                }
                let (a, b) = self.0.split_at(n);
                self.0 = b;
                Ok(a)
            }
            fn u8(&mut self) -> Result<u8> {
                Ok(self.take(1)?[0])
            }
            fn u16(&mut self) -> Result<usize> {
                let b = self.take(2)?;
                Ok(u16::from_le_bytes([b[0], b[1]]) as usize)
            }
            fn u32(&mut self) -> Result<usize> {
                let b = self.take(4)?;
                Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
            }
        }
        let mut c = Cur(bytes);
        if c.take(4)? != MAGIC {
            return Err(Error::invalid("iotest: bad magic"));
        }
        let layout = match c.u8()? {
            0 => PixelFormat::Pal8,
            1 => PixelFormat::Rgba,
            2 => PixelFormat::Rgb24,
            t => return Err(Error::invalid(format!("iotest: layout tag {t}"))),
        };
        let quality = match c.u8()? {
            255 => None,
            q => Some(q),
        };
        let width = c.u16()? as u32;
        let height = c.u16()? as u32;
        let n = c.u16()?;
        let palette = c.take(n)?.to_vec();
        let n = c.u16()?;
        let alpha = c.take(n)?.to_vec();
        let n = c.u32()?;
        let pixels = c.take(n)?.to_vec();
        if pixels.len() != width as usize * height as usize * bytes_per_pixel(layout) {
            return Err(Error::invalid("iotest: pixel payload size"));
        }
        let n = c.u32()?;
        let stream_blobs = decode_blobs(c.take(n)?).unwrap_or_default();
        let n = c.u32()?;
        let frame_blobs = decode_blobs(c.take(n)?).unwrap_or_default();
        Ok(Payload {
            layout,
            quality,
            width,
            height,
            palette,
            alpha,
            pixels,
            stream_blobs,
            frame_blobs,
        })
    }
}

// ───────────────────────── demuxer ─────────────────────────

struct IotestDemuxer {
    streams: Vec<StreamInfo>,
    metadata: Vec<(String, String)>,
    packet: Option<Vec<u8>>,
}

fn open_demuxer(
    mut input: Box<dyn ReadSeek>,
    _codecs: &dyn CodecResolver,
) -> Result<Box<dyn Demuxer>> {
    let mut bytes = Vec::new();
    input.read_to_end(&mut bytes)?;
    let p = Payload::decode(&bytes)?;
    let mut params = CodecParameters::video(CodecId::new(NAME));
    params.width = Some(p.width);
    params.height = Some(p.height);
    params.pixel_format = Some(p.layout);
    params.blobs = p.stream_blobs;
    let metadata = p
        .quality
        .map(|q| vec![("quality".to_string(), q.to_string())])
        .unwrap_or_default();
    Ok(Box::new(IotestDemuxer {
        streams: vec![StreamInfo {
            index: 0,
            time_base: TimeBase::MILLIS,
            duration: None,
            start_time: None,
            params,
        }],
        metadata,
        packet: Some(bytes),
    }))
}

impl Demuxer for IotestDemuxer {
    fn format_name(&self) -> &str {
        NAME
    }
    fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }
    fn next_packet(&mut self) -> Result<Packet> {
        match self.packet.take() {
            Some(data) => Ok(Packet::new(0, TimeBase::MILLIS, data)),
            None => Err(Error::Eof),
        }
    }
    fn metadata(&self) -> &[(String, String)] {
        &self.metadata
    }
}

// ───────────────────────── decoder ─────────────────────────

struct IotestDecoder {
    id: CodecId,
    pending: Option<VideoFrame>,
    flushed: bool,
}

fn open_decoder(_params: &CodecParameters) -> Result<Box<dyn Decoder>> {
    Ok(Box::new(IotestDecoder {
        id: CodecId::new(NAME),
        pending: None,
        flushed: false,
    }))
}

impl Decoder for IotestDecoder {
    fn codec_id(&self) -> &CodecId {
        &self.id
    }
    fn send_packet(&mut self, packet: &Packet) -> Result<()> {
        let p = Payload::decode(&packet.data)?;
        let stride = p.width as usize * bytes_per_pixel(p.layout);
        let mut frame = VideoFrame {
            pts: packet.pts,
            planes: vec![VideoPlane {
                stride,
                data: p.pixels,
            }],
        };
        if !p.palette.is_empty() {
            frame.set_palette(p.palette);
        }
        if !p.alpha.is_empty() {
            frame.set_palette_alpha(p.alpha);
        }
        frame.set_blobs(p.frame_blobs);
        self.pending = Some(frame);
        Ok(())
    }
    fn receive_frame(&mut self) -> Result<Frame> {
        match self.pending.take() {
            Some(f) => Ok(Frame::Video(f)),
            None if self.flushed => Err(Error::Eof),
            None => Err(Error::NeedMore),
        }
    }
    fn flush(&mut self) -> Result<()> {
        self.flushed = true;
        Ok(())
    }
}

// ───────────────────────── encoder ─────────────────────────

/// The encoder's declared option schema: a `quality` knob, so the
/// gateway forwards `SaveOptions::quality` to it.
#[derive(Default)]
struct IotestOptions {
    quality: Option<u8>,
}

impl CodecOptionsStruct for IotestOptions {
    const SCHEMA: &'static [OptionField] = &[OptionField {
        name: "quality",
        kind: OptionKind::U32,
        default: OptionValue::U32(100),
        help: "recorded in the file header (advisory; the codec is lossless)",
    }];
    fn apply(&mut self, key: &str, value: &OptionValue) -> Result<()> {
        if key == "quality" {
            self.quality = Some(value.as_u32()?.min(254) as u8);
        }
        Ok(())
    }
}

struct IotestEncoder {
    id: CodecId,
    params: CodecParameters,
    quality: Option<u8>,
    pending: Option<Packet>,
}

fn open_encoder(params: &CodecParameters) -> Result<Box<dyn Encoder>> {
    let layout = params
        .pixel_format
        .ok_or_else(|| Error::invalid("iotest encoder: no pixel format"))?;
    if layout_tag(layout).is_none() {
        return Err(Error::unsupported(format!(
            "iotest encoder: {layout:?} is not Pal8 / Rgba / Rgb24"
        )));
    }
    let opts: IotestOptions = oxideav_core::parse_options(&params.options)?;
    Ok(Box::new(IotestEncoder {
        id: CodecId::new(NAME),
        params: params.clone(),
        quality: opts.quality,
        pending: None,
    }))
}

impl Encoder for IotestEncoder {
    fn codec_id(&self) -> &CodecId {
        &self.id
    }
    fn output_params(&self) -> &CodecParameters {
        &self.params
    }
    fn send_frame(&mut self, frame: &Frame) -> Result<()> {
        let Frame::Video(vf) = frame else {
            return Err(Error::invalid("iotest encoder: not a video frame"));
        };
        let layout = self.params.pixel_format.expect("checked at construction");
        let (width, height) = (
            self.params.width.unwrap_or(0),
            self.params.height.unwrap_or(0),
        );
        let plane = vf
            .image_planes()
            .first()
            .ok_or_else(|| Error::invalid("iotest encoder: frame has no plane"))?;
        let row = width as usize * bytes_per_pixel(layout);
        let mut pixels = Vec::with_capacity(row * height as usize);
        for y in 0..height as usize {
            let at = y * plane.stride;
            let line = plane
                .data
                .get(at..at + row)
                .ok_or_else(|| Error::invalid("iotest encoder: short plane"))?;
            pixels.extend_from_slice(line);
        }
        let payload = Payload {
            layout,
            quality: self.quality,
            width,
            height,
            palette: vf.palette().map(<[u8]>::to_vec).unwrap_or_default(),
            alpha: vf.palette_alpha().map(<[u8]>::to_vec).unwrap_or_default(),
            pixels,
            stream_blobs: self.params.blobs.clone(),
            frame_blobs: vf.blobs(),
        };
        let mut pkt = Packet::new(0, TimeBase::MILLIS, payload.encode());
        pkt.pts = vf.pts;
        self.pending = Some(pkt);
        Ok(())
    }
    fn receive_packet(&mut self) -> Result<Packet> {
        self.pending.take().ok_or(Error::NeedMore)
    }
    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

// ───────────────────────── muxer ─────────────────────────

struct IotestMuxer {
    out: Box<dyn WriteSeek>,
}

fn open_muxer(output: Box<dyn WriteSeek>, streams: &[StreamInfo]) -> Result<Box<dyn Muxer>> {
    if streams.len() != 1 {
        return Err(Error::unsupported("iotest: exactly one stream"));
    }
    Ok(Box::new(IotestMuxer { out: output }))
}

impl Muxer for IotestMuxer {
    fn format_name(&self) -> &str {
        NAME
    }
    fn write_header(&mut self) -> Result<()> {
        Ok(())
    }
    fn write_packet(&mut self, packet: &Packet) -> Result<()> {
        self.out.write_all(&packet.data)?;
        Ok(())
    }
    fn write_trailer(&mut self) -> Result<()> {
        self.out.flush()?;
        Ok(())
    }
}

/// A file of the test format holding `pixels` in `layout`, with the
/// given palette / alpha / blobs — built directly, without the
/// encoder, so a test can start from bytes on disk.
pub(crate) fn file(
    layout: PixelFormat,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    palette: &[[u8; 4]],
    stream_blobs: Vec<MetadataBlob>,
    frame_blobs: Vec<MetadataBlob>,
) -> Vec<u8> {
    let has_alpha = palette.iter().any(|e| e[3] != 255);
    Payload {
        layout,
        quality: None,
        width,
        height,
        palette: palette.iter().flat_map(|e| [e[0], e[1], e[2]]).collect(),
        alpha: if has_alpha {
            palette.iter().map(|e| e[3]).collect()
        } else {
            Vec::new()
        },
        pixels,
        stream_blobs,
        frame_blobs,
    }
    .encode()
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxideav_core::BlobKind;

    #[test]
    fn wire_form_round_trips() {
        let bytes = file(
            PixelFormat::Pal8,
            2,
            1,
            vec![0, 1],
            &[[1, 2, 3, 255], [4, 5, 6, 0]],
            vec![MetadataBlob::new(BlobKind::ICC, vec![9u8; 4])],
            vec![MetadataBlob::new(BlobKind::EXIF, vec![8u8; 2])],
        );
        let p = Payload::decode(&bytes).unwrap();
        assert_eq!((p.layout, p.width, p.height), (PixelFormat::Pal8, 2, 1));
        assert_eq!(p.palette, vec![1, 2, 3, 4, 5, 6]);
        assert_eq!(p.alpha, vec![255, 0]);
        assert_eq!(p.pixels, vec![0, 1]);
        assert_eq!(p.stream_blobs[0].kind, BlobKind::ICC);
        assert_eq!(p.frame_blobs[0].kind, BlobKind::EXIF);
        assert!(Payload::decode(&bytes[..bytes.len() - 1]).is_err());
        assert_eq!(
            probe(&ProbeData {
                buf: &bytes,
                ext: None
            }),
            100
        );
    }

    #[test]
    fn gateway_round_trip_carries_every_record() {
        // The format behaves through the gateway the way the facade
        // relies on: blobs at both levels, palette + alpha, quality.
        let ctx = context();
        let mut img = oxideav_image::Image::from_indexed(
            2,
            1,
            vec![0, 1],
            &[[10, 20, 30, 255], [40, 50, 60, 0]],
        )
        .unwrap();
        img.push_blob(MetadataBlob::new(BlobKind::XMP, b"<x/>".to_vec()));
        let opts = oxideav_image::SaveOptions::new().with_quality(42);
        let bytes = oxideav_image::encode(&ctx, &img, EXT, &opts).unwrap();
        let f = oxideav_image::decode_bytes(&ctx, &bytes).unwrap();
        assert_eq!(f.container(), NAME);
        assert_eq!(f.metadata(), &[("quality".to_string(), "42".to_string())]);
        let p = f.primary();
        assert_eq!(p.format(), PixelFormat::Pal8);
        assert_eq!(p.palette_alpha(), Some(&[255u8, 0][..]));
        assert_eq!(p.xmp(), Some(&b"<x/>"[..]));
        assert_eq!(p.to_rgba8().unwrap(), vec![10, 20, 30, 255, 40, 50, 60, 0]);
    }
}
