# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/OxideAV/oxideav-io/compare/v0.1.1...v0.2.0) - 2026-10-23

### Other

- edition 2024, MSRV 1.95
- probe surfaces stream-level blobs under the strip rules; Probe / StreamInfo / PingFormat / TranscodeOptions / Transform hygiene
- round 477 — metadata and palette-alpha ruling, OpenOptions / SaveOptions builders, HEIF brand naming, buffered sources
- metadata chain through the fleet and a 25-container ping / probe / gateway matrix
- SaveOptions adopts the gateway's blobs, strip rules and encoder options; save_rgba_with + zero-config savers
- OpenOptions adopts the gateway's strip rules, pixel budget and decoder options; buffered sources
- RgbaImage carries metadata blobs and composites palette alpha; constructors + non_exhaustive
- an in-crate test container + codec drives the facade on the registry slice
- allow / deny lists ride the gateway; ImageError::Restricted maps to Error::Restricted
- ride the oxideav-image gateway for every still-image path

### Changed

- edition 2024, MSRV 1.95
- **Allow / deny lists ride the gateway.** `OpenOptions`'s
  `allow_containers` / `deny_containers` / `allow_codecs` / `deny_codecs`
  are handed to `oxideav_image::OpenOptions` (`with_allow_*` /
  `with_deny_*`, exposed as `OpenOptions::to_image_options`) instead of
  being enforced by a shared-reader header parse in this crate; the
  gateway's `ImageError::Restricted` maps to `Error::Restricted` (it
  used to fold into `Decode`). Same contract: the container is checked
  right after the probe, every video stream's codec from the stream
  table before a decoder exists.
- **Images ride `oxideav-image`.** Every still-image decode, pixel-layout
  conversion, encode and mux goes through the `oxideav-image` gateway
  (Layer 2 of `IMAGE_CRATE_API.md`); the crate's own frame packing
  (`oxideav-pixfmt` direct dependency), encoder ladder, in-memory mux
  cursor and default-codec table are gone (about 450 lines of duplicated
  pipeline; net −57 non-test lines once the new entry points, the
  allow / deny admission and the `RgbaImage` bridge are counted).
  `registry` now pulls `oxideav-core` + `oxideav-image`.
- `SaveOptions::container` accepts a file extension as well as a
  container name; `PixelChoice::Auto` is the gateway's ladder (image's
  own layout, then `Rgba`, `Rgb24`, `Gray8`, `Yuv444P`, `Yuv420P`,
  `Rgba64Le`, stepping on encoder **or** muxer refusal); `quality` is
  forwarded only to encoders whose option schema declares it.
- `Opened::Image` is built from the native `oxideav_image::Image`
  (stream colour signal honoured by the conversion).
- Gateway errors fold onto the facade's variants: `UnknownFormat` →
  `Probe` on read / `Unsupported` on write, `InvalidData` / `NoImage` /
  `LimitExceeded` → `Invalid`.
- The Y4M save test now requires the round trip (a `rawvideo` encoder is
  registered); the first layout Y4M accepts on the gateway's ladder is
  `Gray8` (`Cmono`).

### Added

- **`probe` surfaces stream-level blobs.** `StreamInfo::blobs` (what the
  demuxer declared on the stream, read from the header, after the
  `OpenOptions` strip rules) with `blob(&kind)` / `icc()` / `exif()` /
  `xmp()` / `iptc()` / `has_metadata()`, and the same accessors on
  `Probe` for its first video stream (`Probe::blobs()`).
- `TranscodeOptions::new()` / `with_open` / `with_save` /
  `with_transform(s)` builders.
- **`SaveOptions` adopts the gateway's knobs**: `blobs`
  (`with_blob` / `with_blobs` — written on top of the picture's own,
  replacing the source blob of the same kind), `strip_metadata`
  (`without_metadata()`) and `strip_blobs` (`with_stripped_blobs`)
  withholding the picture's blobs from the encoder, `options`
  (`with_option`) forwarded verbatim; builders for the existing fields
  (`new`, `with_container`, `with_codec`, `with_pixel`, `with_quality`);
  `to_image_options()` maps them one for one.
- A kind named in `SaveOptions::blobs` is removed from the picture at
  both levels before the encode, so the replacement holds whichever
  level the encoder reads (the gateway replaces at the stream level
  only; a per-picture blob of the same kind would win on reopen).
- `save_rgba_with(ctx, &RgbaImage, sink, &SaveOptions)` — the
  flattened-buffer save `save_with` runs for `Opened::Image`, with the
  buffer's `blobs` written — and the zero-config `save_image(&Image,
  path)` / `save_rgba(&RgbaImage, path)`.
- **`OpenOptions` adopts the gateway's knobs**: `strip_metadata` /
  `strip_blobs` (`without_metadata()`, `with_stripped_blobs(kinds)`)
  apply to the native `Image` / `ImageFile` through the gateway and to
  the flattened `Opened::Image` on the unified path; `max_pixels`
  (`with_max_pixels`) refuses an oversized stream from its geometry
  before any decoder exists, on both paths (`Error::Invalid`);
  `decoder_options` (`with_decoder_option`). Builders for every field
  (`new`, `eager`, `with_eager_image`, `with_ext_hint`,
  `with_allow_containers` / `with_deny_containers` /
  `with_allow_codecs` / `with_deny_codecs`, `with_metadata`), and
  `to_image_options()` maps them one for one.
- `Source::Path` opens the file behind a `BufReader`; `Source::reader(r)`
  boxes any `Read + Seek + Send`, `Source::buffered(r)` wraps it in a
  `BufReader` first; `Sink::writer(w)` boxes any `Write + Send`.
- **`RgbaImage` carries metadata.** A `blobs: Vec<MetadataBlob>` field
  with `blob(&kind)` / `blobs_of` / `icc()` / `exif()` / `xmp()` /
  `iptc()` / `has_metadata()` / `with_blob(s)` / `push_blob` /
  `retain_blobs` / `strip_metadata` / `without_metadata`.
  `RgbaImage::from_image` (hence `Opened::Image`, `open_rgba*` /
  `open_rgb*`) lifts the native picture's resolved blobs (frame first,
  then stream kinds the frame lacks); `to_image` / `into_image` attach
  them as the picture's own, so the flattened save path writes them
  (ruling: the blobs live on the buffer, not on `Opened`). The
  transcode transforms (`Convert`, `Resize`) keep them.
- `RgbaImage::new(width, height, pixels, stride)` / `rgba(..)` /
  `rgb(..)` constructors (geometry-checked, `Error::Invalid` on a
  mismatch or a zero side); the struct is `#[non_exhaustive]`.
- An indexed picture's palette alpha is composited into the RGBA
  flattening (transparent entry → alpha 0) and dropped by the RGB24
  one, as the gateway's views do (pixfmt 0.1.11 / image 0.0.4).
- Fleet-level suites (`full` feature): `tests/metadata_chain.rs` — ICC /
  Exif / XMP through every io shape with HEIF as the persisting hop, a
  png → jpg → heic chain that never corrupts or duplicates a blob and
  forwards the list, strip / replace rules on open and on save, the
  zero-config savers; `tests/image_containers.rs` — `ping_format` /
  `probe` / gateway `ImageFile::container()` agreement over all 25 image
  crates (HEIF-family brands → `heif` with stream codec `heif`, JPEG XL
  by both signatures, SVG as `Opened::Vector`), from bytes and from a
  buffered path, and container-name restrictions.
- An in-crate test container + codec (`iotest`, tests only) that
  carries stream and per-picture blobs, a palette with its alpha record
  and the forwarded `quality`, so the lean `registry` slice the crate's
  CI builds now drives the whole facade pipeline — ping / probe / open /
  flatten / save, allow / deny refusals on every entry point — without
  `oxideav-meta` or any format crate.
- `open_image` / `open_image_with` → `oxideav_image::Image` (primary
  picture, native layout; only the first picture is decoded) and
  `open_image_file` / `open_image_file_with` → `oxideav_image::ImageFile`
  (every picture, container name, metadata). `OpenOptions` allow / deny
  lists are enforced before the gateway runs (container probe; stream
  table header parse for the codec lists — a denied codec never decodes).
- `save_image_with(ctx, &Image, sink, &SaveOptions)` encodes a native
  picture without flattening.
- `RgbaImage::from_image` / `from_image_rgb` / `to_image` / `into_image` /
  `pixel_format`, `TryFrom<Image> for RgbaImage` and `TryFrom<RgbaImage>
  for Image` (both by value and by reference).
- `PixelChoice::pixel_format()`, `SaveOptions::to_image_options()`.
- `Image` and `ImageFile` re-exported at the crate root.

### Changed (breaking)

- `Probe`, `StreamInfo`, `PingFormat` (facade-produced records, read by
  field), `TranscodeOptions` and the `Transform` enum are
  `#[non_exhaustive]`.
- `SaveOptions` is `#[non_exhaustive]`: build it with `SaveOptions::new()`
  and the `with_*` builders instead of a struct literal.
- `OpenOptions` is `#[non_exhaustive]`: build it with `OpenOptions::new()`
  / `eager()` and the `with_*` builders instead of a struct literal.
- `RgbaImage` is `#[non_exhaustive]`: build it with the constructors
  (`RgbaImage::rgba(w, h, pixels)?`) instead of a struct literal.

### Deprecated

- `open_rgba`, `open_rgb`, `open_rgba_with`, `open_rgb_with` — thin
  flatteners of `open_image*`; use the native `Image` and convert with
  `to_rgba8` / `to_rgb8` / `RgbaImage::from_image` when bytes are needed.

## [0.1.1](https://github.com/OxideAV/oxideav-io/compare/v0.1.0...v0.1.1) - 2026-10-04

### Fixed

- PixelChoice::Auto now walks an encoder-capability candidate ladder; y4m derives its rawvideo payload codec

### Other

- a muxer that rejects the pixel layout steps the candidate ladder
- README examples use the current registry API
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
