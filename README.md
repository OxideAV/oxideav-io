# oxideav-io

A generic Rust entry point for **opening** (and, soon, writing and
transcoding) media with [OxideAV]. Hand it a path — or a URI, a byte
buffer, or any seekable reader — and it auto-detects the format and
dispatches through the `oxideav-core` registries to give you back a
decoded value.

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
| `registry` |   | Base layer. Caller supplies a populated `RuntimeContext` and uses the `*_with(ctx, …)` functions. No `oxideav-meta` dependency — keeps meta a pure aggregator. |
| `pdf`      | via `full` | Eager PDF → `Scene` decode. |
| `mesh`     | via `full` | Eager 3D model → `Scene3D` decode. |

Building with `--no-default-features --features registry` gives a lean
facade that never pulls in `oxideav-meta`; the caller is responsible for
registering whatever codecs/containers it needs.

## Status

Phase 1 (this release): the **read** facade. Writing (`save`) and
transform-on-save / transcoding (`transcode`) are planned follow-ups.

## License

MIT © Karpelès Lab Inc.

[OxideAV]: https://github.com/OxideAV
[`Opened`]: https://docs.rs/oxideav-io
[`OpenOptions`]: https://docs.rs/oxideav-io
