//! 工作台编辑 interface：文档文本与 presentation 状态分别读取。
use super::image_document::ImageView;
use super::text::TextSnapshot;
use super::*;
use lyrel_markdown_editor::EditorState as MarkdownEditor;
use std::cell::RefCell;

#[derive(Clone)]
pub(crate) enum EditorInput {
    Code(Entity<EditorState>),
    Markdown(Entity<MarkdownEditor>),
    /// 只读静态图片：没有文本，只有取景。
    Image(Entity<ImageView>),
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
}
#[derive(Clone)]
pub(crate) enum WeakEditorInput {
    Code(gpui_kit::WeakEntity<EditorState>),
    Markdown(gpui_kit::WeakEntity<MarkdownEditor>),
    Image(gpui_kit::WeakEntity<ImageView>),
}
impl WeakEditorInput {
    pub(crate) fn upgrade(&self) -> Option<EditorInput> {
        match self {
            Self::Code(input) => input.upgrade().map(EditorInput::Code),
            Self::Markdown(input) => input.upgrade().map(EditorInput::Markdown),
            Self::Image(input) => input.upgrade().map(EditorInput::Image),
        }
    }
}
impl EditorInput {
    pub(crate) fn downgrade(&self) -> WeakEditorInput {
        match self {
            Self::Code(input) => WeakEditorInput::Code(input.downgrade()),
            Self::Markdown(input) => WeakEditorInput::Markdown(input.downgrade()),
            Self::Image(input) => WeakEditorInput::Image(input.downgrade()),
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
    /// 这份输入的文本能力；只读图片返回 `None`。
    ///
    /// 需要文本的调用点从这里取能力再操作，而不是读一个伪造的空文档。
    pub(crate) fn text(&self) -> Option<TextInput<'_>> {
        match self {
            Self::Code(input) => Some(TextInput::Code(input)),
            Self::Markdown(input) => Some(TextInput::Markdown(input)),
            Self::Image(_) => None,
        }
    }
    /// 有文本可编辑、可查找、可保存的输入。图片不是。
    pub(crate) fn is_text(&self) -> bool {
        self.text().is_some()
    }
    pub(crate) fn finish_composition(&self, _: &mut Window, cx: &mut gpui_kit::App) {
        if let Some(input) = self.markdown() {
            input.update(cx, |state, cx| state.finish_composition(cx));
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
        }
    }
    pub(crate) fn set_selected_range(&self, range: std::ops::Range<usize>, cx: &mut gpui_kit::App) {
        match self {
            Self::Code(input) => input.update(cx, |state, cx| state.set_selected_range(range, cx)),
            Self::Markdown(input) => input.update(cx, |state, cx| state.select_range(range, cx)),
        }
    }
    pub(crate) fn add_selection(&self, range: std::ops::Range<usize>, cx: &mut gpui_kit::App) {
        match self {
            Self::Code(input) => input.update(cx, |state, cx| state.add_selection(range, cx)),
            Self::Markdown(input) => input.update(cx, |state, cx| state.add_selection(range, cx)),
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
        }
    }
    pub(crate) fn scroll_offset(&self) -> gpui_kit::Point<gpui_kit::Pixels> {
        match self.input {
            EditorInput::Code(input) => input.read(self.cx).scroll_offset(),
            EditorInput::Markdown(input) => input.read(self.cx).scroll_offset(),
            EditorInput::Image(input) => input.read(self.cx).scroll().offset(),
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
            TextInput::Markdown(input) => TextSnapshot::Markdown(
                input
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
        }
    }
    pub(crate) fn cursor_position(&self) -> Position {
        self.text().offset_to_position(self.cursor())
    }
    pub(crate) fn selected_range(&self) -> std::ops::Range<usize> {
        match self.input {
            TextInput::Code(input) => input.read(self.cx).selected_range(),
            TextInput::Markdown(input) => input.read(self.cx).selected_range(),
        }
    }
    pub(crate) fn occurrence_range(&self) -> std::ops::Range<usize> {
        match self.input {
            TextInput::Code(_) => self.selected_range(),
            TextInput::Markdown(input) => input.read(self.cx).occurrence_range(),
        }
    }
    pub(crate) fn range_to_bounds(
        &self,
        range: &std::ops::Range<usize>,
    ) -> Option<gpui_kit::Bounds<gpui_kit::Pixels>> {
        match self.input {
            TextInput::Code(input) => input.read(self.cx).range_to_bounds(range),
            TextInput::Markdown(input) => input.read(self.cx).range_to_bounds(range),
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
            cx.new(|cx| ImageView::new(document, cx))
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
    }
}

#[cfg(any(test, feature = "native-markdown-tests"))]
#[path = "workbench_views.rs"]
pub(super) mod workbench_views;
