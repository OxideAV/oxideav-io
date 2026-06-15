//! `oxideav-io` — a generic Rust entry point for opening media.
//!
//! Hand it a [`Source`] (a path, a URI resolved through the context's
//! [`SourceRegistry`](oxideav_core::SourceRegistry), an in-memory
//! buffer, or any seekable reader) and it auto-detects the format and
//! dispatches through the [`oxideav-core`] registries to return a
//! decoded [`Opened`] value:
//!
//! * still images decode eagerly to a packed [`RgbaImage`];
//! * SVG / vector inputs yield a single `VectorFrame`;
//! * audio & video stay lazy behind a streaming [`MediaReader`].
//!
//! Both **unified** and **specialized** entry points are provided, and
//! every opener takes an [`OpenOptions`] that can restrict which
//! container / codec is allowed to run.
//!
//! ```no_run
//! use oxideav_io::{open_with, open_rgba_with, OpenOptions, Opened, Source};
//! # fn demo(ctx: &oxideav_core::RuntimeContext) -> oxideav_io::Result<()> {
//! match open_with(ctx, Source::Path("photo.png".as_ref()), &OpenOptions::eager())? {
//!     Opened::Image(img) => println!("{}x{}", img.width, img.height),
//!     Opened::Media(_reader) => println!("an a/v stream"),
//!     _ => {}
//! }
//!
//! // Specialized: decode straight to packed pixels.
//! let rgba = open_rgba_with(ctx, Source::Path("photo.png".as_ref()), &OpenOptions::default())?;
//! assert_eq!(rgba.stride, rgba.width as usize * 4);
//! # Ok(()) }
//! ```
//!
//! ## Context
//!
//! Every entry point takes a caller-supplied
//! [`oxideav_core::RuntimeContext`] and uses the `*_with(ctx, …)`
//! functions. The caller registers whatever codecs / containers it needs
//! (or reuses a context it already has). A meta-backed zero-config
//! `open(path)` that auto-registers every codec lives in the umbrella —
//! it cannot ship in this standalone crate because `oxideav-meta`'s full
//! fleet only resolves inside the workspace.

#![forbid(unsafe_code)]

#[cfg(not(feature = "registry"))]
compile_error!("oxideav-io requires the `registry` feature (enabled by default)");

#[cfg(feature = "registry")]
mod error;
#[cfg(feature = "registry")]
mod image;
#[cfg(feature = "registry")]
mod open;
#[cfg(feature = "registry")]
mod probe;
#[cfg(feature = "registry")]
mod source;

#[cfg(feature = "registry")]
pub use error::{Error, Result};
#[cfg(feature = "registry")]
pub use image::RgbaImage;
#[cfg(feature = "registry")]
pub use open::{
    open_media_with, open_rgb_with, open_rgba_with, open_with, DecodedFrame, MediaReader,
    OpenOptions, Opened,
};
#[cfg(feature = "registry")]
pub use probe::MediaKind;
#[cfg(feature = "registry")]
pub use source::Source;
