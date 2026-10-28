//! Root composition keeps the same retained entity, focus path and overlay order.
//! Detailed frame painting remains TerminalElement's responsibility.

use super::*;
use gpui_kit::component::menu::ContextMenuExt;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::ActiveTheme as _;
impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_frame_facts();
        self.sync_typeahead_owner();
        self.sync_scrollbar();
        self.sync_composer(window, cx);
        if self.shell_owns_prompt() {
            self.input.settle_shell_owner(&self.terminal);
        } else if self.input_active() {
            self.input.engage_hold_into_editor();
            if self.terminal.zle_reading() {
                self.input.flush_typeahead(&self.terminal);
            }
        }
        let entity = cx.entity();
        let search_bar = self
            .search
            .as_ref()
            .map(|s| self.render_search_bar(s, window, cx));

        let focused = self.focus_handle.is_focused(window);
        let input_bar = self
            .input_active()
            .then(|| self.render_input_bar(focused, cx));
        let completion_menu = self
            .input_active()
            .then(|| self.render_completion_menu(cx))
            .flatten();
        let reverse_search_menu = self
            .input_active()
            .then(|| self.render_reverse_search_menu(cx))
            .flatten();
        let (composer_docked, composer_over) = match self.render_composer(window, cx) {
            Some((el, true)) => (Some(el), None),
            Some((el, false)) => (None, Some(el)),
            None => (None, None),
        };
        let integration_notice = self.render_integration_notice(cx);
        let remote_completion_notice = self.render_remote_completion_notice(cx);

        let has_selection = self.any_selection();
        let menu_view = cx.entity();

        div()
            .id("terminal-surface")
            .test_support()
            // The surface, not the grid inside it, is what carries the role:
            // a11y focus is only ever reported for a `div` that tracks a focus
            // handle *and* has a node of its own, so a terminal with no role
            // here is a window whose focused element is the window.
            .role(gpui_kit::Role::MultilineTextInput)
            .track_focus(&self.focus_handle)
            .key_context(self.key_context(cx))
            .size_full()
            .relative()
            // A column so the composer docks under the grid and takes its rows
            // from it: the grid's laid-out height is what sizes the pty.
            .flex()
            .flex_col()
            .overflow_hidden()
            .px(px(GRID_PAD_X))
            .py(px(GRID_PAD_Y))
            .text_color(cx.theme().foreground)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_scroll(cx.listener(Self::on_scroll))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _ev: &MouseDownEvent, window, cx| {
                    if window.default_prevented() {
                        return;
                    }
                    window.focus(&this.focus_handle, cx);
                }),
            )
            // Latch the context-menu verdict for this click. gpui-component's
            // `ContextMenu` element owns the right mouse-down that opens the
            // popup and hands the builder no event to inspect, so the modifiers
            // have to be recorded here. That element wraps this one, so its
            // listener actually fires *before* this one — harmless, because it
            // only builds the menu from a `window.defer` callback that runs
            // once the whole mouse dispatch has unwound.
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, ev: &MouseDownEvent, _window, _cx| {
                    this.links
                        .record_menu_permission(this.mouse_mode(), ev.modifiers.shift);
                }),
            )
            .drag_over::<ExternalPaths>(|s, _, _, cx| s.bg(cx.theme().drag_border.opacity(0.12)))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                window.focus(&this.focus_handle, cx);
                this.drop_files(paths, cx);
            }))
            .drag_over::<crate::ui::file_tree::RemotePathDrag>(|s, _, _, cx| {
                s.bg(cx.theme().drag_border.opacity(0.12))
            })
            .on_drop(cx.listener(
                |this, drag: &crate::ui::file_tree::RemotePathDrag, window, cx| {
                    window.focus(&this.focus_handle, cx);
                    this.drop_remote_path(drag, cx);
                },
            ))
            .on_action(cx.listener(|this, _: &CopyText, _w, cx| {
                this.copy_contextual(false, cx);
            }))
            .on_action(cx.listener(|this, _: &CutText, _w, cx| {
                this.cut_contextual(cx);
            }))
            .on_action(cx.listener(|this, _: &PasteText, _w, cx| this.paste_from_clipboard(cx)))
            .on_action(cx.listener(|this, _: &AlternatePaste, _w, cx| this.alternate_paste(cx)))
            .on_action(cx.listener(|this, _: &SelectAll, _w, cx| this.select_all_contextual(cx)))
            .on_action(cx.listener(|this, _: &UndoEdit, _w, cx| this.undo_edit(false, cx)))
            .on_action(cx.listener(|this, _: &RedoEdit, _w, cx| this.undo_edit(true, cx)))
            .on_action(
                cx.listener(|this, _: &FindInTerminal, window, cx| this.open_search(window, cx)),
            )
            // Off macOS these two live on F3 and Shift+F3, which is also where
            // PSReadLine keeps CharacterSearch and readline users put their
            // own widgets. With no find bar open there is no next match to
            // step to, so the keystroke is given back the way `EditorSave`
            // gives back Ctrl+S — otherwise the action swallows the key and
            // the shell never sees it (#834).
            .on_action(cx.listener(|this, _: &FindNext, _w, cx| {
                if this.search.is_none() {
                    cx.propagate();
                    return;
                }
                this.step_match(Direction::Right, cx);
            }))
            .on_action(cx.listener(|this, _: &FindPrevious, _w, cx| {
                if this.search.is_none() {
                    cx.propagate();
                    return;
                }
                this.step_match(Direction::Left, cx);
            }))
            .on_action(cx.listener(|this, _: &ClearScrollback, _w, cx| this.clear_scrollback(cx)))
            .on_action(cx.listener(|this, _: &OpenLinkUnderPointer, window, cx| {
                this.open_menu_link(window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenLinkWithDefaultApp, window, cx| {
                this.open_menu_link_with_default_app(window, cx);
            }))
            .on_action(cx.listener(|this, _: &RevealLinkUnderPointer, window, cx| {
                this.reveal_menu_link(window, cx);
            }))
            .on_action(cx.listener(|this, _: &CopyLinkPathUnderPointer, _w, cx| {
                this.copy_menu_link_path(cx);
            }))
            .on_action(cx.listener(|this, _: &InsertNewline, _w, cx| {
                this.insert_newline_action(cx);
            }))
            .on_action(cx.listener(|this, _: &InsertNewlineFallback, _w, cx| {
                this.insert_newline_fallback_action(cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleComposer, window, cx| {
                this.toggle_composer(window, cx);
            }))
            .on_action(cx.listener(|this, _: &SendTab, _w, cx| {
                this.tab_pressed(true, cx);
            }))
            .on_action(cx.listener(|this, _: &SendBackTab, _w, cx| {
                this.tab_pressed(false, cx);
            }))
            .child(TerminalElement::new(entity))
            .children(composer_docked)
            .child(self.render_scrollbar())
            .children(search_bar)
            .children(input_bar)
            .children(completion_menu)
            .children(reverse_search_menu)
            .children(integration_notice)
            .children(remote_completion_notice)
            .children(composer_over)
            .context_menu(move |menu, window, cx| {
                menus::append_terminal_menu(&menu_view, menu, has_selection, window, cx)
            })
    }
}
