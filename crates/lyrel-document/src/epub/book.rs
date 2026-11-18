//! EPUB structure and chapter loading.
//!
//! Retained from Glim: container → OPF → manifest → spine → nav/NCX, chapter
//! decoding/sanitization, local illustration resolution and the title
//! backfill. Adapted: a caller-supplied SHA-256 fingerprint (the Host read is
//! the version authority), explicit budgets, typed unsupported answers, and
//! block fragments instead of one chapter HTML string.

use std::collections::HashSet;
use std::io::{Read, Seek};

use super::archive::{self, Archive};
use super::decode::{decode, xml};
use super::limits::Limits;
use super::opf;
use super::sanitize::{BlockFragment, Omissions, sanitize_chapter};
use super::{CancelToken, EpubError};

/// Bibliographic metadata. Never used as document identity: a Host-scoped
/// canonical path is the identity, and this metadata can repeat across books.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Metadata {
    pub title: String,
    pub authors: Vec<String>,
    pub language: Option<String>,
}

impl Metadata {
    pub fn author_line(&self) -> String {
        self.authors.join(", ")
    }
}

/// One spine entry in reading order.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SpineItem {
    pub href: String,
    pub title: String,
}

impl SpineItem {
    pub fn new(href: String, title: String) -> Self {
        Self { href, title }
    }
}

/// One contents entry. `depth` is zero-based nesting.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct TocEntry {
    pub title: String,
    pub href: String,
    pub depth: usize,
}

impl TocEntry {
    pub fn new(title: String, href: String, depth: usize) -> Self {
        Self { title, href, depth }
    }
}

/// One local illustration read from the archive.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ChapterImage {
    /// The exact `src` written into the sanitized HTML.
    pub source: String,
    /// Archive member path without a fragment.
    pub path: String,
    /// Encoded bytes; decoding belongs to the GUI's image budget.
    pub bytes: Vec<u8>,
}

impl ChapterImage {
    pub fn new(source: String, path: String, bytes: Vec<u8>) -> Self {
        Self {
            source,
            path,
            bytes,
        }
    }
}

/// A named location mapped to a sanitized block.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct AnchorLocation {
    pub id: String,
    pub block: usize,
    pub text_offset: usize,
}

impl AnchorLocation {
    pub fn new(id: String, block: usize, text_offset: usize) -> Self {
        Self {
            id,
            block,
            text_offset,
        }
    }
}

/// Whether chapter loading reads illustration bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageMode {
    Include,
    Skip,
}

/// A parsed, reflowable EPUB.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct EpubBook {
    pub metadata: Metadata,
    pub spine: Vec<SpineItem>,
    pub toc: Vec<TocEntry>,
    /// SHA-256 of the coherent bytes this structure came from.
    pub fingerprint: String,
}

impl EpubBook {
    pub fn new(
        metadata: Metadata,
        spine: Vec<SpineItem>,
        toc: Vec<TocEntry>,
        fingerprint: String,
    ) -> Self {
        Self {
            metadata,
            spine,
            toc,
            fingerprint,
        }
    }

    pub fn title(&self) -> &str {
        &self.metadata.title
    }

    pub fn chapter_count(&self) -> usize {
        self.spine.len()
    }

    pub fn chapter_title(&self, index: usize) -> Option<&str> {
        self.spine.get(index).map(|item| item.title.as_str())
    }

    pub fn chapter_href(&self, index: usize) -> Option<&str> {
        self.spine.get(index).map(|item| item.href.as_str())
    }
}

/// One sanitized chapter.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct Chapter {
    pub index: usize,
    pub href: String,
    pub title: String,
    pub blocks: Vec<BlockFragment>,
    pub images: Vec<ChapterImage>,
    pub anchors: Vec<AnchorLocation>,
    pub omissions: Omissions,
}

/// Opens the package and navigation, refusing unsupported books before a
/// chapter list can be shown.
pub(crate) fn open<R: Read + Seek>(
    source: R,
    fingerprint: String,
    fallback_title: &str,
    limits: &Limits,
    cancel: &CancelToken,
) -> Result<EpubBook, EpubError> {
    super::check_cancel(cancel)?;
    let mut archive = Archive::open(source, limits)?;
    let root = opf::rootfile(&mut archive, limits, cancel)?;
    let package = opf::package(&mut archive, &root, limits, cancel)?;
    if let Some(error) = opf::fixed_layout(&package) {
        return Err(error);
    }
    opf::check_protection(&mut archive, &package, limits, cancel)?;
    let mut spine = package
        .spine
        .iter()
        .enumerate()
        .map(|(index, item)| SpineItem {
            href: item.href.clone(),
            title: format!("Chapter {}", index + 1),
        })
        .collect::<Vec<_>>();
    let mut toc = package.toc;
    opf::finalize(&mut spine, &mut toc, limits)?;
    let mut metadata = package.metadata;
    if metadata.title.trim().is_empty() {
        metadata.title = fallback_title.to_owned();
    }
    if metadata.title.trim().is_empty() {
        metadata.title = "EPUB".into();
    }
    Ok(EpubBook {
        metadata,
        spine,
        toc,
        fingerprint,
    })
}

/// Reads and sanitizes one chapter from a fresh handle on the same bytes.
pub(crate) fn read_chapter<R: Read + Seek>(
    book: &EpubBook,
    source: R,
    index: usize,
    images: ImageMode,
    limits: &Limits,
    cancel: &CancelToken,
) -> Result<Chapter, EpubError> {
    super::check_cancel(cancel)?;
    let item = book.spine.get(index).ok_or_else(|| EpubError::Corrupt {
        detail: format!("chapter {index} does not exist"),
    })?;
    let mut archive = Archive::open(source, limits)?;
    let bytes = archive.entry(&item.href, limits.xml_bytes as u64, cancel)?;
    let text = decode(&bytes)?;
    let document = xml(&text, limits)?;
    let (blocks, mut omissions) = sanitize_chapter(&document, &item.href, limits, cancel)?;
    let anchors = blocks
        .iter()
        .enumerate()
        .flat_map(|(block, fragment)| {
            fragment.anchors.iter().map(move |anchor| AnchorLocation {
                id: anchor.id.clone(),
                block,
                text_offset: anchor.text_offset,
            })
        })
        .collect::<Vec<_>>();
    let mut chapter_images = Vec::new();
    if images == ImageMode::Include {
        let mut seen = HashSet::new();
        let mut total = 0u64;
        for source in blocks.iter().flat_map(|block| block.images.iter()) {
            super::check_cancel(cancel)?;
            if source == super::sanitize::BLOCKED_IMAGE_SRC {
                continue;
            }
            let path = archive::path_of(source).to_owned();
            if path.is_empty() || !seen.insert(path.clone()) {
                continue;
            }
            if chapter_images.len() >= limits.images_per_chapter {
                return Err(EpubError::limit(
                    "chapter images",
                    limits.images_per_chapter as u64,
                ));
            }
            match archive.entry(&path, limits.image_bytes, cancel) {
                Ok(bytes) => {
                    total = total.saturating_add(bytes.len() as u64);
                    if total > limits.chapter_image_bytes {
                        return Err(EpubError::limit(
                            "chapter image bytes",
                            limits.chapter_image_bytes,
                        ));
                    }
                    chapter_images.push(ChapterImage {
                        source: source.clone(),
                        path,
                        bytes,
                    });
                }
                Err(EpubError::Missing { .. }) => omissions.missing_images += 1,
                Err(EpubError::Limit { resource, limit }) => {
                    let _ = (resource, limit);
                    omissions.missing_images += 1;
                }
                Err(error) => return Err(error),
            }
        }
    }
    Ok(Chapter {
        index,
        href: item.href.clone(),
        title: item.title.clone(),
        blocks,
        images: chapter_images,
        anchors,
        omissions,
    })
}

/// Maps a TOC or internal href to its spine index and optional fragment.
pub fn locate(book: &EpubBook, href: &str) -> Option<(usize, Option<String>)> {
    let (path, fragment) = archive::split_fragment(href);
    let index = book.spine.iter().position(|item| item.href == path)?;
    Some((index, fragment.map(str::to_owned)))
}

/// A host-independent image placeholder source. This is not a URI the UI may
/// load; the book view maps it to a generated visible block.
pub const BLOCKED_IMAGE_SOURCE: &str = super::sanitize::BLOCKED_IMAGE_SRC;
