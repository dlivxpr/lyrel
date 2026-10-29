//! Workbench-facing content variant: Markdown never allocates an EditorState.
use super::markdown::{Command, EditKind, Selection, SourcePresentation};
use super::*;
use gpui_kit::base::input::Rope;
use std::cell::RefCell;

#[derive(Clone)]
pub(crate) enum EditorInput {
    Code(Entity<EditorState>),
    Markdown(Entity<SourcePresentation>),
}

#[derive(Clone)]
pub(crate) enum WeakEditorInput {
    Code(gpui_kit::WeakEntity<EditorState>),
    Markdown(gpui_kit::WeakEntity<SourcePresentation>),
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
    pub(crate) fn add_selection(&self, range: std::ops::Range<usize>, cx: &mut gpui_kit::App) {
        match self {
            Self::Code(input) => input.update(cx, |state, cx| state.add_selection(range, cx)),
            Self::Markdown(input) => input.update(cx, |state, cx| state.add_selection(range, cx)),
        }
    }
    pub(crate) fn code(&self) -> Option<&Entity<EditorState>> {
        if let Self::Code(input) = self {
            Some(input)
        } else {
            None
        }
    }
    pub(crate) fn markdown(&self) -> Option<&Entity<SourcePresentation>> {
        if let Self::Markdown(input) = self {
            Some(input)
        } else {
            None
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
    pub(crate) fn finish_composition(&self, window: &mut Window, cx: &mut gpui_kit::App) {
        if let Some(view) = self.markdown() {
            let document = view.read(cx).document().clone();
            SourcePresentation::finish_active_composition(&document, window, cx);
        }
    }
    pub(crate) fn set_selected_range(&self, range: std::ops::Range<usize>, cx: &mut gpui_kit::App) {
        match self {
            Self::Code(input) => input.update(cx, |state, cx| state.set_selected_range(range, cx)),
            Self::Markdown(input) => input.update(cx, |state, cx| {
                state.select_range(range, cx);
            }),
        }
    }
    pub(crate) fn read<'a>(&'a self, cx: &'a gpui_kit::App) -> InputRead<'a> {
        match self {
            Self::Code(input) => InputRead {
                code: Some(input.read(cx)),
                source: None,
                cx,
            },
            Self::Markdown(input) => InputRead {
                code: None,
                source: Some(input.read(cx)),
                cx,
            },
        }
    }
    pub(crate) fn focus(&self, window: &mut Window, cx: &mut gpui_kit::App) {
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
            Self::Markdown(input) => input.update(cx, |state, cx| {
                let at = state
                    .document()
                    .read(cx)
                    .source()
                    .position_to_offset(&position);
                state.command(Command::Select(Selection::caret(at)), window, cx);
            }),
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
                let range16 = old.byte_to_utf16_idx(range.start)..old.byte_to_utf16_idx(range.end);
                input.update(cx, |state, cx| {
                    state.replace_text_in_range(Some(range16), text, window, cx)
                });
            }
            Self::Markdown(input) => input.update(cx, |state, cx| {
                // Explicit workbench/LSP ranges affect only their target, unlike
                // native typing and clipboard actions with occurrence selections.
                state.select_range(range.clone(), cx);
                state.command(
                    Command::Replace {
                        range,
                        text: text.into(),
                        kind: EditKind::Command,
                    },
                    window,
                    cx,
                )
            }),
        }
    }
    pub(crate) fn set_highlighter(&self, language: &'static str, cx: &mut gpui_kit::App) {
        // Source Markdown is projected by the feature; only code has a Kit highlighter.
        if let Some(input) = self.code() {
            input.update(cx, |state, cx| state.set_highlighter(language, cx));
        }
    }
}
pub(crate) struct InputRead<'a> {
    code: Option<&'a EditorState>,
    source: Option<&'a SourcePresentation>,
    cx: &'a gpui_kit::App,
}
impl InputRead<'_> {
    pub(crate) fn text(&self) -> &Rope {
        self.code
            .map(|s| s.text())
            .unwrap_or_else(|| self.source.unwrap().document().read(self.cx).source())
    }
    pub(crate) fn cursor(&self) -> usize {
        self.code
            .map(|s| s.cursor())
            .unwrap_or_else(|| self.source.unwrap().selection().head())
    }
    pub(crate) fn cursor_position(&self) -> Position {
        self.text()
            .offset_to_position(self.cursor().min(self.text().len()))
    }
    pub(crate) fn selected_range(&self) -> std::ops::Range<usize> {
        self.code
            .map(|s| s.selected_range())
            .unwrap_or_else(|| self.source.unwrap().selection().range())
    }
    pub(crate) fn occurrence_range(&self) -> std::ops::Range<usize> {
        self.source.map_or_else(
            || self.selected_range(),
            SourcePresentation::occurrence_range,
        )
    }
    pub(crate) fn focus_handle(&self, cx: &gpui_kit::App) -> gpui_kit::FocusHandle {
        self.code
            .map(|s| s.focus_handle(cx))
            .unwrap_or_else(|| self.source.unwrap().focus_handle(cx))
    }
    pub(crate) fn scroll_offset(&self) -> gpui_kit::Point<gpui_kit::Pixels> {
        self.code
            .map(|s| s.scroll_offset())
            .unwrap_or_else(|| self.source.unwrap().scroll_offset())
    }
    pub(crate) fn range_to_bounds(
        &self,
        range: &std::ops::Range<usize>,
    ) -> Option<gpui_kit::Bounds<gpui_kit::Pixels>> {
        self.code
            .and_then(|s| s.range_to_bounds(range))
            .or_else(|| self.source.and_then(|s| s.range_to_bounds(range, self.cx)))
    }
}

/// Per-presentation storage contains only views, not another document authority.
#[derive(Default)]
pub(super) struct MarkdownPresentations(
    RefCell<
        Vec<(
            TabId,
            split::GroupId,
            Entity<SourcePresentation>,
            Option<Entity<super::markdown_search::MarkdownSearch>>,
        )>,
    >,
);
impl MarkdownPresentations {
    pub(super) fn view(
        &self,
        input: &Entity<SourcePresentation>,
        tab: TabId,
        group: split::GroupId,
        cx: &mut gpui_kit::App,
    ) -> Entity<SourcePresentation> {
        let mut views = self.0.borrow_mut();
        if let Some((_, _, view, _)) = views.iter().find(|(t, g, _, _)| *t == tab && *g == group) {
            return view.clone();
        }
        let view = if views.is_empty() {
            input.clone()
        } else {
            let document = input.read(cx).document().clone();
            let wrap = input.read(cx).is_soft_wrap();
            let indent = input.read(cx).indent();
            cx.new(|cx| {
                let mut view = SourcePresentation::new(document, wrap, cx);
                view.set_indent(indent, cx);
                view
            })
        };
        views.push((tab, group, view.clone(), None));
        view
    }
    pub(super) fn existing(
        &self,
        tab: TabId,
        group: split::GroupId,
    ) -> Option<Entity<SourcePresentation>> {
        self.0
            .borrow()
            .iter()
            .find(|(t, g, _, _)| *t == tab && *g == group)
            .map(|(_, _, v, _)| v.clone())
    }
    pub(super) fn search(
        &self,
        source: &Entity<SourcePresentation>,
    ) -> Option<Entity<super::markdown_search::MarkdownSearch>> {
        self.0
            .borrow()
            .iter()
            .find(|(_, _, view, _)| view.entity_id() == source.entity_id())
            .and_then(|(_, _, _, search)| search.clone())
    }
    pub(super) fn open_search(
        &self,
        source: &Entity<SourcePresentation>,
        window: &mut Window,
        cx: &mut gpui_kit::App,
    ) -> Entity<super::markdown_search::MarkdownSearch> {
        let mut views = self.0.borrow_mut();
        let (_, _, _, search) = views
            .iter_mut()
            .find(|(_, _, view, _)| view.entity_id() == source.entity_id())
            .expect("source presentation is registered before receiving commands");
        search
            .get_or_insert_with(|| {
                cx.new(|cx| super::markdown_search::MarkdownSearch::new(source, window, cx))
            })
            .clone()
    }
    pub(super) fn retain(
        &self,
        mut live: impl FnMut(TabId, split::GroupId) -> bool,
        window: &mut Window,
        cx: &mut gpui_kit::App,
    ) {
        self.0.borrow_mut().retain(|(tab, group, view, _)| {
            let keep = live(*tab, *group);
            if !keep && view.read(cx).is_composing() {
                view.update(cx, |view, cx| {
                    view.command(Command::FinishComposition, window, cx)
                });
            }
            keep
        });
    }
    pub(super) fn set_soft_wrap(&self, wrap: bool, primary: &EditorInput, cx: &mut gpui_kit::App) {
        for (_, _, view, _) in self.0.borrow().iter() {
            if primary
                .markdown()
                .is_none_or(|input| input.entity_id() != view.entity_id())
            {
                view.update(cx, |view, cx| view.set_soft_wrap(wrap, cx));
            }
        }
    }
}

/// Explicit launch gate, set only by --markdown-workbench (never persisted in config).
#[derive(Default)]
pub(crate) struct MarkdownWorkbench;
impl gpui_kit::Global for MarkdownWorkbench {}
