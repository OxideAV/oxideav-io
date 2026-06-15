# oxideav-io

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
| `Image(RgbaImage)` | still images (PNG, JPEG, BMP, WebP, GIF, TIFF, QOI, …) | yes |
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
let rgba = oxideav_io::open_rgba("photo.jpg")?;   // packed RGBA8888
let rgb  = oxideav_io::open_rgb("photo.jpg")?;    // packed RGB24
let av   = oxideav_io::open_media("clip.mp4")?;   // always lazy
# Ok::<(), oxideav_io::Error>(())
```

Every opener has a `_with(ctx, source, opts)` sibling and takes an
[`OpenOptions`] that can **restrict which container / codec is allowed to
run** (`allow_containers` / `deny_containers` / `allow_codecs` /
`deny_codecs`) — handy for sandboxing untrusted input to a known-safe
format set.

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
(`Sink::Path` / `Sink::Writer(Box<dyn Write + Send>)` /
`Sink::Buffer(&mut Vec<u8>)`) and a `SaveOptions { container, codec,
pixel, quality }`. `PixelChoice::{Auto, Rgb, Rgba}` selects the packed
layout — `Auto` consults the codec's accepted-format set and prefers an
alpha-capable layout. The whole container is assembled in memory first,
so a seekable muxer works even over a non-seekable writer. 3D meshes
re-encode through the mesh registry by the sink's extension; PDF/document
`Scene` writing is out of scope for now.

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
`transforms` feature) and `Transform::Convert(PixelChoice)` (via
`oxideav-pixfmt`) cover the still-image path. The audio/video pipeline
path (built on `oxideav-pipeline`) is the next step — a/v inputs return
an `Unsupported` error today.

## Sources

`Source` accepts whatever you have:

* `Source::Path(p)` — a filesystem path, opened directly;
* `Source::Uri(u)` — resolved through the context's `SourceRegistry`
  (`file://`, `mem://`, `data:`, `http(s)://`, …);
* `Source::Bytes(b)` — an in-memory buffer;
* `Source::Reader(r)` — any `Read + Seek + Send` you already hold.

## Features

| Feature    | Default | Effect |
|------------|:-------:|--------|
| `full`     | ✅ | Zero-config `open(path)` — builds a `RuntimeContext` from `oxideav-meta` covering every codec/container/source. Turns on `pdf` + `mesh`. |
| `registry` | via `full` | Base layer. Caller supplies a populated `RuntimeContext` and uses the `*_with(ctx, …)` functions; no `oxideav-meta` dependency. |
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
still-image **transcode** path (Phase 3) are all in place. The remaining
follow-up is the audio/video transcode path on top of `oxideav-pipeline`
(decode → filter graph → encode → mux).

## License

MIT © Karpelès Lab Inc.

[OxideAV]: https://github.com/OxideAV
[`Opened`]: https://docs.rs/oxideav-io
[`OpenOptions`]: https://docs.rs/oxideav-io
