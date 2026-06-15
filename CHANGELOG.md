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
  - Feature layout: default lean `registry` base; opt-in `full`
    (meta-backed zero-config `open(path)`) with `pdf` / `mesh`
    eager-decode sub-features. `full` is opt-in rather than default
    because its `oxideav-meta` fleet resolves only inside the workspace
    until every sibling is published to crates.io.
