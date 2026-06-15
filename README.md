# oxideav-io

A generic Rust entry point for **opening** (and, soon, writing and
transcoding) media with [OxideAV]. Hand it a path — or a URI, a byte
buffer, or any seekable reader — plus a populated
`oxideav_core::RuntimeContext`, and it auto-detects the format and
dispatches through the `oxideav-core` registries to give you back a
decoded value.

```rust
use oxideav_io::{open_with, OpenOptions, Opened, Source};

# fn demo(ctx: &oxideav_core::RuntimeContext) -> oxideav_io::Result<()> {
match open_with(ctx, Source::Path("input.png".as_ref()), &OpenOptions::eager())? {
    Opened::Image(img)    => println!("still image {}×{}", img.width, img.height),
    Opened::Media(reader) => println!("a/v stream, {} stream(s)", reader.streams().len()),
    Opened::Vector(_)     => println!("vector graphic (SVG / vector page)"),
    _                     => {}
}
# Ok(()) }
```

## What you get back

`open_with()` returns an [`Opened`] enum:

| Variant               | Produced for                                          | Eager? |
|-----------------------|-------------------------------------------------------|--------|
| `Image(RgbaImage)`    | still images (PNG, JPEG, BMP, WebP, GIF, TIFF, QOI, …) | yes |
| `Vector(VectorFrame)` | SVG / vector graphics                                 | yes |
| `Media(MediaReader)`  | audio & video                                         | lazy |

Still images decode immediately; audio/video stay lazy behind a
streaming `MediaReader` that yields decoded frames on demand. (PDF → a
multi-page `Scene` and 3D models → `Scene3D` are surfaced through the
umbrella build, where their decoder crates resolve — see *Context*.)

## Unified vs specialized openers

Alongside the unified `open_with()`, there are specialized entry points
that decode straight to what you want:

```rust
# fn demo(ctx: &oxideav_core::RuntimeContext) -> oxideav_io::Result<()> {
use oxideav_io::{open_rgba_with, open_rgb_with, open_media_with, OpenOptions, Source};
let rgba = open_rgba_with(ctx, Source::Path("photo.jpg".as_ref()), &OpenOptions::default())?; // RGBA8888
let rgb  = open_rgb_with(ctx,  Source::Path("photo.jpg".as_ref()), &OpenOptions::default())?; // RGB24
let av   = open_media_with(ctx, Source::Path("clip.mp4".as_ref()), &OpenOptions::default())?; // lazy
# Ok(()) }
```

Every opener takes an [`OpenOptions`] that can **restrict which container
/ codec is allowed to run** (`allow_containers` / `deny_containers` /
`allow_codecs` / `deny_codecs`) — handy for sandboxing untrusted input to
a known-safe format set.

## Sources

`Source` accepts whatever you have:

* `Source::Path(p)` — a filesystem path, opened directly;
* `Source::Uri(u)` — resolved through the context's `SourceRegistry`
  (`file://`, `mem://`, `data:`, `http(s)://`, …);
* `Source::Bytes(b)` — an in-memory buffer;
* `Source::Reader(r)` — any `Read + Seek + Send` you already hold.

## Context

Every entry point takes a caller-supplied `oxideav_core::RuntimeContext`
and uses the `*_with(ctx, …)` functions. The caller registers whatever
codecs / containers it needs, or reuses a context it already has.

A meta-backed zero-config `open(path)` that auto-registers every codec is
provided by the **umbrella** rather than this standalone crate:
`oxideav-meta`'s full codec fleet only resolves inside the workspace (via
`[patch.crates-io]`), so depending on it here — under any feature — would
make the crate fail the standard `--all-features` crate CI and block it
from crates.io. The same constraint is why the eager PDF / 3D decode
paths live in the umbrella build for now.

## Status

Phase 1 (this release): the **read** facade. Writing (`save`) and
transform-on-save / transcoding (`transcode`) are planned follow-ups,
along with the umbrella-side zero-config `open(path)` and eager PDF / 3D
paths.

## License

MIT © Karpelès Lab Inc.

[OxideAV]: https://github.com/OxideAV
[`Opened`]: https://docs.rs/oxideav-io
[`OpenOptions`]: https://docs.rs/oxideav-io
