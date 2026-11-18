//! Reading state, validated independently of storage.
//!
//! Glim's `glim-core/src/books.rs` model and the ranges in
//! `glim-services/src/books/preferences.rs` are retained, with Lyrel units:
//! font size and page width are rem-based so interface scaling reflows the
//! book, and the persistent key is the owning Host plus canonical path rather
//! than a book-content hash. Typography, reading position and bookmarks live
//! in one small record per Host/document; the file's SHA-256 is only a version
//! guard, so a changed file keeps typography and resets position/bookmarks.

use serde::{Deserialize, Serialize};

use super::limits::{MAX_BLOCKS_PER_CHAPTER, MAX_CHAPTERS};

/// Current on-disk record schema.
pub const RECORD_SCHEMA: u32 = 1;
/// Longest bookmark label, in characters.
pub const MAX_LABEL_CHARS: usize = 120;
/// Bookmarks retained for one book.
pub const MAX_BOOKMARKS: usize = 500;

/// The reading page may follow the workbench or choose a separate light/dark palette.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReaderTheme {
    #[default]
    Application,
    Light,
    Dark,
}

/// Typography that applies to every chapter of one book.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct ReaderPrefs {
    /// Body size in rem, relative to the window's interface scale.
    pub font_scale: f32,
    /// Line height relative to the body size.
    pub line_height: f32,
    /// Reading column width in rem.
    pub page_width_rem: f32,
    /// Reading-only palette; never changes the workbench or book bytes.
    pub theme: ReaderTheme,
}

impl Default for ReaderPrefs {
    fn default() -> Self {
        Self {
            font_scale: 1.15,
            line_height: 1.7,
            page_width_rem: 44.,
            theme: ReaderTheme::Application,
        }
    }
}

/// Where the reader was, in spine and block coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct ReadingPosition {
    pub chapter: usize,
    pub block: usize,
    /// Progress within the chapter, for display and coarse bookmarks. The
    /// block is authoritative when restoring.
    pub fraction: f32,
}

impl ReadingPosition {
    pub fn new(chapter: usize, block: usize, fraction: f32) -> Self {
        Self {
            chapter,
            block,
            fraction,
        }
    }
}

/// One bookmark: a stable chapter/block anchor plus a readable label.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Bookmark {
    pub chapter: usize,
    pub block: usize,
    pub fraction: f32,
    pub label: String,
}

impl Bookmark {
    pub fn new(chapter: usize, block: usize, fraction: f32, label: String) -> Self {
        Self {
            chapter,
            block,
            fraction,
            label,
        }
    }
}

/// One book's persisted reading record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ReaderRecord {
    pub schema: u32,
    /// Owning Host identity (`HostId.0`); equal paths on different Hosts are
    /// different documents.
    pub host: u64,
    /// Canonical path on that Host, for display and identity verification.
    pub path: String,
    /// SHA-256 of the book bytes this record was last used with.
    pub fingerprint: String,
    pub prefs: ReaderPrefs,
    pub reading: ReadingPosition,
    pub bookmarks: Vec<Bookmark>,
}

impl ReaderRecord {
    pub fn new(host: u64, path: impl Into<String>, fingerprint: impl Into<String>) -> Self {
        Self {
            schema: RECORD_SCHEMA,
            host,
            path: path.into(),
            fingerprint: fingerprint.into(),
            prefs: ReaderPrefs::default(),
            reading: ReadingPosition::default(),
            bookmarks: Vec::new(),
        }
    }

    /// A changed book keeps the reader's typography but cannot restore a
    /// position or bookmark from different content.
    pub fn adopt_fingerprint(&mut self, fingerprint: &str) {
        if self.fingerprint != fingerprint {
            self.fingerprint = fingerprint.to_owned();
            self.reading = ReadingPosition::default();
            self.bookmarks.clear();
        }
    }
}

/// Validates a record read from local storage.
///
/// Invalid values are refused wholesale by the caller, which falls back to
/// defaults rather than persisting an unreadable record again.
pub fn validate_record(record: &ReaderRecord) -> bool {
    record.schema == RECORD_SCHEMA
        && record.path.len() <= 64 * 1024
        && record.fingerprint.len() == 64
        && record
            .fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        && record.prefs.font_scale.is_finite()
        && (0.75..=2.25).contains(&record.prefs.font_scale)
        && record.prefs.line_height.is_finite()
        && (1.2..=2.4).contains(&record.prefs.line_height)
        && record.prefs.page_width_rem.is_finite()
        && (24.0..=80.0).contains(&record.prefs.page_width_rem)
        && record.reading.chapter < MAX_CHAPTERS
        && record.reading.block < MAX_BLOCKS_PER_CHAPTER
        && record.reading.fraction.is_finite()
        && (0.0..=1.0).contains(&record.reading.fraction)
        && record.bookmarks.len() <= MAX_BOOKMARKS
        && record.bookmarks.iter().all(|bookmark| {
            bookmark.chapter < MAX_CHAPTERS
                && bookmark.block < MAX_BLOCKS_PER_CHAPTER
                && bookmark.fraction.is_finite()
                && (0.0..=1.0).contains(&bookmark.fraction)
                && !bookmark.label.is_empty()
                && bookmark.label.chars().count() <= MAX_LABEL_CHARS
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> ReaderRecord {
        ReaderRecord::new(7, "books/one.epub", "a".repeat(64))
    }

    #[test]
    fn a_valid_record_round_trips_and_nan_is_refused() {
        let mut value = record();
        value.prefs.font_scale = 1.4;
        value.reading = ReadingPosition {
            chapter: 2,
            block: 40,
            fraction: 0.5,
        };
        assert!(validate_record(&value));
        let json = serde_json::to_string(&value).unwrap();
        let back: ReaderRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back, value);
        value.reading.fraction = f32::NAN;
        assert!(!validate_record(&value));
    }

    #[test]
    fn different_content_keeps_typography_and_clears_position() {
        let mut value = record();
        value.prefs.font_scale = 1.6;
        value.reading.chapter = 3;
        value.bookmarks.push(Bookmark {
            chapter: 1,
            block: 2,
            fraction: 0.2,
            label: "One".into(),
        });
        value.adopt_fingerprint(&"b".repeat(64));
        assert_eq!(value.prefs.font_scale, 1.6);
        assert_eq!(value.reading, ReadingPosition::default());
        assert!(value.bookmarks.is_empty());
    }
}
