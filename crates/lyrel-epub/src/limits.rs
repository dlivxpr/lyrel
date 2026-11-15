//! Bounded work for an EPUB preview.
//!
//! Upstream Glim sets most book budgets to "unlimited" because it reads local
//! files it already trusts. A workbench opens files from a remote Host and
//! from strangers, so every stage here has a finite budget and reports which
//! one it hit. The values are deliberately generous for ordinary trade books
//! (a 586 KiB, 3000-paragraph chapter is far below them) while keeping a
//! hostile archive from asking for unbounded work.
//!
//! Units are bytes, nodes, entries, or items as named. A chapter read allocates
//! at most `chapter_bytes` for decoded XML, `chapter_html_bytes` for the
//! sanitized fragments and `images_per_chapter * image_bytes` for encoded
//! illustration data. Pixels are bounded again by `markdown::images` when the
//! GUI decodes them.

/// Whole-book input accepted from a Host before parsing (64 MiB).
pub const MAX_INPUT_BYTES: u64 = 64 * 1024 * 1024;
/// ZIP central-directory entries (files and directories).
pub const MAX_ARCHIVE_ENTRIES: usize = 10_000;
/// Declared uncompressed bytes across all ZIP entries (256 MiB).
pub const MAX_ARCHIVE_UNCOMPRESSED: u64 = 256 * 1024 * 1024;
/// One package, nav document, NCX, container, or chapter XML document (8 MiB).
pub const MAX_XML_BYTES: usize = 8 * 1024 * 1024;
/// roxmltree nodes in one XML document.
pub const MAX_XML_NODES: usize = 200_000;
/// Spine items (readable chapters).
pub const MAX_CHAPTERS: usize = 2_000;
/// Table-of-contents entries, including NCX fallback.
pub const MAX_TOC_ENTRIES: usize = 5_000;
/// Nested table-of-contents indentation.
pub const MAX_TOC_DEPTH: usize = 32;
/// Top-level sanitized fragments in one chapter.
pub const MAX_BLOCKS_PER_CHAPTER: usize = 12_000;
/// Sanitized HTML in one block; larger blocks fail explicitly instead of
/// asking the GUI to lay out an unbounded paragraph.
pub const MAX_BLOCK_BYTES: usize = 128 * 1024;
/// Sanitized HTML in one chapter (16 MiB).
pub const MAX_CHAPTER_HTML_BYTES: usize = 16 * 1024 * 1024;
/// Nested HTML elements while sanitizing.
pub const MAX_HTML_DEPTH: usize = 128;
/// Illustration references per chapter.
pub const MAX_IMAGES_PER_CHAPTER: usize = 64;
/// Encoded bytes for one illustration (32 MiB, the workbench image budget).
pub const MAX_IMAGE_BYTES: u64 = 32 * 1024 * 1024;
/// Encoded bytes for all illustrations in one chapter (128 MiB).
pub const MAX_CHAPTER_IMAGE_BYTES: u64 = 128 * 1024 * 1024;
/// Text retained by the full-book search index (32 MiB).
pub const MAX_INDEX_TEXT_BYTES: usize = 32 * 1024 * 1024;
/// Matches returned for one query; beyond this the answer is honestly partial.
pub const MAX_SEARCH_HITS: usize = 5_000;
/// Longest accepted query, in bytes.
pub const MAX_QUERY_BYTES: usize = 512;

/// The budgets a caller may observe. Kept as a struct so tests can exercise a
/// tight budget without building a giant fixture.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Limits {
    pub input_bytes: u64,
    pub archive_entries: usize,
    pub archive_uncompressed: u64,
    pub xml_bytes: usize,
    pub xml_nodes: usize,
    pub chapters: usize,
    pub toc_entries: usize,
    pub toc_depth: usize,
    pub blocks_per_chapter: usize,
    pub block_bytes: usize,
    pub chapter_html_bytes: usize,
    pub html_depth: usize,
    pub images_per_chapter: usize,
    pub image_bytes: u64,
    pub chapter_image_bytes: u64,
    pub index_text_bytes: usize,
    pub search_hits: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            input_bytes: MAX_INPUT_BYTES,
            archive_entries: MAX_ARCHIVE_ENTRIES,
            archive_uncompressed: MAX_ARCHIVE_UNCOMPRESSED,
            xml_bytes: MAX_XML_BYTES,
            xml_nodes: MAX_XML_NODES,
            chapters: MAX_CHAPTERS,
            toc_entries: MAX_TOC_ENTRIES,
            toc_depth: MAX_TOC_DEPTH,
            blocks_per_chapter: MAX_BLOCKS_PER_CHAPTER,
            block_bytes: MAX_BLOCK_BYTES,
            chapter_html_bytes: MAX_CHAPTER_HTML_BYTES,
            html_depth: MAX_HTML_DEPTH,
            images_per_chapter: MAX_IMAGES_PER_CHAPTER,
            image_bytes: MAX_IMAGE_BYTES,
            chapter_image_bytes: MAX_CHAPTER_IMAGE_BYTES,
            index_text_bytes: MAX_INDEX_TEXT_BYTES,
            search_hits: MAX_SEARCH_HITS,
        }
    }
}
