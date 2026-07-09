//! Criterion benchmarks for the probe hot paths.
//!
//! Mirrors the bench shape the other OxideAV crates track (cinepak /
//! tta / flac / bmp): every fixture is synthesized on the fly — nothing
//! is committed — and each scenario isolates one rung of the
//! discrimination ladder so future optimisation rounds can A/B their
//! changes:
//!
//! - **ping_png_4x4**: fast-path hit on a tiny image — the "cheap
//!   identify" baseline (magic peek + probe window over a ~100 B file).
//! - **ping_wav_4mib**: fast-path hit on a large file — pins that ping
//!   cost does *not* scale with file size (the read-budget contract).
//! - **ping_noise_256k_miss**: fast-path miss — every registered
//!   container probe rejects the buffer; this is the worst case for the
//!   fast path (full probe window scanned, no early exit).
//! - **probe_png_4x4** / **probe_wav_4mib**: full-probe counterparts
//!   (demuxer open + stream-table parse on top of detection), giving
//!   the ping-vs-probe cost ratio.
//!
//! Requires the default `full` feature (meta-backed registry); under a
//! lean feature set the bench compiles to an empty `main` so
//! `--all-targets` builds stay green.

#[cfg(feature = "full")]
mod imp {
    use criterion::{BatchSize, Criterion};
    use oxideav_core::RuntimeContext;
    use oxideav_io::{
        ping_format_with, probe_with, save_with, OpenOptions, Opened, RgbaImage, SaveOptions, Sink,
        Source,
    };

    fn ctx() -> RuntimeContext {
        let mut c = RuntimeContext::new();
        oxideav_meta::register_all(&mut c);
        c
    }

    fn png_fixture(c: &RuntimeContext) -> Vec<u8> {
        let img = RgbaImage {
            width: 4,
            height: 4,
            pixels: [64u8, 128, 192, 255].repeat(16),
            stride: 16,
        };
        let mut buf = Vec::new();
        let opts = SaveOptions {
            container: Some("png".into()),
            ..SaveOptions::default()
        };
        save_with(c, &Opened::Image(img), Sink::Buffer(&mut buf), &opts).expect("seed PNG");
        buf
    }

    fn wav_fixture(data_len: u32) -> Vec<u8> {
        let mut b = Vec::with_capacity(44 + data_len as usize);
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&8000u32.to_le_bytes());
        b.extend_from_slice(&16000u32.to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data_len.to_le_bytes());
        b.resize(44 + data_len as usize, 0);
        b
    }

    pub fn run() {
        let mut criterion = Criterion::default().configure_from_args();
        let c = ctx();
        let opts = OpenOptions::default();

        let png = png_fixture(&c);
        let wav = wav_fixture(4 * 1024 * 1024);
        let noise = vec![0xB7u8; 256 * 1024];

        criterion.bench_function("ping_png_4x4", |b| {
            b.iter_batched(
                || png.clone(),
                |bytes| ping_format_with(&c, Source::bytes(&bytes), &opts).expect("ping png"),
                BatchSize::SmallInput,
            )
        });
        criterion.bench_function("ping_wav_4mib", |b| {
            b.iter(|| ping_format_with(&c, Source::bytes(&wav), &opts).expect("ping wav"))
        });
        criterion.bench_function("ping_noise_256k_miss", |b| {
            b.iter(|| {
                ping_format_with(&c, Source::bytes(&noise), &opts).expect_err("noise must not ping")
            })
        });
        criterion.bench_function("probe_png_4x4", |b| {
            b.iter(|| probe_with(&c, Source::bytes(&png), &opts).expect("probe png"))
        });
        criterion.bench_function("probe_wav_4mib", |b| {
            b.iter(|| probe_with(&c, Source::bytes(&wav), &opts).expect("probe wav"))
        });

        criterion.final_summary();
    }
}

#[cfg(feature = "full")]
fn main() {
    imp::run()
}

#[cfg(not(feature = "full"))]
fn main() {}
