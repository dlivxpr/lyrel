//! Typed failures for opening and reading an EPUB.
//!
//! The GUI maps these to user-facing copy; the parser never emits a bare
//! string that claims to be a whole book. `NotEpub` is deliberately narrow:
//! it means "this ZIP has no EPUB container", so a generic archive can keep
//! the workbench's existing external-open behaviour. Every other failure is a
//! book-shaped answer and must be shown inside the book document.

use std::fmt;

/// One reason a book, its packaging, or one chapter could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EpubError {
    /// A ZIP archive without `META-INF/container.xml` (or a container that
    /// names no package). Used only to decline an unclaimed container.
    NotEpub,
    /// The archive or an XML document is not structurally valid.
    Corrupt { detail: String },
    /// Text or XML uses an encoding or declaration the reader cannot honour.
    Encoding { detail: String },
    /// A required archive member is absent.
    Missing { entry: String },
    /// A link or manifest path escapes the container.
    PathEscapes { href: String },
    /// The file declares DRM, rights management, or encrypted resources.
    Protected { detail: String },
    /// The document declares a fixed (pre-paginated) layout.
    FixedLayout { global: bool, items: usize },
    /// A byte, node, entry, chapter, or block budget was exceeded.
    Limit { resource: &'static str, limit: u64 },
    /// The caller cancelled the read.
    Cancelled,
    /// The underlying archive or file I/O failed.
    Io { detail: String },
}

impl EpubError {
    pub(crate) fn corrupt(detail: impl Into<String>) -> Self {
        Self::Corrupt {
            detail: detail.into(),
        }
    }

    pub(crate) fn encoding(detail: impl Into<String>) -> Self {
        Self::Encoding {
            detail: detail.into(),
        }
    }

    pub(crate) fn limit(resource: &'static str, limit: u64) -> Self {
        Self::Limit { resource, limit }
    }
}

impl fmt::Display for EpubError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotEpub => f.write_str("the archive is not an EPUB"),
            Self::Corrupt { detail } => write!(f, "corrupt EPUB: {detail}"),
            Self::Encoding { detail } => write!(f, "unsupported EPUB text encoding: {detail}"),
            Self::Missing { entry } => write!(f, "missing EPUB resource: {entry}"),
            Self::PathEscapes { href } => write!(f, "EPUB path escapes the container: {href}"),
            Self::Protected { detail } => write!(f, "protected EPUB: {detail}"),
            Self::FixedLayout { global, items } => {
                if *global {
                    f.write_str("EPUB declares a fixed (pre-paginated) layout for the whole book")
                } else {
                    write!(
                        f,
                        "EPUB declares a fixed (pre-paginated) layout for {items} spine item(s)"
                    )
                }
            }
            Self::Limit { resource, limit } => {
                write!(f, "EPUB {resource} exceeds the {limit} limit")
            }
            Self::Cancelled => f.write_str("EPUB read was cancelled"),
            Self::Io { detail } => write!(f, "EPUB I/O failed: {detail}"),
        }
    }
}

impl std::error::Error for EpubError {}

impl From<zip::result::ZipError> for EpubError {
    fn from(error: zip::result::ZipError) -> Self {
        match error {
            zip::result::ZipError::Io(inner) => Self::Io {
                detail: inner.to_string(),
            },
            zip::result::ZipError::FileNotFound => Self::Missing {
                entry: "<archive member>".into(),
            },
            zip::result::ZipError::UnsupportedArchive(message)
                if message.contains("Encrypted") || message.contains("encrypted") =>
            {
                Self::Protected {
                    detail: message.into(),
                }
            }
            other => Self::Corrupt {
                detail: other.to_string(),
            },
        }
    }
}

impl From<std::io::Error> for EpubError {
    fn from(error: std::io::Error) -> Self {
        Self::Io {
            detail: error.to_string(),
        }
    }
}

impl From<roxmltree::Error> for EpubError {
    fn from(error: roxmltree::Error) -> Self {
        match error {
            roxmltree::Error::NodesLimitReached => Self::Limit {
                resource: "XML nodes",
                limit: 0,
            },
            other => Self::Corrupt {
                detail: format!("invalid XML: {other}"),
            },
        }
    }
}
