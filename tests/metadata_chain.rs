//! Metadata blobs (ICC / Exif / XMP) through every io shape, against
//! the real fleet.
//!
//! HEIF is the format whose registry encoder and demuxer carry blobs
//! today (the gateway's round-476 survey: 0 of 25 image crates pushed
//! blobs through the registry on 2026-10-15; heif adopted them since),
//! so it is the persisting hop of every chain here. The png and jpeg
//! hops pin what the facade guarantees regardless of the crate: a hop
//! never corrupts or duplicates a blob (it either carries the list
//! unchanged or carries nothing), and the facade forwards it to the
//! next hop.

#![cfg(feature = "full")]

use std::sync::OnceLock;

use oxideav_core::{BlobKind, MetadataBlob, RuntimeContext};
use oxideav_io::{
    open_image_file_with, open_image_with, open_with, save_image_with, save_rgba_with, save_with,
    OpenOptions, Opened, RgbaImage, SaveOptions, Sink, Source,
};

fn ctx() -> &'static RuntimeContext {
    static CTX: OnceLock<RuntimeContext> = OnceLock::new();
    CTX.get_or_init(|| {
        let mut c = RuntimeContext::new();
        oxideav_meta::register_all(&mut c);
        c
    })
}

fn icc() -> MetadataBlob {
    MetadataBlob::new(BlobKind::ICC, vec![0x11u8; 128])
}
fn exif() -> MetadataBlob {
    // A TIFF-structured block: byte order, 42, first-IFD offset, empty IFD.
    MetadataBlob::new(BlobKind::EXIF, b"MM\0\x2a\0\0\0\x08\0\0".to_vec())
}
fn xmp() -> MetadataBlob {
    MetadataBlob::new(
        BlobKind::XMP,
        b"<x:xmpmeta xmlns:x='adobe:ns:meta/'></x:xmpmeta>".to_vec(),
    )
}

/// 4×4 opaque RGBA picture carrying the three blobs.
fn source() -> RgbaImage {
    RgbaImage::rgba(4, 4, [10u8, 20, 30, 255].repeat(16))
        .unwrap()
        .with_blobs(vec![icc(), exif(), xmp()])
}

fn save(img: &RgbaImage, format: &str, opts: SaveOptions) -> Vec<u8> {
    let mut buf = Vec::new();
    save_rgba_with(
        ctx(),
        img,
        Sink::Buffer(&mut buf),
        &opts.with_container(format),
    )
    .unwrap_or_else(|e| panic!("save {format}: {e}"));
    buf
}

fn open_flat(bytes: &[u8], opts: &OpenOptions) -> RgbaImage {
    match open_with(
        ctx(),
        Source::bytes(bytes),
        &opts.clone().with_eager_image(true),
    ) {
        Ok(Opened::Image(img)) => img,
        Ok(other) => panic!("expected Image, got {other:?}"),
        Err(e) => panic!("open: {e}"),
    }
}

fn same_blobs(a: &[MetadataBlob], b: &[MetadataBlob]) -> bool {
    let mut a: Vec<_> = a.iter().map(|b| (b.kind.clone(), b.data.clone())).collect();
    let mut b: Vec<_> = b.iter().map(|b| (b.kind.clone(), b.data.clone())).collect();
    a.sort();
    b.sort();
    a == b
}

#[test]
fn heic_persists_blobs_through_every_io_shape() {
    let src = source();
    let bytes = save(&src, "heic", SaveOptions::new());
    // Native picture.
    let img = open_image_with(ctx(), Source::bytes(&bytes), &OpenOptions::default()).unwrap();
    assert_eq!(img.icc(), Some(&icc().data[..]));
    assert_eq!(img.exif(), Some(&exif().data[..]));
    assert_eq!(img.xmp(), Some(&xmp().data[..]));
    // Whole file.
    let file = open_image_file_with(ctx(), Source::bytes(&bytes), &OpenOptions::default()).unwrap();
    assert_eq!(file.container(), "heif");
    assert!(file.has_metadata());
    assert_eq!(file.icc(), Some(&icc().data[..]));
    // Flattened.
    let flat = open_flat(&bytes, &OpenOptions::new());
    assert!(same_blobs(&flat.blobs, &src.blobs), "{:?}", flat.blobs);
    assert_eq!(flat.pixels, src.pixels);
    // Flattened picture saved again through `save_with(Opened::Image)`:
    // the blobs ride the buffer into the next file.
    let mut again = Vec::new();
    save_with(
        ctx(),
        &Opened::Image(flat),
        Sink::Buffer(&mut again),
        &SaveOptions::new().with_container("heic"),
    )
    .unwrap();
    let back = open_flat(&again, &OpenOptions::new());
    assert!(same_blobs(&back.blobs, &src.blobs));
    // And the native save path writes a native picture's blobs.
    let mut native = Vec::new();
    save_image_with(
        ctx(),
        &img,
        Sink::Buffer(&mut native),
        &SaveOptions::new().with_container("heic"),
    )
    .unwrap();
    let back = open_image_with(ctx(), Source::bytes(&native), &OpenOptions::default()).unwrap();
    assert_eq!(back.xmp(), Some(&xmp().data[..]));
}

#[test]
fn png_jpg_heic_chain_keeps_the_blobs() {
    // Each hop: save, reopen flattened, check the hop either carried
    // the blobs unchanged or carried none (never a corrupted or partial
    // list), then forward them. The heic end must hold all three.
    let src = source();
    let mut current = src.clone();
    let mut carried = Vec::new();
    for format in ["png", "jpg", "heic"] {
        let bytes = save(&current, format, SaveOptions::new());
        let reopened = open_flat(&bytes, &OpenOptions::new());
        assert_eq!(
            (reopened.width, reopened.height),
            (4, 4),
            "{format}: geometry"
        );
        if reopened.has_metadata() {
            assert!(
                same_blobs(&reopened.blobs, &src.blobs),
                "{format}: blobs came back changed: {:?}",
                reopened.blobs
            );
            carried.push(format);
            current = reopened;
        } else {
            // The crate does not embed blobs on the registry path yet;
            // the facade forwards them to the next hop.
            current = reopened.with_blobs(current.blobs.clone());
        }
    }
    assert!(
        carried.contains(&"heic"),
        "heic must persist the blobs (carried by: {carried:?})"
    );
    assert!(same_blobs(&current.blobs, &src.blobs));
}

#[test]
fn strip_rules_drop_blobs_on_open_and_on_save() {
    let src = source();
    let bytes = save(&src, "heic", SaveOptions::new());
    // On open: every shape.
    let none = OpenOptions::new().without_metadata();
    assert!(!open_image_with(ctx(), Source::bytes(&bytes), &none)
        .unwrap()
        .has_metadata());
    assert!(!open_image_file_with(ctx(), Source::bytes(&bytes), &none)
        .unwrap()
        .has_metadata());
    assert!(!open_flat(&bytes, &none).has_metadata());
    let no_exif = OpenOptions::new().with_stripped_blobs([BlobKind::EXIF]);
    let flat = open_flat(&bytes, &no_exif);
    assert!(flat.exif().is_none());
    assert!(flat.icc().is_some() && flat.xmp().is_some());
    // On save: withheld from the encoder.
    let stripped = save(&src, "heic", SaveOptions::new().without_metadata());
    assert!(!open_flat(&stripped, &OpenOptions::new()).has_metadata());
    let no_xmp = save(
        &src,
        "heic",
        SaveOptions::new().with_stripped_blobs([BlobKind::XMP]),
    );
    let flat = open_flat(&no_xmp, &OpenOptions::new());
    assert!(flat.xmp().is_none());
    assert!(flat.icc().is_some() && flat.exif().is_some());
    // Replace by kind on save.
    let other_icc = MetadataBlob::new(BlobKind::ICC, vec![0x22u8; 64]);
    let replaced = save(
        &src,
        "heic",
        SaveOptions::new().with_blob(other_icc.clone()),
    );
    let flat = open_flat(&replaced, &OpenOptions::new());
    assert_eq!(flat.icc(), Some(&other_icc.data[..]));
    assert_eq!(flat.blobs_of(&BlobKind::ICC).len(), 1);
    assert_eq!(flat.exif(), Some(&exif().data[..]));
    // Strip the source's, still write the options' own.
    let only_icc = save(
        &src,
        "heic",
        SaveOptions::new()
            .without_metadata()
            .with_blob(other_icc.clone()),
    );
    let flat = open_flat(&only_icc, &OpenOptions::new());
    assert_eq!(flat.blobs.len(), 1);
    assert_eq!(flat.icc(), Some(&other_icc.data[..]));
}

#[test]
fn zero_config_savers_write_by_path() {
    let dir = std::env::temp_dir();
    let pid = std::process::id();
    let heic = dir.join(format!("oxideav-io-r477-chain-{pid}.heic"));
    let png = dir.join(format!("oxideav-io-r477-chain-{pid}.png"));
    let src = source();
    oxideav_io::save_rgba(&src, &heic).unwrap();
    let flat = match oxideav_io::open(&heic).unwrap() {
        Opened::Image(img) => img,
        other => panic!("expected Image, got {other:?}"),
    };
    assert!(same_blobs(&flat.blobs, &src.blobs));
    let native = oxideav_io::open_image(&heic).unwrap();
    oxideav_io::save_image(&native, &png).unwrap();
    let pixels = oxideav_io::open_image(&png).unwrap().to_rgba8().unwrap();
    assert_eq!(pixels, src.pixels);
    let _ = std::fs::remove_file(&heic);
    let _ = std::fs::remove_file(&png);
}
