# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1](https://github.com/OxideAV/oxideav-io/compare/v0.1.0...v0.1.1) - 2026-07-09

### Fixed

- PixelChoice::Auto now walks an encoder-capability candidate ladder; y4m derives its rawvideo payload codec

### Other

- Source::Uri integration (data: / file:// probing) + anchor .gitignore Cargo.lock
- criterion benches for the probe hot paths + BENCHMARKS.md baseline
- probe contract, typed accessors, and seekability contract in README + CHANGELOG
- fixture-backed probe coverage matrix (14 formats, both tiers)
- typed accessors on Probe / StreamInfo
- enforce + pin the ping_format read-budget contract
- regroup xorshift seed literal digits (clippy unusual_byte_groupings)
- misdetection-hardening sweep for the probe dispatcher

### Added

- **Enforced `ping_format` read budget.** New public constant
  `PING_FORMAT_MAX_READ_BYTES` (257 KiB = 1 KiB magic peek + the
  registry's fixed 256 KiB probe window). The fast path now wraps the
  source in a metering reader that *fails* any read past the budget, so
  the "ping is cheap" promise holds even if a future probe
  implementation overreaches. Contract pinned by tests: bounded I/O on
  multi-MiB inputs (also when detection fails), no demuxer is ever
  opened (a valid signature with a corrupt body still pings), and a
  caller-supplied reader is probed from byte 0 and handed back at its
  original position.
- **Typed accessors on `Probe` / `StreamInfo`.**
  `video_streams()` / `audio_streams()` / `subtitle_streams()`,
  `first_video()` / `first_audio()`, `has_video()` / `has_audio()`,
  `dimensions()` (first video stream's advertised size), `duration()`
  (a `std::time::Duration`; negative / non-finite advertised values are
  rejected), and `meta(key)` — case-insensitive container-tag lookup.
  `StreamInfo` adds `dimensions()`, `duration()`, and `is_video()` /
  `is_audio()` / `is_subtitle()`.
- **Fixture-backed probe coverage matrix** (tests): PNG / JPEG / PBM /
  DDS (via the save path), PCM→AVI / PCM→Matroska / FLAC→Matroska /
  raw MP3 (via registry encoders + muxers), WAV / Y4M / SRT / WebVTT /
  SVG (hand-rolled minimal fixtures), plus the eager PDF (Scene) and
  STL (Mesh) rungs — each row pins the detected format (both tiers must
  agree), stream counts/kinds, payload codec ids, and advertised
  dimensions / rates / bit rates / durations.
- **Misdetection hardening** (tests): zero-byte / single-byte inputs,
  plain prose, truncated & ambiguous magics, per-byte truncation sweeps
  of facade-produced PNG/JPEG, contradictory extension hints, and
  deterministic garbage buffers all come back as typed errors or honest
  detections — never panics.

- **Two-tier format-probe API.** Identify a source **without a full
  decode**, at two levels of detail.
  - **Fast path** — `ping_format_with(ctx, Source, &OpenOptions)` (lean)
    and the zero-config `ping_format(path)` (under `full`): runs the
    PDF → 3D → container discrimination ladder and stops the instant the
    format is known. **Does not open a demuxer** (never reads the stream
    table). Returns `PingFormat { kind, format }`.
  - **Full probe** — `probe_with(ctx, Source, &OpenOptions)` (lean) and
    the zero-config `probe(path)` (under `full`): opens the demuxer (a
    header parse, not a decode) and reports overall size / duration /
    metadata plus a per-stream summary. Returns `Probe { kind, container,
    byte_size, duration_secs, metadata, streams }`.
  - New public types: `PingFormat`, `Probe`, the facade-owned
    `StreamInfo { index, kind, codec, width, height, sample_rate,
    channels, bit_rate, duration_secs }`, and `StreamKind { Audio, Video,
    Subtitle, Data, Unknown }` (a `From<oxideav_core::MediaType>` mirror
    so the probe surface doesn't leak a core type into lean callers).
  - PDF → `MediaKind::Scene`, 3D → `MediaKind::Mesh` (both with no
    container / empty stream list); everything else →
    `MediaKind::Media` with the detected container name and its stream
    table. Still images report as `Media` with one video-kind stream
    (separating image-vs-1-frame-video needs a decode, which probe
    avoids). Both tiers honour the same `allow_*`/`deny_*` lists as the
    openers.

### Fixed

- **`PixelChoice::Auto` now walks an encoder-capability candidate
  ladder.** It previously consulted the accepted-format sets of *every*
  implementation of the codec (decoders included) and treated an empty
  capability set as "RGBA is fine", so saving to JPEG with the default
  options failed at `send_frame` (the MJPEG encoder only takes RGB24).
  `Auto` now consults encoder implementations only and retries the
  encode per candidate (declared formats alpha-first, then RGBA /
  RGB24 fallbacks). Explicit `Rgb` / `Rgba` choices still yield exactly
  one candidate and fail loudly instead of silently re-packing.
- **Saving to the `y4m` container derives the `rawvideo` payload
  codec** instead of the nonexistent codec id `y4m`. (The registry has
  no `rawvideo` *encoder* yet, so the save still fails — but it now
  asks for the right codec, so the path lights up the moment one is
  registered.)

## [0.0.1](https://github.com/OxideAV/oxideav-io/releases/tag/v0.0.1) - 2026-06-15

### Other

- Drop oxideav-meta/pdf/mesh deps; lean registry-only facade
- Default to lean `registry`; make `full` (meta-backed) opt-in
- bootstrap + read facade (Phase 1)

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
