# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.0.1](https://github.com/OxideAV/oxideav-io/releases/tag/v0.0.1) - 2026-06-15

### Other

- Drop oxideav-meta/pdf/mesh deps; lean registry-only facade
- Default to lean `registry`; make `full` (meta-backed) opt-in
- bootstrap + read facade (Phase 1)

### Added

- Initial **read facade** (Phase 1). A generic entry point that
  auto-detects an image / audio / video / SVG source and dispatches
  through the `oxideav-core` registries.
  - `Source` (path / URI-via-`SourceRegistry` / bytes / reader) and the
    `MediaKind` discrimination ladder.
  - Unified `open_with()` returning an `Opened` enum
    (`Image` / `Vector` / `Media`); still images decode eagerly, audio &
    video stay lazy behind a streaming `MediaReader`.
  - Specialized `open_rgba_with` / `open_rgb_with` / `open_media_with`.
  - `OpenOptions` with `allow_*` / `deny_*` lists to restrict which
    container / codec may run, plus `eager_image`.
  - `RgbaImage` packed-pixel buffer and the `VideoFrame` → RGBA/RGB24
    collapse (via `oxideav-pixfmt`).

### Notes

- The crate intentionally does **not** depend on `oxideav-meta`: meta's
  full codec fleet only resolves inside the workspace (via
  `[patch.crates-io]`), so a meta dependency — under any feature — would
  break the standard `--all-features` crate CI and crates.io publish. The
  zero-config `open(path)` (auto-register every codec) and the eager
  PDF / 3D decode paths are therefore delivered by the umbrella build,
  not this standalone crate.
