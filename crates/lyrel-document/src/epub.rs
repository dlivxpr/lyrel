//! DRM-free reflowable EPUB reading, independent of GPUI and the Host.
//!
//! The parser is adapted from Glim's MIT `glim-services/src/books/mod.rs`
//! (pinned `fc2183f2c5f27ddfcdc4badbab6d83d50616831b`): ZIP container, OPF
//! manifest/spine, EPUB 3 nav, EPUB 2 NCX, archive-local path resolution,
//! chapter sanitization and illustration references. Lyrel keeps the reading
//! code GUI-free because the same book may be opened by a local or remote
//! Host; the caller supplies coherent bytes (a private temporary copy for a
//! remote file) and never a same-named local path. See `UPSTREAM-epub.toml`
//! beside this module.
//!
//! The full-book text index and matching ([`search`]) is a Lyrel addition
//! Glim's pinned reader does not have: one text index built from the
//! sanitized blocks of every spine chapter (never from rendered layout),
//! bounded by [`Limits::index_text_bytes`], with exact case-sensitive
//! substring matching over the same normalized text the reader displays.
//!
//! Cancellation is the crate-wide [`CancelToken`]: a reader thread checks it
//! between entries, XML parses and blocks; no blocking call is abandoned
//! mid-syscall.
//!
//! What is deliberately absent: FB2/CBZ, book editing, publisher CSS,
//! script execution, remote resource loading, DRM decryption, and
//! fixed-layout reading. Those are reported as unsupported, not approximated.

mod archive;
mod book;
mod decode;
mod error;
mod limits;
mod opf;
mod sanitize;
mod search;
mod state;

pub use archive::resolve;
pub use book::locate;
pub use book::{
    AnchorLocation, BLOCKED_IMAGE_SOURCE, Chapter, ChapterImage, EpubBook, ImageMode, Metadata,
    SpineItem, TocEntry,
};
pub use error::EpubError;
pub use limits::{Limits, MAX_QUERY_BYTES};
pub use sanitize::{Anchor, BlockFragment, Omissions, Part, html_with_marks};
pub use search::{BookText, SearchHit, SearchOutput, search};
pub use state::{
    Bookmark, RECORD_SCHEMA, ReaderPrefs, ReaderRecord, ReaderTheme, ReadingPosition,
    validate_record,
};

pub use crate::state::CancelToken;

use std::io::{Read, Seek};

/// Opens an archive and reads its book structure.
///
/// `source` is a coherent version of the file (the caller owns its lifetime).
/// `fingerprint` is the caller's SHA-256 identity for those bytes; it is kept
/// as a version guard for reading state, never as the document key.
pub fn open_book<R: Read + Seek>(
    source: R,
    fingerprint: String,
    fallback_title: &str,
    limits: &Limits,
    cancel: &CancelToken,
) -> Result<EpubBook, EpubError> {
    book::open(source, fingerprint, fallback_title, limits, cancel)
}

/// Reads one spine chapter from a fresh handle on the same coherent bytes.
///
/// The parent's chapter cache owns the result. `images` selects whether
/// local illustration bytes are read and returned; the full-book text index
/// passes [`ImageMode::Skip`] so indexing never decodes or retains pictures.
pub fn read_chapter<R: Read + Seek>(
    book: &EpubBook,
    source: R,
    index: usize,
    images: ImageMode,
    limits: &Limits,
    cancel: &CancelToken,
) -> Result<Chapter, EpubError> {
    book::read_chapter(book, source, index, images, limits, cancel)
}
