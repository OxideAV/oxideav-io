//! The `ping_format` fast-path contract, pinned.
//!
//! `ping_format` promises three things `probe` does not:
//!
//! 1. **Bounded I/O** — it never reads more than
//!    [`PING_FORMAT_MAX_READ_BYTES`] from the source, however large the
//!    file is (one magic peek + the registry's fixed probe window, both
//!    from the start).
//! 2. **No demuxer** — it stops at format identification; the
//!    container's stream table is never parsed, so a file with a valid
//!    signature but a corrupt body still pings.
//! 3. **Cursor discipline** — a caller-supplied reader comes back at the
//!    position it went in at (probing always inspects the *start* of the
//!    stream regardless of that position).

#![cfg(feature = "full")]

use std::io::{Cursor, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use oxideav_core::RuntimeContext;
use oxideav_io::{
    ping_format_with, probe_with, MediaKind, OpenOptions, Source, PING_FORMAT_MAX_READ_BYTES,
};

fn ctx() -> &'static RuntimeContext {
    static CTX: OnceLock<RuntimeContext> = OnceLock::new();
    CTX.get_or_init(|| {
        let mut c = RuntimeContext::new();
        oxideav_meta::register_all(&mut c);
        c
    })
}

// ───────────────────────── metering reader ─────────────────────────

/// Counters shared between the test and the reader it hands the facade.
#[derive(Default)]
struct Meter {
    /// Total bytes handed out by `read`.
    total_read: AtomicU64,
    /// Final cursor position after the facade is done (updated on every
    /// read/seek).
    last_pos: AtomicU64,
    /// Highest byte offset any read reached.
    max_end: AtomicU64,
}

/// A seekable reader that records how much of it was actually read.
struct MeteredReader {
    inner: Cursor<Vec<u8>>,
    meter: Arc<Meter>,
}

impl MeteredReader {
    fn new(bytes: Vec<u8>) -> (Self, Arc<Meter>) {
        let meter = Arc::new(Meter::default());
        (
            MeteredReader {
                inner: Cursor::new(bytes),
                meter: Arc::clone(&meter),
            },
            meter,
        )
    }
}

impl Read for MeteredReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        let pos = self.inner.position();
        self.meter.total_read.fetch_add(n as u64, Ordering::Relaxed);
        self.meter.last_pos.store(pos, Ordering::Relaxed);
        self.meter.max_end.fetch_max(pos, Ordering::Relaxed);
        Ok(n)
    }
}

impl Seek for MeteredReader {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let p = self.inner.seek(pos)?;
        self.meter.last_pos.store(p, Ordering::Relaxed);
        Ok(p)
    }
}

// ───────────────────────── fixtures ─────────────────────────

/// A canonical little-endian PCM WAV: 16-bit mono 8 kHz, `data_len`
/// bytes of silence. Large enough files make byte-budget violations
/// observable.
fn big_wav(data_len: u32) -> Vec<u8> {
    let mut b = Vec::with_capacity(44 + data_len as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&8000u32.to_le_bytes()); // sample rate
    b.extend_from_slice(&16000u32.to_le_bytes()); // byte rate
    b.extend_from_slice(&2u16.to_le_bytes()); // block align
    b.extend_from_slice(&16u16.to_le_bytes()); // bits/sample
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    b.resize(44 + data_len as usize, 0);
    b
}

// ───────────────────────── 1. bounded I/O ─────────────────────────

#[test]
fn ping_read_volume_is_bounded_on_a_large_file() {
    // 4 MiB WAV — far larger than the permitted probe window.
    let (reader, meter) = MeteredReader::new(big_wav(4 * 1024 * 1024));
    let p = ping_format_with(
        ctx(),
        Source::Reader(Box::new(reader)),
        &OpenOptions::default(),
    )
    .expect("ping large WAV");
    assert_eq!(p.kind, MediaKind::Media);
    assert_eq!(p.format.as_deref(), Some("wav"));

    let total = meter.total_read.load(Ordering::Relaxed);
    assert!(
        total <= PING_FORMAT_MAX_READ_BYTES,
        "ping read {total} bytes, more than the contract's {PING_FORMAT_MAX_READ_BYTES}"
    );
    assert!(
        meter.max_end.load(Ordering::Relaxed) <= PING_FORMAT_MAX_READ_BYTES,
        "ping read past the head window"
    );
}

#[test]
fn ping_read_volume_is_bounded_even_when_detection_fails() {
    // 2 MiB of a repeating non-format byte pattern: every probe rejects
    // it, but the rejection itself must stay within the read budget.
    let bytes = vec![0xB7u8; 2 * 1024 * 1024];
    let (reader, meter) = MeteredReader::new(bytes);
    let res = ping_format_with(
        ctx(),
        Source::Reader(Box::new(reader)),
        &OpenOptions::default(),
    );
    assert!(res.is_err(), "pattern noise should not ping: {res:?}");
    assert!(meter.total_read.load(Ordering::Relaxed) <= PING_FORMAT_MAX_READ_BYTES);
}

// ───────────────────────── 2. no demuxer ─────────────────────────

#[test]
fn ping_succeeds_where_probe_needs_the_stream_table() {
    // Valid PNG signature, corrupt body: identification is possible,
    // header parsing is not. ping (no demuxer) must succeed; the full
    // probe (which opens the demuxer) must fail with a typed error.
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    bytes.extend_from_slice(&[0xDE; 64]); // garbage instead of IHDR
    let c = ctx();

    let p = ping_format_with(c, Source::bytes(&bytes), &OpenOptions::default())
        .expect("ping corrupt-body PNG");
    assert_eq!(p.kind, MediaKind::Media);
    assert_eq!(p.format.as_deref(), Some("png"));

    let full = probe_with(c, Source::bytes(&bytes), &OpenOptions::default());
    assert!(
        full.is_err(),
        "full probe opens the demuxer and must reject the corrupt body: {full:?}"
    );
}

// ───────────────────────── 3. cursor discipline ─────────────────────────

#[test]
fn ping_restores_a_midstream_cursor_and_probes_from_the_start() {
    // Hand the facade a reader parked mid-stream: detection must still
    // look at the *start* of the stream (correct answer: wav), and the
    // cursor must come back where it was.
    let (mut reader, meter) = MeteredReader::new(big_wav(1024));
    reader.seek(SeekFrom::Start(37)).expect("park mid-stream");

    let p = ping_format_with(
        ctx(),
        Source::Reader(Box::new(reader)),
        &OpenOptions::default(),
    )
    .expect("ping parked reader");
    assert_eq!(p.format.as_deref(), Some("wav"));
    assert_eq!(
        meter.last_pos.load(Ordering::Relaxed),
        37,
        "cursor must be restored to its pre-ping position"
    );
}

#[test]
fn probe_also_restores_byte_size_cursor_on_the_scene_path() {
    // The PDF (Scene) rung measures byte_size by seeking to the end; the
    // reported size must be the whole stream even for a parked reader.
    let mut bytes = b"%PDF-1.4\n".to_vec();
    bytes.resize(500, b' ');
    let (mut reader, _meter) = MeteredReader::new(bytes);
    reader.seek(SeekFrom::Start(9)).expect("park");
    let info = probe_with(
        ctx(),
        Source::Reader(Box::new(reader)),
        &OpenOptions::default(),
    )
    .expect("probe parked PDF");
    assert_eq!(info.kind, MediaKind::Scene);
    assert_eq!(info.byte_size, Some(500));
}

// ───────────────────────── the constant itself ─────────────────────────

#[test]
fn ping_budget_covers_magic_peek_plus_probe_window() {
    // 1 KiB magic peek + 256 KiB registry probe window.
    assert_eq!(PING_FORMAT_MAX_READ_BYTES, 1024 + 256 * 1024);
}
