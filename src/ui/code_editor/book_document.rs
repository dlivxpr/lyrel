//! One EPUB document: coherent bytes, chapter cache, full-book index, and
//! Host/document-scoped reading state.
//!
//! The Host supplies one versioned byte stream; this module writes it to a
//! private temporary file and parses from that copy only. The original is
//! never opened for writing, and a same-named local path is never substituted
//! for a remote original. Chapters and the search index are produced on the
//! GPUI background executor from fresh handles on the same temporary copy, so
//! late results are validated by document generation before publication.
//!
//! Reading state is keyed by `HostId + canonical path`; the SHA-256 is only a
//! version guard. It is stored under the local config directory with the
//! atomic private writer, coalesced by one writer thread per document, and
//! never written into the book.

use super::markdown::images::Bitmap;
use super::*;
use crate::ui::code_editor::book_search::BookText;
use gpui_kit::{App, Context, SharedString};
use lyrel_core::host::transfer::SEGMENT_BYTES;
use lyrel_core::host::{Host, HostId, MTime, Meta};
use lyrel_epub::{
    Bookmark, CancellationToken, Chapter, EpubBook, EpubError, ImageMode, Limits, ReaderPrefs,
    ReaderRecord, ReadingPosition, TocEntry, locate, validate_record,
};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::NamedTempFile;

/// Decoded pixels retained across one chapter's illustrations. Encoded bytes
/// are bounded by the parser; this is the second, decoded-pixel budget.
const CHAPTER_PIXEL_BUDGET: u64 = 32 * 1024 * 1024;
/// Chapters kept in memory at once. Presentations keep the chapter they draw
/// alive through their own `Arc`, so eviction only retires chapters nobody is
/// looking at.
const CHAPTER_CACHE: usize = 4;
/// The document's settled state after opening the package.
#[derive(Clone)]
pub(crate) enum BookState {
    Ready(Arc<EpubBook>),
    Unsupported(SharedString),
    Failed(SharedString),
}

/// The book text index's visible lifecycle.
#[derive(Clone)]
pub(crate) enum IndexState {
    Idle,
    Building { done: usize, total: usize },
    Ready(Arc<BookText>),
    Failed(SharedString),
}

/// Texture handles that can be captured by `TextView::image_source`, whose
/// resolver must be `Send + Sync`. The `Bitmap` leases that keep them alive
/// stay in [`ChapterData`] for as long as a presentation draws the chapter.
#[derive(Clone)]
pub(crate) struct ChapterImages {
    sources: HashMap<String, Arc<gpui_kit::RenderImage>>,
    placeholder: Arc<gpui_kit::RenderImage>,
}

impl ChapterImages {
    /// Resolves a `TextView` image source. Never returns a URI: an unknown,
    /// blocked or failed source becomes the visible placeholder.
    pub(crate) fn source(&self, source: &str) -> gpui_kit::ImageSource {
        gpui_kit::ImageSource::Render(
            self.sources
                .get(source)
                .cloned()
                .unwrap_or_else(|| Arc::clone(&self.placeholder)),
        )
    }
}

/// A chapter and its UI texture leases shared by reading presentations.
pub(crate) struct ChapterData {
    pub(crate) chapter: Chapter,
    _image_leases: HashMap<String, Bitmap>,
    _placeholder_lease: Bitmap,
    pub(crate) resolver: Arc<ChapterImages>,
    pub(crate) unavailable: Vec<String>,
}

/// Background-only pixels; no GPUI handles cross the worker boundary.
struct PreparedChapter {
    chapter: Chapter,
    images: HashMap<String, image::RgbaImage>,
    unavailable: Vec<String>,
}

/// One file read off its Host and opened as a book answer.
pub(crate) struct LoadedBook {
    pub(crate) path: PathBuf,
    pub(crate) mtime: Option<MTime>,
    pub(crate) fingerprint: String,
    temp: Option<NamedTempFile>,
    book: Option<Arc<EpubBook>>,
    failure: Option<SharedString>,
    unsupported: bool,
    pub(crate) record: ReaderRecord,
}

impl LoadedBook {
    pub(crate) fn is_ready(&self) -> bool {
        self.book.is_some()
    }

    fn state(&self) -> BookState {
        match (&self.book, &self.failure) {
            (Some(book), _) => BookState::Ready(Arc::clone(book)),
            (None, Some(message)) if self.unsupported => BookState::Unsupported(message.clone()),
            (None, Some(message)) => BookState::Failed(message.clone()),
            (None, None) => BookState::Failed(SharedString::from("The EPUB could not be opened")),
        }
    }
}

/// The coherent copy and metadata of a ZIP that carries no EPUB container.
pub(crate) struct NotEpub {
    pub(crate) fingerprint: String,
    pub(crate) mtime: Option<MTime>,
}

/// The result of a container probe: a real book answer or a generic archive.
pub(crate) enum BookOpen {
    Book(LoadedBook),
    NotEpub(NotEpub),
}

/// Reads one owned version into a private temporary file, opens its package,
/// and loads this Host/document's reading record.
pub(crate) fn load(host: &dyn Host, path: PathBuf, meta: Meta) -> std::io::Result<BookOpen> {
    let limits = Limits::default();
    if meta.is_dir {
        return Err(std::io::Error::new(
            std::io::ErrorKind::IsADirectory,
            format!("{} is a directory", path.display()),
        ));
    }
    if meta.len > limits.input_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::FileTooLarge,
            format!(
                "{} is {} bytes, over the {} byte preview budget",
                path.display(),
                meta.len,
                limits.input_bytes
            ),
        ));
    }
    let mut reader = host.open_read(&path)?;
    let bound = reader.meta();
    if bound.len > limits.input_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::FileTooLarge,
            format!(
                "{} is {} bytes, over the {} byte preview budget",
                path.display(),
                bound.len,
                limits.input_bytes
            ),
        ));
    }
    let mut temp = tempfile::Builder::new().prefix("lyrel-epub-").tempfile()?;
    let mut hasher = Sha256::new();
    let mut read = 0u64;
    while read < bound.len {
        let segment = reader.read_segment((bound.len - read).min(SEGMENT_BYTES))?;
        if segment.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "the Host returned no bytes with {read} of {} read",
                    bound.len
                ),
            ));
        }
        if segment.len() as u64 > bound.len - read {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "the Host returned more bytes than the opened version holds",
            ));
        }
        hasher.update(&segment);
        temp.as_file_mut().write_all(&segment)?;
        read += segment.len() as u64;
    }
    drop(reader);
    temp.as_file_mut().flush()?;
    let digest = hasher.finalize();
    let mut fingerprint = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        write!(fingerprint, "{byte:02x}").expect("writing into String");
    }
    let fallback = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let source = temp.reopen()?;
    let result = lyrel_epub::open_book(
        source,
        fingerprint.clone(),
        &fallback,
        &limits,
        &CancellationToken::new(),
    );
    let mtime = bound.mtime.or(meta.mtime);
    let record = read_record(host.id(), &path, &fingerprint);
    match result {
        Ok(book) => Ok(BookOpen::Book(LoadedBook {
            path,
            mtime,
            fingerprint,
            temp: Some(temp),
            book: Some(Arc::new(book)),
            failure: None,
            unsupported: false,
            record,
        })),
        Err(EpubError::NotEpub) => Ok(BookOpen::NotEpub(NotEpub { fingerprint, mtime })),
        Err(error) => {
            let (unsupported, message) = describe(&error);
            Ok(BookOpen::Book(LoadedBook {
                path,
                mtime,
                fingerprint,
                temp: None,
                book: None,
                failure: Some(message),
                unsupported,
                record,
            }))
        }
    }
}

/// Opens a claimed `.epub` even when it is not a container, so the workbench
/// shows an honest book failure rather than handing it to a ZIP application.
pub(crate) fn load_claimed(
    host: &dyn Host,
    path: PathBuf,
    meta: Meta,
) -> std::io::Result<LoadedBook> {
    match load(host, path.clone(), meta)? {
        BookOpen::Book(book) => Ok(book),
        BookOpen::NotEpub(not_epub) => {
            let record = read_record(host.id(), &path, &not_epub.fingerprint);
            Ok(LoadedBook {
                path,
                mtime: not_epub.mtime,
                fingerprint: not_epub.fingerprint,
                temp: None,
                book: None,
                failure: Some(t(L10nKey::EditorBookNotEpub).into()),
                unsupported: false,
                record,
            })
        }
    }
}

fn describe(error: &EpubError) -> (bool, SharedString) {
    match error {
        EpubError::Protected { detail } => (
            true,
            t_fmt(
                L10nKey::EditorBookUnsupportedProtected,
                &[("detail", detail)],
            )
            .into(),
        ),
        EpubError::FixedLayout { global, items } => (
            true,
            if *global {
                t(L10nKey::EditorBookUnsupportedFixed).into()
            } else {
                t_fmt(
                    L10nKey::EditorBookUnsupportedFixedItems,
                    &[("items", &items.to_string())],
                )
                .into()
            },
        ),
        EpubError::Limit { resource, limit } => (
            false,
            t_fmt(
                L10nKey::EditorBookLimit,
                &[("resource", resource), ("limit", &limit.to_string())],
            )
            .into(),
        ),
        EpubError::Corrupt { detail } => (
            false,
            t_fmt(L10nKey::EditorBookCorrupt, &[("detail", detail)]).into(),
        ),
        EpubError::Encoding { detail } => (
            false,
            t_fmt(L10nKey::EditorBookEncoding, &[("detail", detail)]).into(),
        ),
        EpubError::Missing { entry } => (
            false,
            t_fmt(L10nKey::EditorBookMissing, &[("entry", entry)]).into(),
        ),
        EpubError::PathEscapes { href } => (
            false,
            t_fmt(L10nKey::EditorBookPathEscapes, &[("href", href)]).into(),
        ),
        EpubError::Cancelled => (false, t(L10nKey::EditorBookCancelled).into()),
        EpubError::Io { detail } => (
            false,
            t_fmt(L10nKey::EditorBookCorrupt, &[("detail", detail)]).into(),
        ),
        EpubError::NotEpub => (false, t(L10nKey::EditorBookNotEpub).into()),
    }
}

/// The owning document shared by every presentation of one book.
pub(crate) struct BookDocument {
    path: PathBuf,
    state: BookState,
    temp: Option<NamedTempFile>,
    fingerprint: String,
    limits: Limits,
    cache: HashMap<usize, Arc<ChapterData>>,
    order: VecDeque<usize>,
    pending: HashMap<usize, (u64, CancellationToken)>,
    chapter_errors: HashMap<usize, SharedString>,
    generation: u64,
    index: IndexState,
    index_generation: u64,
    index_cancel: CancellationToken,
    record: ReaderRecord,
    record_path: Option<PathBuf>,
    writer: Option<ReaderWriter>,
    save_error: Option<SharedString>,
}

impl BookDocument {
    pub(crate) fn new(loaded: LoadedBook, cx: &mut Context<Self>) -> Self {
        let record_path = record_path(loaded.record.host, &loaded.path);
        let writer = writer_for(&record_path, cx);
        Self {
            path: loaded.path.clone(),
            state: loaded.state(),
            temp: loaded.temp,
            fingerprint: loaded.fingerprint,
            limits: Limits::default(),
            cache: HashMap::new(),
            order: VecDeque::new(),
            pending: HashMap::new(),
            chapter_errors: HashMap::new(),
            generation: 0,
            index: IndexState::Idle,
            index_generation: 0,
            index_cancel: CancellationToken::new(),
            record: loaded.record,
            record_path,
            writer,
            save_error: None,
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn state(&self) -> &BookState {
        &self.state
    }

    pub(crate) fn book(&self) -> Option<Arc<EpubBook>> {
        match &self.state {
            BookState::Ready(book) => Some(Arc::clone(book)),
            BookState::Unsupported(_) | BookState::Failed(_) => None,
        }
    }

    pub(crate) fn title(&self) -> SharedString {
        match &self.state {
            BookState::Ready(book) => book.title().into(),
            BookState::Unsupported(_) | BookState::Failed(_) => self
                .path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into(),
        }
    }

    pub(crate) fn author_line(&self) -> SharedString {
        match &self.state {
            BookState::Ready(book) => book.metadata.author_line().into(),
            _ => SharedString::default(),
        }
    }

    pub(crate) fn chapter_count(&self) -> usize {
        self.book().map_or(0, |book| book.chapter_count())
    }

    pub(crate) fn toc(&self) -> Vec<TocEntry> {
        self.book().map_or_else(Vec::new, |book| book.toc.clone())
    }

    pub(crate) fn chapter_title(&self, index: usize) -> SharedString {
        self.book()
            .and_then(|book| book.chapter_title(index).map(SharedString::from))
            .unwrap_or_default()
    }

    pub(crate) fn chapter_href(&self, index: usize) -> Option<String> {
        self.book()
            .and_then(|book| book.chapter_href(index).map(str::to_owned))
    }

    pub(crate) fn locate(&self, href: &str) -> Option<(usize, Option<String>)> {
        self.book().and_then(|book| locate(&book, href))
    }

    pub(crate) fn chapter_data(&self, index: usize) -> Option<Arc<ChapterData>> {
        self.cache.get(&index).cloned()
    }

    pub(crate) fn chapter_error(&self, index: usize) -> Option<SharedString> {
        self.chapter_errors.get(&index).cloned()
    }

    pub(crate) fn retry_chapter(&mut self, index: usize, cx: &mut Context<Self>) {
        self.chapter_errors.remove(&index);
        self.request_chapter(index, cx);
    }

    /// Requests one chapter, bounded by the document's generation. Repeated
    /// requests for an in-flight or cached chapter do nothing.
    pub(crate) fn request_chapter(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.cache.contains_key(&index) || self.pending.contains_key(&index) {
            return;
        }
        let Some(book) = self.book() else {
            return;
        };
        if index >= book.chapter_count() {
            return;
        }
        let Some(temp) = self.temp.as_ref().map(|temp| temp.path().to_path_buf()) else {
            return;
        };
        let generation = self.generation;
        let token = CancellationToken::new();
        self.pending.insert(index, (generation, token.clone()));
        let limits = self.limits;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let file = std::fs::File::open(&temp).map_err(|error| EpubError::Io {
                        detail: error.to_string(),
                    })?;
                    lyrel_epub::read_chapter(
                        &book,
                        file,
                        index,
                        ImageMode::Include,
                        &limits,
                        &token,
                    )
                    .and_then(|chapter| Self::decode_images(chapter, &token))
                })
                .await;
            let _ = this.update(cx, |document, cx| {
                document.install_chapter(generation, index, result, cx)
            });
        })
        .detach();
    }

    fn install_chapter(
        &mut self,
        generation: u64,
        index: usize,
        result: Result<PreparedChapter, EpubError>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.generation {
            return;
        }
        let Some((_, token)) = self.pending.get(&index) else {
            return;
        };
        if token.is_cancelled() {
            return;
        }
        self.pending.remove(&index);
        match result {
            Ok(chapter) => {
                self.chapter_errors.remove(&index);
                let data = self.upload_images(chapter, cx);
                self.cache.insert(index, Arc::new(data));
                self.order.push_back(index);
                while self.cache.len() > CHAPTER_CACHE {
                    let Some(oldest) = self.order.pop_front() else {
                        break;
                    };
                    if oldest != index {
                        self.cache.remove(&oldest);
                    }
                }
            }
            Err(EpubError::Cancelled) => {}
            Err(error) => {
                let (_, message) = describe(&error);
                self.chapter_errors.insert(index, message);
            }
        }
        cx.notify();
    }

    fn decode_images(
        mut chapter: Chapter,
        token: &CancellationToken,
    ) -> Result<PreparedChapter, EpubError> {
        let mut decoded = HashMap::new();
        let mut unavailable = Vec::new();
        let mut pixels = 0u64;
        for image in std::mem::take(&mut chapter.images) {
            if token.is_cancelled() {
                return Err(EpubError::Cancelled);
            }
            match super::markdown::images::decode_book_illustration(&image.bytes) {
                Ok(rgba) => {
                    let size = rgba.width() as u64 * rgba.height() as u64;
                    if pixels.saturating_add(size) > CHAPTER_PIXEL_BUDGET {
                        unavailable.push(image.source);
                        continue;
                    }
                    pixels += size;
                    decoded.insert(image.source, rgba);
                }
                Err(_) => unavailable.push(image.source),
            }
        }
        if token.is_cancelled() {
            return Err(EpubError::Cancelled);
        }
        Ok(PreparedChapter {
            chapter,
            images: decoded,
            unavailable,
        })
    }

    fn upload_images(&self, prepared: PreparedChapter, cx: &mut Context<Self>) -> ChapterData {
        let PreparedChapter {
            chapter,
            images,
            unavailable,
        } = prepared;
        let decoded: HashMap<_, _> = images
            .into_iter()
            .map(|(source, rgba)| (source, super::markdown::images::bitmap(rgba, cx)))
            .collect();
        let placeholder = placeholder_bitmap(cx);
        let resolver = Arc::new(ChapterImages {
            sources: decoded
                .iter()
                .map(|(source, bitmap)| (source.clone(), bitmap.image()))
                .collect(),
            placeholder: placeholder.image(),
        });
        ChapterData {
            chapter,
            _image_leases: decoded,
            _placeholder_lease: placeholder,
            resolver,
            unavailable,
        }
    }

    pub(crate) fn index_state(&self) -> &IndexState {
        &self.index
    }

    pub(crate) fn index_text(&self) -> Option<Arc<BookText>> {
        match &self.index {
            IndexState::Ready(text) => Some(Arc::clone(text)),
            _ => None,
        }
    }

    /// Builds the search index once, one chapter at a time, so progress stays
    /// visible and cancellation can take effect between chapters.
    pub(crate) fn ensure_index(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.index, IndexState::Idle) {
            return;
        }
        let Some(book) = self.book() else {
            return;
        };
        let Some(temp) = self.temp.as_ref().map(|temp| temp.path().to_path_buf()) else {
            return;
        };
        let limits = self.limits;
        let total = book.chapter_count();
        let generation = self.index_generation.wrapping_add(1);
        self.index_generation = generation;
        let token = CancellationToken::new();
        self.index_cancel = token.clone();
        self.index = IndexState::Building { done: 0, total };
        cx.spawn(async move |this, cx| {
            let mut chapters: Vec<Chapter> = Vec::with_capacity(total);
            let mut bytes = 0usize;
            for index in 0..total {
                if token.is_cancelled() {
                    return;
                }
                let book = Arc::clone(&book);
                let path = temp.clone();
                let chapter_token = token.clone();
                let result = cx
                    .background_spawn(async move {
                        let file = std::fs::File::open(&path).map_err(|error| EpubError::Io {
                            detail: error.to_string(),
                        })?;
                        lyrel_epub::read_chapter(
                            &book,
                            file,
                            index,
                            ImageMode::Skip,
                            &limits,
                            &chapter_token,
                        )
                    })
                    .await;
                match result {
                    Ok(chapter) => {
                        bytes = bytes.saturating_add(
                            chapter
                                .blocks
                                .iter()
                                .map(|block| block.text.len())
                                .sum::<usize>(),
                        );
                        if bytes > limits.index_text_bytes {
                            let message = t_fmt(
                                L10nKey::EditorBookIndexTooLarge,
                                &[("bytes", &limits.index_text_bytes.to_string())],
                            );
                            let _ = this.update(cx, |document, cx| {
                                if document.index_generation == generation {
                                    document.index = IndexState::Failed(message.into());
                                    cx.notify();
                                }
                            });
                            return;
                        }
                        chapters.push(chapter);
                    }
                    Err(EpubError::Cancelled) => return,
                    Err(error) => {
                        let (_, message) = describe(&error);
                        let _ = this.update(cx, |document, cx| {
                            if document.index_generation == generation {
                                document.index = IndexState::Failed(message);
                                cx.notify();
                            }
                        });
                        return;
                    }
                }
                if this
                    .update(cx, |document, cx| {
                        if document.index_generation == generation {
                            document.index = IndexState::Building {
                                done: index + 1,
                                total,
                            };
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    return;
                }
            }
            match cx
                .background_spawn(async move { BookText::new(chapters, &limits) })
                .await
            {
                Ok(text) => {
                    let _ = this.update(cx, |document, cx| {
                        if document.index_generation == generation {
                            document.index = IndexState::Ready(Arc::new(text));
                            cx.notify();
                        }
                    });
                }
                Err(bytes) => {
                    let message = t_fmt(
                        L10nKey::EditorBookIndexTooLarge,
                        &[("bytes", &bytes.to_string())],
                    );
                    let _ = this.update(cx, |document, cx| {
                        if document.index_generation == generation {
                            document.index = IndexState::Failed(message.into());
                            cx.notify();
                        }
                    });
                }
            }
        })
        .detach();
    }

    pub(crate) fn prefs(&self) -> ReaderPrefs {
        self.record.prefs.clone()
    }

    pub(crate) fn reading(&self) -> ReadingPosition {
        self.record.reading
    }

    pub(crate) fn bookmarks(&self) -> &[Bookmark] {
        &self.record.bookmarks
    }

    pub(crate) fn set_prefs(&mut self, prefs: ReaderPrefs, cx: &mut Context<Self>) {
        self.record.prefs = prefs;
        self.persist(cx);
        cx.notify();
    }

    pub(crate) fn set_reading(&mut self, reading: ReadingPosition) {
        self.record.reading = reading;
    }

    pub(crate) fn toggle_bookmark(
        &mut self,
        chapter: usize,
        block: usize,
        fraction: f32,
        label: String,
        cx: &mut Context<Self>,
    ) -> bool {
        let removed = self
            .record
            .bookmarks
            .iter()
            .position(|bookmark| bookmark.chapter == chapter && bookmark.block == block);
        let added = if let Some(index) = removed {
            self.record.bookmarks.remove(index);
            false
        } else {
            let label = label.chars().take(120).collect::<String>();
            self.record.bookmarks.push(Bookmark {
                chapter,
                block,
                fraction: fraction.clamp(0., 1.),
                label,
            });
            if self.record.bookmarks.len() > 500 {
                self.record.bookmarks.remove(0);
            }
            true
        };
        self.persist(cx);
        cx.notify();
        added
    }

    pub(crate) fn persist(&mut self, cx: &mut Context<Self>) {
        if self.record_path.is_some() {
            let record = Arc::new(self.record.clone());
            if let Some(writer) = &self.writer
                && writer.send(record).is_err()
            {
                self.save_error = Some(t(L10nKey::EditorBookSaveFailed).into());
                cx.notify();
            }
        } else {
            self.save_error = Some(t(L10nKey::EditorBookSaveFailed).into());
            cx.notify();
        }
    }

    pub(crate) fn save_error(&self) -> Option<SharedString> {
        self.save_error.clone()
    }

    /// A fresh coherent read of the same Host path replaced the document.
    pub(crate) fn replace(&mut self, loaded: LoadedBook, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        self.index_generation = self.index_generation.wrapping_add(1);
        self.index_cancel.cancel();
        for (_, (_, token)) in self.pending.drain() {
            token.cancel();
        }
        self.cache.clear();
        self.order.clear();
        self.chapter_errors.clear();
        self.index = IndexState::Idle;
        self.state = loaded.state();
        self.temp = loaded.temp;
        self.fingerprint = loaded.fingerprint;
        self.record = loaded.record;
        self.path = loaded.path;
        let next_record = record_path(self.record.host, &self.path);
        if next_record != self.record_path {
            self.record_path = next_record;
            self.writer = writer_for(&self.record_path, cx);
        }
        cx.notify();
    }
}

/// One reader-state writer plus the task that surfaces its failures.
fn writer_for(path: &Option<PathBuf>, cx: &mut Context<BookDocument>) -> Option<ReaderWriter> {
    let path = path.clone()?;
    let (error_tx, error_rx) = smol::channel::unbounded::<SharedString>();
    let document = cx.entity().downgrade();
    cx.spawn(async move |_, cx| {
        while let Ok(message) = error_rx.recv().await {
            if document
                .update(cx, |document, cx| {
                    document.save_error = Some(message);
                    cx.notify();
                })
                .is_err()
            {
                break;
            }
        }
    })
    .detach();
    Some(ReaderWriter::new(path, error_tx))
}

impl Drop for BookDocument {
    fn drop(&mut self) {
        self.index_cancel.cancel();
        for (_, (_, token)) in self.pending.drain() {
            token.cancel();
        }
        if let Some(writer) = &self.writer {
            let _ = writer.send(Arc::new(self.record.clone()));
        }
    }
}

fn placeholder_bitmap(cx: &mut App) -> Bitmap {
    use gpui_kit::Rgba;
    let background = Rgba::from(cx.theme().muted);
    let border = Rgba::from(cx.theme().border);
    let fill = |color: Rgba| {
        image::Rgba([
            (color.r.clamp(0., 1.) * 255.) as u8,
            (color.g.clamp(0., 1.) * 255.) as u8,
            (color.b.clamp(0., 1.) * 255.) as u8,
            255,
        ])
    };
    let mut rgba = image::RgbaImage::from_pixel(480, 160, fill(background));
    let edge = fill(border);
    for x in 0..480 {
        rgba.put_pixel(x, 0, edge);
        rgba.put_pixel(x, 159, edge);
    }
    for y in 0..160 {
        rgba.put_pixel(0, y, edge);
        rgba.put_pixel(479, y, edge);
    }
    super::markdown::images::bitmap(rgba, cx)
}

/// `config_path` is local state, not Host storage; it never becomes a save
/// target for a remotely owned book.
fn record_path(host: u64, path: &Path) -> Option<PathBuf> {
    let key = format!("{host}:{}", path.to_string_lossy());
    let name = format!("{:016x}.json", lyrel_core::host::fnv1a64(key.as_bytes()));
    crate::core::config::config_path("reader-state").map(|dir| dir.join(name))
}

fn read_record(host: HostId, path: &Path, fingerprint: &str) -> ReaderRecord {
    let mut record = record_path(host.0, path)
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice::<ReaderRecord>(&bytes).ok())
        .filter(validate_record)
        .filter(|record| record.host == host.0 && record.path == path.to_string_lossy().as_ref())
        .unwrap_or_else(|| ReaderRecord::new(host.0, path.to_string_lossy(), fingerprint));
    record.adopt_fingerprint(fingerprint);
    record
}

/// One coalescing writer per document. It is deliberately a plain thread:
/// reading state is a small local file, saves must not be lost when a GPUI task
/// is dropped on close, and no Host or UI context is needed.
struct ReaderWriter {
    tx: Option<std::sync::mpsc::Sender<Arc<ReaderRecord>>>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl ReaderWriter {
    fn new(path: PathBuf, errors: smol::channel::Sender<SharedString>) -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<Arc<ReaderRecord>>();
        let join = std::thread::Builder::new()
            .name("lyrel-book-state".into())
            .spawn(move || {
                while let Ok(mut record) = rx.recv() {
                    while let Ok(next) = rx.try_recv() {
                        record = next;
                    }
                    if let Err(error) = save_record(&path, &record) {
                        log::warn!(
                            "book: could not save reading state to {}: {error}",
                            path.display()
                        );
                        let _ = errors.send_blocking(t(L10nKey::EditorBookSaveFailed).into());
                    }
                }
            })
            .ok();
        Self { tx: Some(tx), join }
    }

    fn send(&self, record: Arc<ReaderRecord>) -> Result<(), ()> {
        self.tx.as_ref().ok_or(())?.send(record).map_err(|_| ())
    }
}

impl Drop for ReaderWriter {
    fn drop(&mut self) {
        self.tx.take();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn save_record(path: &Path, record: &ReaderRecord) -> std::io::Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    std::fs::create_dir_all(parent)?;
    let bytes = serde_json::to_vec(record).map_err(std::io::Error::other)?;
    crate::core::config::write_atomic_private(path, &bytes)
}

/// Tests use this to prove a reopened document restores its state.
#[cfg(test)]
pub(crate) fn record_file(host: HostId, path: &Path) -> Option<PathBuf> {
    record_path(host.0, path)
}
