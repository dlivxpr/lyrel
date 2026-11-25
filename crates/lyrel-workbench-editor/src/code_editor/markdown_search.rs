//! Native Markdown find UI; the shared document remains the only editable text.
use super::*;
use gpui_kit::base::input::SearchMatcher;
use gpui_kit::{App, WeakEntity};
use lyrel_markdown_editor::{DocumentEvent, EditorState as MarkdownEditor};

pub struct MarkdownSearch {
    source: WeakEntity<MarkdownEditor>,
    query: Entity<InputState>,
    replacement: Entity<InputState>,
    focus: gpui_kit::FocusHandle,
    matcher: SearchMatcher,
    matched_query: SharedString,
    query_dirty: bool,
    matched_case_sensitive: bool,
    matched_revision: Option<u64>,
    surface: gpui_kit::component::input::Rope,
    case_sensitive: bool,
    replace_mode: bool,
    open: bool,
    _subscriptions: Vec<Subscription>,
}

impl MarkdownSearch {
    pub fn new(
        source: &Entity<MarkdownEditor>,
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
            cx.subscribe_in(
                &document,
                window,
                |this, _, _: &DocumentEvent, window, cx| {
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
            surface: gpui_kit::component::input::Rope::new(),
            case_sensitive: false,
            replace_mode: false,
            open: false,
            _subscriptions: subscriptions,
        }
    }

    pub fn has_focus(&self, window: &Window, cx: &App) -> bool {
        self.open && self.focus.contains_focused(window, cx)
    }

    pub fn show(&mut self, replace_mode: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.open = true;
        self.replace_mode = replace_mode;
        if let Some(source) = self.source.upgrade() {
            let view = source.read(cx);
            let range = view.selected_range();
            if range.start < range.end {
                let selected = view.text()[range].to_owned();
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
        let surface_key = view.document().read(cx).revision();
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
            self.surface = gpui_kit::component::input::Rope::from(view.text());
            self.matcher.update(&self.surface);
            self.matched_revision = Some(surface_key);
        }
        if query_changed {
            self.matcher.update_query(&query, !self.case_sensitive);
            self.matched_query = query;
            self.matched_case_sensitive = self.case_sensitive;
        }
        self.matcher.update_cursor_by_offset(view.cursor());
        if matches_changed {
            let ranges = self.matcher.matched_ranges().to_vec();
            let active = (!ranges.is_empty()).then_some(self.matcher.current_match_index());
            source.update(cx, |view, cx| view.set_search(ranges, active, cx));
        }
        if select {
            self.select_current(window, cx);
        }
        cx.notify();
    }

    fn select_current(&self, _window: &mut Window, cx: &mut Context<Self>) {
        let range = self
            .matcher
            .matched_ranges()
            .get(self.matcher.current_match_index())
            .cloned();
        if let (Some(source), Some(range)) = (self.source.upgrade(), range) {
            let ranges = self.matcher.matched_ranges().to_vec();
            let active = Some(self.matcher.current_match_index());
            source.update(cx, |view, cx| view.set_search(ranges, active, cx));
            source.update(cx, |view, cx| view.select_range(range, cx));
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
                .map(|range| (range, replacement.clone()))
                .collect()
        } else {
            ranges
                .get(self.matcher.current_match_index())
                .cloned()
                .map(|range| vec![(range, replacement.clone())])
                .unwrap_or_default()
        };
        if let Some(source) = self.source.upgrade() {
            source.update(cx, |view, cx| view.replace_ranges(edits, cx));
        }
        self.refresh(true, window, cx);
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = false;
        self.matched_revision = None;
        if let Some(source) = self.source.upgrade() {
            source.update(cx, |view, cx| view.set_search(Vec::new(), None, cx));
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
                            .tooltip(t(L10nKey::EditorReplace))
                            .selected(self.replace_mode)
                            .aria_expanded(self.replace_mode)
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

impl EditorPanelState {
    pub fn markdown_find_has_focus(
        &self,
        tab: TabId,
        code: &TabCode,
        window: &Window,
        cx: &App,
    ) -> bool {
        self.active_file(code)
            .zip(self.active_editor_input(tab, code))
            .and_then(|(file, input)| file.presentations.search(input.markdown()?))
            .is_some_and(|search| search.read(cx).has_focus(window, cx))
    }

    /// Find next/previous in the front Markdown document. `panel_has_focus`
    /// is the shell's focus answer, passed as a value.
    pub fn markdown_find_step(
        &mut self,
        forward: bool,
        panel_has_focus: bool,
        tab: TabId,
        code: &TabCode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !panel_has_focus {
            return false;
        }
        let Some(input) = self.active_editor_input(tab, code) else {
            return false;
        };
        let Some(source) = input.markdown() else {
            return false;
        };
        let Some(file) = self.active_file(code) else {
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

    /// Opens the front Markdown document's find (or find-and-replace).
    /// `panel_has_focus` is the shell's focus answer, passed as a value.
    pub fn markdown_search(
        &mut self,
        replace: bool,
        panel_has_focus: bool,
        tab: TabId,
        code: &TabCode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !panel_has_focus {
            return false;
        }
        let Some(input) = self.active_editor_input(tab, code) else {
            return false;
        };
        let Some(source) = input.markdown() else {
            return false;
        };
        let Some(file) = self.active_file(code) else {
            return false;
        };
        let search = file.presentations.open_search(source, window, cx);
        search.update(cx, |search, cx| search.show(replace, window, cx));
        true
    }
}
