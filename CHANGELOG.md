# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Initial **read facade** (Phase 1). A generic entry point that
  auto-detects an image / audio / video / 3D / PDF / SVG source and
  dispatches through the `oxideav-core` registries.
  - `Source` (path / URI-via-`SourceRegistry` / bytes / reader) and the
    `MediaKind` discrimination ladder (PDF → 3D → container).
  - Unified `open()` / `open_with()` returning an `Opened` enum
    (`Image` / `Vector` / `Scene` / `Mesh` / `Media`).
  - Specialized `open_rgba` / `open_rgb` / `open_media` (+ `open_scene`
    under `pdf`, `open_mesh` under `mesh`) and their `_with(ctx, …)`
    siblings.
  - `OpenOptions` with `allow_*` / `deny_*` lists to restrict which
    container / codec may run, plus `eager_image`.
  - Lazy `MediaReader` over the opened demuxer + resolved decoders.
  - `RgbaImage` packed-pixel buffer and the `VideoFrame` → RGBA/RGB24
    collapse (via `oxideav-pixfmt`).
  - Feature layout: default `full` (meta-backed zero-config `open(path)`
    + eager PDF / 3D) over a lean `registry` base. `full` pulls
    `oxideav-meta`, whose transitive fleet must be published to
    crates.io for a standalone build to resolve; inside the workspace it
    resolves via `[patch.crates-io]`.
- **Write facade** (Phase 2). Encode an `Opened` value back out through
  the registries.
  - `Sink` (path / boxed `Write` / `&mut Vec<u8>`) — the write-side
    mirror of `Source`. Every sink is committed from one finished byte
    buffer, so seekable muxers work over a non-seekable writer.
  - `SaveOptions { container, codec, pixel: PixelChoice (Auto|Rgb|Rgba),
    quality }`.
  - `save_with(ctx, &Opened, Sink, &SaveOptions)` for images — converts
    via `oxideav-pixfmt` to the codec's accepted layout, runs the
    `Encoder` (`send_frame` / `flush` / `receive_packet`), then the
    container `Muxer` (`write_header` / `write_packet` /
    `write_trailer`). 3D meshes re-encode through the `Mesh3DRegistry`
    by the sink extension (under `mesh`).
  - Under `full`: a no-context `save(opened, path)` convenience that
    derives the container/codec from the path extension.
  - PDF/document `Scene` and lazy a/v `MediaReader` saving are
    intentionally out of scope for now (clear `Unsupported` errors).
- **Transcode facade** (Phase 3, still-image path). `transcode_with(ctx,
  Source, Sink, &TranscodeOptions)` runs decode → transform chain →
  encode → mux.
  - `TranscodeOptions { open, save, transforms }` and a `Transform`
    enum: `Resize { width, height }` (via `oxideav-image-filter`, behind
    the default-on `transforms` feature) and `Convert(PixelChoice)` (via
    `oxideav-pixfmt`).
  - Under `full`: a no-context `transcode(src_path, dst_path)`.
  - The audio/video pipeline path (decode → filter graph → encode → mux
    on `oxideav-pipeline`) is the documented next step; a/v inputs
    return `Unsupported` today.
  - New default-on `transforms` feature (pulls `oxideav-image-filter`);
    droppable for a leaner `registry` build where `Transform::Resize`
    errors but `Transform::Convert` still works.
