//! Input/output addressing.
//!
//! [`Source`] is what the facade reads from. It deliberately offers
//! both the registry-driven path (`Source::Uri`, resolved through
//! `ctx.sources` — the `oxideav-source` [`SourceRegistry`]) and the
//! plain "custom" paths (`Path` / `Bytes` / `Reader`) so callers can
//! hand the facade a file, a URI, an in-memory buffer, or any seekable
//! stream they already hold.

use std::borrow::Cow;
use std::fs::File;
use std::path::Path;

use oxideav_core::{ReadSeek, RuntimeContext, SourceOutput};

use crate::error::{Error, Result};

/// Something the facade can open for reading.
pub enum Source<'a> {
    /// A local filesystem path, opened directly with [`std::fs::File`].
    Path(&'a Path),
    /// A URI resolved through the context's
    /// [`SourceRegistry`](oxideav_core::SourceRegistry)
    /// (`file://`, `mem://`, `data:`, `http(s)://`, … — whatever the
    /// caller registered). Only byte-shape sources are accepted here;
    /// packet/frame/multi-title sources are out of scope for the facade.
    Uri(&'a str),
    /// An in-memory byte buffer.
    Bytes(Cow<'a, [u8]>),
    /// A caller-supplied seekable reader (already open).
    Reader(Box<dyn ReadSeek>),
}

impl<'a> Source<'a> {
    /// Convenience: borrow a byte slice as a source.
    pub fn bytes(b: &'a [u8]) -> Self {
        Source::Bytes(Cow::Borrowed(b))
    }

    /// The lowercase extension hint (no leading dot) implied by this
    /// source's address, if any. Used to disambiguate container probes
    /// and to drive the PDF/3D/SVG discrimination ladder.
    pub(crate) fn ext_hint(&self) -> Option<String> {
        match self {
            Source::Path(p) => p
                .extension()
                .and_then(|e| e.to_str())
                .map(|s| s.to_ascii_lowercase()),
            Source::Uri(u) => ext_of_uri(u).map(|s| s.to_ascii_lowercase()),
            Source::Bytes(_) | Source::Reader(_) => None,
        }
    }

    /// Resolve into a seekable byte stream. `Uri` is dispatched through
    /// `ctx.sources`; the other shapes need no registry.
    pub(crate) fn into_read_seek(self, ctx: &RuntimeContext) -> Result<Box<dyn ReadSeek>> {
        match self {
            Source::Path(p) => {
                let f = File::open(p)
                    .map_err(|e| Error::Io(std::io::Error::new(e.kind(), format!("{p:?}: {e}"))))?;
                Ok(Box::new(f))
            }
            Source::Bytes(b) => Ok(Box::new(std::io::Cursor::new(b.into_owned()))),
            Source::Reader(r) => Ok(r),
            Source::Uri(u) => match ctx.sources.open(u)? {
                SourceOutput::Bytes(b) => Ok(Box::new(b)),
                _ => Err(Error::unsupported(format!(
                    "source '{u}' is not a byte-shape source (packet/frame/multi-title sources are not supported by oxideav-io)"
                ))),
            },
        }
    }
}

/// Extract the extension from a URI's path component (after stripping
/// any query string), mirroring the helper in `oxideav-cli-convert`.
fn ext_of_uri(uri: &str) -> Option<&str> {
    let last = uri.rsplit('/').next().unwrap_or(uri);
    let last = last.split('?').next().unwrap_or(last);
    let last = last.split('#').next().unwrap_or(last);
    let dot = last.rfind('.')?;
    let ext = &last[dot + 1..];
    if ext.is_empty() {
        None
    } else {
        Some(ext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ext_hint_from_path_and_uri() {
        assert_eq!(
            Source::Path(Path::new("/x/y/photo.PNG"))
                .ext_hint()
                .as_deref(),
            Some("png")
        );
        assert_eq!(
            Source::Uri("https://h/a/b/clip.MP4?token=1")
                .ext_hint()
                .as_deref(),
            Some("mp4")
        );
        assert_eq!(Source::Uri("file://noext").ext_hint(), None);
        assert_eq!(Source::bytes(b"x").ext_hint(), None);
    }

    #[test]
    fn ext_of_uri_strips_query_and_fragment() {
        assert_eq!(ext_of_uri("a/b.svg?v=2#frag"), Some("svg"));
        assert_eq!(ext_of_uri("noext"), None);
    }
}
