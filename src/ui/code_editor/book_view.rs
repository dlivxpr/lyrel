//! One presentation of a book: contents, chapter, search, framing, and the
//! reading position it reports.
//!
//! Every block is its own `TextView` inside a virtualized `gpui` list. Kit's
//! HTML parser puts a whole chapter in one `Root` block, so one large
//! `TextView` would lay out all of a 3000-paragraph chapter at once; block
//! fragments restore paragraph-level virtualization without converting to
//! Markdown. Only blocks near the viewport keep a parsed `TextViewState`;
//! leaving that window drops the state and its parsed tree, while the
//! document keeps a bounded chapter cache and the illustration texture leases.
//!
//! HTML has no range-highlight or source-map APIs in Kit 0.9.1, so search
//! marks are injected into a block's sanitized HTML. Links are handled through
//! a queue (the `TextView` resolver must be `Send + Sync`) and never open an
//! external URL.

use super::book_document::{BookDocument, BookState, ChapterData, IndexState};
use super::book_search::{self, SearchHit};
use super::*;
use gpui_kit::base::Selectable as _;
use gpui_kit::base::{TextView, TextViewState, TextViewStyle};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, IconName, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, ListState, Render, SharedString,
    Subscription, Task, Window, div, px, relative, rems,
};
use lyrel_epub::{
    CancellationToken, Limits, ReaderPrefs, ReaderTheme, ReadingPosition, html_with_marks,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

#[cfg(test)]
mod tests;

gpui_kit::actions!(
    book,
    [
        BookNextPage,
        BookPreviousPage,
        BookNextChapter,
        BookPreviousChapter,
        BookToggleContents,
        BookToggleBookmarks,
        BookSearch,
        BookNextResult,
        BookPreviousResult,
        BookAddBookmark,
        BookFontLarger,
        BookFontSmaller,
        BookCycleSpacing,
        BookCycleWidth,
        BookCycleTheme
    ]
);

/// Only a book presentation that holds the keyboard declares this context.
pub(crate) const KEY_CONTEXT: &str = "BookDocument";

/// How long a scroll settles before the reading position is written.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(250);
/// Blocks kept parsed on each side of the viewport.
const STATE_MARGIN: usize = 12;

/// A parsed block and the mark revision it was built from.
struct BlockState {
    entity: Entity<TextViewState>,
    revision: u64,
    _subscription: Subscription,
}

#[derive(Clone)]
enum Intent {
    Follow(String),
    Contents(String),
    Bookmark(usize),
    DeleteBookmark(usize),
}

/// One contents/bookmarks row, independent of render closure lifetimes.
struct SideRow {
    label: SharedString,
    depth: usize,
    bookmark: Option<usize>,
    href: Option<String>,
}

pub(crate) struct BookView {
    document: Entity<BookDocument>,
    focus: FocusHandle,
    list: ListState,
    side_list: ListState,
    chapter: usize,
    data: Option<Arc<ChapterData>>,
    blocks: usize,
    states: Rc<RefCell<HashMap<usize, BlockState>>>,
    visible: Range<usize>,
    toc_open: bool,
    bookmarks_open: bool,
    search_open: bool,
    search_input: Entity<InputState>,
    search_generation: u64,
    search_cancel: CancellationToken,
    search_query: String,
    results: Vec<SearchHit>,
    results_truncated: bool,
    active_result: Option<usize>,
    pending_query: Option<String>,
    marks: HashMap<usize, Vec<Range<usize>>>,
    active_mark: Option<(usize, Range<usize>)>,
    marks_revision: u64,
    pending_restore: Option<usize>,
    pending_anchor: Option<String>,
    loading: bool,
    chapter_error: Option<SharedString>,
    position: ReadingPosition,
    save_task: Option<Task<()>>,
    intent_tx: smol::channel::Sender<Intent>,
    _listener: Task<()>,
    _document: Subscription,
    _search_input: Subscription,
    _search_state: Subscription,
    side_count: usize,
    prefs_key: (u32, u32, u32, ReaderTheme),
    palette: Option<ReadingColors>,
    mark_key: Option<(gpui_kit::Hsla, gpui_kit::Hsla)>,
    mark_color: String,
    active_color: String,
}

impl BookView {
    pub(crate) fn new(
        document: Entity<BookDocument>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (document_reading, chapter_count, reader_theme) = {
            let document = document.read(cx);
            (
                document.reading(),
                document.chapter_count(),
                document.prefs().theme,
            )
        };
        let chapter = document_reading
            .chapter
            .min(chapter_count.saturating_sub(1));
        let block = document_reading.block;
        let list = ListState::new(0, gpui_kit::ListAlignment::Top, px(256.));
        let side_list = ListState::new(0, gpui_kit::ListAlignment::Top, px(256.));
        let focus = cx.focus_handle();
        let search_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t(L10nKey::EditorBookSearchPlaceholder))
        });
        let weak = cx.entity().downgrade();
        list.set_scroll_handler(move |event, window, cx| {
            let _ = weak.update(cx, |view, cx| view.scrolled(event, window, cx));
        });
        let (intent_tx, intent_rx) = smol::channel::unbounded::<Intent>();
        let weak = cx.entity().downgrade();
        let listener = cx.spawn(async move |_, cx| {
            while let Ok(intent) = intent_rx.recv().await {
                if weak.update(cx, |view, cx| view.act(intent, cx)).is_err() {
                    break;
                }
            }
        });
        let _document = cx.observe(&document, |view, _, cx| view.sync(cx));
        let _search_input = cx.subscribe_in(
            &search_input,
            window,
            |view, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => view.query_changed(cx),
                InputEvent::PressEnter { shift, .. } => {
                    if *shift {
                        view.previous_result(cx);
                    } else {
                        view.next_result(cx);
                    }
                    window.focus(&view.focus, cx);
                }
                InputEvent::Focus | InputEvent::Blur => {}
            },
        );
        let _search_state = cx.observe(&search_input, |_, _, cx| cx.notify());
        let mut view = Self {
            document: document.clone(),
            focus,
            list,
            side_list,
            chapter,
            data: None,
            blocks: 0,
            states: Rc::new(RefCell::new(HashMap::new())),
            visible: 0..0,
            toc_open: true,
            bookmarks_open: false,
            search_open: false,
            search_input,
            search_generation: 0,
            search_cancel: CancellationToken::new(),
            search_query: String::new(),
            results: Vec::new(),
            results_truncated: false,
            active_result: None,
            pending_query: None,
            marks: HashMap::new(),
            active_mark: None,
            marks_revision: 0,
            pending_restore: Some(block),
            pending_anchor: None,
            loading: true,
            chapter_error: None,
            position: ReadingPosition {
                chapter,
                block,
                fraction: document_reading.fraction,
            },
            save_task: None,
            intent_tx,
            _listener: listener,
            _document,
            _search_input,
            _search_state,
            side_count: 0,
            prefs_key: (0, 0, 0, ReaderTheme::Application),
            palette: reading_palette(reader_theme, cx),
            mark_key: None,
            mark_color: color_hex(cx.theme().selection),
            active_color: color_hex(cx.theme().warning),
        };
        view.refresh_side_count(cx);
        document.update(cx, |document, cx| document.request_chapter(chapter, cx));
        let _ = window;
        view
    }

    pub(crate) fn document(&self) -> &Entity<BookDocument> {
        &self.document
    }

    pub(crate) fn handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub(crate) fn chapter(&self) -> usize {
        self.chapter
    }

    pub(crate) fn block_count(&self) -> usize {
        self.blocks
    }

    pub(crate) fn block_text(&self, index: usize) -> Option<&str> {
        self.data
            .as_ref()
            .and_then(|data| data.chapter.blocks.get(index))
            .map(|block| block.text.as_str())
    }

    pub(crate) fn search_results(&self) -> &[SearchHit] {
        &self.results
    }

    pub(crate) fn chapter_label(&self, cx: &App) -> SharedString {
        t_fmt(
            L10nKey::EditorBookChapterOf,
            &[
                ("chapter", &(self.chapter + 1).to_string()),
                (
                    "chapters",
                    &self.document.read(cx).chapter_count().to_string(),
                ),
            ],
        )
        .into()
    }

    pub(crate) fn progress_label(&self, cx: &App) -> SharedString {
        format!("{:.0}%", self.progress(cx) * 100.).into()
    }

    fn progress(&self, cx: &App) -> f32 {
        let chapters = self.document.read(cx).chapter_count().max(1);
        ((self.position.chapter as f32 + self.position.fraction.clamp(0., 1.)) / chapters as f32)
            .clamp(0., 1.)
    }

    pub(crate) fn status_text(&self, cx: &App) -> SharedString {
        let mut parts = Vec::new();
        parts.push(self.chapter_label(cx).to_string());
        parts.push(t(L10nKey::EditorBookReadOnly).to_owned());
        if let IndexState::Building { done, total } = self.document.read(cx).index_state() {
            parts.push(
                t_fmt(
                    L10nKey::EditorBookIndexing,
                    &[("done", &done.to_string()), ("total", &total.to_string())],
                )
                .to_string(),
            );
        }
        if !self.results.is_empty() {
            let current = self.active_result.map_or(0, |index| index + 1);
            parts.push(
                t_fmt(
                    L10nKey::EditorBookMatches,
                    &[
                        ("current", &current.to_string()),
                        ("total", &self.results.len().to_string()),
                    ],
                )
                .to_string(),
            );
        }
        parts.join(" · ").into()
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let (chapter_count, prefs, state) = {
            let document = self.document.read(cx);
            (
                document.chapter_count(),
                document.prefs(),
                document.state().clone(),
            )
        };
        if chapter_count > 0 && self.chapter >= chapter_count {
            self.chapter = chapter_count - 1;
        }
        let prefs_key = (
            prefs.font_scale.to_bits(),
            prefs.line_height.to_bits(),
            prefs.page_width_rem.to_bits(),
            prefs.theme,
        );
        if prefs_key != self.prefs_key {
            if prefs.theme != self.prefs_key.3 {
                self.palette = reading_palette(prefs.theme, cx);
            }
            self.prefs_key = prefs_key;
            self.list.remeasure_items(0..self.blocks);
        }
        if let BookState::Unsupported(message) | BookState::Failed(message) = state {
            if self.data.is_none() {
                self.loading = false;
                self.chapter_error = Some(message);
            }
        }
        let data = self.document.read(cx).chapter_data(self.chapter);
        if let Some(data) = data {
            let changed = self
                .data
                .as_ref()
                .is_none_or(|current| !Arc::ptr_eq(current, &data));
            if changed {
                self.install_chapter(data, cx);
            }
        } else if let Some(message) = self.document.read(cx).chapter_error(self.chapter) {
            self.loading = false;
            self.chapter_error = Some(message);
        }
        let ready = self.pending_query.is_some()
            || (self.search_open && self.document.read(cx).index_text().is_some());
        if ready && self.pending_query.is_some() {
            self.schedule_search(cx);
        }
        self.refresh_side_count(cx);
        cx.notify();
    }

    fn install_chapter(&mut self, data: Arc<ChapterData>, cx: &mut Context<Self>) {
        self.blocks = data.chapter.blocks.len();
        self.data = Some(Arc::clone(&data));
        self.states.borrow_mut().clear();
        self.marks.clear();
        self.active_mark = None;
        self.marks_revision = self.marks_revision.wrapping_add(1);
        self.list.reset(self.blocks);
        let mut target = self
            .pending_restore
            .take()
            .or_else(|| self.position.block.into())
            .unwrap_or(0)
            .min(self.blocks.saturating_sub(1));
        if let Some(anchor) = self.pending_anchor.take()
            && let Some(found) = data
                .chapter
                .anchors
                .iter()
                .find(|anchor_location| anchor_location.id == anchor)
        {
            target = found.block.min(self.blocks.saturating_sub(1));
        }
        self.list.scroll_to(gpui_kit::ListOffset {
            item_ix: target,
            offset_in_item: px(0.),
        });
        self.visible = target..(target + 1).min(self.blocks);
        self.ensure_states(target, cx);
        self.rebuild_marks();
        self.loading = false;
        self.chapter_error = None;
        self.position = ReadingPosition {
            chapter: self.chapter,
            block: target,
            fraction: if self.blocks <= 1 {
                0.
            } else {
                target as f32 / (self.blocks - 1) as f32
            },
        };
        let document = self.document.clone();
        document.update(cx, |document, cx| {
            document.set_reading(self.position);
            document.persist(cx);
        });
        self.refresh_side_count(cx);
        cx.notify();
    }

    fn side_rows(&self, cx: &App) -> Vec<SideRow> {
        if self.bookmarks_open {
            self.document
                .read(cx)
                .bookmarks()
                .iter()
                .enumerate()
                .map(|(index, bookmark)| SideRow {
                    label: bookmark.label.clone().into(),
                    depth: 0,
                    bookmark: Some(index),
                    href: None,
                })
                .collect()
        } else {
            self.document
                .read(cx)
                .toc()
                .into_iter()
                .map(|entry| SideRow {
                    label: entry.title.into(),
                    depth: entry.depth.min(8),
                    bookmark: None,
                    href: Some(entry.href),
                })
                .collect()
        }
    }

    fn refresh_side_count(&mut self, cx: &App) {
        let count = self.side_rows(cx).len();
        if count != self.side_count {
            self.side_count = count;
            self.side_list.reset(count);
        }
    }

    fn act(&mut self, intent: Intent, cx: &mut Context<Self>) {
        match intent {
            Intent::Follow(href) => self.follow(&href, cx),
            Intent::Contents(href) => self.go_to_href(&href, cx),
            Intent::Bookmark(index) => self.go_to_bookmark(index, cx),
            Intent::DeleteBookmark(index) => {
                let bookmark = self.document.read(cx).bookmarks().get(index).cloned();
                if let Some(bookmark) = bookmark {
                    self.document.update(cx, |document, cx| {
                        document.toggle_bookmark(
                            bookmark.chapter,
                            bookmark.block,
                            bookmark.fraction,
                            bookmark.label,
                            cx,
                        );
                    });
                    self.refresh_side_count(cx);
                    cx.notify();
                }
            }
        }
    }

    fn follow(&mut self, href: &str, cx: &mut Context<Self>) {
        self.go_to_href(href, cx);
    }

    fn go_to_href(&mut self, href: &str, cx: &mut Context<Self>) {
        let resolved = if href.starts_with('#') {
            let current = self.document.read(cx).chapter_href(self.chapter);
            current
                .and_then(|base| lyrel_epub::resolve(&base, href))
                .unwrap_or_else(|| href.to_owned())
        } else {
            href.to_owned()
        };
        let Some((chapter, fragment)) = self.document.read(cx).locate(&resolved) else {
            return;
        };
        let block = fragment
            .as_deref()
            .and_then(|fragment| {
                self.document
                    .read(cx)
                    .chapter_data(chapter)
                    .and_then(|data| {
                        data.chapter
                            .anchors
                            .iter()
                            .find(|anchor| anchor.id == fragment)
                            .map(|anchor| anchor.block)
                    })
            })
            .unwrap_or(0);
        self.go_to_chapter(chapter, block, fragment, cx);
    }

    fn go_to_chapter(
        &mut self,
        chapter: usize,
        block: usize,
        anchor: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if chapter == self.chapter {
            let block = block.min(self.blocks.saturating_sub(1));
            self.list.scroll_to(gpui_kit::ListOffset {
                item_ix: block,
                offset_in_item: px(0.),
            });
            self.position = ReadingPosition {
                chapter,
                block,
                fraction: if self.blocks <= 1 {
                    0.
                } else {
                    block as f32 / (self.blocks - 1) as f32
                },
            };
            self.ensure_states(block, cx);
            self.document.update(cx, |document, cx| {
                document.set_reading(self.position);
                document.persist(cx);
            });
            cx.notify();
            return;
        }
        self.chapter = chapter;
        self.data = None;
        self.blocks = 0;
        self.states.borrow_mut().clear();
        self.list.reset(0);
        self.loading = true;
        self.chapter_error = None;
        self.pending_restore = Some(block);
        self.pending_anchor = anchor;
        self.position = ReadingPosition {
            chapter,
            block,
            fraction: 0.,
        };
        self.document.update(cx, |document, cx| {
            document.set_reading(self.position);
            document.persist(cx);
            document.request_chapter(chapter, cx);
        });
        self.sync(cx);
        cx.notify();
    }

    fn go_to_bookmark(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(bookmark) = self.document.read(cx).bookmarks().get(index).cloned() else {
            return;
        };
        self.go_to_chapter(bookmark.chapter, bookmark.block, None, cx);
    }

    fn next_page(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        if self.list.is_scrolled_to_end() == Some(true) {
            self.next_chapter(cx);
            return;
        }
        let height = self.list.viewport_bounds().size.height;
        self.list.scroll_by(height * 0.9);
        self.commit_position(cx);
        cx.notify();
    }

    fn previous_page(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        let top = self.list.logical_scroll_top();
        if top.item_ix == 0 && top.offset_in_item <= px(0.) {
            self.previous_chapter(cx);
            return;
        }
        let height = self.list.viewport_bounds().size.height;
        self.list.scroll_by(-height * 0.9);
        self.commit_position(cx);
        cx.notify();
    }

    fn next_chapter(&mut self, cx: &mut Context<Self>) {
        let count = self.document.read(cx).chapter_count();
        if self.chapter + 1 < count {
            self.go_to_chapter(self.chapter + 1, 0, None, cx);
        }
    }

    fn previous_chapter(&mut self, cx: &mut Context<Self>) {
        if self.chapter > 0 {
            self.go_to_chapter(self.chapter - 1, 0, None, cx);
        }
    }

    fn toggle_contents(&mut self, cx: &mut Context<Self>) {
        self.toc_open = !self.toc_open;
        if self.toc_open {
            self.bookmarks_open = false;
        }
        self.refresh_side_count(cx);
        cx.notify();
    }

    fn toggle_bookmarks(&mut self, cx: &mut Context<Self>) {
        self.bookmarks_open = !self.bookmarks_open;
        if self.bookmarks_open {
            self.toc_open = false;
        }
        self.refresh_side_count(cx);
        cx.notify();
    }

    fn toggle_bookmark(&mut self, cx: &mut Context<Self>) {
        let index = self
            .list
            .logical_scroll_top()
            .item_ix
            .min(self.blocks.saturating_sub(1));
        let Some(block) = self
            .data
            .as_ref()
            .and_then(|data| data.chapter.blocks.get(index))
        else {
            return;
        };
        let mut label = String::new();
        let title = self.document.read(cx).chapter_title(self.chapter);
        if !title.is_empty() {
            label.push_str(&title);
            label.push_str(" · ");
        }
        label.extend(block.text.chars().take(90));
        let fraction = if self.blocks <= 1 {
            0.
        } else {
            index as f32 / (self.blocks - 1) as f32
        };
        self.document.update(cx, |document, cx| {
            document.toggle_bookmark(self.chapter, index, fraction, label, cx);
        });
        self.position = ReadingPosition {
            chapter: self.chapter,
            block: index,
            fraction,
        };
        self.refresh_side_count(cx);
        cx.notify();
    }

    fn current_bookmarked(&self, cx: &App) -> bool {
        let index = self.list.logical_scroll_top().item_ix;
        self.document
            .read(cx)
            .bookmarks()
            .iter()
            .any(|bookmark| bookmark.chapter == self.chapter && bookmark.block == index)
    }

    fn change_prefs(&mut self, change: impl FnOnce(&mut ReaderPrefs), cx: &mut Context<Self>) {
        let mut prefs = self.document.read(cx).prefs();
        change(&mut prefs);
        prefs.font_scale = prefs.font_scale.clamp(0.75, 2.25);
        prefs.line_height = prefs.line_height.clamp(1.2, 2.4);
        prefs.page_width_rem = prefs.page_width_rem.clamp(24., 80.);
        self.document
            .update(cx, |document, cx| document.set_prefs(prefs, cx));
        self.list.remeasure_items(0..self.blocks);
        cx.notify();
    }

    fn cycle_theme(&mut self, cx: &mut Context<Self>) {
        self.change_prefs(
            |prefs| {
                prefs.theme = match prefs.theme {
                    ReaderTheme::Application => ReaderTheme::Light,
                    ReaderTheme::Light => ReaderTheme::Dark,
                    ReaderTheme::Dark => ReaderTheme::Application,
                };
            },
            cx,
        );
    }

    fn reading_colors(&self, cx: &App) -> ReadingColors {
        self.palette.unwrap_or_else(|| ReadingColors::from_app(cx))
    }

    fn sync_mark_colors(&mut self, cx: &mut Context<Self>) {
        let colors = self.reading_colors(cx);
        let key = (colors.selection, colors.active);
        if self.mark_key == Some(key) {
            return;
        }
        self.mark_key = Some(key);
        self.mark_color = color_hex(colors.selection);
        self.active_color = color_hex(colors.active);
        self.states.borrow_mut().clear();
        self.ensure_states(self.visible.start, cx);
    }

    fn larger_font(&mut self, cx: &mut Context<Self>) {
        self.change_prefs(|prefs| prefs.font_scale += 0.1, cx);
    }

    fn smaller_font(&mut self, cx: &mut Context<Self>) {
        self.change_prefs(|prefs| prefs.font_scale -= 0.1, cx);
    }

    fn cycle_spacing(&mut self, cx: &mut Context<Self>) {
        let prefs = self.document.read(cx).prefs();
        let next = if prefs.line_height < 1.6 {
            1.7
        } else if prefs.line_height < 1.9 {
            2.0
        } else {
            1.5
        };
        self.change_prefs(move |prefs| prefs.line_height = next, cx);
    }

    fn cycle_width(&mut self, cx: &mut Context<Self>) {
        let prefs = self.document.read(cx).prefs();
        let next = if prefs.page_width_rem < 40. {
            44.
        } else if prefs.page_width_rem < 52. {
            56.
        } else {
            36.
        };
        self.change_prefs(move |prefs| prefs.page_width_rem = next, cx);
    }

    fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_open = true;
        self.document
            .update(cx, |document, cx| document.ensure_index(cx));
        window.focus(&self.search_input.read(cx).focus_handle(cx), cx);
        if self.pending_query.is_none() && !self.search_query.is_empty() {
            self.pending_query = Some(self.search_query.clone());
        }
        self.schedule_search_if_ready(cx);
        cx.notify();
    }

    fn toggle_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_open {
            self.close_search(window, cx);
        } else {
            self.open_search(window, cx);
        }
    }

    fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Results and marks stay for F3/Shift+F3 after the panel returns focus
        // to the book; a new query replaces them.
        self.search_open = false;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn query_changed(&mut self, cx: &mut Context<Self>) {
        let query = self.search_input.read(cx).value().to_string();
        if query == self.search_query {
            return;
        }
        self.search_query = query;
        self.results.clear();
        self.results_truncated = false;
        self.active_result = None;
        self.active_mark = None;
        self.marks.clear();
        self.marks_revision = self.marks_revision.wrapping_add(1);
        self.ensure_states(self.visible.start, cx);
        self.schedule_search_if_ready(cx);
        cx.notify();
    }

    fn schedule_search_if_ready(&mut self, cx: &mut Context<Self>) {
        let query = self.search_query.clone();
        if query.is_empty() {
            self.pending_query = None;
            return;
        }
        let ready = self.document.read(cx).index_text().is_some();
        if ready {
            self.pending_query = Some(query);
            self.schedule_search(cx);
        } else {
            self.pending_query = Some(query);
            self.document
                .update(cx, |document, cx| document.ensure_index(cx));
        }
    }

    fn schedule_search(&mut self, cx: &mut Context<Self>) {
        let Some(query) = self.pending_query.clone() else {
            return;
        };
        let Some(text) = self.document.read(cx).index_text() else {
            return;
        };
        self.pending_query = None;
        self.search_generation = self.search_generation.wrapping_add(1);
        let generation = self.search_generation;
        self.search_cancel.cancel();
        let cancel = CancellationToken::new();
        self.search_cancel = cancel.clone();
        let limits = Limits::default();
        let query_for_task = query.clone();
        cx.spawn(async move |this, cx| {
            let output = cx
                .background_spawn(async move {
                    book_search::search(&text, &query_for_task, &limits, &cancel)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.install_search(generation, query, output, cx)
            });
        })
        .detach();
        cx.notify();
    }

    fn install_search(
        &mut self,
        generation: u64,
        query: String,
        output: book_search::SearchOutput,
        cx: &mut Context<Self>,
    ) {
        if generation != self.search_generation || query != self.search_query {
            return;
        }
        self.results = output.hits;
        self.results_truncated = output.truncated;
        self.active_result = None;
        self.active_mark = None;
        self.rebuild_marks();
        self.ensure_states(self.visible.start, cx);
        cx.notify();
    }

    fn rebuild_marks(&mut self) {
        self.marks.clear();
        self.active_mark = None;
        for (index, hit) in self.results.iter().enumerate() {
            if hit.chapter != self.chapter {
                continue;
            }
            if Some(index) == self.active_result {
                self.active_mark = Some((hit.block, hit.range.clone()));
            } else {
                self.marks
                    .entry(hit.block)
                    .or_default()
                    .push(hit.range.clone());
            }
        }
        self.marks_revision = self.marks_revision.wrapping_add(1);
    }

    fn next_result(&mut self, cx: &mut Context<Self>) {
        if self.results.is_empty() {
            return;
        }
        let next = match self.active_result {
            Some(index) if index + 1 < self.results.len() => index + 1,
            Some(_) => 0,
            None => 0,
        };
        self.jump_to_result(next, cx);
    }

    fn previous_result(&mut self, cx: &mut Context<Self>) {
        if self.results.is_empty() {
            return;
        }
        let previous = match self.active_result {
            Some(0) | None => self.results.len() - 1,
            Some(index) => index - 1,
        };
        self.jump_to_result(previous, cx);
    }

    fn jump_to_result(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(hit) = self.results.get(index).cloned() else {
            return;
        };
        self.active_result = Some(index);
        self.rebuild_marks();
        if hit.chapter != self.chapter {
            self.go_to_chapter(hit.chapter, hit.block, None, cx);
        } else {
            self.list.scroll_to(gpui_kit::ListOffset {
                item_ix: hit.block,
                offset_in_item: px(0.),
            });
            self.ensure_states(hit.block, cx);
            self.position = ReadingPosition {
                chapter: self.chapter,
                block: hit.block,
                fraction: if self.blocks <= 1 {
                    0.
                } else {
                    hit.block as f32 / (self.blocks - 1) as f32
                },
            };
            cx.notify();
        }
    }

    fn scrolled(
        &mut self,
        event: &gpui_kit::ListScrollEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.visible = event.visible_range.clone();
        let blocks = self.blocks;
        if blocks == 0 {
            return;
        }
        let top = self.list.logical_scroll_top();
        let block = top.item_ix.min(blocks - 1);
        let fraction = if blocks <= 1 {
            0.
        } else {
            block as f32 / (blocks - 1) as f32
        };
        self.position = ReadingPosition {
            chapter: self.chapter,
            block,
            fraction,
        };
        let position = self.position;
        self.document
            .update(cx, |document, _| document.set_reading(position));
        self.ensure_states(event.visible_range.start, cx);
        if self.save_task.is_none() {
            self.save_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(SAVE_DEBOUNCE).await;
                let _ = this.update(cx, |view, cx| {
                    view.save_task = None;
                    view.commit_position(cx);
                });
            }));
        }
    }

    fn commit_position(&mut self, cx: &mut Context<Self>) {
        let position = self.position;
        self.document.update(cx, |document, cx| {
            document.set_reading(position);
            document.persist(cx);
        });
    }

    fn ensure_states(&mut self, center: usize, cx: &mut Context<Self>) {
        let Some(data) = self.data.clone() else {
            return;
        };
        if self.blocks == 0 {
            return;
        }
        let start = center.saturating_sub(4);
        let end = (center + 28).min(self.blocks);
        let keep_start = self.visible.start.saturating_sub(STATE_MARGIN);
        let keep_end = (self.visible.end + STATE_MARGIN).min(self.blocks);
        let mut states = self.states.borrow_mut();
        states.retain(|index, _| *index >= keep_start && *index < keep_end.max(end));
        for index in start..end {
            let revision = self.block_revision(index);
            if states
                .get(&index)
                .is_some_and(|state| state.revision == revision)
            {
                continue;
            }
            let html = self.block_html(&data, index);
            let entity = cx.new(|cx| TextViewState::html(&html, cx));
            let subscription = cx.observe(&entity, |_, _, cx| cx.notify());
            states.insert(
                index,
                BlockState {
                    entity,
                    revision,
                    _subscription: subscription,
                },
            );
        }
    }

    fn block_revision(&self, index: usize) -> u64 {
        if self.marks.contains_key(&index)
            || self
                .active_mark
                .as_ref()
                .is_some_and(|(block, _)| *block == index)
        {
            self.marks_revision
        } else {
            0
        }
    }

    fn block_html(&self, data: &ChapterData, index: usize) -> String {
        let Some(block) = data.chapter.blocks.get(index) else {
            return String::new();
        };
        if let Some((block_index, range)) = &self.active_mark
            && *block_index == index
        {
            return html_with_marks(block, std::slice::from_ref(range), &self.active_color);
        }
        if let Some(ranges) = self.marks.get(&index) {
            return html_with_marks(block, ranges, &self.mark_color);
        }
        block.html.clone()
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let prefs = self.document.read(cx).prefs();
        let marked = self.current_bookmarked(cx);
        let chapter = self.chapter_label(cx);
        h_flex()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap_1()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                Button::new("book-contents")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Menu)
                    .label(t(L10nKey::EditorBookContents))
                    .selected(self.toc_open)
                    .accessibility_label(t(L10nKey::EditorBookContents))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_contents(cx))),
            )
            .child(
                Button::new("book-bookmarks")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Star)
                    .label(t(L10nKey::EditorBookBookmarks))
                    .selected(self.bookmarks_open)
                    .accessibility_label(t(L10nKey::EditorBookBookmarks))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_bookmarks(cx))),
            )
            .child(
                Button::new("book-search")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Search)
                    .label(t(L10nKey::EditorBookSearch))
                    .selected(self.search_open)
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_search(window, cx))),
            )
            .child(
                Button::new("book-bookmark")
                    .ghost()
                    .xsmall()
                    .icon(if marked {
                        IconName::StarFill
                    } else {
                        IconName::Star
                    })
                    .selected(marked)
                    .accessibility_label(t(if marked {
                        L10nKey::EditorBookRemoveBookmark
                    } else {
                        L10nKey::EditorBookAddBookmark
                    }))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_bookmark(cx))),
            )
            .child(div().flex_1().min_w_0())
            .child(
                Button::new("book-font-smaller")
                    .ghost()
                    .xsmall()
                    .label("A−")
                    .accessibility_label(t(L10nKey::EditorBookFontSmaller))
                    .on_click(cx.listener(|this, _, _, cx| this.smaller_font(cx))),
            )
            .child(
                div()
                    .id("book-font-size")
                    .test_support()
                    .text_xs()
                    .child(format!("{:.0}%", prefs.font_scale * 100.)),
            )
            .child(
                Button::new("book-font-larger")
                    .ghost()
                    .xsmall()
                    .label("A+")
                    .accessibility_label(t(L10nKey::EditorBookFontLarger))
                    .on_click(cx.listener(|this, _, _, cx| this.larger_font(cx))),
            )
            .child(
                Button::new("book-spacing")
                    .ghost()
                    .xsmall()
                    .label(t_fmt(
                        L10nKey::EditorBookSpacing,
                        &[("value", &format!("{:.1}", prefs.line_height))],
                    ))
                    .on_click(cx.listener(|this, _, _, cx| this.cycle_spacing(cx))),
            )
            .child(
                Button::new("book-width")
                    .ghost()
                    .xsmall()
                    .label(t_fmt(
                        L10nKey::EditorBookWidth,
                        &[("value", &format!("{:.0}", prefs.page_width_rem))],
                    ))
                    .on_click(cx.listener(|this, _, _, cx| this.cycle_width(cx))),
            )
            .child(
                Button::new("book-theme")
                    .ghost()
                    .xsmall()
                    .label(t(match prefs.theme {
                        ReaderTheme::Application => L10nKey::EditorBookThemeApplication,
                        ReaderTheme::Light => L10nKey::EditorBookThemeLight,
                        ReaderTheme::Dark => L10nKey::EditorBookThemeDark,
                    }))
                    .on_click(cx.listener(|this, _, _, cx| this.cycle_theme(cx))),
            )
            .child(
                div()
                    .id("book-chapter-status")
                    .test_support()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(chapter),
            )
            .into_any_element()
    }

    fn render_search(&self, cx: &mut Context<Self>) -> AnyElement {
        let count = if self.results.is_empty() {
            if let IndexState::Failed(message) = self.document.read(cx).index_state() {
                message.to_string()
            } else if self.pending_query.is_some() {
                t(L10nKey::EditorBookSearching).to_string()
            } else {
                t(L10nKey::EditorBookSearchEmpty).to_string()
            }
        } else {
            let current = self.active_result.map_or(0, |index| index + 1);
            let mut label = t_fmt(
                L10nKey::EditorBookMatches,
                &[
                    ("current", &current.to_string()),
                    ("total", &self.results.len().to_string()),
                ],
            );
            if self.results_truncated {
                label.push(' ');
                label.push_str(t(L10nKey::EditorBookMatchesTruncated));
            }
            label
        };
        h_flex()
            .flex_none()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .id("book-search-field")
                    .flex_1()
                    .min_w_0()
                    .on_key_down(cx.listener(|this, event: &gpui_kit::KeyDownEvent, _, cx| {
                        if event.keystroke.key == "escape" {
                            this.search_open = false;
                            this.active_result = None;
                            this.active_mark = None;
                            this.marks.clear();
                            this.marks_revision = this.marks_revision.wrapping_add(1);
                            this.results.clear();
                            this.ensure_states(this.visible.start, cx);
                            cx.notify();
                            cx.stop_propagation();
                        }
                    }))
                    .child(
                        Input::new(&self.search_input)
                            .id("book-search-input")
                            .small(),
                    ),
            )
            .child(
                div()
                    .id("book-search-count")
                    .test_support()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(count),
            )
            .child(
                Button::new("book-search-previous")
                    .ghost()
                    .xsmall()
                    .icon(IconName::ChevronLeft)
                    .disabled(self.results.is_empty())
                    .accessibility_label(t(L10nKey::EditorBookSearchPrevious))
                    .on_click(cx.listener(|this, _, _, cx| this.previous_result(cx))),
            )
            .child(
                Button::new("book-search-next")
                    .ghost()
                    .xsmall()
                    .icon(IconName::ChevronRight)
                    .disabled(self.results.is_empty())
                    .accessibility_label(t(L10nKey::EditorBookSearchNext))
                    .on_click(cx.listener(|this, _, _, cx| this.next_result(cx))),
            )
            .child(
                Button::new("book-search-close")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .accessibility_label(t(L10nKey::EditorBookSearchClose))
                    .on_click(cx.listener(|this, _, window, cx| this.close_search(window, cx))),
            )
            .into_any_element()
    }

    fn render_side(&self, cx: &mut Context<Self>) -> AnyElement {
        let rows = Rc::new(self.side_rows(cx));
        let list = self.side_list.clone();
        let tx = self.intent_tx.clone();
        let remove_tx = tx.clone();
        let bookmarks = self.bookmarks_open;
        let rows_render = Rc::clone(&rows);
        let body = gpui_kit::list(list.clone(), move |index, _, _| {
            let Some(row) = rows_render.get(index) else {
                return div().into_any_element();
            };
            let intent = if let Some(bookmark) = row.bookmark {
                Intent::Bookmark(bookmark)
            } else if let Some(href) = &row.href {
                Intent::Contents(href.clone())
            } else {
                Intent::Follow(String::new())
            };
            let tx = tx.clone();
            let mut element = h_flex()
                .id(("book-side-row", index))
                .w_full()
                .min_w_0()
                .pl(rems(0.25 + row.depth as f32 * 0.75))
                .pr_1()
                .child(
                    Button::new(("book-side-item", index))
                        .ghost()
                        .xsmall()
                        .flex_1()
                        .min_w_0()
                        .label(row.label.clone())
                        .on_click(move |_, _, _| {
                            let _ = tx.send_blocking(intent.clone());
                        }),
                );
            if bookmarks && let Some(bookmark) = row.bookmark {
                let remove = remove_tx.clone();
                element = element.child(
                    Button::new(("book-side-remove", bookmark))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Close)
                        .accessibility_label(t(L10nKey::EditorBookRemoveBookmark))
                        .on_click(move |_, _, _| {
                            let _ = remove.send_blocking(Intent::DeleteBookmark(bookmark));
                        }),
                );
            }
            element.into_any_element()
        })
        .size_full();
        let scrollbar = crate::ui::scrollbar::with_vertical_scrollbar(
            "book-side-scrollbar",
            body,
            &self.side_list,
        );
        v_flex()
            .id("book-side")
            .flex_none()
            .w(rems(18.))
            .h_full()
            .min_h_0()
            .border_r_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .p_2()
                    .child(
                        Button::new("book-side-contents")
                            .ghost()
                            .xsmall()
                            .label(t(L10nKey::EditorBookContents))
                            .selected(!self.bookmarks_open)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.bookmarks_open = false;
                                this.toc_open = true;
                                this.refresh_side_count(cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("book-side-bookmarks")
                            .ghost()
                            .xsmall()
                            .label(t(L10nKey::EditorBookBookmarks))
                            .selected(self.bookmarks_open)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.bookmarks_open = true;
                                this.toc_open = false;
                                this.refresh_side_count(cx);
                                cx.notify();
                            })),
                    ),
            )
            .child({
                if self.side_count == 0 && self.bookmarks_open {
                    div()
                        .p_3()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(t(L10nKey::EditorBookNoBookmarks))
                        .into_any_element()
                } else {
                    v_flex()
                        .flex_1()
                        .min_h_0()
                        .child(scrollbar)
                        .into_any_element()
                }
            })
            .into_any_element()
    }

    fn render_notices(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let mut notices = Vec::new();
        if let Some(data) = &self.data {
            let omissions = data.chapter.omissions.clone();
            let missing = omissions.missing_images + data.unavailable.len();
            if omissions.total() > 0 || missing > 0 {
                notices.push(
                    t_fmt(
                        L10nKey::EditorBookOmissions,
                        &[
                            ("scripts", &omissions.scripts.to_string()),
                            ("styles", &omissions.styles.to_string()),
                            ("remote", &omissions.remote_images.to_string()),
                            ("links", &omissions.blocked_links.to_string()),
                            ("images", &missing.to_string()),
                        ],
                    )
                    .to_string(),
                );
            }
        }
        if let Some(error) = self.document.read(cx).save_error() {
            notices.push(error.to_string());
        }
        if notices.is_empty() {
            return None;
        }
        Some(
            v_flex()
                .id("book-notices")
                .flex_none()
                .gap_1()
                .px_3()
                .py_1()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .children(notices.into_iter().map(|notice| {
                    div()
                        .id(SharedString::from(format!("book-notice-{notice}")))
                        .child(notice)
                }))
                .into_any_element(),
        )
    }

    fn render_content(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.loading {
            return v_flex()
                .flex_1()
                .min_h_0()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(t(L10nKey::EditorBookLoadingChapter))
                .into_any_element();
        }
        if let Some(error) = &self.chapter_error {
            let retry = self.document.clone();
            let chapter = self.chapter;
            return v_flex()
                .flex_1()
                .min_h_0()
                .items_center()
                .justify_center()
                .gap_3()
                .p_8()
                .child(
                    div()
                        .id("book-chapter-error")
                        .test_support()
                        .text_sm()
                        .child(error.clone()),
                )
                .child(
                    Button::new("book-chapter-retry")
                        .label(t(L10nKey::EditorBookRetry))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            retry.update(cx, |document, cx| document.retry_chapter(chapter, cx));
                            this.loading = true;
                            this.chapter_error = None;
                            cx.notify();
                        })),
                )
                .into_any_element();
        }
        let Some(data) = self.data.clone() else {
            return div().flex_1().into_any_element();
        };
        let states = Rc::clone(&self.states);
        let list = self.list.clone();
        let resolver = data.resolver.clone();
        let tx = self.intent_tx.clone();
        let font = self.document.read(cx).prefs().font_scale;
        let line = self.document.read(cx).prefs().line_height;
        let width = self.document.read(cx).prefs().page_width_rem;
        let colors = self.reading_colors(cx);
        let style = reader_style(colors);
        let font_family = cx.theme().font_family.clone();
        let color = colors.muted_foreground;
        let rows = gpui_kit::list(list.clone(), move |index, _, _| {
            let Some(block) = data.chapter.blocks.get(index) else {
                return div().into_any_element();
            };
            let resolver = resolver.clone();
            let tx = tx.clone();
            let font_family = font_family.clone();
            let style = style.clone();
            let body = if let Some(state) = states
                .borrow()
                .get(&index)
                .map(|state| state.entity.clone())
            {
                TextView::new(&state)
                    .selectable(true)
                    .w_full()
                    .font_family(font_family)
                    .text_size(rems(font))
                    .line_height(relative(line))
                    .style(style)
                    .image_source(move |uri| resolver.source(uri.as_ref()))
                    .on_link_click(move |href, _, _, _| {
                        let _ = tx.send_blocking(Intent::Follow(href.to_string()));
                    })
                    .into_any_element()
            } else {
                div()
                    .text_size(rems(font))
                    .line_height(relative(line))
                    .text_color(color)
                    .child(block.text.clone())
                    .into_any_element()
            };
            div()
                .id((
                    "book-block",
                    ((data.chapter.index as u64) << 32) | index as u64,
                ))
                .w_full()
                .pb_2()
                .child(
                    div()
                        .w_full()
                        .max_w(rems(width))
                        .mx_auto()
                        .px_6()
                        .child(body),
                )
                .into_any_element()
        })
        .size_full();
        let scrollbar = crate::ui::scrollbar::with_vertical_scrollbar(
            "book-content-scrollbar",
            rows,
            &self.list,
        );
        v_flex()
            .bg(colors.background)
            .text_color(colors.foreground)
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .child(scrollbar)
            .into_any_element()
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let chapter = self.chapter_label(cx);
        let progress = self.progress_label(cx);
        h_flex()
            .flex_none()
            .items_center()
            .gap_2()
            .px_3()
            .py_1()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                Button::new("book-previous-page")
                    .ghost()
                    .xsmall()
                    .icon(IconName::ChevronLeft)
                    .accessibility_label(t(L10nKey::EditorBookPreviousPage))
                    .on_click(cx.listener(|this, _, _, cx| this.previous_page(cx))),
            )
            .child(
                div()
                    .id("book-footer-position")
                    .test_support()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("{chapter} · {progress}")),
            )
            .child(
                Button::new("book-next-page")
                    .ghost()
                    .xsmall()
                    .icon(IconName::ChevronRight)
                    .accessibility_label(t(L10nKey::EditorBookNextPage))
                    .on_click(cx.listener(|this, _, _, cx| this.next_page(cx))),
            )
            .child(div().flex_1().min_w_0())
            .child(
                div()
                    .id("book-readonly")
                    .test_support()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(t(L10nKey::EditorBookReadOnly)),
            )
            .into_any_element()
    }

    fn render_unavailable(&self, message: &SharedString, cx: &mut Context<Self>) -> AnyElement {
        let document = self.document.read(cx);
        v_flex()
            .id("book-unavailable")
            .test_support()
            .flex_1()
            .min_h_0()
            .items_center()
            .justify_center()
            .gap_3()
            .p_8()
            .child(
                Icon::new(IconName::BookOpen)
                    .large()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(div().text_base().child(document.title()))
            .when(!document.author_line().is_empty(), |column| {
                column.child(div().text_sm().child(document.author_line()))
            })
            .child(
                div()
                    .id("book-unavailable-message")
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(message.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.document.read(cx).path().display().to_string()),
            )
            .into_any_element()
    }
}

/// Kit's `<mark>` only accepts CSS hex or four names, so a semantic theme
/// color is converted here instead of leaking a raw palette value into the
/// book HTML. Alpha is included when the token is translucent.
fn color_hex(color: gpui_kit::Hsla) -> String {
    let rgba = gpui_kit::Rgba::from(color);
    let channel = |value: f32| (value.clamp(0., 1.) * 255.).round() as u8;
    if rgba.a < 1. {
        format!(
            "#{:02x}{:02x}{:02x}{:02x}",
            channel(rgba.r),
            channel(rgba.g),
            channel(rgba.b),
            channel(rgba.a)
        )
    } else {
        format!(
            "#{:02x}{:02x}{:02x}",
            channel(rgba.r),
            channel(rgba.g),
            channel(rgba.b)
        )
    }
}

fn reader_style(colors: ReadingColors) -> TextViewStyle {
    TextViewStyle::default()
        .with_dark(colors.background.l < colors.foreground.l)
        .with_foreground(colors.foreground)
        .with_muted_foreground(colors.muted_foreground)
        .with_link(colors.link)
        .with_selection(colors.selection)
        .with_border(colors.border)
        .with_code_background(colors.code_background)
        .with_paragraph_gap(rems(0.9))
}

#[derive(Clone, Copy)]
struct ReadingColors {
    background: gpui_kit::Hsla,
    foreground: gpui_kit::Hsla,
    muted_foreground: gpui_kit::Hsla,
    link: gpui_kit::Hsla,
    selection: gpui_kit::Hsla,
    border: gpui_kit::Hsla,
    code_background: gpui_kit::Hsla,
    active: gpui_kit::Hsla,
}

impl ReadingColors {
    fn from_app(cx: &App) -> Self {
        let theme = cx.theme();
        Self {
            background: theme.background,
            foreground: theme.foreground,
            muted_foreground: theme.muted_foreground,
            link: theme.primary,
            selection: theme.selection,
            border: theme.border,
            code_background: theme.muted,
            active: theme.warning.opacity(0.25),
        }
    }
}

fn reading_palette(mode: ReaderTheme, cx: &App) -> Option<ReadingColors> {
    use crate::ui::presets;
    let id = match mode {
        ReaderTheme::Application => return None,
        ReaderTheme::Light => "light",
        ReaderTheme::Dark => "dark",
    };
    let fallback;
    let preset = match cx
        .try_global::<presets::Themes>()
        .and_then(|themes| themes.0.iter().find(|theme| theme.id == id))
    {
        Some(preset) => preset,
        None => {
            fallback = presets::by_id(cx, id);
            &fallback
        }
    };
    let neutrals = preset.neutrals();
    let semantics = preset.semantics();
    Some(ReadingColors {
        background: gpui_kit::rgb(neutrals.background).into(),
        foreground: gpui_kit::rgb(neutrals.foreground).into(),
        muted_foreground: gpui_kit::rgb(neutrals.muted_foreground).into(),
        link: gpui_kit::rgb(semantics.link.ink).into(),
        selection: gpui_kit::rgb(neutrals.selection).into(),
        border: gpui_kit::rgb(neutrals.border).into(),
        code_background: gpui_kit::rgb(neutrals.muted).into(),
        active: gpui_kit::rgb(semantics.warning.fill).into(),
    })
}

impl Drop for BookView {
    fn drop(&mut self) {
        self.search_cancel.cancel();
    }
}

impl Focusable for BookView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for BookView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_mark_colors(cx);
        let state = self.document.read(cx).state().clone();
        let title = self.document.read(cx).title();
        let author = self.document.read(cx).author_line();
        let body = match state {
            BookState::Unsupported(message) | BookState::Failed(message) => {
                self.render_unavailable(&message, cx)
            }
            BookState::Ready(_) => {
                let notices = self.render_notices(cx);
                let side_open = self.toc_open || self.bookmarks_open;
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .when_some(notices, |column, notices| column.child(notices))
                    .child(
                        h_flex()
                            .items_stretch()
                            .flex_1()
                            .min_h_0()
                            .min_w_0()
                            .when(side_open, |row| row.child(self.render_side(cx)))
                            .child(self.render_content(cx)),
                    )
                    .child(self.render_footer(cx))
                    .into_any_element()
            }
        };
        v_flex()
            .id("book-document")
            .test_support()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .min_h_0()
            .min_w_0()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if !this.focus.contains_focused(window, cx) {
                        window.focus(&this.focus, cx);
                    }
                }),
            )
            .on_action(cx.listener(|this, _: &BookNextPage, _, cx| this.next_page(cx)))
            .on_action(cx.listener(|this, _: &BookPreviousPage, _, cx| this.previous_page(cx)))
            .on_action(cx.listener(|this, _: &BookNextChapter, _, cx| this.next_chapter(cx)))
            .on_action(
                cx.listener(|this, _: &BookPreviousChapter, _, cx| this.previous_chapter(cx)),
            )
            .on_action(cx.listener(|this, _: &BookToggleContents, _, cx| this.toggle_contents(cx)))
            .on_action(
                cx.listener(|this, _: &BookToggleBookmarks, _, cx| this.toggle_bookmarks(cx)),
            )
            .on_action(cx.listener(|this, _: &BookSearch, window, cx| this.open_search(window, cx)))
            .on_action(cx.listener(|this, _: &BookNextResult, _, cx| this.next_result(cx)))
            .on_action(cx.listener(|this, _: &BookPreviousResult, _, cx| this.previous_result(cx)))
            .on_action(cx.listener(|this, _: &BookAddBookmark, _, cx| this.toggle_bookmark(cx)))
            .on_action(cx.listener(|this, _: &BookFontLarger, _, cx| this.larger_font(cx)))
            .on_action(cx.listener(|this, _: &BookFontSmaller, _, cx| this.smaller_font(cx)))
            .on_action(cx.listener(|this, _: &BookCycleSpacing, _, cx| this.cycle_spacing(cx)))
            .on_action(cx.listener(|this, _: &BookCycleWidth, _, cx| this.cycle_width(cx)))
            .on_action(cx.listener(|this, _: &BookCycleTheme, _, cx| this.cycle_theme(cx)))
            .child(
                v_flex()
                    .flex_none()
                    .gap_0()
                    .px_3()
                    .pt_2()
                    .pb_1()
                    .child(
                        div()
                            .id("book-title")
                            .test_support()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .truncate()
                            .child(title),
                    )
                    .when(!author.is_empty(), |column| {
                        column.child(
                            div()
                                .id("book-author")
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .truncate()
                                .child(author),
                        )
                    }),
            )
            .child(self.render_toolbar(cx))
            .when(self.search_open, |column| {
                column.child(self.render_search(cx))
            })
            .child(body)
            .into_any_element()
    }
}
