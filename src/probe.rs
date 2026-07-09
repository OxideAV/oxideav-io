//! Format discrimination.
//!
//! Mirrors the routing ladder buried in `oxideav-cli-convert`'s
//! `build_summary` (PDF → 3D → SVG → container), consolidated here into
//! a single [`MediaKind`] classifier. PDF and 3D are detected up front
//! (by magic bytes / extension) because they decode to a `Scene` /
//! `Scene3D` rather than through the codec+container registry. Raster
//! images, A/V, and vector (SVG) streams all flow through the container
//! registry and are reported as [`MediaKind::Media`]; the frame-kind of
//! the first decoded frame later distinguishes image vs vector vs A/V.

use std::io::SeekFrom;

use oxideav_core::ReadSeek;

use crate::error::Result;

/// The broad category a source falls into, decided before any heavy
/// decode.
///
/// This is the category the discrimination ladder resolves *without*
/// decoding any frames: PDF and 3D are detected up front (by magic /
/// extension), everything else is routed through the container+codec
/// registry. Note that **still images report as [`MediaKind::Media`]**
/// — they live in the codec/container registry alongside A/V, and
/// telling a single-frame image apart from a 1-frame video requires a
/// decode, which [`probe`](crate::probe) deliberately avoids.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind {
    /// A PDF document → decodes eagerly to an `oxideav_scene::Scene`.
    Scene,
    /// A 3D model → decodes eagerly to an `oxideav_mesh3d::Scene3D`.
    Mesh,
    /// Everything routed through the container+codec registry: raster
    /// images, audio, video, and vector (SVG) streams.
    Media,
}

/// The broad media-type of a single stream, mirroring
/// [`oxideav_core::MediaType`] but kept as the facade's own enum so the
/// probe surface doesn't leak a core type into callers that only enabled
/// the `registry` feature for the lean entry points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamKind {
    /// An audio stream.
    Audio,
    /// A video stream (includes single-frame still images).
    Video,
    /// A timed-text / subtitle stream.
    Subtitle,
    /// An opaque data stream.
    Data,
    /// The container did not classify the stream.
    Unknown,
}

impl From<oxideav_core::MediaType> for StreamKind {
    fn from(t: oxideav_core::MediaType) -> Self {
        use oxideav_core::MediaType as Mt;
        match t {
            Mt::Audio => StreamKind::Audio,
            Mt::Video => StreamKind::Video,
            Mt::Subtitle => StreamKind::Subtitle,
            Mt::Data => StreamKind::Data,
            Mt::Unknown => StreamKind::Unknown,
        }
    }
}

/// The fast-path result of [`ping_format`](crate::ping_format) /
/// [`ping_format_with`](crate::ping_format_with): *just* the source's
/// broad kind and detected container/format name.
///
/// This is the cheapest tier — it runs the PDF / 3D / container-probe
/// ladder but **does not open a demuxer**, so it never reads the
/// container's stream table. Use it when all you need is "what format is
/// this?"; reach for [`probe`](crate::probe) when you want stream/size/
/// duration detail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PingFormat {
    /// The broad category resolved by the discrimination ladder.
    pub kind: MediaKind,
    /// The detected container/format name (e.g. `"png"`, `"matroska"`,
    /// `"pdf"`, `"stl"`). For the eager PDF / 3D paths this is the format
    /// keyword (`"pdf"` / the 3D extension); for the registry path it is
    /// the container the registry's probe selected.
    pub format: Option<String>,
}

/// Cheap, decode-free description of one stream inside a probed source.
///
/// Populated from the demuxer's stream table (which a container fills
/// from its header), so building one never decodes a frame.
#[derive(Clone, Debug, PartialEq)]
pub struct StreamInfo {
    /// The stream's index within the container.
    pub index: u32,
    /// Audio / video / subtitle / data classification.
    pub kind: StreamKind,
    /// The registered codec id (e.g. `"png"`, `"h264"`, `"aac"`).
    pub codec: String,
    /// Pixel width, when the container advertises one (video / image).
    pub width: Option<u32>,
    /// Pixel height, when the container advertises one (video / image).
    pub height: Option<u32>,
    /// Audio sample rate in Hz, when advertised.
    pub sample_rate: Option<u32>,
    /// Audio channel count, when advertised.
    pub channels: Option<u16>,
    /// Declared bit rate in bits/second, when the container advertises
    /// one. `None` for streams whose rate is not stored in the header.
    pub bit_rate: Option<u64>,
    /// Stream duration in seconds, derived from the container's per-stream
    /// duration ticks and time base. `None` when the container does not
    /// advertise a duration for this stream.
    pub duration_secs: Option<f64>,
}

/// The result of [`probe`](crate::probe) / [`probe_with`](crate::probe_with):
/// a source's broad kind, its detected container, overall size / duration
/// / metadata, and a per-stream summary — all obtained without a full
/// decode (header + container stream-table parse only).
#[derive(Clone, Debug, PartialEq)]
pub struct Probe {
    /// The broad category resolved by the discrimination ladder.
    pub kind: MediaKind,
    /// The detected container format name (e.g. `"png"`, `"matroska"`),
    /// when the source routed through the container registry. `None` for
    /// the eager PDF / 3D paths, which have no container concept here.
    pub container: Option<String>,
    /// Total source size in bytes, when it can be measured cheaply (a
    /// seekable reader's stream length). `None` for sources whose length
    /// is not known without consuming them.
    pub byte_size: Option<u64>,
    /// Overall container duration in seconds, when known — the
    /// container-level duration if it advertises one, else the longest
    /// per-stream duration. `None` when neither is available (e.g. most
    /// still images).
    pub duration_secs: Option<f64>,
    /// Container-level metadata as ordered (key, value) pairs (title,
    /// artist, …). Empty when the container carries none.
    pub metadata: Vec<(String, String)>,
    /// Per-stream summary. Empty for the eager PDF / 3D paths.
    pub streams: Vec<StreamInfo>,
}

/// File extensions that route to the 3D decode path.
pub(crate) const MESH3D_EXTS: &[&str] = &["stl", "obj", "gltf", "glb", "usdz", "fbx"];

/// How many leading bytes to sniff for magic-number detection.
const MAGIC_LEN: usize = 1024;

/// The hard upper bound on how many bytes
/// [`ping_format`](crate::ping_format) / [`ping_format_with`](crate::ping_format_with)
/// may **read** from a source, in total.
///
/// This is the fast path's contract: one [`MAGIC_LEN`]-byte magic peek
/// plus the container registry's fixed 256 KiB probe window — both taken
/// from the **start** of the source, with the cursor restored afterwards.
/// `ping_format` enforces the bound at the reader level (a read past it
/// fails with an I/O error), so the promise holds even if a future probe
/// implementation tries to read more. The full [`probe`](crate::probe)
/// tier is *not* bounded: its demuxer header parse may legitimately seek
/// anywhere (e.g. an index or stream table stored at the end of the
/// container).
pub const PING_FORMAT_MAX_READ_BYTES: u64 = (MAGIC_LEN + 256 * 1024) as u64;

/// A [`ReadSeek`] adapter that fails any read past a total-bytes budget.
///
/// Wrapped around the source inside `ping_format_with` to *enforce*
/// [`PING_FORMAT_MAX_READ_BYTES`]: seeking is free (probing seeks to the
/// start and restores the cursor), but the cumulative bytes handed out by
/// `read` may not exceed the budget. The budget equals the fast path's
/// worst-case legitimate consumption, so a well-behaved probe never trips
/// it — it exists to turn a future overreaching probe into a loud,
/// deterministic error instead of a silently slow `ping_format`.
pub(crate) struct BoundedReader<R> {
    inner: R,
    /// Total bytes handed out by `read` so far.
    consumed: u64,
    /// Maximum total bytes `read` may hand out.
    budget: u64,
}

impl<R: ReadSeek> BoundedReader<R> {
    pub(crate) fn new(inner: R, budget: u64) -> Self {
        BoundedReader {
            inner,
            consumed: 0,
            budget,
        }
    }
}

impl<R: ReadSeek> std::io::Read for BoundedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let remaining = self.budget.saturating_sub(self.consumed);
        if remaining == 0 {
            return Err(std::io::Error::other(format!(
                "ping_format read budget exhausted ({} bytes): probing must not \
                 read more than PING_FORMAT_MAX_READ_BYTES",
                self.budget
            )));
        }
        let cap = usize::try_from(remaining)
            .unwrap_or(usize::MAX)
            .min(buf.len());
        let n = self.inner.read(&mut buf[..cap])?;
        self.consumed += n as u64;
        Ok(n)
    }
}

impl<R: ReadSeek> std::io::Seek for BoundedReader<R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

/// Read up to [`MAGIC_LEN`] leading bytes without disturbing the cursor
/// (restored to its prior position on return).
pub(crate) fn peek_magic(input: &mut dyn ReadSeek) -> Result<Vec<u8>> {
    let saved = input.stream_position()?;
    input.seek(SeekFrom::Start(0))?;
    let mut buf = vec![0u8; MAGIC_LEN];
    let mut got = 0;
    while got < buf.len() {
        match input.read(&mut buf[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                let _ = input.seek(SeekFrom::Start(saved));
                return Err(e.into());
            }
        }
    }
    buf.truncate(got);
    input.seek(SeekFrom::Start(saved))?;
    Ok(buf)
}

/// True when `magic`/`ext` indicate a PDF document. The `%PDF-`
/// signature can sit a few bytes in (some files have a leading BOM /
/// junk), so we scan the first 64 bytes; the extension is a fallback.
pub(crate) fn is_pdf(magic: &[u8], ext: Option<&str>) -> bool {
    let scan = &magic[..magic.len().min(64)];
    scan.windows(5).any(|w| w == b"%PDF-") || ext == Some("pdf")
}

/// True when the extension names a supported 3D model format. (3D
/// formats are detected by extension only — several share weak or
/// absent magic numbers, e.g. ASCII OBJ.)
pub(crate) fn is_mesh3d(ext: Option<&str>) -> bool {
    ext.map(|e| MESH3D_EXTS.contains(&e)).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_detected_by_magic_and_extension() {
        assert!(is_pdf(b"%PDF-1.7\n...", None));
        assert!(is_pdf(b"\xef\xbb\xbf%PDF-1.4", None)); // leading BOM
        assert!(is_pdf(b"not a pdf", Some("pdf")));
        assert!(!is_pdf(b"not a pdf", Some("png")));
        assert!(!is_pdf(b"GIF89a", None));
    }

    #[test]
    fn mesh3d_detected_by_extension_only() {
        assert!(is_mesh3d(Some("stl")));
        assert!(is_mesh3d(Some("glb")));
        assert!(is_mesh3d(Some("fbx")));
        assert!(!is_mesh3d(Some("png")));
        assert!(!is_mesh3d(None));
    }

    #[test]
    fn stream_kind_from_core_media_type() {
        use oxideav_core::MediaType as Mt;
        assert_eq!(StreamKind::from(Mt::Audio), StreamKind::Audio);
        assert_eq!(StreamKind::from(Mt::Video), StreamKind::Video);
        assert_eq!(StreamKind::from(Mt::Subtitle), StreamKind::Subtitle);
        assert_eq!(StreamKind::from(Mt::Data), StreamKind::Data);
        assert_eq!(StreamKind::from(Mt::Unknown), StreamKind::Unknown);
    }

    #[test]
    fn probe_value_is_constructible_and_eq() {
        let p = Probe {
            kind: MediaKind::Media,
            container: Some("matroska".to_string()),
            byte_size: Some(4096),
            duration_secs: Some(12.5),
            metadata: vec![("title".to_string(), "demo".to_string())],
            streams: vec![StreamInfo {
                index: 0,
                kind: StreamKind::Video,
                codec: "h264".to_string(),
                width: Some(1920),
                height: Some(1080),
                sample_rate: None,
                channels: None,
                bit_rate: Some(2_000_000),
                duration_secs: Some(12.5),
            }],
        };
        assert_eq!(p.clone(), p);
        assert_eq!(p.streams[0].kind, StreamKind::Video);
        assert_eq!(p.metadata[0].0, "title");
    }

    #[test]
    fn bounded_reader_enforces_its_budget() {
        use std::io::{Cursor, Read, Seek};
        let data = (0u8..64).collect::<Vec<u8>>();
        let mut r = BoundedReader::new(Cursor::new(data), 10);

        // Within budget: normal reads, capped at the remaining budget.
        let mut buf = [0u8; 8];
        assert_eq!(r.read(&mut buf).unwrap(), 8);
        assert_eq!(r.read(&mut buf).unwrap(), 2, "read is capped at budget");

        // Budget exhausted: any further non-empty read is an error…
        let err = r.read(&mut buf).unwrap_err();
        assert!(err.to_string().contains("budget"), "got: {err}");
        // …but empty reads and seeks stay free.
        assert_eq!(r.read(&mut []).unwrap(), 0);
        assert_eq!(r.seek(SeekFrom::Start(0)).unwrap(), 0);
    }

    #[test]
    fn bounded_reader_eof_does_not_consume_budget() {
        use std::io::{Cursor, Read};
        let mut r = BoundedReader::new(Cursor::new(vec![1u8, 2, 3]), 10);
        let mut buf = [0u8; 8];
        assert_eq!(r.read(&mut buf).unwrap(), 3);
        // EOF: Ok(0) forever, never a budget error (only 3 bytes consumed).
        assert_eq!(r.read(&mut buf).unwrap(), 0);
        assert_eq!(r.read(&mut buf).unwrap(), 0);
    }

    #[test]
    fn ping_format_value_is_constructible_and_eq() {
        let a = PingFormat {
            kind: MediaKind::Scene,
            format: Some("pdf".to_string()),
        };
        assert_eq!(a.clone(), a);
        assert_eq!(a.kind, MediaKind::Scene);
    }
}
