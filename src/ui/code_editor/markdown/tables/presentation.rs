//! Presentation commands mutate only explicit metadata tokens in the owning
//! MarkdownDocument. A pointer gesture owns numeric preview state until release.
use super::super::tables::metadata::{self, Style};
use super::*;
use crate::ui::i18n::{L10nKey, t};

gpui_kit::actions!(
    markdown_table_presentation,
    [
        FitColumn,
        WidenColumn,
        NarrowColumn,
        StyleGrid,
        StyleStriped,
        StyleHeader,
        StyleMinimal
    ]
);

impl SourcePresentation {
    fn apply_table_presentation(
        &mut self,
        edit: (Range<usize>, String),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (range, text) = edit;
        if self
            .document
            .read(cx)
            .source()
            .slice(range.clone())
            .to_string()
            == text
        {
            return;
        }
        let map = |at: usize| {
            if at >= range.end {
                at - range.len() + text.len()
            } else {
                at
            }
        };
        let selection = Selection::new(
            map(self.state.selection.anchor),
            map(self.state.selection.head),
        );
        self.command(
            Command::BatchReplaceSelected {
                kind: EditKind::Command,
                edits: vec![(range, text)],
                selection,
            },
            window,
            cx,
        );
        self.focus.focus(window, cx);
    }

    pub(super) fn table_style(
        &mut self,
        style: Style,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(cell) = self.active_table() else {
            return;
        };
        if self.projection.revision != self.document.read(cx).revision() {
            return;
        }
        let edit = metadata::style_edit(self.document.read(cx).source(), cell.table().start, style);
        self.apply_table_presentation(edit, window, cx);
    }

    pub(super) fn table_fit(
        &mut self,
        delta: Option<f32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(cell) = self.active_table() else {
            return;
        };
        if self.projection.revision != self.document.read(cx).revision() {
            return;
        }
        let Some(widths) =
            self.tables
                .adjusted_widths(&self.projection.leaves, cell, window.rem_size(), delta)
        else {
            return;
        };
        let edit =
            metadata::widths_edit(self.document.read(cx).source(), cell.table().start, &widths);
        self.apply_table_presentation(edit, window, cx);
    }

    pub(in crate::ui::code_editor::markdown) fn start_table_resize(
        &mut self,
        id: u64,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.command(Command::FinishComposition, window, cx);
        if !self.live || self.projection.revision != self.document.read(cx).revision() {
            return;
        }
        let Some(leaf) = self.projection.leaves.iter().find(|leaf| leaf.id == id) else {
            return;
        };
        let Kind::TableCell(cell) = leaf.kind else {
            return;
        };
        let at = leaf.content_start();
        if event.click_count == 2 {
            self.command(Command::Select(Selection::caret(at)), window, cx);
            self.table_fit(None, window, cx);
        } else {
            self.tables
                .begin_resize(cell, self.projection.revision, event.position.x);
            self.dragging = false;
            self.drag_pointer = None;
            self.focus.focus(window, cx);
            cx.notify();
        }
        cx.stop_propagation();
    }

    pub(super) fn update_table_resize(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tables.resize_to(position.x, window.rem_size());
        self.geometry = None;
        self.candidate_frame = None;
        self.viewport.heights.clear();
        window.notify_text_selection_changed(&self.focus);
        cx.notify();
    }

    pub(super) fn finish_table_resize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((cell, revision, widths)) = self.tables.finish_resize(window.rem_size()) else {
            return;
        };
        if !self.live
            || revision != self.document.read(cx).revision()
            || revision != self.projection.revision
        {
            return;
        }
        let edit =
            metadata::widths_edit(self.document.read(cx).source(), cell.table().start, &widths);
        self.apply_table_presentation(edit, window, cx);
    }

    pub(super) fn table_presentation_menu(
        &self,
        menu: gpui_kit::component::menu::PopupMenu,
    ) -> gpui_kit::component::menu::PopupMenu {
        let active = self.active_table();
        let disabled = active.is_none();
        let style = active.map(|cell| self.tables.style(cell));
        menu.separator()
            .menu_with_disabled(
                t(L10nKey::EditorTableFitColumn),
                Box::new(FitColumn),
                disabled,
            )
            .menu_with_disabled(
                t(L10nKey::EditorTableWidenColumn),
                Box::new(WidenColumn),
                disabled,
            )
            .menu_with_disabled(
                t(L10nKey::EditorTableNarrowColumn),
                Box::new(NarrowColumn),
                disabled,
            )
            .menu_with_check_and_disabled(
                t(L10nKey::EditorTableStyleGrid),
                style == Some(Style::Grid),
                Box::new(StyleGrid),
                disabled,
            )
            .menu_with_check_and_disabled(
                t(L10nKey::EditorTableStyleStriped),
                style == Some(Style::Striped),
                Box::new(StyleStriped),
                disabled,
            )
            .menu_with_check_and_disabled(
                t(L10nKey::EditorTableStyleHeader),
                style == Some(Style::Header),
                Box::new(StyleHeader),
                disabled,
            )
            .menu_with_check_and_disabled(
                t(L10nKey::EditorTableStyleMinimal),
                style == Some(Style::Minimal),
                Box::new(StyleMinimal),
                disabled,
            )
    }
}
