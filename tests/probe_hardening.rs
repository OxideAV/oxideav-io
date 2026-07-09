//! Misdetection hardening for the probe/facade dispatcher.
//!
//! Degenerate inputs — zero-byte files, single bytes, plain text,
//! ambiguous or truncated magic numbers, and deterministic garbage —
//! must come back as **typed errors or honest detections, never
//! panics**. Every entry point of the discrimination ladder is swept:
//! `ping_format_with` (fast path), `probe_with` (full probe), and
//! `open_with` (the real opener).
//!
//! Runs under the default `full` feature (needs the meta-backed
//! registry so the sweep exercises every registered container probe).

#![cfg(feature = "full")]

use std::sync::OnceLock;

use oxideav_core::RuntimeContext;
use oxideav_io::{open_with, ping_format_with, probe_with, Error, MediaKind, OpenOptions, Source};

/// One shared meta-backed context for the whole suite (building it per
/// test would re-register the full fleet dozens of times).
fn ctx() -> &'static RuntimeContext {
    static CTX: OnceLock<RuntimeContext> = OnceLock::new();
    CTX.get_or_init(|| {
        let mut c = RuntimeContext::new();
        oxideav_meta::register_all(&mut c);
        c
    })
}

fn opts_with_ext(ext: Option<&str>) -> OpenOptions {
    OpenOptions {
        ext_hint: ext.map(str::to_string),
        ..OpenOptions::default()
    }
}

/// Run all three ladder entry points over one input, asserting none of
/// them panics and every `Ok(probe)` upholds the structural invariants
/// (a `Media` probe names its container; `byte_size` is exact).
fn sweep(bytes: &[u8], ext: Option<&str>) {
    let c = ctx();
    let opts = opts_with_ext(ext);

    // Fast path.
    let _ = ping_format_with(c, Source::bytes(bytes), &opts);

    // Full probe: check invariants on success.
    if let Ok(p) = probe_with(c, Source::bytes(bytes), &opts) {
        assert_eq!(
            p.byte_size,
            Some(bytes.len() as u64),
            "probe byte_size must equal the input length (ext={ext:?})"
        );
        if p.kind == MediaKind::Media {
            assert!(
                p.container.is_some(),
                "a Media probe must name its container (ext={ext:?})"
            );
        } else {
            assert!(
                p.container.is_none() && p.streams.is_empty(),
                "Scene/Mesh probes carry no container/streams (ext={ext:?})"
            );
        }
    }

    // Real opener (decodes headers + first frames when it can).
    let _ = open_with(c, Source::bytes(bytes), &opts);
}

// ───────────────────────── zero / tiny inputs ─────────────────────────

#[test]
fn empty_input_is_a_typed_probe_error() {
    let c = ctx();
    let res = ping_format_with(c, Source::bytes(b""), &OpenOptions::default());
    assert!(matches!(res, Err(Error::Probe(_))), "got {res:?}");
    let res = probe_with(c, Source::bytes(b""), &OpenOptions::default());
    assert!(matches!(res, Err(Error::Probe(_))), "got {res:?}");
    let res = open_with(c, Source::bytes(b""), &OpenOptions::default());
    assert!(matches!(res, Err(Error::Probe(_))), "got {res:?}");
}

#[test]
fn empty_file_by_path_is_a_typed_probe_error() {
    let dir = std::env::temp_dir();
    let path = dir.join(format!("oxideav-io-empty-{}.bin", std::process::id()));
    std::fs::write(&path, b"").expect("create empty file");
    let res = probe_with(ctx(), Source::Path(&path), &OpenOptions::default());
    let _ = std::fs::remove_file(&path);
    assert!(matches!(res, Err(Error::Probe(_))), "got {res:?}");
}

#[test]
fn missing_file_by_path_is_io_error_not_panic() {
    let res = probe_with(
        ctx(),
        Source::Path(std::path::Path::new("/definitely/not/a/real/file.mkv")),
        &OpenOptions::default(),
    );
    assert!(matches!(res, Err(Error::Io(_))), "got {res:?}");
}

#[test]
fn single_byte_inputs_never_panic() {
    for b in [0x00u8, 0x01, 0x1A, 0x42, 0x89, 0xFF] {
        sweep(&[b], None);
    }
}

// ───────────────────────── ambiguous / truncated magic ─────────────────────────

#[test]
fn truncated_and_unknown_riff_headers_never_panic() {
    // Bare fourcc, size-only, and a RIFF whose form type nothing claims.
    sweep(b"RIFF", None);
    sweep(b"RIFF\x10\x00\x00\x00", None);
    sweep(b"RIFF\x10\x00\x00\x00XXXXdata", None);
    sweep(b"RIFF\xff\xff\xff\xffWAVE", None); // declared size overruns EOF
    sweep(b"RIFF\x00\x00\x00\x00AVI ", None); // zero-size AVI shell
}

#[test]
fn magic_only_prefixes_never_panic() {
    // Signatures with nothing behind them: the probe may claim the
    // container, but the (probe-tier) demuxer open must fail cleanly.
    let magics: &[&[u8]] = &[
        &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A], // PNG
        &[0xFF, 0xD8],                                     // JPEG SOI
        &[0x1A, 0x45, 0xDF, 0xA3],                         // EBML
        b"fLaC",                                           // FLAC
        b"OggS",                                           // Ogg page
        b"FORM",                                           // IFF shell
        b"P6",                                             // netpbm
        b"%PDF-",                                          // PDF (Scene path)
        &[0xFF, 0xFB],                                     // MPEG audio sync
        b"\x00\x00\x00\x18ftyp",                           // MP4 ftyp shell
        &[0x47],                                           // MPEG-TS sync
    ];
    for m in magics {
        sweep(m, None);
    }
}

#[test]
fn plain_text_never_panics_and_is_not_media_without_evidence() {
    // Free-form prose: no container should claim it as *audio/video*.
    // (Subtitle formats are text-shaped, so a text detection is only
    // acceptable if the claimed streams are subtitle/data kinds.)
    let text = b"hello world, just some plain prose\nwith two lines\n";
    sweep(text, None);
    if let Ok(p) = probe_with(ctx(), Source::bytes(text), &OpenOptions::default()) {
        for s in &p.streams {
            assert!(
                !matches!(
                    s.kind,
                    oxideav_io::StreamKind::Audio | oxideav_io::StreamKind::Video
                ),
                "plain prose misdetected as A/V: {p:?}"
            );
        }
    }
}

// ───────────────────────── extension-hint pressure ─────────────────────────

#[test]
fn wrong_extension_hints_on_garbage_never_panic() {
    // An extension hint that contradicts the payload must not crash any
    // ladder rung — including the eager PDF / 3D paths, which trust the
    // extension and then have to fail their decode cleanly.
    let payloads: &[&[u8]] = &[
        b"",
        b"\x00\x00\x00\x00\x00\x00\x00\x00",
        b"not really the format the extension promises",
        &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
    ];
    let exts = [
        "png", "jpg", "wav", "mkv", "mp4", "avi", "ogg", "flac", "mp3", "srt", "svg", "pdf", "stl",
        "obj", "gltf", "glb", "usdz", "fbx", "txt", "xyz",
    ];
    for payload in payloads {
        for ext in exts {
            sweep(payload, Some(ext));
        }
    }
}

#[test]
fn pdf_extension_on_empty_bytes_is_scene_but_open_fails_cleanly() {
    // `.pdf` wins the ladder by extension even with no bytes; the cheap
    // tiers classify, the real opener must fail with a typed error.
    let c = ctx();
    let opts = opts_with_ext(Some("pdf"));
    let p = ping_format_with(c, Source::bytes(b""), &opts).expect("ping .pdf");
    assert_eq!(p.kind, MediaKind::Scene);
    let res = open_with(c, Source::bytes(b""), &opts);
    assert!(matches!(res, Err(Error::Decode(_))), "got {res:?}");
}

#[test]
fn mesh_extension_on_garbage_is_mesh_but_open_fails_cleanly() {
    let c = ctx();
    let opts = opts_with_ext(Some("stl"));
    let garbage = vec![0xA5u8; 32];
    let p = ping_format_with(c, Source::bytes(&garbage), &opts).expect("ping .stl");
    assert_eq!(p.kind, MediaKind::Mesh);
    // Binary STL declares a triangle count; 32 bytes cannot satisfy the
    // 84-byte minimum, so the decode must fail (typed), not panic.
    let res = open_with(c, Source::bytes(&garbage), &opts);
    assert!(res.is_err(), "expected a decode failure, got {res:?}");
}

// ───────────────────────── truncation sweeps ─────────────────────────

/// Build a tiny valid PNG through the facade's own save path.
fn tiny_png() -> Vec<u8> {
    use oxideav_io::{save_with, Opened, RgbaImage, SaveOptions, Sink};
    let img = RgbaImage {
        width: 3,
        height: 2,
        pixels: [10u8, 20, 30, 255].repeat(6),
        stride: 12,
    };
    let mut buf = Vec::new();
    let opts = SaveOptions {
        container: Some("png".into()),
        ..SaveOptions::default()
    };
    save_with(ctx(), &Opened::Image(img), Sink::Buffer(&mut buf), &opts).expect("encode seed PNG");
    buf
}

/// Build a tiny valid JPEG through the facade's own save path.
fn tiny_jpeg() -> Vec<u8> {
    use oxideav_io::{save_with, Opened, PixelChoice, RgbaImage, SaveOptions, Sink};
    let img = RgbaImage {
        width: 3,
        height: 2,
        pixels: [10u8, 20, 30, 255].repeat(6),
        stride: 12,
    };
    let mut buf = Vec::new();
    let opts = SaveOptions {
        container: Some("jpeg".into()),
        pixel: PixelChoice::Rgb,
        ..SaveOptions::default()
    };
    save_with(ctx(), &Opened::Image(img), Sink::Buffer(&mut buf), &opts).expect("encode seed JPEG");
    buf
}

/// Every prefix of a valid file must probe without panicking — this is
/// the "truncated header" surface a network fetch or damaged disk hands
/// the dispatcher.
fn truncation_sweep(full: &[u8], ext: Option<&str>) {
    for len in 0..=full.len() {
        sweep(&full[..len], ext);
    }
}

#[test]
fn png_truncation_sweep_never_panics() {
    let png = tiny_png();
    truncation_sweep(&png, None);
    truncation_sweep(&png, Some("png"));
}

#[test]
fn jpeg_truncation_sweep_never_panics() {
    let jpeg = tiny_jpeg();
    truncation_sweep(&jpeg, None);
    truncation_sweep(&jpeg, Some("jpg"));
}

// ───────────────────────── deterministic garbage fuzz ─────────────────────────

/// xorshift64* — deterministic, dependency-free pseudo-random stream.
struct XorShift64(u64);

impl XorShift64 {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn fill(&mut self, buf: &mut [u8]) {
        for chunk in buf.chunks_mut(8) {
            let v = self.next().to_le_bytes();
            chunk.copy_from_slice(&v[..chunk.len()]);
        }
    }
}

#[test]
fn garbage_buffers_never_panic_the_dispatcher() {
    let mut rng = XorShift64(0x0A11_D01D_CAFE_F00D);
    let exts = [None, Some("png"), Some("wav"), Some("mkv"), Some("srt")];
    for round in 0..192 {
        let len = (rng.next() % 2048) as usize;
        let mut buf = vec![0u8; len];
        rng.fill(&mut buf);
        // Some rounds get a plausible magic stapled on garbage so the
        // per-container header parsers (not just the probes) get hit.
        match round % 6 {
            1 if len >= 8 => {
                buf[..8].copy_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])
            }
            2 if len >= 12 => {
                buf[..4].copy_from_slice(b"RIFF");
                buf[8..12].copy_from_slice(b"WAVE");
            }
            3 if len >= 4 => buf[..4].copy_from_slice(&[0x1A, 0x45, 0xDF, 0xA3]),
            4 if len >= 4 => buf[..4].copy_from_slice(b"OggS"),
            5 if len >= 4 => buf[..4].copy_from_slice(b"fLaC"),
            _ => {}
        }
        sweep(&buf, exts[round % exts.len()]);
    }
}
