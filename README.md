# oxideav-io

[![CI](https://github.com/OxideAV/oxideav-io/actions/workflows/ci.yml/badge.svg)](https://github.com/OxideAV/oxideav-io/actions/workflows/ci.yml) [![crates.io](https://img.shields.io/crates/v/oxideav-io.svg)](https://crates.io/crates/oxideav-io) [![docs.rs](https://docs.rs/oxideav-io/badge.svg)](https://docs.rs/oxideav-io) [![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A generic Rust entry point for **opening**, **saving**, and
**transcoding** media with [OxideAV]. Hand it a path — or a URI, a byte
buffer, or any seekable reader — and it auto-detects the format and
dispatches through the `oxideav-core` registries to give you back a
decoded value, write one back out, or convert from one format to
another.

```rust
use oxideav_io::{open, Opened};

match open("input.png")? {
    Opened::Image(img)   => println!("still image {}×{}", img.width, img.height),
    Opened::Media(reader) => println!("a/v stream, {} stream(s)", reader.streams().len()),
    Opened::Vector(_)    => println!("vector graphic (SVG / vector PDF page)"),
    _                    => {}
}
# Ok::<(), oxideav_io::Error>(())
```

## What you get back

`open()` returns an [`Opened`] enum:

| Variant          | Produced for                              | Eager? |
|------------------|-------------------------------------------|--------|
| `Image(RgbaImage)` | still images (PNG, JPEG, BMP, HEIC, GIF, TIFF, …), packed RGBA8888 with the picture's metadata blobs and an indexed picture's palette alpha composited in — see [Images ride oxideav-image](#images-ride-oxideav-image) for the native picture | yes |
| `Vector(VectorFrame)` | SVG / vector graphics                  | yes |
| `Scene(Scene)`   | PDF documents (one entry per page) — `pdf` feature | yes |
| `Mesh(Scene3D)`  | 3D models (STL/OBJ/glTF/GLB/USDZ/FBX) — `mesh` feature | yes |
| `Media(MediaReader)` | audio & video                          | lazy |

Images, vector, PDF, and 3D decode immediately; audio/video stay lazy
behind a streaming `MediaReader` that yields decoded frames on demand.

## Unified vs specialized openers

Alongside the unified `open()`, there are specialized entry points that
decode straight to what you want:

```rust
let img  = oxideav_io::open_image("photo.jpg")?;      // native oxideav_image::Image
let rgba: Vec<u8> = img.to_rgba8()?;                   // packed bytes only when you need them
let file = oxideav_io::open_image_file("anim.gif")?;  // every frame + container + metadata
let av   = oxideav_io::open_media("clip.mp4")?;       // always lazy
# Ok::<(), oxideav_io::Error>(())
```

`open_rgba` / `open_rgb` (and their `_with` forms) still exist as
deprecated thin flatteners of `open_image`.

Every opener has a `_with(ctx, source, opts)` sibling and takes an
[`OpenOptions`] (built with `OpenOptions::new()` / `eager()` and
`with_*` builders — the struct is `#[non_exhaustive]`) that can
**restrict which container / codec is allowed to run**
(`with_allow_containers` / `with_deny_containers` / `with_allow_codecs`
/ `with_deny_codecs` — a refusal is `Error::Restricted`), **strip
metadata** (`without_metadata()`, `with_stripped_blobs([BlobKind::EXIF])`),
**cap the decode** (`with_max_pixels(n)` refuses an oversized stream
from its geometry before any decoder exists) and pass **decoder
options** (`with_decoder_option`) — handy for sandboxing untrusted
input to a known-safe format set.

```rust
use oxideav_core::BlobKind;
use oxideav_io::OpenOptions;
let opts = OpenOptions::eager()
    .with_allow_containers(["png", "jpeg", "heif"])
    .with_stripped_blobs([BlobKind::EXIF])   // shed location data
    .with_max_pixels(50_000_000);
```

## Images ride oxideav-image

Since round 473 every still-image decode, pixel-layout conversion,
encode and mux in this crate goes through the
[`oxideav-image`](https://crates.io/crates/oxideav-image) gateway
(Layer 2 of the workspace's `IMAGE_CRATE_API.md`). `oxideav-io` no
longer carries its own probe → demux → decode → pack pipeline for
stills, nor its own encoder / muxer ladder; what it adds on top of the
gateway is:

* **source / sink addressing** — paths, URIs through the
  `SourceRegistry`, byte buffers, readers and writers;
* the **discrimination ladder** — PDF → `Scene`, 3D → `Mesh`, vector →
  `Vector`, single-frame video → `Image`, everything else → lazy
  `Media`;
* the **allow / deny container + codec lists** of `OpenOptions`, handed
  to the gateway's own (`OpenOptions::to_image_options` →
  `oxideav_image::OpenOptions::with_allow_*` / `with_deny_*`): the
  container is checked right after the probe, every video stream's
  codec from the stream table before a decoder exists — a refused file
  costs one header parse and comes back as `Error::Restricted`. The
  unified `open` / `probe` / `ping_format` paths apply the same lists
  themselves (the fast path only the container lists: it never reads a
  stream table).

What you get:

| Entry point | Returns | Notes |
|---|---|---|
| `open_image(path)` / `open_image_with(ctx, src, opts)` | `oxideav_image::Image` | the primary picture in its native layout (`width()`, `height()`, `format()`, `palette()`, `color_signal()`, `to_rgba8()`, `to_rgb8()`, `to_format(..)`, `crop(..)`); only the first picture is decoded |
| `open_image_file(path)` / `open_image_file_with(..)` | `oxideav_image::ImageFile` | every picture in file order with per-frame `delay()` / `timestamp()`, plus `container()` and `metadata()` |
| `open(path)` → `Opened::Image(RgbaImage)` | packed RGBA8888 | unchanged shape; built from the native picture |
| `save_image_with(ctx, &Image, sink, &SaveOptions)` | — | encode a native picture without flattening it first; its blobs are written |
| `save_rgba_with(ctx, &RgbaImage, sink, &SaveOptions)` | — | the flattened-buffer save (`save_with` for `Opened::Image`); the buffer's `blobs` are written |
| `save_image(&Image, path)` / `save_rgba(&RgbaImage, path)` | — | zero-config forms, container from the extension |
| `RgbaImage::from_image` / `from_image_rgb` / `to_image` / `TryFrom` both ways | — | the bridge between the flattened buffer and the native picture; blobs lifted and re-attached |

`Image` and `ImageFile` are re-exported at the crate root, so callers
need no direct `oxideav-image` dependency to name them. Errors from the
gateway fold onto this crate's variants (`UnknownFormat` → `Probe` on
read, `Unsupported` on write; `Restricted` → `Restricted`; `InvalidData`
/ `NoImage` / `LimitExceeded` → `Invalid`; registry errors as before).

### Metadata and palette alpha

Opaque payloads a format embeds — an ICC profile, Exif, XMP, IPTC, a
custom kind — are framework `MetadataBlob`s (core 0.1.40) and reach
every io shape:

* the native `Image` keeps the gateway's two levels (`frame_blobs()` /
  `stream_blobs()`) and the resolved view (`blobs()`, `icc()`, `exif()`,
  `xmp()`, `iptc()`, `blob(&kind)`: the frame's blobs first, then the
  stream blobs of kinds the frame does not carry); `ImageFile` the
  stream's (`blobs()`, `stream_blobs(i)`, the same accessors);
* the flattened `RgbaImage` — hence `Opened::Image`, `open_rgba*` /
  `open_rgb*` and the transcode chain — carries `blobs: Vec<MetadataBlob>`
  with the same accessors plus `with_blob(s)` / `push_blob` /
  `retain_blobs` / `strip_metadata` / `without_metadata`.

**Ruling:** on the flattened shape the blobs live on `RgbaImage` itself,
not on the surrounding `Opened` — the buffer is the unit that is saved,
transformed and handed around, and `Opened::Image` has no second slot.
`RgbaImage::from_image` lifts the native picture's *resolved* list, so
flattening never loses a kind; `to_image` attaches them as the
picture's own, which a single-picture encode promotes to the stream
level (the gateway's rule) — so `save_with(&Opened::Image(img), ..)`
writes exactly what `open` read, unless `SaveOptions` strips or
replaces it.

An indexed (`Pal8`) picture's palette alpha (GIF transparent index, PNG
`tRNS`, TGA 32-bit colour maps) is composited into the RGBA flattening
— a transparent entry yields alpha `0` — and dropped by the RGB24 one
(`from_image_rgb`, `open_rgb*`), as the gateway's views do.

What persists on disk is the format crate's business, not this
crate's: the gateway's survey (its README, *Survey*) lists which of the
25 image crates push blobs and palette alpha through the registry. The
`tests/metadata_chain.rs` suite pins what io guarantees against the
real fleet — HEIF persists ICC / Exif / XMP today; a png → jpg → heic
chain through io never corrupts or duplicates a blob and forwards the
list to the next hop; `without_metadata()` drops them on open and on
save.

## Probing (identify without decoding)

When you only need to know *what* a file is — not its pixels or samples —
the facade offers two tiers, both running the same discrimination ladder
but stopping early; **neither decodes a frame**.

### `ping_format()` — fast path (format only)

The cheapest answer to "what format is this?". It runs the ladder and
stops the instant the format is known — it **does not open a demuxer**, so
it never reads the container's stream table.

```rust
use oxideav_io::{ping_format, MediaKind};

let p = ping_format("clip.mkv")?;
println!("{:?} / {:?}", p.kind, p.format); // e.g. Media / Some("matroska")
# Ok::<(), oxideav_io::Error>(())
```

```rust,ignore
pub struct PingFormat {
    pub kind: MediaKind,        // Scene | Mesh | Media
    pub format: Option<String>, // "pdf", "stl", "matroska", "png", …
}
```

The fast path's cheapness is a **hard, enforced contract**, not a
courtesy: `ping_format` never reads more than
`PING_FORMAT_MAX_READ_BYTES` (257 KiB — a 1 KiB magic peek plus the
registry's fixed 256 KiB probe window, both taken from the *start* of
the source). The source is wrapped in a metering reader that fails any
read past the budget, so a misbehaving probe can never turn ping into a
whole-file scan. A caller-supplied `Source::Reader` is probed from
byte 0 regardless of its current position and handed back at the
position it went in at.

### `probe()` — full probe (size / duration / streams)

When you want detail — overall byte size, duration, container metadata,
and a per-stream summary — call `probe()`. It opens the demuxer (a header
parse, not a decode) and reports what the container advertises.

```rust
use oxideav_io::{probe, MediaKind, StreamKind};

let info = probe("clip.mkv")?;
match info.kind {
    MediaKind::Scene => println!("a PDF document"),
    MediaKind::Mesh  => println!("a 3D model"),
    MediaKind::Media => {
        println!("container: {:?}", info.container);
        for s in &info.streams {
            match s.kind {
                StreamKind::Video => println!("#{} video {}: {:?}x{:?}",
                    s.index, s.codec, s.width, s.height),
                StreamKind::Audio => println!("#{} audio {}: {:?} Hz, {:?} ch",
                    s.index, s.codec, s.sample_rate, s.channels),
                _ => println!("#{} {:?} {}", s.index, s.kind, s.codec),
            }
        }
    }
}
# Ok::<(), oxideav_io::Error>(())
```

`probe()` returns a `Probe`:

```rust,ignore
pub struct Probe {
    pub kind: MediaKind,                 // Scene | Mesh | Media
    pub container: Option<String>,       // e.g. "png", "matroska"; None for PDF/3D
    pub byte_size: Option<u64>,          // total size, when cheaply measurable
    pub duration_secs: Option<f64>,      // container or longest-stream duration
    pub metadata: Vec<(String, String)>, // title, artist, … (container-level)
    pub streams: Vec<StreamInfo>,        // empty for PDF/3D
}
```

Each `StreamInfo` carries `index`, `kind` (`StreamKind::{Audio, Video,
Subtitle, Data, Unknown}`), the `codec` id, and whatever the container
advertises cheaply (`width` / `height` / `sample_rate` / `channels` /
`bit_rate` / `duration_secs`).

Typed accessors cover the common questions without manual stream
iteration:

```rust
let info = oxideav_io::probe("clip.mkv")?;
if info.has_video() {
    println!("size: {:?}", info.dimensions());          // first video stream's (w, h)
}
for a in info.audio_streams() {
    println!("audio #{}: {} {:?} Hz", a.index, a.codec, a.sample_rate);
}
println!("title: {:?}", info.meta("title"));            // case-insensitive tag lookup
println!("length: {:?}", info.duration());              // std::time::Duration
# Ok::<(), oxideav_io::Error>(())
```

(`video_streams()` / `subtitle_streams()`, `first_video()` /
`first_audio()`, `has_audio()`, and per-stream `dimensions()` /
`duration()` / `is_video()` / `is_audio()` / `is_subtitle()` round out
the set.)

Stream-level metadata blobs are in the header too: each `StreamInfo`
carries `blobs` (what the demuxer declared on the stream, after the
`OpenOptions` strip rules) with `icc()` / `exif()` / `xmp()` / `iptc()` /
`blob(&kind)`, and `Probe::blobs()` / `icc()` / … read the first video
stream's — so a profile or an XMP packet can be inspected without
decoding a pixel. Per-picture blobs need a decode (`open_image*`).

The discrimination ladder mirrors `open()`:

1. PDF magic (`%PDF-`) or `.pdf` extension → `MediaKind::Scene`;
2. a 3D extension (`stl`/`obj`/`gltf`/`glb`/`usdz`/`fbx`) → `MediaKind::Mesh`;
3. otherwise the container registry's probe → `MediaKind::Media`, with the
   detected container and its stream table.

Because telling a single-frame still image apart from a 1-frame video
would require a decode, **still images report as `MediaKind::Media`**
(with one video-kind stream), not as a distinct image kind — that
distinction is made by `open()` *after* decoding.

Container names are the registry's, the same ones the gateway's
`ImageFile::container()` reports (`tests/image_containers.rs` pins
`ping_format` / `probe` / gateway agreement over all 25 image crates):
a JPEG is `jpeg`, a PPM `pbm`, an OpenEXR `openexr`, a JPEG 2000
codestream `jpeg2000` and its box file `jp2`, and **every HEIF-family
brand — `.heic`, `.heif`, `.avif` — is container `heif` with stream
codec `heif`**; the brand is container metadata (`probe.meta("major_brand")`),
so the allow / deny lists cannot single a brand out.

Both tiers use the process-wide `oxideav-meta` context (the `full`
feature). The lean forms `ping_format_with(ctx, source, &OpenOptions)` and
`probe_with(ctx, source, &OpenOptions)` take a caller-supplied
`RuntimeContext` and honour the same `allow_*`/`deny_*` lists as the
openers, so a denied container/codec is rejected before any header is
fully parsed.

## Saving

`save()` is the write-side mirror of `open()`. It re-encodes an `Opened`
value through the codec + container registries, picking the container and
codec from the destination extension (or from `SaveOptions`):

```rust
use oxideav_io::{open, save, Opened};

let opened = open("photo.png")?;
save(&opened, "photo.jpg")?;      // re-encode PNG → JPEG by extension
# Ok::<(), oxideav_io::Error>(())
```

`save_with(ctx, &opened, sink, &SaveOptions)` takes a `Sink`
(`Sink::Path` / `Sink::Writer(Box<dyn Write + Send>)` — or
`Sink::writer(w)` — / `Sink::Buffer(&mut Vec<u8>)`) and a `SaveOptions`
(`SaveOptions::new()` and `with_*` builders; `#[non_exhaustive]`)
mapped onto `oxideav_image::encode`:

```rust
use oxideav_core::{BlobKind, MetadataBlob};
use oxideav_io::{PixelChoice, SaveOptions};
# let profile = vec![0u8; 4];
let opts = SaveOptions::new()
    .with_container("heic")
    .with_quality(85)
    .with_blob(MetadataBlob::new(BlobKind::ICC, profile)) // replaces the source's ICC
    .with_stripped_blobs([BlobKind::EXIF])                 // withhold the source's Exif
    .with_pixel(PixelChoice::Auto);
```

* `container` is a registered container name **or a file extension**
  (`"png"`, `"jpeg"`, `"jpg"`, `"heic"`, …); `None` derives it from the
  sink's extension. The payload codec is the container's default as the
  gateway resolves it (`jpeg` → `mjpeg`, `y4m` → `rawvideo`, prefixed
  encoder families such as `webp_vp8l`, …) unless `codec` forces one.
* `PixelChoice::Auto` is the gateway's pixel-format ladder — the image's
  own layout first, then `Rgba`, `Rgb24`, `Gray8`, `Yuv444P`,
  `Yuv420P`, `Rgba64Le` — stepping whenever the encoder **or** the muxer
  refuses a layout; `Rgb` / `Rgba` replace the ladder by that single
  layout and fail loudly if it is refused.
* `quality` reaches the encoder only when its declared option schema
  has a `quality` field (lossless encoders would reject the unknown
  option); `with_option(name, value)` forwards any other declared
  encoder option verbatim (an undeclared name is the encoder's refusal,
  nothing is committed).
* the picture's blobs are handed to the encoder at both levels;
  `with_blob(b)` writes `b` and replaces the source blob of its kind —
  at both levels, so the replacement holds whichever level the encoder
  reads — `without_metadata()` / `with_stripped_blobs(kinds)` withhold
  the source's (blobs given in the options are still written).

The whole container is assembled in memory first, so a seekable muxer
works even over a non-seekable writer. 3D meshes re-encode through the
mesh registry by the sink's extension; PDF/document `Scene` writing is
out of scope for now.

## Transcoding

`transcode()` chains decode → optional still-image transforms → encode
→ mux:

```rust
use oxideav_io::{transcode_with, Source, Sink, TranscodeOptions, Transform};
# let ctx = oxideav_core::RuntimeContext::new();

let opts = TranscodeOptions {
    transforms: vec![Transform::Resize { width: 320, height: 240 }],
    ..Default::default()
};
let mut out = Vec::new();
transcode_with(&ctx, Source::Path("in.png".as_ref()), Sink::Buffer(&mut out), &opts)?;
# Ok::<(), oxideav_io::Error>(())
```

`Transform::Resize` (via `oxideav-image-filter`, behind the default-on
`transforms` feature) and `Transform::Convert(PixelChoice)` (via the
gateway's `Image::to_format`) cover the still-image path. The audio/video pipeline
path (built on `oxideav-pipeline`) is the next step — a/v inputs return
an `Unsupported` error today.

## Sources

`Source` accepts whatever you have:

* `Source::Path(p)` — a filesystem path, opened behind a `BufReader`;
* `Source::Uri(u)` — resolved through the context's `SourceRegistry`
  (`file://`, `mem://`, `data:`, `http(s)://`, …);
* `Source::Bytes(b)` — an in-memory buffer (`Source::bytes(&[u8])`);
* `Source::Reader(r)` — any `Read + Seek + Send` you already hold, as
  is; `Source::reader(r)` boxes one, `Source::buffered(r)` wraps it in a
  `BufReader` first (the right shape for an unbuffered file or
  socket-backed seekable stream).

### Seekability contract

Every source shape resolves to a **seekable** byte stream — that is the
facade's I/O contract, and it is what format detection is built on:

* probing always inspects the **start** of the stream: the facade seeks
  to byte 0, reads its window, and **restores the cursor** to wherever
  it was when the source was handed in;
* `probe()` measures `byte_size` by seeking to the end (and seeking
  back), and its demuxer header parse may seek anywhere the container
  keeps its stream table;
* only `SourceRegistry` outputs that are byte-shaped (seekable) are
  accepted through `Source::Uri`; packet/frame/multi-title sources are
  rejected with a typed `Unsupported` error.

A truly non-seekable stream (a socket, a pipe) must be buffered by the
caller first — `Source::Bytes` on the collected bytes is the supported
path. In exchange, every guarantee above holds for *any* reader you
hand in, parked at *any* position.

## Features

| Feature    | Default | Effect |
|------------|:-------:|--------|
| `full`     | ✅ | Zero-config `open(path)` — builds a `RuntimeContext` from `oxideav-meta` covering every codec/container/source. Turns on `pdf` + `mesh`. |
| `registry` | via `full` | Base layer. Caller supplies a populated `RuntimeContext` and uses the `*_with(ctx, …)` functions; pulls `oxideav-core` + `oxideav-image`, no `oxideav-meta` dependency. |
| `pdf`      | via `full` | Eager PDF → `Scene` decode. |
| `mesh`     | via `full` | Eager 3D model → `Scene3D` decode + 3D save. |
| `transforms` | via `full` | `Transform::Resize` for transcode (pulls `oxideav-image-filter`). |

The default `full` feature is batteries-included. For a lean build with
no `oxideav-meta` dependency — caller registers whatever
codecs/containers it needs, or reuses a context it already has — drop to
`registry`:

```toml
oxideav-io = { version = "0.0", default-features = false, features = ["registry"] }
```

`full` pulls `oxideav-meta`, whose transitive codec fleet must be
published to crates.io at compatible versions for a standalone build to
resolve; inside the workspace it always resolves via `[patch.crates-io]`.

## Status

The **read** facade (Phase 1), the **write** facade (Phase 2), and the
still-image **transcode** path (Phase 3) are all in place, with the
still-image half riding the `oxideav-image` gateway since round 473 and
adopting its whole 0.0.4 surface since round 477 (allow / deny lists on
the gateway, metadata blobs and palette alpha on every shape, strip /
replace rules on open and save, the pixel budget, buffered sources).
The lean `registry` slice — what the crate's own CI builds — drives the
whole pipeline through an in-crate test container + codec, so every
facade behaviour above is covered without the sibling fleet. The
**probe** subsystem is contract-hardened: an enforced `ping_format`
read budget, a fixture-backed coverage matrix (PNG / JPEG / PBM / DDS /
WAV / AVI / Matroska / MP3 / SRT / WebVTT / Y4M / SVG / PDF / STL —
both tiers pinned to agree), and a misdetection-hardening suite
(truncation sweeps, ambiguous magics, garbage fuzz — typed errors,
never panics). The remaining follow-up is the audio/video transcode
path on top of `oxideav-pipeline` (decode → filter graph → encode →
mux).

## License

MIT © Karpelès Lab Inc.

[OxideAV]: https://github.com/OxideAV
[`Opened`]: https://docs.rs/oxideav-io
[`OpenOptions`]: https://docs.rs/oxideav-io
