//! 工作台编辑 interface：文档文本与 presentation 状态分别读取。
use super::book_view::BookView;
use super::drawio_document::DrawioView;
use super::image_document::ImageView;
use super::pdf_document::PdfView;
use super::sqlite_document::SqliteView;
use super::text::TextSnapshot;
use super::xml_document::XmlView;
use super::*;
use lyrel_markdown_editor::{Document, EditorState as MarkdownEditor};
use std::cell::RefCell;

#[derive(Clone)]
pub(crate) enum EditorInput {
    Code(Entity<EditorState>),
    Markdown(Entity<MarkdownEditor>),
    /// 只读静态图片：没有文本，只有取景。
    Image(Entity<ImageView>),
    /// 只读 SQLite 数据库：没有文本，只有表结构与分页数据。
    Sqlite(Entity<SqliteView>),
    /// 只读 EPUB：章节、目录、搜索与阅读位置由书文档拥有。
    Book(Entity<BookView>),
    /// 只读 PDF：没有文本编辑，只有页、缩放与密码。
    Pdf(Entity<PdfView>),
    /// SVG 文档：共享逐行保真 Document 的源码与安全原生预览。
    Svg(Entity<SvgView>),
    /// JSON 家族：共享逐行保真 Document 的源码与无损结构/记录视图。
    Json(Entity<JsonView>),
    /// CSV／TSV 文档：共享逐行保真 Document 的源码与虚拟化表格。
    Csv(Entity<CsvView>),
    /// XML 文档：共享逐行保真 Document 的源码与有序、命名空间感知的结构树。
    Xml(Entity<XmlView>),
    /// draw.io 图：字节与页表属于文档，渲染只属于固定离线引擎。
    Drawio(Entity<DrawioView>),
}

/// 一份文档输入的文本能力：代码与 Markdown 有文本，图片没有。
///
/// 它是借用视图，不持有第二份状态：所有文本读写都直接落到原实体上。
/// `EditorInput::text()` 是唯一的取得方式，所以"对图片做文本操作"在类型上
/// 就不成立，而不是一个静默失败的空操作。
#[derive(Clone, Copy)]
pub(crate) enum TextInput<'a> {
    Code(&'a Entity<EditorState>),
    Markdown(&'a Entity<MarkdownEditor>),
    Svg(&'a Entity<SvgView>),
    Json(&'a Entity<JsonView>),
    Csv(&'a Entity<CsvView>),
    Xml(&'a Entity<XmlView>),
}
#[derive(Clone)]
pub(crate) enum WeakEditorInput {
    Code(gpui_kit::WeakEntity<EditorState>),
    Markdown(gpui_kit::WeakEntity<MarkdownEditor>),
    Image(gpui_kit::WeakEntity<ImageView>),
    Sqlite(gpui_kit::WeakEntity<SqliteView>),
    Book(gpui_kit::WeakEntity<BookView>),
    Pdf(gpui_kit::WeakEntity<PdfView>),
    Svg(gpui_kit::WeakEntity<SvgView>),
    Json(gpui_kit::WeakEntity<JsonView>),
    Csv(gpui_kit::WeakEntity<CsvView>),
    Xml(gpui_kit::WeakEntity<XmlView>),
    Drawio(gpui_kit::WeakEntity<DrawioView>),
}
impl WeakEditorInput {
    pub(crate) fn upgrade(&self) -> Option<EditorInput> {
        match self {
            Self::Code(input) => input.upgrade().map(EditorInput::Code),
            Self::Markdown(input) => input.upgrade().map(EditorInput::Markdown),
            Self::Image(input) => input.upgrade().map(EditorInput::Image),
            Self::Sqlite(input) => input.upgrade().map(EditorInput::Sqlite),
            Self::Book(input) => input.upgrade().map(EditorInput::Book),
            Self::Pdf(input) => input.upgrade().map(EditorInput::Pdf),
            Self::Svg(input) => input.upgrade().map(EditorInput::Svg),
            Self::Json(input) => input.upgrade().map(EditorInput::Json),
            Self::Csv(input) => input.upgrade().map(EditorInput::Csv),
            Self::Xml(input) => input.upgrade().map(EditorInput::Xml),
            Self::Drawio(input) => input.upgrade().map(EditorInput::Drawio),
        }
    }
}
impl EditorInput {
    pub(crate) fn downgrade(&self) -> WeakEditorInput {
        match self {
            Self::Code(input) => WeakEditorInput::Code(input.downgrade()),
            Self::Markdown(input) => WeakEditorInput::Markdown(input.downgrade()),
            Self::Image(input) => WeakEditorInput::Image(input.downgrade()),
            Self::Sqlite(input) => WeakEditorInput::Sqlite(input.downgrade()),
            Self::Book(input) => WeakEditorInput::Book(input.downgrade()),
            Self::Pdf(input) => WeakEditorInput::Pdf(input.downgrade()),
            Self::Svg(input) => WeakEditorInput::Svg(input.downgrade()),
            Self::Json(input) => WeakEditorInput::Json(input.downgrade()),
            Self::Csv(input) => WeakEditorInput::Csv(input.downgrade()),
            Self::Xml(input) => WeakEditorInput::Xml(input.downgrade()),
            Self::Drawio(input) => WeakEditorInput::Drawio(input.downgrade()),
        }
    }
    pub(crate) fn code(&self) -> Option<&Entity<EditorState>> {
        match self {
            Self::Code(input) => Some(input),
            _ => None,
        }
    }
    pub(crate) fn markdown(&self) -> Option<&Entity<MarkdownEditor>> {
        match self {
            Self::Markdown(input) => Some(input),
            _ => None,
        }
    }
    pub(crate) fn image(&self) -> Option<&Entity<ImageView>> {
        match self {
            Self::Image(input) => Some(input),
            _ => None,
        }
    }
    pub(crate) fn sqlite(&self) -> Option<&Entity<SqliteView>> {
        match self {
            Self::Sqlite(input) => Some(input),
            _ => None,
        }
    }
    pub(crate) fn book(&self) -> Option<&Entity<BookView>> {
        match self {
            Self::Book(input) => Some(input),
            _ => None,
        }
    }
    pub(crate) fn pdf(&self) -> Option<&Entity<PdfView>> {
        match self {
            Self::Pdf(input) => Some(input),
            _ => None,
        }
    }
    pub(crate) fn svg(&self) -> Option<&Entity<SvgView>> {
        match self {
            Self::Svg(input) => Some(input),
            _ => None,
        }
    }
    pub(crate) fn json(&self) -> Option<&Entity<JsonView>> {
        match self {
            Self::Json(input) => Some(input),
            _ => None,
        }
    }
    pub(crate) fn csv(&self) -> Option<&Entity<CsvView>> {
        match self {
            Self::Csv(input) => Some(input),
            _ => None,
        }
    }
    pub(crate) fn xml(&self) -> Option<&Entity<XmlView>> {
        match self {
            Self::Xml(input) => Some(input),
            _ => None,
        }
    }
    pub(crate) fn drawio(&self) -> Option<&Entity<DrawioView>> {
        match self {
            Self::Drawio(input) => Some(input),
            _ => None,
        }
    }
    /// The diagram's owning document; None for every other input. Saves and
    /// the dirty sweep read its bytes and edit generation.
    pub(crate) fn drawio_document<'a>(
        &self,
        cx: &'a gpui_kit::App,
    ) -> Option<&'a super::drawio_document::DrawioDocument> {
        self.drawio().map(|view| view.read(cx).document().read(cx))
    }
    /// Markdown, SVG, the JSON family and CSV keep the same line-preserving
    /// source authority; every save, reload and dirty comparison reads this
    /// document rather than a presentation's cached text.
    pub(crate) fn source_document(&self, cx: &gpui_kit::App) -> Option<Entity<Document>> {
        match self {
            Self::Markdown(view) => Some(view.read(cx).document().clone()),
            Self::Svg(view) => Some(view.read(cx).document().read(cx).document().clone()),
            Self::Json(view) => Some(view.read(cx).document().read(cx).document().clone()),
            Self::Csv(view) => Some(view.read(cx).document().read(cx).document().clone()),
            Self::Xml(view) => Some(view.read(cx).document().read(cx).document().clone()),
            Self::Code(_)
            | Self::Image(_)
            | Self::Pdf(_)
            | Self::Book(_)
            | Self::Sqlite(_)
            | Self::Drawio(_) => None,
        }
    }
    /// 这份输入的文本能力；只读图片返回 `None`。
    ///
    /// 需要文本的调用点从这里取能力再操作，而不是读一个伪造的空文档。
    pub(crate) fn text(&self) -> Option<TextInput<'_>> {
        match self {
            Self::Code(input) => Some(TextInput::Code(input)),
            Self::Markdown(input) => Some(TextInput::Markdown(input)),
            Self::Svg(input) => Some(TextInput::Svg(input)),
            Self::Json(input) => Some(TextInput::Json(input)),
            Self::Csv(input) => Some(TextInput::Csv(input)),
            Self::Xml(input) => Some(TextInput::Xml(input)),
            Self::Image(_) | Self::Pdf(_) | Self::Book(_) | Self::Sqlite(_) | Self::Drawio(_) => {
                None
            }
        }
    }
    /// 有文本可编辑、可查找、可保存的输入。图片和 draw.io 图不是。
    pub(crate) fn is_text(&self) -> bool {
        self.text().is_some()
    }
    /// Inputs the workbench's save lifecycle writes: every text document and
    /// the draw.io engine document, whose bytes the engine commits back. A
    /// picture, database, book or PDF stays read-only.
    pub(crate) fn is_savable(&self) -> bool {
        self.is_text() || self.drawio().is_some()
    }
    /// The generation a save captures and re-checks after the write lands.
    /// Source-authority text uses its document revision; a draw.io document
    /// uses its edit generation, which engine commits advance.
    pub(crate) fn source_edit_revision(&self, cx: &gpui_kit::App) -> Option<u64> {
        if let Some(document) = self.source_document(cx) {
            return Some(document.read(cx).revision());
        }
        self.drawio()
            .map(|view| view.read(cx).document().read(cx).edit_revision())
    }
    pub(crate) fn finish_composition(&self, _: &mut Window, cx: &mut gpui_kit::App) {
        if let Some(input) = self.markdown() {
            input.update(cx, |state, cx| state.finish_composition(cx));
        } else if let Some(view) = self.svg() {
            view.update(cx, |view, cx| view.finish_composition(cx));
        } else if let Some(view) = self.json() {
            view.update(cx, |view, cx| view.finish_composition(cx));
        } else if let Some(view) = self.csv() {
            view.update(cx, |view, cx| view.finish_composition(cx));
        } else if let Some(view) = self.xml() {
            view.update(cx, |view, cx| view.finish_composition(cx));
        }
    }
    pub(crate) fn read<'a>(&'a self, cx: &'a gpui_kit::App) -> InputRead<'a> {
        InputRead { input: self, cx }
    }
    pub(crate) fn focus(&self, window: &mut Window, cx: &mut gpui_kit::App) {
        let focus = self.read(cx).focus_handle(cx);
        if !focus.contains_focused(window, cx) {
            self.finish_composition(window, cx);
            focus.focus(window, cx);
        }
    }
    /// 取景与文本共用的滚动：图片的取景就是它的偏移。
    pub(crate) fn set_scroll_offset(
        &self,
        offset: gpui_kit::Point<gpui_kit::Pixels>,
        cx: &mut gpui_kit::App,
    ) {
        match self {
            Self::Code(input) => input.update(cx, |state, cx| state.set_scroll_offset(offset, cx)),
            Self::Markdown(input) => {
                input.update(cx, |state, cx| state.set_scroll_offset(offset, cx))
            }
            Self::Image(input) => input.update(cx, |view, cx| {
                view.scroll().set_offset(offset);
                cx.notify();
            }),
            // 表格的滚动属于呈现自己的 `TableState`；打开文档时的光标放置
            // 不适用于数据库，因此这里不假装有第二个滚动所有者。
            Self::Sqlite(_) => {}
            // A book's position is block/chapter state stored in its document,
            // not a free pixel offset that a tab restore could impose.
            Self::Book(_) => {}
            Self::Pdf(input) => input.update(cx, |view, cx| {
                view.scroll().set_offset(offset);
                cx.notify();
            }),
            Self::Svg(view) => view.update(cx, |view, cx| view.set_scroll_offset(offset, cx)),
            Self::Json(view) => view.update(cx, |view, cx| view.set_scroll_offset(offset, cx)),
            Self::Csv(view) => view.update(cx, |view, cx| view.set_scroll_offset(offset, cx)),
            Self::Xml(view) => view.update(cx, |view, cx| view.set_scroll_offset(offset, cx)),
            // The engine owns its pan/zoom view; the workbench has no second
            // scroll axis to synchronise.
            Self::Drawio(_) => {}
        }
    }
    pub(crate) fn set_highlighter(&self, language: &'static str, cx: &mut gpui_kit::App) {
        if let Some(input) = self.code() {
            input.update(cx, |state, cx| state.set_highlighter(language, cx));
        }
    }
}
impl TextInput<'_> {
    pub(crate) fn read<'a>(&'a self, cx: &'a gpui_kit::App) -> TextRead<'a> {
        TextRead { input: *self, cx }
    }
    pub(crate) fn set_soft_wrap(&self, wrap: bool, window: &mut Window, cx: &mut gpui_kit::App) {
        match self {
            Self::Code(input) => {
                input.update(cx, |state, cx| state.set_soft_wrap(wrap, window, cx))
            }
            Self::Markdown(input) => input.update(cx, |state, cx| state.set_soft_wrap(wrap, cx)),
            Self::Svg(view) => view.update(cx, |view, cx| view.set_soft_wrap(wrap, cx)),
            Self::Json(view) => view.update(cx, |view, cx| view.set_soft_wrap(wrap, cx)),
            Self::Csv(view) => view.update(cx, |view, cx| view.set_soft_wrap(wrap, cx)),
            Self::Xml(view) => view.update(cx, |view, cx| view.set_soft_wrap(wrap, cx)),
        }
    }
    /// Whether this presentation keeps the shared line-preserving source and
    /// its history (Markdown, SVG, the JSON family and CSV), rather than the
    /// ordinary code codec.
    pub(crate) fn is_source(&self) -> bool {
        matches!(
            self,
            Self::Markdown(_) | Self::Svg(_) | Self::Json(_) | Self::Csv(_) | Self::Xml(_)
        )
    }
    pub(crate) fn set_selected_range(&self, range: std::ops::Range<usize>, cx: &mut gpui_kit::App) {
        match self {
            Self::Code(input) => input.update(cx, |state, cx| state.set_selected_range(range, cx)),
            Self::Markdown(input) => input.update(cx, |state, cx| state.select_range(range, cx)),
            Self::Svg(view) => view.update(cx, |view, cx| view.select_source_range(range, cx)),
            Self::Json(view) => view.update(cx, |view, cx| view.select_source_range(range, cx)),
            Self::Csv(view) => view.update(cx, |view, cx| view.select_source_range(range, cx)),
            Self::Xml(view) => view.update(cx, |view, cx| view.select_source_range(range, cx)),
        }
    }
    pub(crate) fn add_selection(&self, range: std::ops::Range<usize>, cx: &mut gpui_kit::App) {
        match self {
            Self::Code(input) => input.update(cx, |state, cx| state.add_selection(range, cx)),
            Self::Markdown(input) => input.update(cx, |state, cx| state.add_selection(range, cx)),
            Self::Svg(view) => view.update(cx, |view, cx| view.add_source_selection(range, cx)),
            Self::Json(view) => view.update(cx, |view, cx| view.add_source_selection(range, cx)),
            Self::Csv(view) => view.update(cx, |view, cx| view.add_source_selection(range, cx)),
            Self::Xml(view) => view.update(cx, |view, cx| view.add_source_selection(range, cx)),
        }
    }
    pub(crate) fn set_cursor_position(
        &self,
        position: Position,
        window: &mut Window,
        cx: &mut gpui_kit::App,
    ) {
        match self {
            Self::Code(input) => input.update(cx, |state, cx| {
                state.set_cursor_position(position, window, cx)
            }),
            Self::Markdown(input) => {
                let at = self.read(cx).text().position_to_offset(&position);
                input.update(cx, |state, cx| state.select_range(at..at, cx));
            }
            Self::Svg(view) => {
                let at = self.read(cx).text().position_to_offset(&position);
                view.update(cx, |view, cx| view.select_source_range(at..at, cx));
            }
            Self::Json(view) => {
                let at = self.read(cx).text().position_to_offset(&position);
                view.update(cx, |view, cx| view.select_source_range(at..at, cx));
            }
            Self::Csv(view) => {
                let at = self.read(cx).text().position_to_offset(&position);
                view.update(cx, |view, cx| view.select_source_range(at..at, cx));
            }
            Self::Xml(view) => {
                let at = self.read(cx).text().position_to_offset(&position);
                view.update(cx, |view, cx| view.select_source_range(at..at, cx));
            }
        }
    }
    /// 文本替换。只读文档不持有文本能力，所以这个方法在这里根本就不存在。
    pub(crate) fn replace(
        &self,
        range: std::ops::Range<usize>,
        text: &str,
        window: &mut Window,
        cx: &mut gpui_kit::App,
    ) {
        match self {
            Self::Code(input) => {
                let old = input.read(cx).text();
                let range = old.byte_to_utf16_idx(range.start)..old.byte_to_utf16_idx(range.end);
                input.update(cx, |state, cx| {
                    state.replace_text_in_range(Some(range), text, window, cx)
                });
            }
            Self::Markdown(input) => {
                input.update(cx, |state, cx| state.replace_range(range, text, cx))
            }
            Self::Svg(view) => {
                view.update(cx, |view, cx| view.replace_source_range(range, text, cx))
            }
            Self::Json(view) => {
                view.update(cx, |view, cx| view.replace_source_range(range, text, cx))
            }
            Self::Csv(view) => {
                view.update(cx, |view, cx| view.replace_source_range(range, text, cx))
            }
            Self::Xml(view) => {
                view.update(cx, |view, cx| view.replace_source_range(range, text, cx))
            }
        }
    }
    pub(crate) fn replace_ranges(
        &self,
        replacements: Vec<(std::ops::Range<usize>, String)>,
        cx: &mut gpui_kit::App,
    ) {
        match self {
            Self::Code(_) => {}
            Self::Markdown(input) => {
                input.update(cx, |state, cx| state.replace_ranges(replacements, cx))
            }
            Self::Svg(view) => {
                view.update(cx, |view, cx| view.replace_source_ranges(replacements, cx))
            }
            Self::Json(view) => {
                view.update(cx, |view, cx| view.replace_source_ranges(replacements, cx))
            }
            Self::Csv(view) => {
                view.update(cx, |view, cx| view.replace_source_ranges(replacements, cx))
            }
            Self::Xml(view) => {
                view.update(cx, |view, cx| view.replace_source_ranges(replacements, cx))
            }
        }
    }
    pub(crate) fn replace_lines(
        &self,
        range: std::ops::Range<usize>,
        lines: Vec<std::ops::Range<usize>>,
        leading: bool,
        trailing: bool,
        cx: &mut gpui_kit::App,
    ) {
        match self {
            Self::Code(_) => {}
            Self::Markdown(input) => input.update(cx, |state, cx| {
                state.replace_lines(range, lines, leading, trailing, cx)
            }),
            Self::Svg(view) => view.update(cx, |view, cx| {
                view.replace_source_lines(range, lines, leading, trailing, cx)
            }),
            Self::Json(view) => view.update(cx, |view, cx| {
                view.replace_source_lines(range, lines, leading, trailing, cx)
            }),
            Self::Csv(view) => view.update(cx, |view, cx| {
                view.replace_source_lines(range, lines, leading, trailing, cx)
            }),
            Self::Xml(view) => view.update(cx, |view, cx| {
                view.replace_source_lines(range, lines, leading, trailing, cx)
            }),
        }
    }
    pub(crate) fn set_history_selection(
        &self,
        before: std::ops::Range<usize>,
        after: std::ops::Range<usize>,
        cx: &mut gpui_kit::App,
    ) {
        match self {
            Self::Code(_) => {}
            Self::Markdown(input) => input.update(cx, |state, cx| {
                state.set_history_selection(before, after, cx)
            }),
            Self::Svg(view) => view.update(cx, |view, cx| {
                view.set_source_history_selection(before, after, cx)
            }),
            Self::Json(view) => view.update(cx, |view, cx| {
                view.set_source_history_selection(before, after, cx)
            }),
            Self::Csv(view) => view.update(cx, |view, cx| {
                view.set_source_history_selection(before, after, cx)
            }),
            Self::Xml(view) => view.update(cx, |view, cx| {
                view.set_source_history_selection(before, after, cx)
            }),
        }
    }
}
pub(crate) struct InputRead<'a> {
    input: &'a EditorInput,
    cx: &'a gpui_kit::App,
}
impl InputRead<'_> {
    /// 焦点与滚动是所有文档共有的呈现状态；文本事实见 `TextRead`。
    pub(crate) fn focus_handle(&self, cx: &gpui_kit::App) -> gpui_kit::FocusHandle {
        match self.input {
            EditorInput::Code(input) => input.read(self.cx).focus_handle(cx),
            EditorInput::Markdown(input) => input.read(self.cx).focus_handle(cx),
            EditorInput::Image(input) => input.read(self.cx).handle(),
            EditorInput::Sqlite(input) => input.read(self.cx).focus_handle(cx),
            EditorInput::Book(input) => input.read(self.cx).handle(),
            EditorInput::Pdf(input) => input.read(self.cx).handle(),
            EditorInput::Svg(view) => view.read(self.cx).focus_handle(cx),
            EditorInput::Json(view) => view.read(self.cx).focus_handle(cx),
            EditorInput::Csv(view) => view.read(self.cx).focus_handle(cx),
            EditorInput::Xml(view) => view.read(self.cx).focus_handle(cx),
            EditorInput::Drawio(view) => view.read(self.cx).handle(),
        }
    }
    pub(crate) fn scroll_offset(&self) -> gpui_kit::Point<gpui_kit::Pixels> {
        match self.input {
            EditorInput::Code(input) => input.read(self.cx).scroll_offset(),
            EditorInput::Markdown(input) => input.read(self.cx).scroll_offset(),
            EditorInput::Image(input) => input.read(self.cx).scroll().offset(),
            EditorInput::Sqlite(_) => gpui_kit::Point::default(),
            EditorInput::Book(_) => gpui_kit::Point::default(),
            EditorInput::Pdf(input) => input.read(self.cx).scroll().offset(),
            EditorInput::Svg(view) => view.read(self.cx).scroll_offset(self.cx),
            EditorInput::Json(view) => view.read(self.cx).scroll_offset(self.cx),
            EditorInput::Csv(view) => view.read(self.cx).scroll_offset(self.cx),
            EditorInput::Xml(view) => view.read(self.cx).scroll_offset(self.cx),
            EditorInput::Drawio(_) => gpui_kit::Point::default(),
        }
    }
}

/// 一份文本输入的一次读取。只有 `EditorInput::text()` 能造出它，
/// 所以这里没有一个回答是给图片准备的。
pub(crate) struct TextRead<'a> {
    input: TextInput<'a>,
    cx: &'a gpui_kit::App,
}

impl TextRead<'_> {
    pub(crate) fn text(&self) -> TextSnapshot {
        match self.input {
            TextInput::Code(input) => TextSnapshot::Code(input.read(self.cx).text().clone()),
            TextInput::Markdown(input) => TextSnapshot::Shared(
                input
                    .read(self.cx)
                    .document()
                    .read(self.cx)
                    .snapshot()
                    .shared_text(),
            ),
            TextInput::Svg(view) => TextSnapshot::Shared(
                view.read(self.cx)
                    .document()
                    .read(self.cx)
                    .document()
                    .read(self.cx)
                    .snapshot()
                    .shared_text(),
            ),
            TextInput::Json(view) => TextSnapshot::Shared(
                view.read(self.cx)
                    .document()
                    .read(self.cx)
                    .document()
                    .read(self.cx)
                    .snapshot()
                    .shared_text(),
            ),
            TextInput::Csv(view) => TextSnapshot::Shared(
                view.read(self.cx)
                    .document()
                    .read(self.cx)
                    .document()
                    .read(self.cx)
                    .snapshot()
                    .shared_text(),
            ),
            TextInput::Xml(view) => TextSnapshot::Shared(
                view.read(self.cx)
                    .document()
                    .read(self.cx)
                    .document()
                    .read(self.cx)
                    .snapshot()
                    .shared_text(),
            ),
        }
    }
    pub(crate) fn cursor(&self) -> usize {
        match self.input {
            TextInput::Code(input) => input.read(self.cx).cursor(),
            TextInput::Markdown(input) => input.read(self.cx).cursor(),
            TextInput::Svg(view) => view.read(self.cx).source().read(self.cx).cursor(),
            TextInput::Json(view) => view.read(self.cx).source().read(self.cx).cursor(),
            TextInput::Csv(view) => view.read(self.cx).source().read(self.cx).cursor(),
            TextInput::Xml(view) => view.read(self.cx).source().read(self.cx).cursor(),
        }
    }
    pub(crate) fn cursor_position(&self) -> Position {
        self.text().offset_to_position(self.cursor())
    }
    pub(crate) fn selected_range(&self) -> std::ops::Range<usize> {
        match self.input {
            TextInput::Code(input) => input.read(self.cx).selected_range(),
            TextInput::Markdown(input) => input.read(self.cx).selected_range(),
            TextInput::Svg(view) => view.read(self.cx).source().read(self.cx).selected_range(),
            TextInput::Json(view) => view.read(self.cx).source().read(self.cx).selected_range(),
            TextInput::Csv(view) => view.read(self.cx).source().read(self.cx).selected_range(),
            TextInput::Xml(view) => view.read(self.cx).source().read(self.cx).selected_range(),
        }
    }
    pub(crate) fn occurrence_range(&self) -> std::ops::Range<usize> {
        match self.input {
            TextInput::Code(_) => self.selected_range(),
            TextInput::Markdown(input) => input.read(self.cx).occurrence_range(),
            TextInput::Svg(view) => view.read(self.cx).source().read(self.cx).occurrence_range(),
            TextInput::Json(view) => view.read(self.cx).source().read(self.cx).occurrence_range(),
            TextInput::Csv(view) => view.read(self.cx).source().read(self.cx).occurrence_range(),
            TextInput::Xml(view) => view.read(self.cx).source().read(self.cx).occurrence_range(),
        }
    }
    pub(crate) fn range_to_bounds(
        &self,
        range: &std::ops::Range<usize>,
    ) -> Option<gpui_kit::Bounds<gpui_kit::Pixels>> {
        match self.input {
            TextInput::Code(input) => input.read(self.cx).range_to_bounds(range),
            TextInput::Markdown(input) => input.read(self.cx).range_to_bounds(range),
            TextInput::Svg(view) => view
                .read(self.cx)
                .source()
                .read(self.cx)
                .range_to_bounds(range),
            TextInput::Json(view) => view
                .read(self.cx)
                .source()
                .read(self.cx)
                .range_to_bounds(range),
            TextInput::Csv(view) => view
                .read(self.cx)
                .source()
                .read(self.cx)
                .range_to_bounds(range),
            TextInput::Xml(view) => view
                .read(self.cx)
                .source()
                .read(self.cx)
                .range_to_bounds(range),
        }
    }
}
/// 一份文件在各个分组里的呈现。
///
/// Markdown 的呈现是可以编辑的副本，图片的呈现只是取景：同一份文档在每个
/// 分组各有缩放与滚动，这就是 presentation state，而不是第二份文档。
#[derive(Default)]
pub(super) struct Presentations {
    markdown: RefCell<
        Vec<(
            TabId,
            split::GroupId,
            Entity<MarkdownEditor>,
            Option<Entity<super::markdown_search::MarkdownSearch>>,
            Subscription,
        )>,
    >,
    images: RefCell<Vec<(TabId, split::GroupId, Entity<ImageView>, Subscription)>>,
    sqlite: RefCell<Vec<(TabId, split::GroupId, Entity<SqliteView>, Subscription)>>,
    books: RefCell<Vec<(TabId, split::GroupId, Entity<BookView>, Subscription)>>,
    pdfs: RefCell<Vec<(TabId, split::GroupId, Entity<PdfView>, Subscription)>>,
    svgs: RefCell<Vec<(TabId, split::GroupId, Entity<SvgView>, Subscription)>>,
    jsons: RefCell<Vec<(TabId, split::GroupId, Entity<JsonView>, Subscription)>>,
    csvs: RefCell<Vec<(TabId, split::GroupId, Entity<CsvView>, Subscription)>>,
    xmls: RefCell<Vec<(TabId, split::GroupId, Entity<XmlView>, Subscription)>>,
    drawios: RefCell<Vec<(TabId, split::GroupId, Entity<DrawioView>, Subscription)>>,
}
impl Presentations {
    pub(super) fn markdown_view<E: 'static>(
        &self,
        input: &Entity<MarkdownEditor>,
        tab: TabId,
        group: split::GroupId,
        window: &mut Window,
        cx: &mut Context<E>,
    ) -> Entity<MarkdownEditor> {
        let mut views = self.markdown.borrow_mut();
        if let Some((_, _, view, _, _)) = views
            .iter()
            .find(|(t, g, _, _, _)| *t == tab && *g == group)
        {
            return view.clone();
        }
        let view = if views.is_empty() {
            input.clone()
        } else {
            let document = input.read(cx).document().clone();
            let wrap = input.read(cx).is_soft_wrap();
            let live = input.read(cx).is_live();
            let source_only = input.read(cx).is_source_only();
            let tab_indent = input.read(cx).tab_indent();
            let selection = input.read(cx).selected_range();
            let scroll = input.read(cx).scroll_offset();
            let view = cx.new(|cx| {
                let mut view = MarkdownEditor::for_document(document, cx);
                markdown::configure(&mut view, cx);
                view.set_tab_indent(tab_indent);
                view.set_source_only(source_only, cx);
                if !live {
                    view.clear_markdown_style(cx);
                }
                view.set_soft_wrap(wrap, cx);
                view.select_range(selection, cx);
                view.set_scroll_offset(scroll, cx);
                view
            });
            super::markdown_formula::install(&view, window, cx);
            view
        };
        let observe = cx.observe(&view, |_, _, cx| cx.notify());
        views.push((tab, group, view.clone(), None, observe));
        view
    }
    pub(super) fn markdown_existing(
        &self,
        tab: TabId,
        group: split::GroupId,
    ) -> Option<Entity<MarkdownEditor>> {
        self.markdown
            .borrow()
            .iter()
            .find(|(t, g, _, _, _)| *t == tab && *g == group)
            .map(|(_, _, view, _, _)| view.clone())
    }

    /// 图片文档在一个分组里的呈现。第一个分组直接用文档自带的那个呈现，
    /// 之后的分组各得一个新取景，共用同一份解码结果。
    pub(super) fn image_view<E: 'static>(
        &self,
        primary: &Entity<ImageView>,
        tab: TabId,
        group: split::GroupId,
        cx: &mut Context<E>,
    ) -> Entity<ImageView> {
        let mut views = self.images.borrow_mut();
        if let Some((_, _, view, _)) = views.iter().find(|(t, g, _, _)| *t == tab && *g == group) {
            return view.clone();
        }
        let view = if views.is_empty() {
            primary.clone()
        } else {
            let document = primary.read(cx).document().clone();
            let page = primary.read(cx).page();
            cx.new(|cx| ImageView::new_at(document, page, cx))
        };
        let observe = cx.observe(&view, |_, _, cx| cx.notify());
        views.push((tab, group, view.clone(), observe));
        view
    }

    pub(super) fn image_existing(
        &self,
        tab: TabId,
        group: split::GroupId,
    ) -> Option<Entity<ImageView>> {
        self.images
            .borrow()
            .iter()
            .find(|(t, g, _, _)| *t == tab && *g == group)
            .map(|(_, _, view, _)| view.clone())
    }

    /// 每个数据库分组保留独立焦点与表格状态，共享同一份读取结果。
    pub(super) fn sqlite_view<E: 'static>(
        &self,
        primary: &Entity<SqliteView>,
        tab: TabId,
        group: split::GroupId,
        window: &mut Window,
        cx: &mut Context<E>,
    ) -> Entity<SqliteView> {
        let mut views = self.sqlite.borrow_mut();
        if let Some((_, _, view, _)) = views.iter().find(|(t, g, _, _)| *t == tab && *g == group) {
            return view.clone();
        }
        let view = if views.is_empty() {
            primary.clone()
        } else {
            let document = primary.read(cx).document().clone();
            cx.new(|cx| SqliteView::new(document, window, cx))
        };
        let observe = cx.observe(&view, |_, _, cx| cx.notify());
        views.push((tab, group, view.clone(), observe));
        view
    }

    pub(super) fn sqlite_existing(
        &self,
        tab: TabId,
        group: split::GroupId,
    ) -> Option<Entity<SqliteView>> {
        self.sqlite
            .borrow()
            .iter()
            .find(|(t, g, _, _)| *t == tab && *g == group)
            .map(|(_, _, view, _)| view.clone())
    }

    /// A book's first group draws the document's primary presentation; later
    /// groups get their own chapter, scroll, search and focus over the same
    /// parsed book and reading record.
    pub(super) fn book_view<E: 'static>(
        &self,
        primary: &Entity<BookView>,
        tab: TabId,
        group: split::GroupId,
        window: &mut Window,
        cx: &mut Context<E>,
    ) -> Entity<BookView> {
        let mut views = self.books.borrow_mut();
        if let Some((_, _, view, _)) = views.iter().find(|(t, g, _, _)| *t == tab && *g == group) {
            return view.clone();
        }
        let view = if views.is_empty() {
            primary.clone()
        } else {
            let document = primary.read(cx).document().clone();
            cx.new(|cx| BookView::new(document, window, cx))
        };
        let observe = cx.observe(&view, |_, _, cx| cx.notify());
        views.push((tab, group, view.clone(), observe));
        view
    }

    pub(super) fn book_existing(
        &self,
        tab: TabId,
        group: split::GroupId,
    ) -> Option<Entity<BookView>> {
        self.books
            .borrow()
            .iter()
            .find(|(t, g, _, _)| *t == tab && *g == group)
            .map(|(_, _, view, _)| view.clone())
    }

    /// 一份 PDF 在一个分组里的呈现。与图片一样：第一组用文档自带的呈现，
    /// 其余分组各有自己的页与缩放，共享同一份解析结果。
    pub(super) fn pdf_view<E: 'static>(
        &self,
        primary: &Entity<PdfView>,
        tab: TabId,
        group: split::GroupId,
        window: &mut Window,
        cx: &mut Context<E>,
    ) -> Entity<PdfView> {
        let mut views = self.pdfs.borrow_mut();
        if let Some((_, _, view, _)) = views.iter().find(|(t, g, _, _)| *t == tab && *g == group) {
            return view.clone();
        }
        let view = if views.is_empty() {
            primary.clone()
        } else {
            let document = primary.read(cx).document().clone();
            cx.new(|cx| PdfView::new(document, window, cx))
        };
        let observe = cx.observe(&view, |_, _, cx| cx.notify());
        views.push((tab, group, view.clone(), observe));
        view
    }

    /// An SVG document's presentation in one group. The first group uses the
    /// document's own view; a split group gets its own mode, source caret and
    /// preview framing over the same Document and preview entity.
    pub(super) fn svg_view<E: 'static>(
        &self,
        primary: &Entity<SvgView>,
        tab: TabId,
        group: split::GroupId,
        cx: &mut Context<E>,
    ) -> Entity<SvgView> {
        let mut views = self.svgs.borrow_mut();
        if let Some((_, _, view, _)) = views.iter().find(|(t, g, _, _)| *t == tab && *g == group) {
            return view.clone();
        }
        let view = if views.is_empty() {
            primary.clone()
        } else {
            let document = primary.read(cx).document().clone();
            let mode = primary.read(cx).mode().opposite();
            let (wrap, indent) = {
                let source = primary.read(cx).source().read(cx);
                (source.is_soft_wrap(), source.tab_indent())
            };
            let selection = primary.read(cx).selected_range(cx);
            let source_scroll = primary.read(cx).source_scroll_offset(cx);
            let preview_scroll = primary.read(cx).preview_scroll_offset();
            let view = cx.new(|cx| SvgView::new(document, mode, wrap, indent, cx));
            view.update(cx, |view, cx| {
                view.seed_presentation(selection, source_scroll, preview_scroll, cx)
            });
            view
        };
        let observe = cx.observe(&view, |_, _, cx| cx.notify());
        views.push((tab, group, view.clone(), observe));
        view
    }

    pub(super) fn pdf_existing(
        &self,
        tab: TabId,
        group: split::GroupId,
    ) -> Option<Entity<PdfView>> {
        self.pdfs
            .borrow()
            .iter()
            .find(|(t, g, _, _)| *t == tab && *g == group)
            .map(|(_, _, view, _)| view.clone())
    }

    pub(super) fn svg_existing(
        &self,
        tab: TabId,
        group: split::GroupId,
    ) -> Option<Entity<SvgView>> {
        self.svgs
            .borrow()
            .iter()
            .find(|(t, g, _, _)| *t == tab && *g == group)
            .map(|(_, _, view, _)| view.clone())
    }

    /// A JSON document's presentation in one group. The first group uses the
    /// document's own view; a split group gets its own mode, structure folds,
    /// selection, source caret and scroll over the same Document.
    pub(super) fn json_view<E: 'static>(
        &self,
        primary: &Entity<JsonView>,
        tab: TabId,
        group: split::GroupId,
        cx: &mut Context<E>,
    ) -> Entity<JsonView> {
        let mut views = self.jsons.borrow_mut();
        if let Some((_, _, view, _)) = views.iter().find(|(t, g, _, _)| *t == tab && *g == group) {
            return view.clone();
        }
        let view = if views.is_empty() {
            primary.clone()
        } else {
            let document = primary.read(cx).document().clone();
            let mode = primary.read(cx).mode().opposite();
            let (wrap, indent) = {
                let source = primary.read(cx).source().read(cx);
                (source.is_soft_wrap(), source.tab_indent())
            };
            let selection = primary.read(cx).source().read(cx).selected_range();
            let source_scroll = primary.read(cx).source_scroll_offset(cx);
            let structure_scroll = primary.read(cx).structure_scroll_offset();
            let (expanded, selected) = primary.read(cx).expansion_state();
            let view = cx.new(|cx| JsonView::new(document, mode, wrap, indent, cx));
            view.update(cx, |view, cx| {
                view.seed_presentation(
                    selection,
                    source_scroll,
                    structure_scroll,
                    expanded,
                    selected,
                    cx,
                )
            });
            view
        };
        let observe = cx.observe(&view, |_, _, cx| cx.notify());
        views.push((tab, group, view.clone(), observe));
        view
    }

    /// A CSV/TSV document's presentation in one group. The first group uses
    /// the document's own view; a split group gets its own mode, table
    /// selection, filter/sort and scrolls over the same Document and table.
    pub(super) fn csv_view<E: 'static>(
        &self,
        primary: &Entity<CsvView>,
        tab: TabId,
        group: split::GroupId,
        window: &mut Window,
        cx: &mut Context<E>,
    ) -> Entity<CsvView> {
        let mut views = self.csvs.borrow_mut();
        if let Some((_, _, view, _)) = views.iter().find(|(t, g, _, _)| *t == tab && *g == group) {
            return view.clone();
        }
        let view = if views.is_empty() {
            primary.clone()
        } else {
            let document = primary.read(cx).document().clone();
            let mode = primary.read(cx).mode().opposite();
            let (wrap, indent) = {
                let source = primary.read(cx).source().read(cx);
                (source.is_soft_wrap(), source.tab_indent())
            };
            let selection = primary.read(cx).selection();
            let source_selection = primary.read(cx).selected_range(cx);
            let source_scroll = primary.read(cx).source_scroll_offset(cx);
            let table_scroll = primary.read(cx).table_scroll_offset();
            let table_horizontal = primary.read(cx).horizontal_offset();
            cx.new(|cx| {
                let mut view = CsvView::new(document, mode, wrap, indent, window, cx);
                view.seed_presentation(
                    selection,
                    source_selection,
                    source_scroll,
                    table_scroll,
                    table_horizontal,
                    cx,
                );
                view
            })
        };
        let observe = cx.observe(&view, |_, _, cx| cx.notify());
        views.push((tab, group, view.clone(), observe));
        view
    }

    /// An XML document's presentation in one group. The first group uses the
    /// document's own view; a split group gets its own mode, structure folds,
    /// selection, source caret and scroll over the same Document.
    pub(super) fn xml_view<E: 'static>(
        &self,
        primary: &Entity<XmlView>,
        tab: TabId,
        group: split::GroupId,
        cx: &mut Context<E>,
    ) -> Entity<XmlView> {
        let mut views = self.xmls.borrow_mut();
        if let Some((_, _, view, _)) = views.iter().find(|(t, g, _, _)| *t == tab && *g == group) {
            return view.clone();
        }
        let view = if views.is_empty() {
            primary.clone()
        } else {
            let document = primary.read(cx).document().clone();
            let mode = primary.read(cx).mode().opposite();
            let (wrap, indent) = {
                let source = primary.read(cx).source().read(cx);
                (source.is_soft_wrap(), source.tab_indent())
            };
            let selection = primary.read(cx).source().read(cx).selected_range();
            let source_scroll = primary.read(cx).source_scroll_offset(cx);
            let structure_scroll = primary.read(cx).structure_scroll_offset();
            let (expanded, selected) = primary.read(cx).expansion_state();
            let view = cx.new(|cx| XmlView::new(document, mode, wrap, indent, cx));
            view.update(cx, |view, cx| {
                view.seed_presentation(
                    selection,
                    source_scroll,
                    structure_scroll,
                    expanded,
                    selected,
                    cx,
                )
            });
            view
        };
        let observe = cx.observe(&view, |_, _, cx| cx.notify());
        views.push((tab, group, view.clone(), observe));
        view
    }

    /// A draw.io document's presentation in one group. The first group uses
    /// the document's own view; a split group gets its own page/zoom and its
    /// own engine session over the same bytes and page list.
    pub(super) fn drawio_view<E: 'static>(
        &self,
        primary: &Entity<DrawioView>,
        tab: TabId,
        group: split::GroupId,
        cx: &mut Context<E>,
    ) -> Entity<DrawioView> {
        let mut views = self.drawios.borrow_mut();
        if let Some((_, _, view, _)) = views.iter().find(|(t, g, _, _)| *t == tab && *g == group) {
            return view.clone();
        }
        let view = if views.is_empty() {
            primary.clone()
        } else {
            let document = primary.read(cx).document().clone();
            let page = primary.read(cx).page();
            cx.new(|cx| DrawioView::new_at(document, page, cx))
        };
        let observe = cx.observe(&view, |_, _, cx| cx.notify());
        views.push((tab, group, view.clone(), observe));
        view
    }

    pub(super) fn json_existing(
        &self,
        tab: TabId,
        group: split::GroupId,
    ) -> Option<Entity<JsonView>> {
        self.jsons
            .borrow()
            .iter()
            .find(|(t, g, _, _)| *t == tab && *g == group)
            .map(|(_, _, view, _)| view.clone())
    }

    pub(super) fn csv_existing(
        &self,
        tab: TabId,
        group: split::GroupId,
    ) -> Option<Entity<CsvView>> {
        self.csvs
            .borrow()
            .iter()
            .find(|(t, g, _, _)| *t == tab && *g == group)
            .map(|(_, _, view, _)| view.clone())
    }

    pub(super) fn xml_existing(
        &self,
        tab: TabId,
        group: split::GroupId,
    ) -> Option<Entity<XmlView>> {
        self.xmls
            .borrow()
            .iter()
            .find(|(t, g, _, _)| *t == tab && *g == group)
            .map(|(_, _, view, _)| view.clone())
    }

    /// Split draw.io presentations created per group, in creation order;
    /// the owning primary view is not repeated here.
    pub(super) fn drawios(&self) -> Vec<(TabId, split::GroupId, Entity<DrawioView>)> {
        self.drawios
            .borrow()
            .iter()
            .map(|(tab, group, view, _)| (tab.clone(), *group, view.clone()))
            .collect()
    }

    pub(super) fn drawio_existing(
        &self,
        tab: TabId,
        group: split::GroupId,
    ) -> Option<Entity<DrawioView>> {
        self.drawios
            .borrow()
            .iter()
            .find(|(t, g, _, _)| *t == tab && *g == group)
            .map(|(_, _, view, _)| view.clone())
    }

    pub(super) fn search(
        &self,
        source: &Entity<MarkdownEditor>,
    ) -> Option<Entity<super::markdown_search::MarkdownSearch>> {
        self.markdown
            .borrow()
            .iter()
            .find(|(_, _, view, _, _)| view == source)
            .and_then(|(_, _, _, search, _)| search.clone())
    }
    pub(super) fn open_search(
        &self,
        source: &Entity<MarkdownEditor>,
        window: &mut Window,
        cx: &mut gpui_kit::App,
    ) -> Entity<super::markdown_search::MarkdownSearch> {
        let mut views = self.markdown.borrow_mut();
        let (_, _, _, search, _) = views
            .iter_mut()
            .find(|(_, _, view, _, _)| view == source)
            .expect("registered presentation");
        search
            .get_or_insert_with(|| {
                cx.new(|cx| super::markdown_search::MarkdownSearch::new(source, window, cx))
            })
            .clone()
    }
    pub(super) fn retain(
        &self,
        mut live: impl FnMut(TabId, split::GroupId) -> bool,
        _: &mut Window,
        cx: &mut gpui_kit::App,
    ) {
        self.markdown
            .borrow_mut()
            .retain(|(tab, group, view, _, _)| {
                let keep = live(*tab, *group);
                if !keep {
                    view.update(cx, |view, cx| view.finish_owned_composition(cx));
                }
                keep
            });
        // 图片呈现没有进行中的输入法组合，也没有缓存的地图与公式：退场只需
        // 让最后一个句柄落下，解码结果由文档本身继续持有。
        self.images
            .borrow_mut()
            .retain(|(tab, group, _, _)| live(*tab, *group));
        // 数据库呈现同样没有输入组合；读取视图与页数据由文档持有，退场只让
        // 这个分组的焦点、滚动与表格选择落下。
        self.sqlite
            .borrow_mut()
            .retain(|(tab, group, _, _)| live(*tab, *group));
        self.books
            .borrow_mut()
            .retain(|(tab, group, _, _)| live(*tab, *group));
        self.pdfs
            .borrow_mut()
            .retain(|(tab, group, _, _)| live(*tab, *group));
        // SVG 的源码呈现持有可提交的 preedit；离开分组时先结束自己的 lease，
        // Document 与预览仍由文件的其余呈现或缓冲区持有。
        self.svgs.borrow_mut().retain(|(tab, group, view, _)| {
            let keep = live(*tab, *group);
            if !keep {
                view.update(cx, |view, cx| view.finish_composition(cx));
            }
            keep
        });
        // JSON presentation 的源码面同样可能持有 preedit；结构状态只属于
        // 这个呈现，退场不需要触碰共享 Document。
        self.jsons.borrow_mut().retain(|(tab, group, view, _)| {
            let keep = live(*tab, *group);
            if !keep {
                view.update(cx, |view, cx| view.finish_composition(cx));
            }
            keep
        });
        self.csvs.borrow_mut().retain(|(tab, group, view, _)| {
            let keep = live(*tab, *group);
            if !keep {
                view.update(cx, |view, cx| view.finish_composition(cx));
            }
            keep
        });
        self.xmls.borrow_mut().retain(|(tab, group, view, _)| {
            let keep = live(*tab, *group);
            if !keep {
                view.update(cx, |view, cx| view.finish_composition(cx));
            }
            keep
        });
        // Dropping the last presentation drops its native surface and session;
        // there is no engine-owned editing state to flush.
        self.drawios
            .borrow_mut()
            .retain(|(tab, group, _, _)| live(*tab, *group));
    }
    /// File identity changes update capability without replacing source or history.
    pub(super) fn set_source_only(
        &self,
        source_only: bool,
        input: &EditorInput,
        cx: &mut gpui_kit::App,
    ) {
        if let Some(primary) = input.markdown() {
            primary.update(cx, |view, cx| {
                view.set_source_only(source_only, cx);
                view.set_host_link_navigation(!source_only);
            });
            for (_, _, view, _, _) in self.markdown.borrow().iter() {
                if view != primary {
                    view.update(cx, |view, cx| {
                        view.set_source_only(source_only, cx);
                        view.set_host_link_navigation(!source_only);
                    });
                }
            }
        }
    }
    pub(super) fn set_soft_wrap(&self, wrap: bool, _: &EditorInput, cx: &mut gpui_kit::App) {
        for (_, _, view, _, _) in self.markdown.borrow().iter() {
            view.update(cx, |view, cx| view.set_soft_wrap(wrap, cx));
        }
        for (_, _, view, _) in self.svgs.borrow().iter() {
            view.update(cx, |view, cx| view.set_soft_wrap(wrap, cx));
        }
        for (_, _, view, _) in self.jsons.borrow().iter() {
            view.update(cx, |view, cx| view.set_soft_wrap(wrap, cx));
        }
        for (_, _, view, _) in self.csvs.borrow().iter() {
            view.update(cx, |view, cx| view.set_soft_wrap(wrap, cx));
        }
        for (_, _, view, _) in self.xmls.borrow().iter() {
            view.update(cx, |view, cx| view.set_soft_wrap(wrap, cx));
        }
    }
}

#[cfg(any(test, feature = "native-markdown-tests"))]
#[path = "workbench_views.rs"]
pub(super) mod workbench_views;
