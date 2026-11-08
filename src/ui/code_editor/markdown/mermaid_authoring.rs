use super::{Selection, SourcePresentation};
use crate::ui::code_editor::markdown::projection::{Kind, Leaf};
use crate::ui::i18n::{L10nKey, t};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _,
    button::{Button, ButtonVariants as _},
};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Context, FocusHandle, Window, div};

gpui_kit::actions!(markdown_mermaid, [EditDiagramSource, ShowDiagramResult]);

#[derive(Default)]
pub(super) struct Authoring {
    preview: Option<Preview>,
}

struct Preview {
    id: u64,
    revision: u64,
    selection: Selection,
    focus: FocusHandle,
}

impl SourcePresentation {
    fn mermaid_target(&self, target: Option<(u64, u64)>, cx: &App) -> Option<&Leaf> {
        if !self.live || self.projection.revision != self.document.read(cx).revision() {
            return None;
        }
        self.projection.leaves.iter().find(|leaf| {
            matches!(leaf.kind, Kind::Mermaid(_))
                && target.map_or_else(
                    || {
                        leaf.range.start <= self.state.selection.head()
                            && self.state.selection.head() <= leaf.range.end
                    },
                    |(id, revision)| leaf.id == id && self.projection.revision == revision,
                )
        })
    }

    pub(super) fn edit_mermaid_source(
        &mut self,
        target: Option<(u64, u64)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(leaf) = self.mermaid_target(target, cx) else {
            cx.propagate();
            return;
        };
        // Returning from result view keeps the actual source selection. Only
        // entering from elsewhere chooses the first byte of the fenced body.
        let inside = leaf.range.start <= self.state.selection.head()
            && self.state.selection.head() <= leaf.range.end;
        let at = leaf.range.start + leaf.text.find('\n').map_or(0, |at| at + 1);
        self.mermaid_authoring.preview = None;
        if !inside {
            self.select_range(at..at, cx);
        }
        self.focus.focus(window, cx);
        self.geometry = None;
        cx.notify();
    }

    pub(super) fn show_mermaid_result(
        &mut self,
        target: Option<(u64, u64)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.state.composition.is_some() {
            return false;
        }
        let Some(leaf) = self.mermaid_target(target, cx) else {
            return false;
        };
        let id = leaf.id;
        let focus = cx.focus_handle().tab_stop(true);
        focus.focus(window, cx);
        self.mermaid_authoring.preview = Some(Preview {
            id,
            revision: self.projection.revision,
            selection: self.state.selection,
            focus,
        });
        self.geometry = None;
        cx.notify();
        true
    }

    pub(super) fn leave_mermaid_result(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.mermaid_authoring.preview.take().is_none() {
            return false;
        }
        self.focus.focus(window, cx);
        self.geometry = None;
        cx.notify();
        true
    }

    pub(super) fn render_mermaid(
        &self,
        leaf: &Leaf,
        source: AnyElement,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selection = self.state.selection.range();
        let preview = self
            .mermaid_authoring
            .preview
            .as_ref()
            .filter(|preview| preview.id == leaf.id);
        let collapsed = preview.is_some_and(|preview| {
            preview.revision == self.projection.revision
                && preview.selection == self.state.selection
                && preview.focus.contains_focused(window, cx)
        });
        let active =
            !collapsed && selection.start <= leaf.range.end && selection.end >= leaf.range.start;
        let target = (leaf.id, self.projection.revision);
        let controls = div()
            .flex()
            .gap_2()
            .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| {
                cx.stop_propagation()
            })
            .child(
                Button::new(("markdown-mermaid-edit", leaf.id))
                    .ghost()
                    .label(t(L10nKey::EditorMarkdownMermaidEdit))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.edit_mermaid_source(Some(target), window, cx);
                    })),
            )
            .when(active, |controls| {
                controls.child(
                    Button::new(("markdown-mermaid-result", leaf.id))
                        .ghost()
                        .label(t(L10nKey::EditorMarkdownMermaidResult))
                        .disabled(self.state.composition.is_some())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.show_mermaid_result(Some(target), window, cx);
                        })),
                )
            });
        let result = self
            .mermaid
            .render(leaf, active, source, controls.into_any_element(), cx);
        div()
            .id(("markdown-mermaid-preview", leaf.id))
            .w_full()
            .role(gpui_kit::Role::Group)
            .aria_label(t(L10nKey::EditorMarkdownMermaidResultLabel))
            .when_some(preview, |block, preview| {
                block
                    .track_focus(&preview.focus)
                    .key_context("MarkdownMermaidResult")
                    .focus_visible(|style| style.border_1().border_color(cx.theme().primary))
            })
            .child(result)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{EditDiagramSource, ShowDiagramResult};
    use crate::ui::app::test_window::harness_with_tabs;
    use crate::ui::code_editor::markdown::Selection;
    use gpui_kit::Focusable as _;
    use gpui_kit::test::{TestEventExt as _, TestQueryExt as _, TestWindowExt as _};
    use std::time::{Duration, Instant};

    #[gpui_kit::test]
    fn mermaid_result_and_source_actions_preserve_document_selection_and_history(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("authoring.md");
        let original =
            b"before\r\n\r\n```mermaid\r\nflowchart LR\r\nA[Start] --> B[End]\r\n```\n\nafter";
        std::fs::write(&path, original).unwrap();
        let source = String::from_utf8(original.to_vec())
            .unwrap()
            .replace("\r\n", "\n");
        let (app, mut context, _streams) = harness_with_tabs(cx, 1);
        app.update_in(&mut context, |app, window, cx| {
            app.open_file_in_editor(&path, window, cx)
        });
        let deadline = Instant::now() + Duration::from_secs(20);
        let view = loop {
            context.run_until_parked();
            if let Some(view) = app.read_with(&context, |app, _| {
                app.active_editor_input()
                    .and_then(|input| input.markdown().cloned())
            }) {
                break view;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        };
        let fence = source.find("```mermaid").unwrap();
        let image_id = format!("markdown-mermaid-image-{fence}");
        loop {
            context.executor().advance_clock(Duration::from_millis(100));
            context.run_until_parked();
            if context.update(|window, cx| {
                window.render_frame(cx);
                window.try_find(image_id.clone()).is_some()
            }) {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let source_id = view.read_with(&context, |view, _| {
            let leaf = view
                .projection
                .leaves
                .iter()
                .find(|leaf| leaf.range.start == fence)
                .unwrap();
            ("markdown-mermaid-source", leaf.id)
        });
        context.update(|window, cx| {
            let edit = crate::ui::i18n::t(crate::ui::i18n::L10nKey::EditorMarkdownMermaidEdit);
            let point = window.find_by_label(edit).bounds().center();
            window.pointer_down(point, gpui_kit::MouseButton::Left, cx);
            window.pointer_up(point, gpui_kit::MouseButton::Left, cx);
            window.render_frame(cx);
            assert!(window.try_find(source_id).is_some());
        });
        let at = source.find("Start").unwrap();
        view.update_in(&mut context, |view, window, cx| {
            view.select_range(at..at + 5, cx);
            view.focus_handle(cx).focus(window, cx);
        });
        let identity = view.read_with(&context, |view, _| view.document().entity_id());
        let offset = view.read_with(&context, |view, _| view.scroll.offset());
        context.dispatch_action(ShowDiagramResult);
        context.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.try_find(source_id).is_none());
            assert!(window.try_find(image_id.clone()).is_some());
            window.press("x", cx);
        });
        view.read_with(&context, |view, cx| {
            assert_eq!(view.selection(), Selection::new(at, at + 5));
            // Preview owns focus; typing must not target hidden source.
            assert_eq!(view.scroll.offset(), offset);
            assert_eq!(view.document().read(cx).source().to_string(), source);
        });
        context.update(|window, cx| {
            assert!(!view.read(cx).focus_handle(cx).is_focused(window));
        });
        context.dispatch_action(EditDiagramSource);
        context.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.try_find(source_id).is_some());
            window.input("Changed", cx);
        });
        context.dispatch_action(gpui_kit::component::input::Undo);
        view.read_with(&context, |view, cx| {
            assert_eq!(view.document().entity_id(), identity);
            assert_eq!(view.document().read(cx).source().to_string(), source);
        });
        context.dispatch_action(gpui_kit::component::input::Redo);
        context.dispatch_action(ShowDiagramResult);
        context.update(|window, cx| {
            window.render_frame(cx);
            window.press("escape", cx);
        });
        view.read_with(&context, |view, cx| {
            assert_eq!(view.selection(), Selection::caret(at + "Changed".len()));
            assert_eq!(
                view.document().read(cx).source().to_string(),
                source.replacen("Start", "Changed", 1)
            );
        });
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }
}
