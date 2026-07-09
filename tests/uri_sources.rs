//! `Source::Uri` integration: probing through the context's
//! `SourceRegistry` drivers.
//!
//! The facade's other source shapes (path / bytes / reader) are covered
//! by the matrix and contract suites; this file pins the URI plumbing —
//! RFC 2397 `data:` URIs, `file://` URIs (including the extension hint
//! extracted from the URI's path component), and the typed rejection of
//! unresolvable or non-byte-shaped URIs.

#![cfg(feature = "full")]

use std::sync::OnceLock;

use oxideav_core::RuntimeContext;
use oxideav_io::{ping_format_with, probe_with, MediaKind, OpenOptions, Source};

fn ctx() -> &'static RuntimeContext {
    static CTX: OnceLock<RuntimeContext> = OnceLock::new();
    CTX.get_or_init(|| {
        let mut c = RuntimeContext::new();
        oxideav_meta::register_all(&mut c);
        c
    })
}

/// Minimal 1×1 binary PPM: one red pixel.
const TINY_PPM: &[u8] = b"P6\n1 1\n255\n\xff\x00\x00";

/// Dependency-free base64 (standard alphabet, padded) for building
/// `data:` URIs in-test.
fn base64(bytes: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[test]
fn data_uri_probes_through_the_source_registry() {
    let uri = format!("data:image/x-portable-pixmap;base64,{}", base64(TINY_PPM));
    let c = ctx();

    let p = ping_format_with(c, Source::Uri(&uri), &OpenOptions::default()).expect("ping data:");
    assert_eq!(p.kind, MediaKind::Media);
    assert_eq!(p.format.as_deref(), Some("pbm"));

    let info = probe_with(c, Source::Uri(&uri), &OpenOptions::default()).expect("probe data:");
    assert_eq!(info.container.as_deref(), Some("pbm"));
    assert_eq!(info.byte_size, Some(TINY_PPM.len() as u64), "decoded size");
    assert_eq!(info.streams.len(), 1);
    assert_eq!(info.dimensions(), Some((1, 1)));
}

#[test]
fn file_uri_probes_and_carries_the_extension_hint() {
    let path = std::env::temp_dir().join(format!("oxideav-io-uri-{}.ppm", std::process::id()));
    std::fs::write(&path, TINY_PPM).expect("write temp ppm");
    let uri = format!("file://{}", path.display());
    let c = ctx();

    let p = ping_format_with(c, Source::Uri(&uri), &OpenOptions::default());
    let info = probe_with(c, Source::Uri(&uri), &OpenOptions::default());
    let _ = std::fs::remove_file(&path);

    let p = p.expect("ping file://");
    assert_eq!(
        (p.kind, p.format.as_deref()),
        (MediaKind::Media, Some("pbm"))
    );
    let info = info.expect("probe file://");
    assert_eq!(info.container.as_deref(), Some("pbm"));
    assert_eq!(info.byte_size, Some(TINY_PPM.len() as u64));
}

#[test]
fn pdf_extension_in_a_uri_takes_the_scene_rung() {
    // The extension hint extracted from a URI's path component drives
    // the eager ladder exactly as a filesystem extension would.
    let path = std::env::temp_dir().join(format!("oxideav-io-uri-{}.pdf", std::process::id()));
    std::fs::write(&path, b"%PDF-1.4\nstub").expect("write temp pdf");
    let uri = format!("file://{}", path.display());
    let p = ping_format_with(ctx(), Source::Uri(&uri), &OpenOptions::default());
    let _ = std::fs::remove_file(&path);
    let p = p.expect("ping file://...pdf");
    assert_eq!(
        (p.kind, p.format.as_deref()),
        (MediaKind::Scene, Some("pdf"))
    );
}

#[test]
fn unresolvable_uris_are_typed_errors() {
    let c = ctx();
    for uri in [
        "bogus-scheme://nothing/here",
        "file:///definitely/not/a/real/file.ppm",
        "data:image/png;base64,%%%not-base64%%%",
    ] {
        let res = probe_with(c, Source::Uri(uri), &OpenOptions::default());
        assert!(res.is_err(), "'{uri}' must fail typed, got {res:?}");
    }
}
