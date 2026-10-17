//! `ping_format` / `probe` / gateway agreement over every registered
//! image container (the 25 image crates of `IMAGE_CRATE_API.md`,
//! HEIF-family brands included).
//!
//! Each fixture is produced by the fleet's own encoders through the
//! gateway, hand-rolled for SVG (a vector format the raster encoders
//! cannot feed) and for JPEG XL (no registry encoder today, so only
//! its two signatures — codestream and box container — are pinged);
//! then the facade's fast path, its full probe and the gateway's
//! `ImageFile::container()` must name the same container, from bytes
//! and from a buffered file path.

#![cfg(feature = "full")]

use std::sync::OnceLock;

use oxideav_core::RuntimeContext;
use oxideav_io::{
    open_image_file_with, open_with, ping_format_with, probe_with, MediaKind, OpenOptions, Opened,
    Source,
};

fn ctx() -> &'static RuntimeContext {
    static CTX: OnceLock<RuntimeContext> = OnceLock::new();
    CTX.get_or_init(|| {
        let mut c = RuntimeContext::new();
        oxideav_meta::register_all(&mut c);
        c
    })
}

/// A 4×4 opaque picture encoded as `format` (name or extension)
/// through the gateway.
fn gateway_fixture(format: &str) -> Vec<u8> {
    let img = oxideav_image::Image::from_rgba8(4, 4, [10u8, 20, 30, 255].repeat(16)).unwrap();
    oxideav_image::encode(ctx(), &img, format, &oxideav_image::SaveOptions::new())
        .unwrap_or_else(|e| panic!("encode {format}: {e}"))
}

/// JPEG XL signatures: the box container (`JXL ` signature box then
/// `ftyp`) and the bare codestream marker. No registry encoder exists
/// for the codec today, so these are identification-only fixtures.
fn jxl_container_signature() -> Vec<u8> {
    let mut b = vec![
        0, 0, 0, 0x0C, b'J', b'X', b'L', b' ', 0x0D, 0x0A, 0x87, 0x0A,
    ];
    b.extend_from_slice(&[0, 0, 0, 0x14]);
    b.extend_from_slice(b"ftypjxl \0\0\0\0jxl ");
    b
}

fn jxl_codestream_signature() -> Vec<u8> {
    vec![0xFF, 0x0A, 0, 0, 0, 0]
}

fn svg_fixture() -> Vec<u8> {
    b"<?xml version=\"1.0\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"4\" height=\"4\"><rect width=\"4\" height=\"4\" fill=\"#0a141e\"/></svg>\n".to_vec()
}

/// (how the fixture is requested, the container every tier must name)
const ROWS: &[(&str, &str)] = &[
    ("png", "png"),
    ("jpg", "jpeg"),
    ("heic", "heif"),
    ("heif", "heif"),
    ("avif", "heif"),
    ("webp", "webp"),
    ("gif", "gif"),
    ("tiff", "tiff"),
    ("bmp", "bmp"),
    ("qoi", "qoi"),
    ("tga", "tga"),
    ("pcx", "pcx"),
    ("ppm", "pbm"),
    ("ff", "farbfeld"),
    ("wbmp", "wbmp"),
    ("hdr", "hdr"),
    ("exr", "openexr"),
    ("dds", "dds"),
    ("ico", "ico"),
    ("pict", "pict"),
    ("icer", "icer"),
    ("jp2", "jp2"),
    ("j2k", "jpeg2000"),
    ("jxs", "jxs"),
    ("ilbm", "iff_ilbm"),
    ("svg", "svg"),
];

fn fixture(request: &str) -> Vec<u8> {
    match request {
        "svg" => svg_fixture(),
        other => gateway_fixture(other),
    }
}

#[test]
fn every_image_container_pings_probes_and_opens_consistently() {
    let c = ctx();
    let mut containers = std::collections::BTreeSet::new();
    for (request, want) in ROWS {
        let bytes = fixture(request);
        // Bytes, no extension hint: content detection alone.
        let ping = ping_format_with(c, Source::bytes(&bytes), &OpenOptions::default())
            .unwrap_or_else(|e| panic!("{request}: ping: {e}"));
        assert_eq!(ping.kind, MediaKind::Media, "{request}");
        assert_eq!(
            ping.format.as_deref(),
            Some(*want),
            "{request}: ping format"
        );
        let probe = probe_with(c, Source::bytes(&bytes), &OpenOptions::default())
            .unwrap_or_else(|e| panic!("{request}: probe: {e}"));
        assert_eq!(
            probe.container.as_deref(),
            Some(*want),
            "{request}: probe container"
        );
        assert!(probe.has_video(), "{request}: a video-kind stream");
        if *request == "svg" {
            // A vector format: the raster gateway has no picture for it;
            // the facade's unified open yields the vector frame.
            match open_with(c, Source::bytes(&bytes), &OpenOptions::eager()) {
                Ok(Opened::Vector(_)) => {}
                other => panic!("svg: expected Opened::Vector, got {other:?}"),
            }
        } else {
            let file = open_image_file_with(c, Source::bytes(&bytes), &OpenOptions::default())
                .unwrap_or_else(|e| panic!("{request}: gateway open: {e}"));
            assert_eq!(file.container(), *want, "{request}: gateway container");
            assert_eq!(
                (file.primary().width(), file.primary().height()),
                (4, 4),
                "{request}: geometry"
            );
        }
        // With the extension as a hint, the answer is the same.
        let hinted = OpenOptions::new().with_ext_hint(*request);
        let ping = ping_format_with(c, Source::bytes(&bytes), &hinted).unwrap();
        assert_eq!(
            ping.format.as_deref(),
            Some(*want),
            "{request}: hinted ping"
        );
        containers.insert(*want);
    }
    // JPEG XL: identification only (see the fixture).
    for (name, bytes) in [
        ("jxl container", jxl_container_signature()),
        ("jxl codestream", jxl_codestream_signature()),
    ] {
        let ping = ping_format_with(c, Source::bytes(&bytes), &OpenOptions::default())
            .unwrap_or_else(|e| panic!("{name}: ping: {e}"));
        assert_eq!(ping.format.as_deref(), Some("jpegxl"), "{name}");
        let hinted = OpenOptions::new().with_ext_hint("jxl");
        let ping = ping_format_with(c, Source::bytes(&bytes), &hinted).unwrap();
        assert_eq!(ping.format.as_deref(), Some("jpegxl"), "{name}: hinted");
        containers.insert("jpegxl");
    }
    // The 25 image crates: jpeg2000 answers for two containers (jp2 /
    // jpeg2000), jpegxs for jxs, iff for iff_ilbm, heif for every
    // HEIF-family brand.
    assert_eq!(containers.len(), 25, "{containers:?}");
}

#[test]
fn heif_brands_name_the_heif_container_from_a_buffered_path() {
    let c = ctx();
    let dir = std::env::temp_dir();
    let pid = std::process::id();
    for brand in ["heic", "heif", "avif"] {
        let path = dir.join(format!("oxideav-io-r477-brand-{pid}.{brand}"));
        std::fs::write(&path, gateway_fixture(brand)).unwrap();
        let ping = ping_format_with(c, Source::Path(&path), &OpenOptions::default()).unwrap();
        assert_eq!(ping.format.as_deref(), Some("heif"), "{brand}: path ping");
        let probe = probe_with(c, Source::Path(&path), &OpenOptions::default()).unwrap();
        assert_eq!(
            probe.container.as_deref(),
            Some("heif"),
            "{brand}: path probe"
        );
        let file = open_image_file_with(c, Source::Path(&path), &OpenOptions::default()).unwrap();
        assert_eq!(file.container(), "heif", "{brand}: path open");
        // Every brand is one container and one stream codec id, "heif";
        // the brand itself is container metadata.
        assert_eq!(probe.streams[0].codec, "heif", "{brand}: codec");
        let major = if brand == "avif" { "avif" } else { "heic" };
        assert_eq!(probe.meta("major_brand"), Some(major), "{brand}: brand");
        let _ = std::fs::remove_file(&path);
    }
}

#[test]
fn format_restricted_image_opens_are_refused_per_container_name() {
    // The allow list names containers as the gateway does: a HEIF
    // brand is "heif", a JPEG is "jpeg", a PPM is "pbm".
    let c = ctx();
    let heic = gateway_fixture("heic");
    let only_png = OpenOptions::new().with_allow_containers(["png"]);
    let r = open_image_file_with(c, Source::bytes(&heic), &only_png);
    assert!(matches!(r, Err(oxideav_io::Error::Restricted(_))), "{r:?}");
    let heif_ok = OpenOptions::new().with_allow_containers(["heif"]);
    assert!(open_image_file_with(c, Source::bytes(&heic), &heif_ok).is_ok());
    // The codec lists name stream codec ids: "heif" covers every
    // HEIF-family brand (an AVIF file is codec "heif" too), so a brand
    // cannot be singled out by codec — read `major_brand` instead.
    let no_heif_codec = OpenOptions::new().with_deny_codecs(["heif"]);
    for brand in ["heic", "avif"] {
        let bytes = gateway_fixture(brand);
        let r = open_image_file_with(c, Source::bytes(&bytes), &no_heif_codec);
        assert!(
            matches!(r, Err(oxideav_io::Error::Restricted(_))),
            "{brand}: {r:?}"
        );
    }
    let jpeg = gateway_fixture("jpg");
    let r = open_image_file_with(c, Source::bytes(&jpeg), &only_png);
    assert!(matches!(r, Err(oxideav_io::Error::Restricted(_))), "{r:?}");
    let mjpeg_only = OpenOptions::new().with_allow_codecs(["mjpeg"]);
    assert!(open_image_file_with(c, Source::bytes(&jpeg), &mjpeg_only).is_ok());
}
