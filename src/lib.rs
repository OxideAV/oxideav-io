//! `oxideav-io` — a generic Rust entry point for opening media.
//!
//! Hand it a path (or a URI, bytes, or a reader) and it auto-detects the
//! format and dispatches through the [`oxideav-core`] registries to
//! return a decoded [`Opened`] value:
//!
//! * still images decode eagerly to a packed [`RgbaImage`];
//! * PDF documents decode to an `oxideav_scene::Scene` (one page each);
//! * 3D models decode to an `oxideav_mesh3d::Scene3D`;
//! * SVG / vector inputs yield a single `VectorFrame`;
//! * audio & video stay lazy behind a streaming [`MediaReader`].
//!
//! There are both **unified** and **specialized** entry points, and
//! every opener takes an [`OpenOptions`] that can restrict which
//! container / codec is allowed to run.
//!
//! ```no_run
//! # #[cfg(feature = "full")] {
//! use oxideav_io::{open, Opened};
//! match open("photo.png").unwrap() {
//!     Opened::Image(img) => println!("{}x{}", img.width, img.height),
//!     Opened::Media(_reader) => println!("an a/v stream"),
//!     _ => {}
//! }
//!
//! // Specialized: decode straight to packed pixels.
//! let rgba = oxideav_io::open_rgba("photo.png").unwrap();
//! assert_eq!(rgba.stride, rgba.width as usize * 4);
//! # }
//! ```
//!
//! ## Features
//!
//! The default **`full`** feature is batteries-included: the no-context
//! `open(path)` helpers build a [`RuntimeContext`](oxideav_core::RuntimeContext)
//! from `oxideav-meta` covering every codec / container the workspace
//! knows about, plus the eager PDF / 3D decode paths.
//!
//! For a lean build with no `oxideav-meta` dependency, drop to the
//! `registry` layer — the caller then supplies a populated
//! `RuntimeContext` and uses the `*_with(ctx, …)` functions:
//!
//! ```toml
//! oxideav-io = { version = "0.0", default-features = false, features = ["registry"] }
//! ```

#![forbid(unsafe_code)]

#[cfg(not(feature = "registry"))]
compile_error!(
    "oxideav-io requires at least the `registry` feature \
     (enabled by the default `full` feature)"
);

#[cfg(feature = "registry")]
mod error;
#[cfg(feature = "registry")]
mod image;
#[cfg(feature = "registry")]
mod open;
#[cfg(feature = "registry")]
mod probe;
#[cfg(feature = "registry")]
mod save;
#[cfg(feature = "registry")]
mod source;
#[cfg(feature = "registry")]
mod transcode;

#[cfg(feature = "registry")]
pub use error::{Error, Result};
#[cfg(feature = "registry")]
pub use image::RgbaImage;
#[cfg(feature = "registry")]
pub use open::{
    open_media_with, open_rgb_with, open_rgba_with, open_with, ping_format_with, probe_with,
    DecodedFrame, MediaReader, OpenOptions, Opened,
};
#[cfg(feature = "registry")]
pub use probe::{MediaKind, PingFormat, Probe, StreamInfo, StreamKind, PING_FORMAT_MAX_READ_BYTES};
#[cfg(feature = "registry")]
pub use save::{save_with, PixelChoice, SaveOptions};
#[cfg(feature = "registry")]
pub use source::{Sink, Source};
#[cfg(feature = "registry")]
pub use transcode::{transcode_with, TranscodeOptions, Transform};

#[cfg(all(feature = "registry", feature = "mesh"))]
pub use open::open_mesh_with;
#[cfg(all(feature = "registry", feature = "pdf"))]
pub use open::open_scene_with;

// ───────────────────────── zero-config (`full`) ─────────────────────────

/// The lazily-built, process-wide context used by the no-argument
/// `open()` / `open_*()` helpers. Populated once from
/// `oxideav_meta::register_all` (which wires every codec, container, and
/// source driver the workspace knows about).
#[cfg(feature = "full")]
fn default_context() -> &'static oxideav_core::RuntimeContext {
    use std::sync::OnceLock;
    static CTX: OnceLock<oxideav_core::RuntimeContext> = OnceLock::new();
    CTX.get_or_init(|| {
        let mut ctx = oxideav_core::RuntimeContext::new();
        oxideav_meta::register_all(&mut ctx);
        ctx
    })
}

/// Open a file by path, auto-detecting its format. Still images come
/// back as [`Opened::Image`]; audio/video as [`Opened::Media`].
///
/// Uses a process-wide context built from `oxideav-meta`. For a
/// caller-controlled context use [`open_with`].
#[cfg(feature = "full")]
pub fn open(path: impl AsRef<std::path::Path>) -> Result<Opened> {
    open_with(
        default_context(),
        Source::Path(path.as_ref()),
        &OpenOptions::eager(),
    )
}

/// **Fast path** — identify a file's [`MediaKind`] and container/format
/// name as cheaply as possible, without opening a demuxer or reading the
/// container's stream table.
///
/// Use this when all you need is "what format is this file?"; reach for
/// [`probe`] when you want stream / size / duration detail. Uses a
/// process-wide context built from `oxideav-meta`. For a caller-controlled
/// context use [`ping_format_with`].
///
/// ```no_run
/// # #[cfg(feature = "full")] {
/// use oxideav_io::{ping_format, MediaKind};
/// let p = oxideav_io::ping_format("clip.mkv").unwrap();
/// println!("{:?} / {:?}", p.kind, p.format);
/// # }
/// ```
#[cfg(feature = "full")]
pub fn ping_format(path: impl AsRef<std::path::Path>) -> Result<PingFormat> {
    ping_format_with(
        default_context(),
        Source::Path(path.as_ref()),
        &OpenOptions::default(),
    )
}

/// **Full probe** — identify a file's [`MediaKind`], container, overall
/// size / duration / metadata, and stream summary **without a full
/// decode** — the read-only inspection counterpart to [`open`].
///
/// Reads only headers (and, for the registry path, the container's
/// stream table, duration, and metadata); no frames are decoded. For a
/// faster "format only" answer use [`ping_format`]. Uses a process-wide
/// context built from `oxideav-meta`. For a caller-controlled context use
/// [`probe_with`].
///
/// ```no_run
/// # #[cfg(feature = "full")] {
/// use oxideav_io::{probe, MediaKind};
/// let info = oxideav_io::probe("clip.mkv").unwrap();
/// assert_eq!(info.kind, MediaKind::Media);
/// println!("{} bytes, {:?} s", info.byte_size.unwrap_or(0), info.duration_secs);
/// for s in &info.streams {
///     println!("#{} {:?} {}", s.index, s.kind, s.codec);
/// }
/// # }
/// ```
#[cfg(feature = "full")]
pub fn probe(path: impl AsRef<std::path::Path>) -> Result<Probe> {
    probe_with(
        default_context(),
        Source::Path(path.as_ref()),
        &OpenOptions::default(),
    )
}

/// Open a file and decode its first frame to packed RGBA8888.
#[cfg(feature = "full")]
pub fn open_rgba(path: impl AsRef<std::path::Path>) -> Result<RgbaImage> {
    open_rgba_with(
        default_context(),
        Source::Path(path.as_ref()),
        &OpenOptions::default(),
    )
}

/// Open a file and decode its first frame to packed RGB24.
#[cfg(feature = "full")]
pub fn open_rgb(path: impl AsRef<std::path::Path>) -> Result<RgbaImage> {
    open_rgb_with(
        default_context(),
        Source::Path(path.as_ref()),
        &OpenOptions::default(),
    )
}

/// Open a file as a lazy [`MediaReader`], regardless of frame count.
#[cfg(feature = "full")]
pub fn open_media(path: impl AsRef<std::path::Path>) -> Result<MediaReader> {
    open_media_with(
        default_context(),
        Source::Path(path.as_ref()),
        &OpenOptions::default(),
    )
}

/// Save an opened value to a file by path, choosing the container +
/// codec from the path's extension.
///
/// Uses a process-wide context built from `oxideav-meta`. For a
/// caller-controlled context use [`save_with`].
#[cfg(feature = "full")]
pub fn save(opened: &Opened, path: impl AsRef<std::path::Path>) -> Result<()> {
    save_with(
        default_context(),
        opened,
        Sink::Path(path.as_ref()),
        &SaveOptions::default(),
    )
}

/// Transcode a file from `src_path` to `dst_path`, auto-detecting the
/// input format and choosing the output container/codec from the
/// destination extension. Still-image inputs only for now.
///
/// Uses a process-wide context built from `oxideav-meta`. For a
/// caller-controlled context (and transform chain) use [`transcode_with`].
#[cfg(feature = "full")]
pub fn transcode(
    src_path: impl AsRef<std::path::Path>,
    dst_path: impl AsRef<std::path::Path>,
) -> Result<()> {
    transcode_with(
        default_context(),
        Source::Path(src_path.as_ref()),
        Sink::Path(dst_path.as_ref()),
        &TranscodeOptions::default(),
    )
}

/// Open a PDF file as an `oxideav_scene::Scene` (one entry per page).
#[cfg(all(feature = "full", feature = "pdf"))]
pub fn open_scene(path: impl AsRef<std::path::Path>) -> Result<oxideav_scene::Scene> {
    open_scene_with(
        default_context(),
        Source::Path(path.as_ref()),
        &OpenOptions::default(),
    )
}

/// Open a 3D model file as an `oxideav_mesh3d::Scene3D`.
#[cfg(all(feature = "full", feature = "mesh"))]
pub fn open_mesh(path: impl AsRef<std::path::Path>) -> Result<oxideav_mesh3d::Scene3D> {
    open_mesh_with(
        default_context(),
        Source::Path(path.as_ref()),
        &OpenOptions::default(),
    )
}
