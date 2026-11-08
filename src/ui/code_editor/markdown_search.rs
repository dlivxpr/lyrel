//! Native Markdown find UI; the shared document remains the only editable text.
use super::markdown::{
    Command, DocumentEvent, EditKind, SearchSurfaceChanged, SearchText, Selection,
    SourcePresentation,
};
use super::*;
use gpui_kit::base::input::SearchMatcher;
use gpui_kit::{App, WeakEntity};

pub(super) struct MarkdownSearch {
    source: WeakEntity<SourcePresentation>,
    query: Entity<InputState>,
    replacement: Entity<InputState>,
    focus: gpui_kit::FocusHandle,
    matcher: SearchMatcher,
    matched_query: SharedString,
    query_dirty: bool,
    matched_case_sensitive: bool,
    matched_revision: Option<(u64, u64, bool)>,
    surface: SearchText,
    case_sensitive: bool,
    replace_mode: bool,
    open: bool,
    _subscriptions: Vec<Subscription>,
}

impl MarkdownSearch {
    pub(super) fn new(
        source: &Entity<SourcePresentation>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder(t(L10nKey::SearchFind)));
        let replacement = cx
            .new(|cx| InputState::new(window, cx).placeholder(t(L10nKey::EditorSearchReplacement)));
        let document = source.read(cx).document().clone();
        let subscriptions = vec![
            cx.subscribe_in(&query, window, |this, _, event, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.query_dirty = true;
                    this.refresh(true, window, cx);
                }
            }),
            cx.subscribe_in(&document, window, |this, _, event, window, cx| {
                if this.open
                    && matches!(
                        event,
                        DocumentEvent::Edited(_) | DocumentEvent::PresentationChanged
                    )
                {
                    this.refresh(false, window, cx);
                }
            }),
            cx.subscribe_in(
                source,
                window,
                |this, _, _: &SearchSurfaceChanged, window, cx| {
                    if this.open {
                        this.refresh(false, window, cx);
                    }
                },
            ),
        ];
        Self {
            source: source.downgrade(),
            query,
            replacement,
            focus: cx.focus_handle(),
            matcher: SearchMatcher::new(),
            matched_query: SharedString::default(),
            query_dirty: true,
            matched_case_sensitive: false,
            matched_revision: None,
            surface: SearchText::default(),
            case_sensitive: false,
            replace_mode: false,
            open: false,
            _subscriptions: subscriptions,
        }
    }

    pub(super) fn has_focus(&self, window: &Window, cx: &App) -> bool {
        self.open && self.focus.contains_focused(window, cx)
    }

    pub(super) fn show(&mut self, replace_mode: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.open = true;
        self.replace_mode = replace_mode;
        if let Some(source) = self.source.upgrade() {
            let view = source.read(cx);
            let range = view.selection().range();
            if range.start < range.end {
                let surface = view.search_text(cx);
                let selected = surface
                    .text
                    .slice(surface.visible_offset(range.start)..surface.visible_offset(range.end))
                    .to_string();
                if !selected.is_empty() {
                    self.query_dirty = true;
                    self.query
                        .update(cx, |input, cx| input.set_value(selected, window, cx));
                }
            }
        }
        self.query.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        self.refresh(true, window, cx);
    }

    fn refresh(&mut self, select: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(source) = self.source.upgrade() else {
            return;
        };
        let view = source.read(cx);
        let at = view.selection().range().start;
        let surface_key = view.search_surface_key(cx);
        let query = if self.query_dirty {
            self.query_dirty = false;
            self.query.read(cx).value()
        } else {
            self.matched_query.clone()
        };
        let query_changed =
            self.matched_query != query || self.matched_case_sensitive != self.case_sensitive;
        let matches_changed = query_changed || self.matched_revision != Some(surface_key);
        if self.matched_revision != Some(surface_key) {
            if query_changed {
                self.matcher.query = None;
            }
            self.surface = view.search_text(cx);
            self.matcher.update(&self.surface.text);
            self.matched_revision = Some(surface_key);
        }
        if query_changed {
            self.matcher.update_query(&query, !self.case_sensitive);
            self.matched_query = query;
            self.matched_case_sensitive = self.case_sensitive;
        }
        self.matcher
            .update_cursor_by_offset(self.surface.visible_offset(at));
        if matches_changed {
            let matches = std::rc::Rc::new(
                self.matcher
                    .matched_ranges()
                    .iter()
                    .cloned()
                    .flat_map(|range| self.surface.source_ranges(range))
                    .collect(),
            );
            source.update(cx, |view, cx| view.set_search_matches(matches, cx));
        }
        if select {
            self.select_current(window, cx);
        }
        cx.notify();
    }

    fn select_current(&self, window: &mut Window, cx: &mut Context<Self>) {
        let range = self
            .matcher
            .matched_ranges()
            .get(self.matcher.current_match_index())
            .cloned();
        if let (Some(source), Some(range)) = (self.source.upgrade(), range) {
            let fragments = self.surface.source_ranges(range);
            let (Some(first), Some(last)) = (fragments.first(), fragments.last()) else {
                return;
            };
            let range = first.start..last.end;
            source.update(cx, |view, cx| {
                view.command(
                    Command::Select(Selection::new(range.start, range.end)),
                    window,
                    cx,
                )
            });
        }
    }

    fn step(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.refresh(false, window, cx);
        if forward {
            self.matcher.next();
        } else {
            self.matcher.next_back();
        }
        self.select_current(window, cx);
        cx.notify();
    }

    fn replace(&mut self, all: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.refresh(false, window, cx);
        let replacement = self.replacement.read(cx).value().to_string();
        let ranges = self.matcher.matched_ranges();
        let edits = if all {
            ranges
                .iter()
                .cloned()
                .flat_map(|range| self.surface.replacement(range, &replacement))
                .collect()
        } else {
            ranges
                .get(self.matcher.current_match_index())
                .cloned()
                .map(|range| self.surface.replacement(range, &replacement))
                .unwrap_or_default()
        };
        if let Some(source) = self.source.upgrade() {
            source.update(cx, |view, cx| {
                view.command(
                    Command::BatchReplace {
                        edits,
                        kind: EditKind::Command,
                    },
                    window,
                    cx,
                )
            });
        }
        self.refresh(true, window, cx);
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = false;
        self.matched_revision = None;
        if let Some(source) = self.source.upgrade() {
            source.update(cx, |view, cx| {
                view.set_search_matches(std::rc::Rc::default(), cx)
            });
            source.read(cx).focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }
}

impl Render for MarkdownSearch {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::component::input::{Enter, Escape, IndentInline, OutdentInline};
        use gpui_kit::component::{Disableable as _, Selectable as _};
        if !self.open {
            return gpui_kit::Empty.into_any_element();
        }
        v_flex()
            .id("markdown-search")
            .key_context("SearchPanel MarkdownSearch")
            .track_focus(&self.focus)
            .flex_none()
            .w_full()
            .gap_1()
            .p_2()
            .border_b_1()
            .border_color(cx.theme().sidebar_border)
            .on_action(
                cx.listener(|this, action: &Enter, window, cx| {
                    this.step(!action.shift, window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &Escape, window, cx| this.close(window, cx)))
            .on_action(cx.listener(|this, _: &IndentInline, window, cx| {
                let focus = if this.replace_mode
                    && this.query.read(cx).focus_handle(cx).is_focused(window)
                {
                    this.replacement.read(cx).focus_handle(cx)
                } else {
                    this.query.read(cx).focus_handle(cx)
                };
                focus.focus(window, cx);
            }))
            .on_action(cx.listener(|this, _: &OutdentInline, window, cx| {
                let focus = if this.replace_mode
                    && this.query.read(cx).focus_handle(cx).is_focused(window)
                {
                    this.replacement.read(cx).focus_handle(cx)
                } else {
                    this.query.read(cx).focus_handle(cx)
                };
                focus.focus(window, cx);
            }))
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Input::new(&self.query)
                            .id("markdown-query")
                            .small()
                            .flex_1(),
                    )
                    .child(
                        Button::new("markdown-match-case")
                            .xsmall()
                            .ghost()
                            .icon(IconName::CaseSensitive)
                            .tooltip(t(L10nKey::SearchMatchCase))
                            .selected(self.case_sensitive)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.case_sensitive = !this.case_sensitive;
                                this.refresh(true, window, cx);
                            })),
                    )
                    .child(
                        Button::new("markdown-toggle-replace")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Replace)
                            .tooltip(t(L10nKey::EditorSearchReplacement))
                            .selected(self.replace_mode)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.replace_mode = !this.replace_mode;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("markdown-find-prev")
                            .xsmall()
                            .ghost()
                            .icon(IconName::ChevronLeft)
                            .tooltip(t(L10nKey::AppMenuFindPrevious))
                            .disabled(self.matcher.is_empty())
                            .on_click(
                                cx.listener(|this, _, window, cx| this.step(false, window, cx)),
                            ),
                    )
                    .child(
                        Button::new("markdown-find-next")
                            .xsmall()
                            .ghost()
                            .icon(IconName::ChevronRight)
                            .tooltip(t(L10nKey::AppMenuFindNext))
                            .disabled(self.matcher.is_empty())
                            .on_click(
                                cx.listener(|this, _, window, cx| this.step(true, window, cx)),
                            ),
                    )
                    .child(
                        div()
                            .id("markdown-match-count")
                            .test_support()
                            .role(gpui_kit::Role::Status)
                            .aria_label(self.matcher.label())
                            .text_sm()
                            .child(self.matcher.label()),
                    )
                    .child(
                        Button::new("markdown-find-close")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Close)
                            .tooltip(t(L10nKey::Close))
                            .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))),
                    ),
            )
            .when(self.replace_mode, |root| {
                root.child(
                    h_flex()
                        .gap_1()
                        .child(
                            Input::new(&self.replacement)
                                .id("markdown-replacement")
                                .small()
                                .flex_1(),
                        )
                        .child(
                            Button::new("markdown-replace-one")
                                .small()
                                .label(t(L10nKey::EditorReplace))
                                .disabled(self.matcher.is_empty())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.replace(false, window, cx)
                                })),
                        )
                        .child(
                            Button::new("markdown-replace-all")
                                .small()
                                .label(t(L10nKey::EditorSearchReplaceAll))
                                .disabled(self.matcher.is_empty())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.replace(true, window, cx)
                                })),
                        ),
                )
            })
            .into_any_element()
    }
}

impl LyrelApp {
    pub(crate) fn editor_markdown_find_step(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.editor_panel_has_focus(window, cx) {
            return false;
        }
        let Some(input) = self.active_editor_input() else {
            return false;
        };
        let Some(source) = input.markdown() else {
            return false;
        };
        let Some(file) = self.active_buffer() else {
            return false;
        };
        let search = file.presentations.open_search(source, window, cx);
        search.update(cx, |search, cx| {
            if search.matched_query.is_empty() {
                search.show(false, window, cx);
            } else {
                search.open = true;
                search.step(forward, window, cx);
            }
        });
        true
    }
    pub(crate) fn editor_markdown_search(
        &mut self,
        replace: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.editor_panel_has_focus(window, cx) {
            return false;
        }
        let Some(input) = self.active_editor_input() else {
            return false;
        };
        let Some(source) = input.markdown() else {
            return false;
        };
        let Some(file) = self.active_buffer() else {
            return false;
        };
        let search = file.presentations.open_search(source, window, cx);
        search.update(cx, |search, cx| search.show(replace, window, cx));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{TestAppContext, VisualTestContext};

    fn settle(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| window.render_frame(cx));
        vcx.run_until_parked();
    }

    #[gpui_kit::test]
    fn live_search_replaces_across_hidden_markers_without_deleting_syntax(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("visible-find.md");
        let original = "alpha **beta** gamma\n";
        std::fs::write(&path, original).unwrap();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_file_in_editor(&path, window, cx)
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !app.read_with(&vcx, |app, _| app.active_buffer().is_some()) {
            settle(&mut vcx);
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        if !app.read_with(&vcx, |app, cx| {
            app.active_editor_input()
                .unwrap()
                .markdown()
                .unwrap()
                .read(cx)
                .is_live()
        }) {
            vcx.dispatch_action(crate::core::actions::ToggleDocumentPreview);
        }
        settle(&mut vcx);
        vcx.dispatch_action(gpui_kit::component::input::Replace);
        vcx.update(|window, cx| window.input("alpha beta", cx));
        settle(&mut vcx);
        while !vcx.update(|window, _| window.find("markdown-match-count").label() == Some("1/1")) {
            settle(&mut vcx);
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        vcx.update(|window, cx| {
            assert_eq!(window.find("markdown-match-count").label(), Some("1/1"));
            window.click("markdown-replacement", cx);
            window.input("new", cx);
            window.click("markdown-replace-one", cx);
        });
        settle(&mut vcx);
        app.read_with(&vcx, |app, cx| {
            assert_eq!(
                app.active_buffer()
                    .unwrap()
                    .input
                    .read(cx)
                    .text()
                    .to_string(),
                "new**** gamma\n"
            )
        });
        vcx.update(|window, cx| window.press("escape", cx));
        vcx.dispatch_action(gpui_kit::component::input::Undo);
        settle(&mut vcx);
        app.read_with(&vcx, |app, cx| {
            assert_eq!(
                app.active_buffer()
                    .unwrap()
                    .input
                    .read(cx)
                    .text()
                    .to_string(),
                original
            )
        });
        vcx.dispatch_action(gpui_kit::component::input::Search);
        vcx.update(|window, cx| window.input("**", cx));
        settle(&mut vcx);
        vcx.update(|window, _| {
            assert_eq!(window.find("markdown-match-count").label(), Some("0/0"))
        });
    }

    #[gpui_kit::test]
    fn markdown_find_replace_all_is_undoable_and_escape_restores_source_focus(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("find.md");
        std::fs::write(&path, "中🎉 alpha\nAlpha omega alpha").unwrap();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        vcx.simulate_resize(gpui_kit::size(px(1600.), px(900.)));
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_file_in_editor(&path, window, cx)
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !app.read_with(&vcx, |app, _| app.active_buffer().is_some()) {
            settle(&mut vcx);
            assert!(
                std::time::Instant::now() < deadline,
                "Markdown open did not settle"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        app.update_in(&mut vcx, |app, window, cx| {
            app.toggle_document_preview(window, cx)
        });
        vcx.dispatch_action(gpui_kit::component::input::Replace);
        settle(&mut vcx);
        vcx.update(|window, cx| window.input("alpha", cx));
        settle(&mut vcx);
        app.read_with(&vcx, |app, cx| {
            assert_eq!(
                app.active_editor_input().unwrap().read(cx).selected_range(),
                8..13
            )
        });
        vcx.update(|window, cx| {
            assert_eq!(window.find("markdown-match-count").label(), Some("1/3"));
            window.dispatch_action(Box::new(crate::terminal::view::FindNext), cx);
        });
        settle(&mut vcx);
        app.read_with(&vcx, |app, cx| {
            assert_eq!(
                app.active_editor_input().unwrap().read(cx).selected_range(),
                14..19
            )
        });
        vcx.update(|window, cx| {
            window.dispatch_action(Box::new(crate::terminal::view::FindPrevious), cx);
            window.click("markdown-match-case", cx);
        });
        settle(&mut vcx);
        vcx.update(|window, cx| {
            assert_eq!(window.find("markdown-match-count").label(), Some("1/2"));
            window.click("markdown-match-case", cx);
        });
        settle(&mut vcx);
        vcx.update(|window, cx| {
            window.click("markdown-replacement", cx);
            window.input("新", cx);
            window.click("markdown-replace-one", cx);
        });
        settle(&mut vcx);
        app.read_with(&vcx, |app, cx| {
            assert_eq!(
                app.active_buffer()
                    .unwrap()
                    .input
                    .read(cx)
                    .text()
                    .to_string(),
                "中🎉 新\nAlpha omega alpha"
            )
        });
        vcx.update(|window, cx| window.press("escape", cx));
        settle(&mut vcx);
        vcx.dispatch_action(gpui_kit::component::input::Undo);
        settle(&mut vcx);
        app.read_with(&vcx, |app, cx| {
            assert_eq!(
                app.active_buffer()
                    .unwrap()
                    .input
                    .read(cx)
                    .text()
                    .to_string(),
                "中🎉 alpha\nAlpha omega alpha"
            )
        });
        vcx.dispatch_action(gpui_kit::component::input::Replace);
        settle(&mut vcx);
        vcx.update(|window, cx| {
            window.click("markdown-replace-all", cx);
        });
        settle(&mut vcx);
        app.read_with(&vcx, |app, cx| {
            assert_eq!(
                app.active_buffer()
                    .unwrap()
                    .input
                    .read(cx)
                    .text()
                    .to_string(),
                "中🎉 新\n新 omega 新"
            );
            assert!(app.active_buffer().unwrap().dirty);
        });
        vcx.update(|window, cx| window.press("escape", cx));
        settle(&mut vcx);
        app.update_in(&mut vcx, |app, window, cx| {
            assert!(app.editor_has_focus(window, cx))
        });
        vcx.dispatch_action(gpui_kit::component::input::Undo);
        settle(&mut vcx);
        app.read_with(&vcx, |app, cx| {
            assert_eq!(
                app.active_buffer()
                    .unwrap()
                    .input
                    .read(cx)
                    .text()
                    .to_string(),
                "中🎉 alpha\nAlpha omega alpha"
            );
            assert!(!app.active_buffer().unwrap().dirty);
        });
    }
}
