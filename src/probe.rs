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

/// File extensions that route to the 3D decode path.
pub(crate) const MESH3D_EXTS: &[&str] = &["stl", "obj", "gltf", "glb", "usdz", "fbx"];

/// How many leading bytes to sniff for magic-number detection.
const MAGIC_LEN: usize = 1024;

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
}
