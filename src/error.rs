//! Error and result types for the facade.
//!
//! The facade wraps the underlying `oxideav-core` error plus a handful
//! of facade-specific conditions (unsupported kind, probe failure, a
//! codec/container excluded by the caller's allow/deny lists).

use std::fmt;

/// Result alias used throughout `oxideav-io`.
pub type Result<T> = std::result::Result<T, Error>;

/// What went wrong while opening, saving, or transcoding.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// An I/O failure reading the source or writing the sink.
    Io(std::io::Error),
    /// The format / kind is recognised but not handled by this build
    /// (e.g. a 3D model opened in a build without the `mesh` feature).
    Unsupported(String),
    /// Format auto-detection failed — no registered container probe
    /// matched and no usable extension hint was available.
    Probe(String),
    /// The resolved codec or container was excluded by the caller's
    /// `allow_*` / `deny_*` lists in [`crate::OpenOptions`] /
    /// [`crate::SaveOptions`].
    Restricted(String),
    /// Decoding / demuxing failed below the facade.
    Decode(String),
    /// A facade-level invariant was violated (bad argument, empty
    /// stream, missing dimensions, …).
    Invalid(String),
}

impl Error {
    pub(crate) fn unsupported(msg: impl Into<String>) -> Self {
        Error::Unsupported(msg.into())
    }
    pub(crate) fn probe(msg: impl Into<String>) -> Self {
        Error::Probe(msg.into())
    }
    pub(crate) fn restricted(msg: impl Into<String>) -> Self {
        Error::Restricted(msg.into())
    }
    pub(crate) fn invalid(msg: impl Into<String>) -> Self {
        Error::Invalid(msg.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "io error: {e}"),
            Error::Unsupported(m) => write!(f, "unsupported: {m}"),
            Error::Probe(m) => write!(f, "format detection failed: {m}"),
            Error::Restricted(m) => write!(f, "restricted by options: {m}"),
            Error::Decode(m) => write!(f, "decode error: {m}"),
            Error::Invalid(m) => write!(f, "invalid: {m}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

#[cfg(feature = "registry")]
impl From<oxideav_core::Error> for Error {
    fn from(e: oxideav_core::Error) -> Self {
        // Preserve the variant intent where it maps cleanly; otherwise
        // fold into Decode with the rendered message.
        use oxideav_core::Error as Ce;
        match e {
            Ce::Unsupported(m) => Error::Unsupported(m),
            Ce::FormatNotFound(m) => Error::Probe(format!("no demuxer for format '{m}'")),
            other => Error::Decode(other.to_string()),
        }
    }
}
