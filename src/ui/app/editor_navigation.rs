//! Shell orchestration for editor navigation and the header's file strip.
//!
//! Back/forward history, Go to Symbol, the breadcrumbs over the text, and
//! the strip of files in the header keep their document state on
//! `Entity<EditorPanelState>` (`ui::code_editor::{nav, strip}`); this module
//! owns the workbench side — the active tab and its `TabCode`, the search
//! palette the symbol picker lives in, the file tree's project spelling the
//! breadcrumbs render with, and the focus and modal decisions that decide
//! whether a strip or navigation key may act. The panel answers through the
//! same shell methods the workbench has always called; only where they land
//! has changed (#327: no editor module holds a production `impl LyrelApp`).

use gpui_kit::component::input::Input;
use gpui_kit::component::{Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::{Anchor, AnyElement, FontWeight, SharedString, anchored, deferred, div, px, rems};

use super::*;
use crate::ui::document_column::{
    DocumentChrome, HEADER_CELL_H as CELL_H, HEADER_CELL_RADIUS as CELL_RADIUS,
    HEADER_CELL_TEXT as CELL_TEXT,
};
use crate::ui::search::{CommandKind, Item, SearchTab};
use lyrel_workbench_editor::code_editor::outline::{self, SymbolKind};
use lyrel_workbench_editor::code_editor::{
    BreadcrumbFacts, BufferId, CURSOR_SCROLL_ATTEMPTS, NavOpen, PICKER_LIST_MAX_H, PICKER_ROW_H,
    PICKER_W, TabCode, path_segments, place_cursor,
};
use lyrel_workbench_editor::lsp::{Found, WorkspaceHit};

impl LyrelApp {
    // ---- The active tab, and the code it shows ----

    pub(crate) fn nav_tab(&self) -> Option<TabId> {
        self.tabs.get(self.tabs.active_index()).map(|t| t.id())
    }

    /// Samples the caret for jumps: the shell's frame hook, run whenever the
    /// editor draws. The history itself lives on the editor entity.
    pub(crate) fn editor_nav_tick(&mut self, cx: &mut Context<Self>) {
        let (tab, code) = (self.nav_tab(), self.tab_code());
        self.editor
            .update(cx, |editor, cx| editor.nav_tick(tab, code, cx));
    }

    /// Back (or, with `forward`, Forward) through the active tab's history.
    pub(crate) fn editor_navigate(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.nav_tab() else { return };
        let code = self.tab_code();
        let Some(open) = self
            .editor
            .update(cx, |editor, cx| editor.navigate(tab, code, forward, cx))
        else {
            return;
        };
        match open {
            // An open buffer: shown in its tab, then the caret lands where
            // the history entry says.
            NavOpen::Show { id, pos } => {
                self.editor_show_in_tab(tab, id, true, window, cx);
                let group = self.tab_code().map(|code| code.group());
                if let Some(input) =
                    group.and_then(|group| self.editor.read(cx).editor_input_for(id, tab, group))
                {
                    place_cursor(input, pos, CURSOR_SCROLL_ATTEMPTS, window, cx);
                }
            }
            // The same road a file link takes, so a file closed since opens
            // again.
            NavOpen::Open { host, path, pos } => {
                self.editor_open_on_host_at(host, &path, Some(pos), window, cx)
            }
        }
    }

    // ---- Go to Symbol ----

    /// Go to Symbol in the file in front: the search, on its Symbols tab.
    /// The same chord puts it away again.
    pub(crate) fn editor_go_to_symbol(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self.search.view() {
            if view.read(cx).tab() == SearchTab::Symbols {
                self.close_search(window, cx);
                return;
            }
            self.close_search(window, cx);
        }
        let Some(tab) = self.nav_tab() else { return };
        let Some(code) = self.tab_code() else { return };
        let Some(id) = code.active_id() else { return };
        let is_text = self
            .editor
            .read(cx)
            .active_file(code)
            .is_some_and(|file| file.input.is_text());
        // 图片没有符号：打开一个空的符号选择器只会让人以为文件读坏了。
        if !is_text {
            return;
        }
        let input = match self.editor.read(cx).active_editor_input(tab, code) {
            Some(input) => input,
            None => return,
        };
        let Some(text) = input.text() else {
            return;
        };
        let (origin, scroll) = (
            text.read(cx).cursor_position(),
            input.read(cx).scroll_offset(),
        );
        let outline = self
            .editor
            .update(cx, |editor, cx| editor.outline(id, true, cx))
            .unwrap_or_default();
        let rows = symbol_rows(&outline);
        let here = outline
            .chain_at(origin)
            .last()
            .map(|&ix| symbol_command(&outline.symbols[ix]));
        self.open_search(SearchTab::Symbols, "", window, cx);
        self.editor.update(cx, |editor, _| {
            editor.nav_start_preview(id, input.downgrade(), origin, scroll);
        });
        let live = self.lsp_project_symbol_query(window, cx);
        if let Some(view) = self.search.view() {
            view.update(cx, |view, cx| {
                view.set_symbols(rows, here, window, cx);
                if let Some(live) = live {
                    view.set_live_query(live, window, cx);
                }
            });
        }
    }

    /// The editor's search row as this window can fill it: Files always,
    /// Symbols with a text document in front.
    pub(crate) fn editor_search_tabs(&self, cx: &gpui_kit::App) -> Vec<SearchTab> {
        let mut tabs = vec![SearchTab::Files];
        if self
            .tab_code()
            .and_then(|code| self.editor.read(cx).active_file(code))
            .is_some_and(|file| file.input.is_text())
        {
            tabs.push(SearchTab::Symbols);
        }
        tabs
    }

    /// Begins a preview in the file in front for a picker someone else
    /// filled — Find References (`ui::lsp`) — so arrowing through it moves
    /// the caret and closing it puts the caret back, as Go to Symbol does.
    pub(crate) fn editor_begin_preview(&mut self, cx: &mut gpui_kit::App) {
        let (Some(tab), Some(code)) = (self.nav_tab(), self.tab_code()) else {
            return;
        };
        self.editor.update(cx, |editor, cx| {
            editor.begin_preview(tab, code, cx);
        });
    }

    /// The buffer a preview is running in, if one is.
    pub(crate) fn editor_preview_buffer(&self, cx: &gpui_kit::App) -> Option<BufferId> {
        self.editor.read(cx).preview_buffer()
    }

    /// Shows a symbol the picker is on, leaving the keyboard in the picker.
    pub(crate) fn editor_symbol_preview(&mut self, line: u32, column: u32, cx: &mut Context<Self>) {
        self.editor.update(cx, |editor, cx| {
            editor.symbol_preview(line, column, cx);
        });
    }

    /// The picker's choice is final: the caret stays where it took it.
    pub(crate) fn editor_symbol_preview_commit(&mut self, cx: &mut Context<Self>) {
        self.editor.update(cx, |editor, _| editor.preview_commit());
    }

    /// The picker went away without a choice: the caret and the scroll go
    /// back to where they were, and so does the keyboard.
    pub(crate) fn editor_symbol_preview_cancel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.update(cx, |editor, cx| {
            editor.preview_cancel(window, cx);
        });
    }

    /// Puts the caret on a 0-based position in the buffer in front.
    pub(crate) fn editor_go_to_position(
        &mut self,
        line: u32,
        column: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.nav_tab() else {
            return;
        };
        let code = self.tab_code();
        self.editor.update(cx, |editor, cx| {
            editor.go_to_position(tab, code, line, column, window, cx);
        });
    }

    /// The palette's editor rows, offered while a file is in front of it.
    pub(crate) fn editor_palette_items(&self, cx: &gpui_kit::App) -> Vec<Item> {
        use crate::ui::search::CommandGroup;
        if !self.code_panel_visible()
            || self
                .tab_code()
                .and_then(|code| self.editor.read(cx).active_file(code))
                .is_none()
        {
            return Vec::new();
        }
        [
            (L10nKey::CmdEditorGoToSymbol, CommandKind::EditorGoToSymbol),
            (L10nKey::CmdEditorGoBack, CommandKind::EditorNavigateBack),
            (
                L10nKey::CmdEditorGoForward,
                CommandKind::EditorNavigateForward,
            ),
            (L10nKey::CmdEditorSplitRight, CommandKind::EditorSplitRight),
            (
                L10nKey::EditorFocusLeftGroup,
                CommandKind::EditorFocusLeftGroup,
            ),
            (
                L10nKey::EditorFocusRightGroup,
                CommandKind::EditorFocusRightGroup,
            ),
            (
                L10nKey::FocusDocumentTabNavigation,
                CommandKind::FocusDocumentTabNavigation,
            ),
            (
                L10nKey::EditorDocumentOverflow,
                CommandKind::EditorDocumentOverflow,
            ),
        ]
        .into_iter()
        .map(|(key, kind)| Item::localized(key, kind).in_group(CommandGroup::View))
        .collect()
    }

    /// The editor's own chords, handled by the element that holds it.
    pub(crate) fn editor_nav_actions<E: InteractiveElement>(
        &self,
        element: E,
        cx: &mut Context<Self>,
    ) -> E {
        use lyrel_ui_base::actions::{
            EditorDocumentOverflow, EditorFocusLeftGroup, EditorFocusRightGroup, EditorGoToSymbol,
            EditorNavigateBack, EditorNavigateForward, EditorSplitRight,
            FocusDocumentTabNavigation,
        };
        element
            .on_action(cx.listener(|this, _: &EditorSplitRight, window, cx| {
                if !this.editor_split(window, cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &EditorFocusLeftGroup, window, cx| {
                if !this.editor_focus_group(true, window, cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &EditorFocusRightGroup, window, cx| {
                if !this.editor_focus_group(false, window, cx) {
                    cx.propagate();
                }
            }))
            .on_action(
                cx.listener(|this, _: &FocusDocumentTabNavigation, window, cx| {
                    if !this.focus_document_tab_navigation(window, cx) {
                        cx.propagate();
                    }
                }),
            )
            .on_action(cx.listener(|this, _: &EditorDocumentOverflow, window, cx| {
                if !this.editor_document_overflow(window, cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &EditorGoToSymbol, window, cx| {
                this.editor_go_to_symbol(window, cx)
            }))
            .on_action(cx.listener(|this, _: &EditorNavigateBack, window, cx| {
                this.editor_navigate(false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &EditorNavigateForward, window, cx| {
                this.editor_navigate(true, window, cx)
            }))
    }

    // ---- Breadcrumbs ----

    /// The row over the text: where the file is, then which symbols the caret
    /// is inside. The symbols open Go to Symbol.
    pub(crate) fn render_editor_breadcrumbs(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let id = self.tab_code().and_then(TabCode::active_id)?;
        let slot = self.tab_code().map_or(0, TabCode::focused_slot);
        self.render_editor_breadcrumbs_for(id, slot, true, window, cx)
    }

    /// The breadcrumbs of one group's file. `focused` is whether that group
    /// has the focus; the other group's symbols first bring the focus over.
    pub(crate) fn render_editor_breadcrumbs_for(
        &mut self,
        id: BufferId,
        slot: usize,
        focused: bool,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let code = self.tab_code()?;
        let group = if code.focused_slot() == slot {
            code.group()
        } else {
            code.split.as_ref()?.id
        };
        let tab = self.tabs.get(self.tabs.active_index())?.id();
        let BreadcrumbFacts {
            label,
            path,
            untitled,
            language,
            host,
            caret,
        } = self.editor.read(cx).breadcrumb_facts(id, tab, group, cx)?;
        // A picture has no caret to chain symbols from.
        let caret = caret?;
        let segments: Vec<String> = if untitled {
            vec![label.to_string()]
        } else {
            let roots = self.tab_code().map(|c| c.roots.as_slice()).unwrap_or(&[]);
            let local = host == self.spawn_host(cx);
            match local {
                true => path_segments(
                    &self.file_tree.read(cx).project_spelling(host, roots, &path),
                    roots,
                ),
                false => path_segments(&path, &[]),
            }
        };
        let file = (!untitled).then(|| path.clone());
        let supported = !untitled && outline::supports(language);
        let chain: Vec<(SymbolKind, String)> = match supported {
            true => self
                .editor
                .update(cx, |editor, cx| editor.outline(id, false, cx))
                .map(|o| {
                    o.chain_at(caret)
                        .into_iter()
                        .map(|ix| (o.symbols[ix].kind, o.symbols[ix].name.clone()))
                        .collect()
                })
                .unwrap_or_default(),
            false => Vec::new(),
        };

        let theme = cx.theme();
        let (fg, muted) = (theme.foreground, theme.muted_foreground);
        // A chevron a rung fainter than the names, so the path reads as its
        // names and the separators recede.
        let sep = || {
            div().flex_none().px(px(4.)).child(
                Icon::new(IconName::ChevronRight)
                    .size(px(9.))
                    .text_color(muted.opacity(0.6)),
            )
        };
        let last = segments.len().saturating_sub(1);
        let mut path_row = h_flex()
            .flex_shrink(1.)
            .min_w_0()
            .overflow_hidden()
            .items_center();
        for (i, seg) in segments.into_iter().enumerate() {
            if i > 0 {
                path_row = path_row.child(sep());
            }
            path_row = path_row.child(
                div()
                    .flex_none()
                    .whitespace_nowrap()
                    .text_color(if i == last { fg } else { muted })
                    .child(seg),
            );
        }
        let path_row = div()
            .id(("editor-breadcrumb-path", slot))
            .flex_shrink(1.)
            .min_w_0()
            .overflow_hidden()
            .cursor_pointer()
            .child(path_row)
            .on_click(cx.listener(move |this, _, _window, cx| {
                if let Some(file) = &file {
                    this.file_tree_reveal_path(file, cx);
                }
            }));

        let symbols = supported.then(|| {
            let mut row = h_flex().min_w_0().overflow_hidden().items_center();
            if chain.is_empty() {
                row = row.child(sep()).child(div().text_color(muted).child("…"));
            }
            for (kind, name) in chain {
                row = row.child(sep()).child(
                    h_flex()
                        .flex_none()
                        .gap(px(4.))
                        .whitespace_nowrap()
                        // An impl is named by its header, `impl` and all.
                        .when(kind != SymbolKind::Impl, |d| {
                            d.child(div().text_color(muted).child(kind.tag()))
                        })
                        // The symbol is code, so it is set as code, a rung
                        // darker than the path it hangs off.
                        .child(
                            div()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_color(fg.opacity(0.6))
                                .child(name),
                        ),
                );
            }
            div()
                .id(("editor-breadcrumb-symbols", slot))
                .flex_shrink(1.)
                .min_w_0()
                .overflow_hidden()
                .rounded(px(4.))
                .cursor_pointer()
                .hover(|s| s.bg(cx.theme().muted))
                .child(row)
                .tooltip(|window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(t(L10nKey::EditorGoToSymbolAction))
                        .build(window, cx)
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    if !focused {
                        let left = slot == 0;
                        this.editor_focus_group(left, window, cx);
                    }
                    this.editor_go_to_symbol(window, cx)
                }))
        });

        Some(
            h_flex()
                .id(("editor-breadcrumbs", slot))
                .flex_none()
                .w_full()
                .h(px(22.))
                .items_center()
                .overflow_hidden()
                .px(px(crate::ui::window_chrome::CONTENT_INSET))
                .text_size(gpui_kit::rems(crate::ui::right_panel::META))
                .child(path_row)
                .children(symbols)
                .into_any_element(),
        )
    }

    // ---- The header's file strip ----

    /// How many names the strip holds. Fixed counts rather than a measure:
    /// names differ so much in width that a measured fit would reshuffle the
    /// strip whenever a long one came forward.
    pub(crate) fn editor_strip_cap(&self, chrome: DocumentChrome, cx: &gpui_kit::App) -> usize {
        let (left, right) = (self.left_panel_open(cx), self.right_panel_open(cx));
        match chrome {
            DocumentChrome::Fill if left && right => 5,
            DocumentChrome::Fill => 7,
            _ if right => 2,
            _ => 3,
        }
    }

    /// The cell Enter or Space would activate: the focused group's remembered
    /// cell while it is still one of the cells on screen, otherwise the
    /// document in front, otherwise the first cell.
    pub(crate) fn editor_strip_target(
        &self,
        visible: &[BufferId],
        cx: &gpui_kit::App,
    ) -> Option<BufferId> {
        self.editor.read(cx).strip_target(self.tab_code(), visible)
    }

    /// Whether a real terminal or the ordinary editor content owns the
    /// keyboard. Navigation entry must not fire through a picker, an editor
    /// prompt, or another workbench input merely because it has Input context.
    fn document_navigation_content_has_focus(&self, window: &Window, cx: &Context<Self>) -> bool {
        if self.editor_has_focus(window, cx) || self.editor.read(cx).strip_keyboard_focused(window)
        {
            return true;
        }
        self.tabs.get(self.tabs.active_index()).is_some_and(|tab| {
            tab.pane
                .leaves()
                .iter()
                .any(|leaf| leaf.contains_focused(window, cx))
        })
    }

    /// Moves the keyboard into the focused group's strip, on the document it
    /// remembers. Nothing is brought to the front: entering the strip shows
    /// what Enter would open, and leaves the text as it was.
    pub(crate) fn focus_document_tab_navigation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.code_panel_visible()
            || self.tab_code().is_none_or(|code| code.files.is_empty())
            || self.sidebar_nav_overlay_open(window, cx)
            || !self.document_navigation_content_has_focus(window, cx)
        {
            return false;
        }
        self.editor
            .update(cx, |editor, cx| editor.focus_strip_keyboard(window, cx));
        cx.notify();
        true
    }

    /// Opens — or closes — the `+N` list from the keyboard. The list holds
    /// every open document, hidden ones first, and its filter box takes the
    /// typing; choosing one brings it to the front.
    pub(crate) fn editor_document_overflow(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.code_panel_visible() || self.tab_code().is_none_or(|code| code.files.is_empty()) {
            return false;
        }
        if self.editor.read(cx).strip_picker_open() {
            // The same chord closes the list only while the list itself holds
            // the keyboard. A picker sitting under a search, a prompt or
            // another overlay does not answer for that surface's keys.
            if !self.editor.read(cx).strip_picker_has_focus(window, cx) {
                return false;
            }
        } else if self.sidebar_nav_overlay_open(window, cx)
            || !self.document_navigation_content_has_focus(window, cx)
        {
            return false;
        }
        self.editor_toggle_strip_picker(window, cx, true);
        true
    }

    /// The strip's keyboard, while the strip holds it: the focused cell moves
    /// with Left/Right — wrapping at either end — and Home/End, and only
    /// Enter or Space change the document in front. Escape hands the keyboard
    /// back to the group's text; anything else is not the strip's.
    ///
    /// `visible` is the cells the last draw put on screen. A document closed
    /// since is dropped here rather than acted on by position.
    pub(crate) fn editor_strip_key(
        &mut self,
        visible: &[BufferId],
        ev: &gpui_kit::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(self.tabs.active_index()).map(|t| t.id()) else {
            return;
        };
        let Some(code) = self.tabs.active_mut().and_then(|t| t.code.as_deref_mut()) else {
            return;
        };
        self.editor.update(cx, |editor, cx| {
            editor.strip_key(tab, code, visible, ev, window, cx);
        });
    }

    /// Pointer activation of a strip cell: the document comes to the front
    /// and its text takes the keyboard, as clicking a tab always did.
    pub(crate) fn editor_strip_activate(
        &mut self,
        id: BufferId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(self.tabs.active_index()).map(|t| t.id()) else {
            return;
        };
        let Some(code) = self.tabs.active_mut().and_then(|t| t.code.as_deref_mut()) else {
            return;
        };
        self.editor.update(cx, |editor, cx| {
            editor.strip_activate(tab, code, id, window, cx);
        });
    }

    /// Opens — or closes — the `+N` list. `from_keyboard` says the chord,
    /// not the pointer, asked, which decides where Escape returns the
    /// keyboard.
    pub(crate) fn editor_toggle_strip_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        from_keyboard: bool,
    ) {
        let Some(tab) = self.nav_tab() else {
            return;
        };
        let code = self.tab_code();
        let file_count = code.map_or(0, |code| code.files.len());
        self.editor.update(cx, |editor, cx| {
            editor.toggle_strip_picker(tab, code, file_count, from_keyboard, window, cx);
        });
    }

    /// Closes the `+N` list, returning the keyboard where it came from.
    pub(crate) fn editor_close_strip_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab = self.nav_tab().zip(self.tab_code());
        self.editor.update(cx, |editor, cx| {
            if let Some((tab, code)) = tab {
                editor.close_strip_picker(tab, Some(code), window, cx);
            } else {
                editor.dismiss_strip_picker();
                cx.notify();
            }
        });
    }

    /// A row's choice from the list: the document comes to the front, and
    /// the keyboard goes back to where the list was opened from.
    fn editor_strip_pick(&mut self, id: BufferId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(self.tabs.active_index()).map(|t| t.id()) else {
            return;
        };
        let Some(code) = self.tabs.active_mut().and_then(|t| t.code.as_deref_mut()) else {
            return;
        };
        self.editor.update(cx, |editor, cx| {
            editor.strip_pick(tab, code, id, window, cx);
        });
    }

    fn on_strip_picker_key(
        &mut self,
        ev: &gpui_kit::KeyDownEvent,
        cap: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(self.tabs.active_index()).map(|t| t.id()) else {
            return;
        };
        let local_host = self.spawn_host(cx);
        let Some(code) = self.tabs.active_mut().and_then(|t| t.code.as_deref_mut()) else {
            return;
        };
        self.editor.update(cx, |editor, cx| {
            editor.strip_picker_key(
                lyrel_workbench_editor::code_editor::strip::StripTarget { tab, code },
                local_host,
                ev,
                cap,
                window,
                cx,
            );
        });
    }

    /// Closes the file with this buffer in the front tab's strip, wherever it
    /// has moved to since a menu was built for it.
    fn strip_close_buffer(&mut self, id: BufferId, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pos) = self
            .tab_code()
            .and_then(|code| code.files.iter().position(|file| *file == id))
        {
            self.editor_close_file(pos, window, cx);
        }
    }

    /// Every file in the strip but the one in front — only the saved ones
    /// when `saved_only`.
    fn editor_strip_others(&self, saved_only: bool, cx: &gpui_kit::App) -> Vec<BufferId> {
        self.editor
            .read(cx)
            .strip_others(self.tab_code(), saved_only)
    }

    /// The `+N` button at the end of the strip, with its list when open.
    pub(crate) fn render_strip_overflow(
        &self,
        hidden: &[BufferId],
        cap: usize,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let open = self.editor.read(cx).strip_picker_open();
        // The chip exists only when there is something behind it, but the
        // list can be opened from the keyboard with every file in the strip,
        // so it is still anchored at the strip's end then.
        if hidden.is_empty() && !open {
            return None;
        }
        let theme = cx.theme();
        let hidden_dirty = hidden.iter().any(|id| {
            self.editor
                .read(cx)
                .buffers()
                .get(*id)
                .is_some_and(|f| f.dirty)
        });
        let button = h_flex()
            .id("editor-strip-more")
            .test_support()
            .occlude()
            .flex_none()
            .h(px(CELL_H))
            .px(px(8.))
            .gap(px(5.))
            .items_center()
            .rounded(px(CELL_RADIUS))
            .text_size(rems(12. / 16.))
            .font_features(crate::ui::theme::tabular_figures())
            .text_color(theme.muted_foreground)
            .role(gpui_kit::Role::Button)
            .aria_label(t(L10nKey::EditorStripAllFiles))
            .when(open, |d| d.bg(theme.sidebar_accent))
            .hover(|s| s.bg(theme.sidebar_accent).text_color(theme.foreground))
            .when(hidden_dirty, |d| {
                d.child(
                    div()
                        .size(px(crate::ui::tab_strip::ROW_STATUS_DOT))
                        .rounded_full()
                        .bg(theme.muted_foreground),
                )
            })
            .child(format!("+{}", hidden.len()))
            .child(
                Icon::new(IconName::ChevronDown)
                    .xsmall()
                    .text_color(theme.muted_foreground),
            )
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(t(L10nKey::EditorStripAllFiles))
                    .build(window, cx)
            })
            .on_hover(cx.listener(|this, over: &bool, _, cx| {
                let over = *over;
                this.editor
                    .update(cx, |editor, _| editor.set_strip_picker_hover(over));
            }))
            .on_click(cx.listener(|this, _, window, cx| {
                this.editor_toggle_strip_picker(window, cx, false);
            }));
        Some(
            div()
                .relative()
                .flex_none()
                .children((!hidden.is_empty()).then(|| button.into_any_element()))
                .children(self.render_strip_picker(cap, cx))
                .into_any_element(),
        )
    }

    fn render_strip_picker(&self, cap: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let picker_input = self.editor.read(cx).strip_picker_input()?;
        let (rows, hidden) = {
            let code = self.tab_code();
            let local_host = self.spawn_host(cx);
            self.editor
                .read(cx)
                .strip_matches(code, local_host, cap, cx)
        };
        let querying = !self.editor.read(cx).strip_picker_query(cx).is_empty();
        let hi = self.editor.read(cx).strip_picker_highlight(rows.len());
        let theme = cx.theme();
        let (fg, muted) = (theme.foreground, theme.muted_foreground);
        let row_hover = gpui_kit::rgb(
            cx.global::<lyrel_ui_base::presets::Surfaces>()
                .popover
                .hover,
        );

        let heading = |label: SharedString| {
            div()
                .h(px(24.))
                .pt(px(4.))
                .px(px(8.))
                .flex()
                .items_center()
                .text_size(rems(11. / 16.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(muted)
                .child(label)
        };
        let empty = rows.is_empty();
        let mut list = v_flex()
            .id("editor-strip-list")
            .max_h(px(PICKER_LIST_MAX_H))
            .overflow_y_scroll()
            .gap(px(1.));
        for (i, row) in rows.into_iter().enumerate() {
            if !querying && i == 0 && hidden > 0 {
                list = list.child(heading(
                    t_fmt(L10nKey::EditorStripHidden, &[("n", &hidden.to_string())]).into(),
                ));
            }
            if !querying && i == hidden {
                list = list.child(heading(t(L10nKey::EditorStripInBar).into()));
            }
            let id = row.id;
            let group: SharedString = format!("editor-strip-row-{i}").into();
            let slot = div()
                .id(("editor-strip-row-close", i))
                .relative()
                .flex_none()
                .size(px(18.))
                .rounded(px(4.))
                .hover(|s| s.bg(theme.muted))
                .when(row.dirty, |d| {
                    d.child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .group_hover(group.clone(), |s| s.invisible())
                            .child(div().size(px(6.)).rounded_full().bg(muted)),
                    )
                })
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(row.dirty, |d| d.invisible())
                        .group_hover(group.clone(), |s| s.visible())
                        .child(Icon::new(IconName::Close).xsmall().text_color(muted)),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.strip_close_buffer(id, window, cx);
                }));
            list = list.child(
                h_flex()
                    .id(SharedString::from(format!("editor-strip-row-{i}")))
                    .test_support()
                    .group(group)
                    .flex_none()
                    .h(px(PICKER_ROW_H))
                    .pl(px(8.))
                    .pr(px(4.))
                    .gap(px(8.))
                    .items_center()
                    .rounded(px(6.))
                    .when(i == hi, |d| d.bg(row_hover))
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        this.editor.update(cx, |editor, cx| {
                            editor.set_strip_picker_highlight(i);
                            cx.notify();
                        });
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.editor_strip_pick(id, window, cx);
                    }))
                    .child(
                        div()
                            .min_w(px(40.))
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .text_color(fg)
                            .when(row.front, |d| d.font_weight(FontWeight::MEDIUM))
                            .child(row.name),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .text_size(rems(11.5 / 16.))
                            .text_color(muted)
                            .child(row.dir),
                    )
                    .child(slot),
            );
        }
        if empty {
            list = list.child(
                div()
                    .h(px(PICKER_ROW_H))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .text_color(muted)
                    .child(t(L10nKey::EditorStripNoMatch)),
            );
        }

        let rule = || div().h(px(1.)).mx(px(6.)).my(px(4.)).bg(theme.border);
        let footer_button = |key: &'static str, label: L10nKey| {
            div()
                .id(key)
                .text_color(muted)
                .hover(|s| s.text_color(fg))
                .child(t(label))
        };
        let card = v_flex()
            .occlude()
            .w(px(PICKER_W))
            .p(px(5.))
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .rounded(px(10.))
            .shadow_lg()
            .text_size(rems(CELL_TEXT))
            .on_key_down(
                cx.listener(move |this, ev: &gpui_kit::KeyDownEvent, window, cx| {
                    this.on_strip_picker_key(ev, cap, window, cx);
                }),
            )
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                if this.editor.read(cx).strip_picker_over_button() {
                    return;
                }
                this.editor_close_strip_picker(window, cx);
            }))
            .child(
                h_flex()
                    .h(px(30.))
                    .px(px(4.))
                    .gap(px(3.))
                    .items_center()
                    .child(Icon::new(IconName::Search).xsmall().text_color(muted))
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&picker_input)
                                .appearance(false)
                                .small()
                                .text_size(rems(CELL_TEXT)),
                        ),
                    ),
            )
            .child(rule())
            .child(list)
            .child(rule())
            .child(
                h_flex()
                    .h(px(28.))
                    .px(px(8.))
                    .gap(px(14.))
                    .items_center()
                    .text_size(rems(12. / 16.))
                    .child(
                        footer_button("editor-strip-close-saved", L10nKey::EditorStripCloseSaved)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.editor
                                    .update(cx, |editor, _| editor.dismiss_strip_picker());
                                let ids = this.editor_strip_others(true, cx);
                                this.editor_close_buffers(ids, window, cx);
                                this.focus_editor(window, cx);
                            })),
                    )
                    .child(
                        footer_button("editor-strip-close-others", L10nKey::EditorStripCloseOthers)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.editor
                                    .update(cx, |editor, _| editor.dismiss_strip_picker());
                                let ids = this.editor_strip_others(false, cx);
                                this.editor_close_buffers(ids, window, cx);
                                this.focus_editor(window, cx);
                            })),
                    ),
            );
        Some(
            div()
                .absolute()
                .right_0()
                .top(px(CELL_H + 4.))
                .child(
                    deferred(
                        anchored()
                            .anchor(Anchor::TopRight)
                            .snap_to_window_with_margin(px(8.))
                            .child(card),
                    )
                    .with_priority(1),
                )
                .into_any_element(),
        )
    }
}

/// The search command a symbol row runs: to the symbol's source position.
pub(crate) fn symbol_command(symbol: &outline::Symbol) -> CommandKind {
    CommandKind::GoToSymbol {
        line: symbol.selection.line,
        column: symbol.selection.character,
    }
}

/// The symbol picker's rows, in document order, each with how deep it sits.
pub(crate) fn symbol_rows(outline: &outline::Outline) -> Vec<(usize, Item)> {
    outline
        .symbols
        .iter()
        .enumerate()
        .map(|(ix, s)| {
            let mut item = Item::new(s.name.clone(), symbol_command(s)).with_note(s.kind.tag());
            let path = outline.container_path(ix);
            if !path.is_empty() {
                item = item.with_subtitle(path.join(" › "));
            }
            (s.depth, item)
        })
        .collect()
}

/// A search row for `found`, its path shown relative to `root`.
pub(crate) fn location_item(found: &Found, root: &std::path::Path) -> Item {
    let shown = found.path.strip_prefix(root).unwrap_or(&found.path);
    Item::new(
        format!("{}:{}", shown.display(), found.line + 1),
        CommandKind::GoToLocation {
            path: found.path.clone(),
            line: found.line,
            column: found.column,
        },
    )
    .with_subtitle(found.text.clone())
    .with_alias(found.path.display().to_string())
}

/// A search row for a project symbol, named by where it lives.
pub(crate) fn symbol_hit_item(
    found: &Found,
    symbol: &WorkspaceHit,
    root: &std::path::Path,
) -> Item {
    let kind = serde_json::to_value(symbol.kind)
        .ok()
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let shown = found.path.strip_prefix(root).unwrap_or(&found.path);
    let place = format!("{}:{}", shown.display(), found.line + 1);
    let subtitle = match symbol.container.as_deref().filter(|c| !c.is_empty()) {
        Some(container) => format!("{container} · {place}"),
        None => place,
    };
    Item::new(
        symbol.name.clone(),
        CommandKind::GoToLocation {
            path: found.path.clone(),
            line: found.line,
            column: found.column,
        },
    )
    .with_subtitle(subtitle)
    .with_note(
        lyrel_workbench_editor::code_editor::outline::SymbolKind::from_lsp(kind as u32).tag(),
    )
}

impl LyrelApp {
    /// What the Symbols tab asks as its query changes: the front file's
    /// language server, for symbols anywhere in the project. `None` when
    /// there is no such server, and the tab lists the file's own alone.
    pub(crate) fn lsp_project_symbol_query(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<crate::ui::search::LiveQuery> {
        let code = self.tab_code()?;
        if !self.editor.update(cx, |editor, cx| {
            editor.lsp_project_symbols_available(code, cx)
        }) {
            return None;
        }
        let editor = self.editor.downgrade();
        let handle = window.window_handle();
        let (path, requester) = self
            .editor
            .read(cx)
            .active_local_file(code)
            .map(|f| (f.path().to_path_buf(), f.id()))?;
        Some(std::rc::Rc::new(
            move |query: &str, cx: &mut gpui_kit::App| {
                let (editor, path, query) = (editor.clone(), path.clone(), query.to_owned());
                // Deferred: the query arrives from inside the search's own
                // update, and answering it updates the search.
                cx.defer(move |cx| {
                    let _ = handle.update(cx, |_, window, cx| {
                        let _ = editor.update(cx, |editor, cx| {
                            editor.lsp_workspace_query(&path, requester, query, window, cx)
                        });
                    });
                });
            },
        ))
    }

    /// The Locations list moved onto a place: shown at once when it is in
    /// the file in front, where the preview runs.
    pub(crate) fn lsp_preview_location(
        &mut self,
        path: &std::path::Path,
        line: u32,
        column: u32,
        cx: &mut Context<Self>,
    ) {
        let in_front = self
            .editor_preview_buffer(cx)
            .and_then(|id| self.editor.read(cx).local_file(id))
            .is_some_and(|f| f.path() == path);
        if in_front {
            self.editor_symbol_preview(line, column, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::component::Rope;
    use lsp_types::Position;

    #[test]
    fn symbol_rows_carry_their_containers_and_the_breadcrumb_chain_names_them() {
        let src = "impl LyrelApp {\n    fn open_file(&self) {\n        let x = 1;\n    }\n}\n";
        let outline = outline::outline_of_text("rust", &Rope::from(src));
        let rows = symbol_rows(&outline);
        let shown: Vec<_> = rows
            .iter()
            .map(|(d, i)| {
                (
                    *d,
                    i.title.as_str(),
                    i.subtitle.as_deref(),
                    i.note.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            shown,
            [
                (0, "impl LyrelApp", None, Some("impl")),
                (1, "open_file", Some("impl LyrelApp"), Some("method")),
            ]
        );
        assert_eq!(
            rows[1].1.kind,
            CommandKind::GoToSymbol { line: 1, column: 7 }
        );
        let chain: Vec<_> = outline
            .chain_at(Position::new(2, 8))
            .into_iter()
            .map(|i| {
                format!(
                    "{} {}",
                    outline.symbols[i].kind.tag(),
                    outline.symbols[i].name
                )
            })
            .collect();
        assert_eq!(chain, ["impl impl LyrelApp", "method open_file"]);
    }

    #[test]
    #[allow(deprecated)]
    fn workspace_symbols_become_rows_naming_where_they_live() {
        use lyrel_workbench_editor::lsp::locations::workspace_symbols;
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let uri =
            lyrel_workbench_editor::lsp::path_to_uri(&root.join("src").join("net.rs")).unwrap();
        let hits = workspace_symbols(lsp_types::WorkspaceSymbolResponse::Flat(vec![
            lsp_types::SymbolInformation {
                name: "Server".into(),
                kind: lsp_types::SymbolKind::STRUCT,
                tags: None,
                deprecated: None,
                location: lsp_types::Location::new(
                    uri,
                    lsp_types::Range::new(Position::new(3, 11), Position::new(3, 17)),
                ),
                container_name: Some("net".into()),
            },
        ]));
        assert_eq!(hits.len(), 1);
        let found = Found {
            path: hits[0].path.clone(),
            line: 3,
            column: 11,
            text: String::new(),
        };
        let row = symbol_hit_item(&found, &hits[0], root);
        assert_eq!(row.title, "Server");
        let rel = std::path::Path::new("src").join("net.rs");
        assert_eq!(
            row.subtitle.as_deref(),
            Some(format!("net · {}:4", rel.display()).as_str())
        );
        assert_eq!(row.note.as_deref(), Some("struct"));
    }

    #[test]
    fn a_row_names_its_file_relative_to_the_project() {
        let found = Found {
            path: std::path::PathBuf::from("/p/src/lib.rs"),
            line: 4,
            column: 2,
            text: "x".into(),
        };
        let row = location_item(&found, std::path::Path::new("/p"));
        assert_eq!(row.title, "src/lib.rs:5");
        assert_eq!(row.subtitle.as_deref(), Some("x"));
    }
}
