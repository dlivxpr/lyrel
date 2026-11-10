//! 工作台编辑 interface：文档文本与 presentation 状态分别读取。
use super::text::TextSnapshot;
use super::*;
use lyrel_markdown_editor::EditorState as MarkdownEditor;
use std::cell::RefCell;

#[derive(Clone)]
pub(crate) enum EditorInput {
    Code(Entity<EditorState>),
    Markdown(Entity<MarkdownEditor>),
}
#[derive(Clone)]
pub(crate) enum WeakEditorInput {
    Code(gpui_kit::WeakEntity<EditorState>),
    Markdown(gpui_kit::WeakEntity<MarkdownEditor>),
}
impl WeakEditorInput {
    pub(crate) fn upgrade(&self) -> Option<EditorInput> {
        match self {
            Self::Code(input) => input.upgrade().map(EditorInput::Code),
            Self::Markdown(input) => input.upgrade().map(EditorInput::Markdown),
        }
    }
}
impl EditorInput {
    pub(crate) fn downgrade(&self) -> WeakEditorInput {
        match self {
            Self::Code(input) => WeakEditorInput::Code(input.downgrade()),
            Self::Markdown(input) => WeakEditorInput::Markdown(input.downgrade()),
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
    pub(crate) fn set_soft_wrap(&self, wrap: bool, window: &mut Window, cx: &mut gpui_kit::App) {
        match self {
            Self::Code(input) => {
                input.update(cx, |state, cx| state.set_soft_wrap(wrap, window, cx))
            }
            Self::Markdown(input) => input.update(cx, |state, cx| state.set_soft_wrap(wrap, cx)),
        }
    }
    pub(crate) fn finish_composition(&self, _: &mut Window, cx: &mut gpui_kit::App) {
        if let Some(input) = self.markdown() {
            input.update(cx, |state, cx| state.finish_composition(cx));
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
    pub(crate) fn read<'a>(&'a self, cx: &'a gpui_kit::App) -> InputRead<'a> {
        InputRead { input: self, cx }
    }
    pub(crate) fn focus(&self, window: &mut Window, cx: &mut gpui_kit::App) {
        self.finish_composition(window, cx);
        self.read(cx).focus_handle(cx).focus(window, cx);
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
        }
    }
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
    pub(crate) fn set_highlighter(&self, language: &'static str, cx: &mut gpui_kit::App) {
        if let Some(input) = self.code() {
            input.update(cx, |state, cx| state.set_highlighter(language, cx));
        }
    }
}
pub(crate) struct InputRead<'a> {
    input: &'a EditorInput,
    cx: &'a gpui_kit::App,
}
impl InputRead<'_> {
    pub(crate) fn text(&self) -> TextSnapshot {
        match self.input {
            EditorInput::Code(input) => TextSnapshot::Code(input.read(self.cx).text().clone()),
            EditorInput::Markdown(input) => TextSnapshot::Markdown(
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
            EditorInput::Code(input) => input.read(self.cx).cursor(),
            EditorInput::Markdown(input) => input.read(self.cx).cursor(),
        }
    }
    pub(crate) fn cursor_position(&self) -> Position {
        self.text().offset_to_position(self.cursor())
    }
    pub(crate) fn selected_range(&self) -> std::ops::Range<usize> {
        match self.input {
            EditorInput::Code(input) => input.read(self.cx).selected_range(),
            EditorInput::Markdown(input) => input.read(self.cx).selected_range(),
        }
    }
    pub(crate) fn occurrence_range(&self) -> std::ops::Range<usize> {
        match self.input {
            EditorInput::Code(_) => self.selected_range(),
            EditorInput::Markdown(input) => input.read(self.cx).occurrence_range(),
        }
    }
    pub(crate) fn focus_handle(&self, cx: &gpui_kit::App) -> gpui_kit::FocusHandle {
        match self.input {
            EditorInput::Code(input) => input.read(self.cx).focus_handle(cx),
            EditorInput::Markdown(input) => input.read(self.cx).focus_handle(cx),
        }
    }
    pub(crate) fn scroll_offset(&self) -> gpui_kit::Point<gpui_kit::Pixels> {
        match self.input {
            EditorInput::Code(input) => input.read(self.cx).scroll_offset(),
            EditorInput::Markdown(input) => input.read(self.cx).scroll_offset(),
        }
    }
    pub(crate) fn range_to_bounds(
        &self,
        range: &std::ops::Range<usize>,
    ) -> Option<gpui_kit::Bounds<gpui_kit::Pixels>> {
        match self.input {
            EditorInput::Code(input) => input.read(self.cx).range_to_bounds(range),
            EditorInput::Markdown(input) => input.read(self.cx).range_to_bounds(range),
        }
    }
}
#[derive(Default)]
pub(super) struct MarkdownPresentations(
    RefCell<
        Vec<(
            TabId,
            split::GroupId,
            Entity<MarkdownEditor>,
            Option<Entity<super::markdown_search::MarkdownSearch>>,
            Subscription,
        )>,
    >,
);
impl MarkdownPresentations {
    pub(super) fn view<E: 'static>(
        &self,
        input: &Entity<MarkdownEditor>,
        tab: TabId,
        group: split::GroupId,
        window: &mut Window,
        cx: &mut Context<E>,
    ) -> Entity<MarkdownEditor> {
        let mut views = self.0.borrow_mut();
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
    pub(super) fn existing(
        &self,
        tab: TabId,
        group: split::GroupId,
    ) -> Option<Entity<MarkdownEditor>> {
        self.0
            .borrow()
            .iter()
            .find(|(t, g, _, _, _)| *t == tab && *g == group)
            .map(|(_, _, view, _, _)| view.clone())
    }
    pub(super) fn search(
        &self,
        source: &Entity<MarkdownEditor>,
    ) -> Option<Entity<super::markdown_search::MarkdownSearch>> {
        self.0
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
        let mut views = self.0.borrow_mut();
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
        self.0.borrow_mut().retain(|(tab, group, view, _, _)| {
            let keep = live(*tab, *group);
            if !keep {
                view.update(cx, |view, cx| view.finish_composition(cx));
            }
            keep
        });
    }
    /// File identity changes update capability without replacing source or history.
    pub(super) fn set_source_only(
        &self,
        source_only: bool,
        input: &EditorInput,
        cx: &mut gpui_kit::App,
    ) {
        if let Some(primary) = input.markdown() {
            primary.update(cx, |view, cx| view.set_source_only(source_only, cx));
            for (_, _, view, _, _) in self.0.borrow().iter() {
                if view != primary {
                    view.update(cx, |view, cx| view.set_source_only(source_only, cx));
                }
            }
        }
    }
    pub(super) fn set_soft_wrap(&self, wrap: bool, _: &EditorInput, cx: &mut gpui_kit::App) {
        for (_, _, view, _, _) in self.0.borrow().iter() {
            view.update(cx, |view, cx| view.set_soft_wrap(wrap, cx));
        }
    }
}

#[cfg(any(test, feature = "native-markdown-tests"))]
#[path = "workbench_views.rs"]
pub(super) mod workbench_views;
