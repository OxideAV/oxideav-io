//! End-to-end smoke test of the read facade against a synthesized PPM,
//! exercised through the zero-config path API.
//!
//! Runs only under the default `full` feature (needs the meta-backed
//! context with the netpbm codec/container registered).

#![cfg(feature = "full")]

use std::io::Write;

use oxideav_io::{open, open_rgba, Opened};

/// Minimal 2×2 binary PPM (P6, 8-bit RGB), top-down.
/// Pixels: (0,0)=red (1,0)=white (0,1)=blue (1,1)=green.
fn tiny_ppm() -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(b"P6\n2 2\n255\n");
    b.extend_from_slice(&[255, 0, 0, 255, 255, 255]); // row 0
    b.extend_from_slice(&[0, 0, 255, 0, 255, 0]); // row 1
    b
}

fn write_temp_ppm() -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("oxideav_io_smoke_{}.ppm", std::process::id()));
    let mut f = std::fs::File::create(&path).expect("create temp ppm");
    f.write_all(&tiny_ppm()).expect("write temp ppm");
    path
}

#[test]
fn open_rgba_path() {
    let path = write_temp_ppm();
    let img = open_rgba(&path).expect("decode PPM to RGBA");
    let _ = std::fs::remove_file(&path);
    assert_eq!((img.width, img.height), (2, 2));
    assert_eq!(img.stride, 8);
    assert_eq!(img.pixels.len(), 16);
    assert_eq!(&img.pixels[0..4], &[255, 0, 0, 255]); // top-left red, opaque
}

#[test]
fn open_unified_returns_image() {
    let path = write_temp_ppm();
    let opened = open(&path);
    let _ = std::fs::remove_file(&path);
    match opened.expect("open PPM") {
        Opened::Image(img) => assert_eq!((img.width, img.height), (2, 2)),
        _ => panic!("expected Opened::Image for a still PPM"),
    }
}
