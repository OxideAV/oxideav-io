//! Fixture-backed probe coverage matrix.
//!
//! Every fixture is synthesized in-test — either through the facade's
//! own save path, through the registry's encoders + muxers, or as a
//! hand-rolled minimal header — then pushed through **both** probe
//! tiers. Each row pins:
//!
//! * the detected format id (`ping_format` and `probe` must AGREE);
//! * `MediaKind` routing (registry vs the eager PDF / 3D rungs);
//! * stream counts and kinds, and — where the container advertises
//!   them — codec id, dimensions, sample rate, channel count, bit rate,
//!   and derived duration.
//!
//! Formats covered: PNG, JPEG, PBM, DDS (image save path) · WAV, AVI,
//! Matroska×2 (PCM + FLAC payloads), raw MP3 (audio path) · SRT, WebVTT
//! (subtitles) · Y4M (raw video) · SVG (vector) · PDF (Scene) · STL
//! (Mesh).

#![cfg(feature = "full")]

use std::sync::OnceLock;

use oxideav_core::{
    AudioFrame, CodecId, CodecParameters, Frame, MediaType, RuntimeContext, SampleFormat,
    StreamInfo as CoreStreamInfo, TimeBase,
};
use oxideav_io::{
    ping_format_with, probe_with, save_with, MediaKind, OpenOptions, Opened, Probe, RgbaImage,
    SaveOptions, Sink, Source, StreamKind,
};

fn ctx() -> &'static RuntimeContext {
    static CTX: OnceLock<RuntimeContext> = OnceLock::new();
    CTX.get_or_init(|| {
        let mut c = RuntimeContext::new();
        oxideav_meta::register_all(&mut c);
        c
    })
}

// ───────────────────────── row driver ─────────────────────────

/// Push one fixture through both tiers and pin the shared invariants:
/// registry routing, two-tier format agreement, exact byte size.
fn media_row(name: &str, bytes: &[u8], want_container: &str) -> Probe {
    let c = ctx();
    let opts = OpenOptions::default();

    let ping = ping_format_with(c, Source::bytes(bytes), &opts)
        .unwrap_or_else(|e| panic!("{name}: ping failed: {e}"));
    assert_eq!(ping.kind, MediaKind::Media, "{name}: ping kind");
    assert_eq!(
        ping.format.as_deref(),
        Some(want_container),
        "{name}: detected format"
    );

    let info = probe_with(c, Source::bytes(bytes), &opts)
        .unwrap_or_else(|e| panic!("{name}: probe failed: {e}"));
    assert_eq!(info.kind, MediaKind::Media, "{name}: probe kind");
    assert_eq!(
        info.container, ping.format,
        "{name}: ping and probe must agree on the format"
    );
    assert_eq!(
        info.byte_size,
        Some(bytes.len() as u64),
        "{name}: byte_size"
    );
    info
}

// ───────────────────────── fixture builders ─────────────────────────

/// A 4×4 image saved through the facade in the given container, with
/// the default (Auto) pixel choice.
fn image_fixture(container: &str) -> Vec<u8> {
    let img = RgbaImage {
        width: 4,
        height: 4,
        pixels: [64u8, 128, 192, 255].repeat(16),
        stride: 16,
    };
    let mut buf = Vec::new();
    let opts = SaveOptions {
        container: Some(container.into()),
        ..SaveOptions::default()
    };
    save_with(ctx(), &Opened::Image(img), Sink::Buffer(&mut buf), &opts)
        .unwrap_or_else(|e| panic!("save {container} fixture: {e}"));
    buf
}

/// A canonical PCM WAV header + one second of 16-bit mono 8 kHz silence.
fn wav_fixture() -> Vec<u8> {
    let data_len: u32 = 16000; // 1 s × 8000 Hz × 2 bytes
    let mut b = Vec::with_capacity(44 + data_len as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&8000u32.to_le_bytes());
    b.extend_from_slice(&16000u32.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    b.resize(44 + data_len as usize, 0);
    b
}

/// A seekable in-memory sink whose bytes survive the muxer taking
/// ownership of its boxed clone.
#[derive(Clone, Default)]
struct BufSink(std::sync::Arc<std::sync::Mutex<std::io::Cursor<Vec<u8>>>>);

impl std::io::Write for BufSink {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        std::io::Write::write(&mut *self.0.lock().unwrap(), b)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl std::io::Seek for BufSink {
    fn seek(&mut self, p: std::io::SeekFrom) -> std::io::Result<u64> {
        self.0.lock().unwrap().seek(p)
    }
}

/// Encode 4×1024 samples of quiet mono s16 ramp with `codec`, returning
/// the packets and the encoder's output parameters.
fn encode_audio(codec: &str, sample_rate: u32) -> (Vec<oxideav_core::Packet>, CodecParameters) {
    let c = ctx();
    let mut params = CodecParameters::audio(CodecId::new(codec));
    params.sample_rate = Some(sample_rate);
    params.channels = Some(1);
    params.sample_format = Some(SampleFormat::S16);
    let mut enc = c
        .codecs
        .first_encoder(&params)
        .unwrap_or_else(|e| panic!("{codec} encoder: {e}"));
    let frame_len = 1024u32;
    for f in 0..4i64 {
        let mut data = Vec::with_capacity(frame_len as usize * 2);
        for i in 0..frame_len as i32 {
            data.extend_from_slice(&(((i % 64) * 8) as i16).to_le_bytes());
        }
        let frame = AudioFrame {
            samples: frame_len,
            pts: Some(f * i64::from(frame_len)),
            data: vec![data],
        };
        enc.send_frame(&Frame::Audio(frame))
            .unwrap_or_else(|e| panic!("{codec} send_frame: {e}"));
    }
    enc.flush().unwrap_or_else(|e| panic!("{codec} flush: {e}"));
    let mut packets = Vec::new();
    loop {
        match enc.receive_packet() {
            Ok(p) => packets.push(p),
            Err(oxideav_core::Error::NeedMore) | Err(oxideav_core::Error::Eof) => break,
            Err(e) => panic!("{codec} receive_packet: {e}"),
        }
    }
    assert!(!packets.is_empty(), "{codec}: encoder produced no packets");
    let mut out = enc.output_params().clone();
    out.media_type = MediaType::Audio;
    (packets, out)
}

/// Mux pre-encoded audio packets into `container` via the registry.
fn mux_audio(
    container: &str,
    packets: &[oxideav_core::Packet],
    params: CodecParameters,
    sample_rate: u32,
    total_samples: i64,
) -> Vec<u8> {
    let stream = CoreStreamInfo {
        index: 0,
        time_base: TimeBase::new(1, i64::from(sample_rate)),
        duration: Some(total_samples),
        start_time: Some(0),
        params,
    };
    let sink = BufSink::default();
    let mut mux = ctx()
        .containers
        .open_muxer(
            container,
            Box::new(sink.clone()),
            std::slice::from_ref(&stream),
        )
        .unwrap_or_else(|e| panic!("open {container} muxer: {e}"));
    mux.write_header()
        .unwrap_or_else(|e| panic!("{container} header: {e}"));
    for p in packets {
        mux.write_packet(p)
            .unwrap_or_else(|e| panic!("{container} packet: {e}"));
    }
    mux.write_trailer()
        .unwrap_or_else(|e| panic!("{container} trailer: {e}"));
    drop(mux);
    let bytes = sink.0.lock().unwrap().get_ref().clone();
    assert!(!bytes.is_empty(), "{container}: muxer wrote nothing");
    bytes
}

// ───────────────────────── image rows ─────────────────────────

#[test]
fn image_rows_report_one_video_stream_with_dimensions() {
    // (container, expected payload codec id)
    for (container, codec) in [
        ("png", "png"),
        ("jpeg", "mjpeg"),
        ("pbm", "pbm"),
        ("dds", "dds"),
    ] {
        let bytes = image_fixture(container);
        let info = media_row(container, &bytes, container);
        assert_eq!(info.streams.len(), 1, "{container}: stream count");
        assert!(
            info.has_video() && !info.has_audio(),
            "{container}: kinds ({:?})",
            info.streams
        );
        let s = info.first_video().expect("video stream");
        assert_eq!(s.codec, codec, "{container}: payload codec");
        assert_eq!(info.dimensions(), Some((4, 4)), "{container}: dimensions");
    }
}

// ───────────────────────── audio rows ─────────────────────────

#[test]
fn wav_row_reports_pcm_stream_with_rate_channels_duration() {
    let info = media_row("wav", &wav_fixture(), "wav");
    assert_eq!(info.streams.len(), 1);
    let s = info.first_audio().expect("audio stream");
    assert_eq!(s.codec, "pcm_s16le");
    assert_eq!(s.sample_rate, Some(8000));
    assert_eq!(s.channels, Some(1));
    assert_eq!(s.bit_rate, Some(128_000)); // 8000 Hz × 16 bit × mono
    let dur = info.duration_secs.expect("1 s of PCM has a duration");
    assert!((dur - 1.0).abs() < 1e-9, "wav duration: {dur}");
}

#[test]
fn avi_pcm_row_reports_audio_stream_and_duration() {
    let (packets, params) = encode_audio("pcm_s16le", 8000);
    let bytes = mux_audio("avi", &packets, params, 8000, 4096);
    let info = media_row("avi/pcm", &bytes, "avi");
    let s = info.first_audio().expect("audio stream");
    assert_eq!(s.codec, "pcm_s16le");
    assert_eq!(s.sample_rate, Some(8000));
    assert_eq!(s.channels, Some(1));
    let dur = info.duration_secs.expect("AVI advertises a duration");
    // 4096 samples @ 8 kHz = 0.512 s.
    assert!((dur - 0.512).abs() < 1e-6, "avi duration: {dur}");
}

#[test]
fn matroska_rows_carry_pcm_and_flac_payloads() {
    for codec in ["pcm_s16le", "flac"] {
        let (packets, params) = encode_audio(codec, 8000);
        let bytes = mux_audio("matroska", &packets, params, 8000, 4096);
        let info = media_row(&format!("matroska/{codec}"), &bytes, "matroska");
        let s = info.first_audio().expect("audio stream");
        assert_eq!(s.codec, codec, "matroska payload codec");
        assert_eq!(s.sample_rate, Some(8000));
        assert_eq!(s.channels, Some(1));
    }
}

#[test]
fn raw_mp3_stream_is_detected_with_rate_and_duration() {
    // MP3 is a raw packet stream — no muxer, just concatenated frames.
    let (packets, _params) = {
        let c = ctx();
        let mut params = CodecParameters::audio(CodecId::new("mp3"));
        params.sample_rate = Some(44100);
        params.channels = Some(1);
        params.sample_format = Some(SampleFormat::S16);
        let mut enc = c.codecs.first_encoder(&params).expect("mp3 encoder");
        for f in 0..8i64 {
            let mut data = Vec::with_capacity(1152 * 2);
            for i in 0..1152i32 {
                data.extend_from_slice(&(((i % 64) * 8) as i16).to_le_bytes());
            }
            enc.send_frame(&Frame::Audio(AudioFrame {
                samples: 1152,
                pts: Some(f * 1152),
                data: vec![data],
            }))
            .expect("mp3 send_frame");
        }
        enc.flush().expect("mp3 flush");
        let mut pkts = Vec::new();
        loop {
            match enc.receive_packet() {
                Ok(p) => pkts.push(p),
                Err(oxideav_core::Error::NeedMore) | Err(oxideav_core::Error::Eof) => break,
                Err(e) => panic!("mp3 receive_packet: {e}"),
            }
        }
        (pkts, params)
    };
    let bytes: Vec<u8> = packets
        .iter()
        .flat_map(|p| p.data.iter().copied())
        .collect();
    let info = media_row("mp3-raw", &bytes, "mp3");
    let s = info.first_audio().expect("audio stream");
    assert_eq!(s.codec, "mp3");
    assert_eq!(s.sample_rate, Some(44100));
    assert_eq!(s.channels, Some(1));
    // 8 × 1152 samples @ 44.1 kHz ≈ 0.209 s (frame-quantized).
    let dur = info.duration_secs.expect("mp3 stream duration");
    assert!((0.19..0.22).contains(&dur), "mp3 duration: {dur}");
}

// ───────────────────────── subtitle rows ─────────────────────────

#[test]
fn subtitle_rows_report_subtitle_streams_with_duration() {
    let srt = b"1\n00:00:00,000 --> 00:00:01,500\nHello there\n\n\
                2\n00:00:02,000 --> 00:00:03,000\nBye\n\n";
    let info = media_row("srt", srt, "srt");
    assert_eq!(info.streams.len(), 1);
    let s = &info.streams[0];
    assert!(s.is_subtitle(), "srt kind: {s:?}");
    assert_eq!(s.codec, "subrip");
    assert_eq!(info.duration_secs, Some(3.0), "last cue ends at 3 s");

    let vtt = b"WEBVTT\n\n00:00.000 --> 00:01.000\nHi\n\n";
    let info = media_row("webvtt", vtt, "webvtt");
    assert_eq!(info.streams.len(), 1);
    let s = &info.streams[0];
    assert!(s.is_subtitle(), "webvtt kind: {s:?}");
    assert_eq!(s.codec, "webvtt");
    assert_eq!(info.duration_secs, Some(1.0));
}

// ───────────────────────── raw video / vector rows ─────────────────────────

#[test]
fn y4m_row_reports_rawvideo_with_dimensions_and_header_metadata() {
    let mut y4m = b"YUV4MPEG2 W2 H2 F25:1 Ip A1:1 C420jpeg\n".to_vec();
    y4m.extend_from_slice(b"FRAME\n");
    y4m.extend_from_slice(&[16u8; 6]); // one 2×2 4:2:0 frame
    let info = media_row("y4m", &y4m, "y4m");
    let s = info.first_video().expect("video stream");
    assert_eq!(s.codec, "rawvideo");
    assert_eq!(info.dimensions(), Some((2, 2)));
    assert!(
        !info.metadata.is_empty(),
        "y4m stream-header parameters surface as metadata"
    );
}

#[test]
fn svg_row_reports_a_vector_video_stream() {
    let svg = br#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"></svg>"#;
    let info = media_row("svg", svg, "svg");
    assert_eq!(info.streams.len(), 1);
    let s = &info.streams[0];
    // Vector content rides a video-kind stream until decode time.
    assert_eq!(s.kind, StreamKind::Video);
    assert_eq!(s.codec, "svg");
}

// ───────────────────────── eager rungs ─────────────────────────

#[test]
fn pdf_and_stl_rows_take_the_eager_rungs() {
    let c = ctx();
    let pdf = b"%PDF-1.7\n1 0 obj\n<<>>\nendobj\n";
    let p = ping_format_with(c, Source::bytes(pdf), &OpenOptions::default()).expect("ping pdf");
    assert_eq!(
        (p.kind, p.format.as_deref()),
        (MediaKind::Scene, Some("pdf"))
    );
    let info = probe_with(c, Source::bytes(pdf), &OpenOptions::default()).expect("probe pdf");
    assert_eq!(info.kind, MediaKind::Scene);
    assert_eq!(info.byte_size, Some(pdf.len() as u64));
    assert!(info.container.is_none() && info.streams.is_empty());

    let opts = OpenOptions {
        ext_hint: Some("stl".to_string()),
        ..OpenOptions::default()
    };
    let stl = b"solid cube\nendsolid cube\n";
    let p = ping_format_with(c, Source::bytes(stl), &opts).expect("ping stl");
    assert_eq!(
        (p.kind, p.format.as_deref()),
        (MediaKind::Mesh, Some("stl"))
    );
    let info = probe_with(c, Source::bytes(stl), &opts).expect("probe stl");
    assert_eq!(info.kind, MediaKind::Mesh);
    assert!(info.container.is_none() && info.streams.is_empty());
}

// ───────────────────────── registry coverage floor ─────────────────────────

#[test]
fn every_matrix_container_is_registered_and_the_fleet_is_broad() {
    let c = ctx();
    let demuxers: Vec<String> = c
        .containers
        .demuxer_names()
        .map(|s| s.to_string())
        .collect();
    for want in [
        "png", "jpeg", "pbm", "dds", "wav", "avi", "matroska", "mp3", "srt", "webvtt", "y4m", "svg",
    ] {
        assert!(
            demuxers.iter().any(|d| d == want),
            "matrix container '{want}' is not registered"
        );
    }
    // Coverage floor, not an exact pin — the fleet only grows.
    assert!(
        demuxers.len() >= 50,
        "expected a broad demuxer fleet, got {}: {demuxers:?}",
        demuxers.len()
    );
}
