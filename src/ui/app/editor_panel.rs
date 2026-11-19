//! Shell orchestration for the editor panel.
//!
//! The editor's document state and document-local mutations live on
//! `Entity<EditorPanelState>` in `ui::code_editor`; this module owns the
//! tab/workspace/session side — placement, close and focus decisions, frame
//! sync, menus and rendering — and answers the panel's cross-panel intents
//! through the `EditorPanelEvent` subscription installed at bootstrap. No
//! editor module holds a production `impl LyrelApp` (#327).

use std::path::{Path, PathBuf};

use gpui_kit::AnyElement;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Icon, IconName, Sizable as _, h_flex, v_flex};

use super::*;
use crate::ui::code_editor::commands::{self, Command};
use crate::ui::code_editor::problems::{Severity, collect, headline};
use crate::ui::code_editor::{
    AfterUnsaved, BarKind, BufferId, BufferKind, EditorPanelEvent, EditorPanelState,
    EditorStatusCommand, TabCode,
};
use crate::ui::host_ops::SharedHost;
use crate::ui::lsp::LspStore;
use gpui_kit::component::input::Position;

/// The pane's height: a handful of rows, leaving the editor the rest.
const PANE_HEIGHT: f32 = 180.;
/// Past this many rows the list stops drawing and says how many are left.
const MAX_ROWS: usize = 1000;

impl LyrelApp {
    // ---- Tab/editor-group accessors ----

    pub(crate) fn tab_code(&self) -> Option<&TabCode> {
        self.tabs.get(self.tabs.active_index())?.code.as_deref()
    }

    pub(crate) fn tab_code_mut(&mut self) -> Option<&mut TabCode> {
        self.tabs.active_mut()?.code.as_deref_mut()
    }

    pub(crate) fn tab_code_mut_or_init(&mut self) -> Option<&mut TabCode> {
        let tab = self.tabs.active_mut()?;
        Some(tab.code.get_or_insert_with(|| Box::new(TabCode::new())))
    }

    pub(crate) fn code_panel_visible(&self) -> bool {
        self.tab_code().is_some_and(|c| c.visible)
    }

    /// Temporary document surfaces keep their own keyboard until dismissed.
    /// The ordinary editor, docked or filled, is content rather than a modal.
    pub(crate) fn document_navigation_overlay_open(&self, cx: &gpui_kit::App) -> bool {
        self.code_panel_visible() && self.editor.read(cx).navigation_overlay_open()
    }

    pub(crate) fn tab_index_of(&self, tab: TabId) -> Option<usize> {
        self.tabs.iter().position(|t| t.id() == tab)
    }

    fn buffer_refs(&self, id: BufferId) -> usize {
        self.tabs
            .iter()
            .filter_map(|t| t.code.as_deref())
            .map(|code| code.views_of(id))
            .sum()
    }

    fn buffer_tabs(&self, id: BufferId) -> usize {
        self.tabs
            .iter()
            .filter_map(|t| t.code.as_deref())
            .filter(|code| code.shows(id))
            .count()
    }

    pub(crate) fn editor_unsaved_in_tab(&self, tab_ix: usize, cx: &gpui_kit::App) -> Vec<BufferId> {
        let Some(tab) = self.tabs.get(tab_ix) else {
            return Vec::new();
        };
        let Some(code) = tab.code.as_deref() else {
            return Vec::new();
        };
        code.all_files()
            .into_iter()
            .filter(|id| {
                self.editor
                    .read(cx)
                    .buffers()
                    .get(*id)
                    .is_some_and(|file| file.dirty)
            })
            .collect()
    }

    pub(crate) fn editor_unsaved(&self, cx: &gpui_kit::App) -> Vec<BufferId> {
        (0..self.tabs.len())
            .flat_map(|ix| self.editor_unsaved_in_tab(ix, cx))
            .collect()
    }

    pub(crate) fn editor_active_location<'a>(&self, cx: &'a App) -> Option<(HostId, &'a Path)> {
        let code = self.tab_code()?;
        self.editor.read(cx).active_location(code)
    }

    pub(crate) fn editor_active_local_file(
        &self,
        cx: &gpui_kit::App,
    ) -> Option<crate::ui::code_editor::LocalFile> {
        let code = self.tab_code()?;
        self.editor.read(cx).active_local_file(code)
    }

    /// The save root the editor writes under: the front tab's first file-tree
    /// root. The editor receives it as a value on every save.
    pub(crate) fn editor_save_root(&self) -> PathBuf {
        self.tab_code()
            .and_then(|code| code.roots.first().cloned())
            .unwrap_or_default()
    }

    // ---- Editor context ----

    /// The workspace/host changed: stale pending opens are dropped, every
    /// buffer's reload generation moves, and open prompts retire.
    pub(crate) fn editor_context_changed(&mut self, cx: &mut Context<Self>) {
        self.editor
            .update(cx, |editor, _| editor.invalidate_context());
    }

    pub(crate) fn editor_rebuild_watcher(&mut self, cx: &mut Context<Self>) {
        let host = self.active_host(cx);
        self.editor.update(cx, |editor, cx| {
            editor.rebuild_watch(host, cx);
        });
        self.lsp_sync_buffers(cx);
    }

    // ---- Opening ----

    /// Shows what a file link in the grid pointed at.
    ///
    /// A file opens in the editor, on the line the link named; the tree
    /// follows along so "what does it say?" and "where does it live?" are
    /// answered by the same click. A directory has no contents to show, so it
    /// is only ever the tree.
    pub(crate) fn open_linked_file(
        &mut self,
        path: &Path,
        line: Option<u32>,
        column: Option<u32>,
        is_dir: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if is_dir {
            self.file_tree_show(path, window, cx);
            return;
        }
        self.open_file_in_editor_at(path, line, column, window, cx);
        self.file_tree_reveal_path(path, cx);
    }

    /// [`Self::open_file_in_editor`], landing the cursor on a line the caller
    /// already knows — what a `src/main.rs:120:3` in the grid was pointing at.
    pub(crate) fn open_file_in_editor_at(
        &mut self,
        path: &Path,
        line: Option<u32>,
        column: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = self.active_host(cx) else {
            return;
        };
        let cursor = line.map(|line| Position {
            line: line.saturating_sub(1),
            character: column.unwrap_or(1).saturating_sub(1),
        });
        self.editor_open_on_host_at(host, path, cursor, window, cx);
    }

    pub(crate) fn open_file_in_editor(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = self.active_host(cx) else {
            return;
        };
        self.editor_open_on_host(host, path, window, cx);
    }

    /// [`Self::open_file_in_editor`] against an explicit host — the SFTP
    /// browser's files live on a host that is never the active one.
    pub(crate) fn editor_open_on_host(
        &mut self,
        host: SharedHost,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor_open_on_host_at(host, path, None, window, cx);
    }

    pub(crate) fn editor_open_on_host_at(
        &mut self,
        host: SharedHost,
        path: &Path,
        cursor: Option<Position>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(self.tabs.active_index()).map(|tab| tab.id()) else {
            return;
        };
        self.raise_code_overlay();
        let Some(group) = self.tab_code_mut_or_init().map(|code| code.group()) else {
            return;
        };
        let workspace = self.workspace;
        let window_host = self.spawn_host(cx);
        let reused = self.editor.update(cx, |editor, cx| {
            editor.open_on_host_at(
                host,
                path,
                cursor,
                None,
                workspace,
                tab,
                Some(group),
                window_host,
                window,
                cx,
            )
        });
        if let Some(id) = reused {
            self.editor_show_in_tab(tab, id, true, window, cx);
        }
    }

    /// Puts a buffer in a tab's strip and, when `front`, in front of its
    /// editor with the panel open and focused.
    pub(crate) fn editor_show_in_tab(
        &mut self,
        tab: TabId,
        id: BufferId,
        front: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab_ix) = self.tab_index_of(tab) else {
            return;
        };
        let Some(t) = self.tabs.get_mut(tab_ix) else {
            return;
        };
        let code = t.code.get_or_insert_with(|| Box::new(TabCode::new()));
        if !front {
            // Listed, not brought forward: the one in front stays in front.
            code.append(id);
            cx.notify();
            return;
        }
        code.show(id);
        code.visible = true;
        if tab_ix == self.tabs.active_index() {
            self.editor.update(cx, |editor, _| editor.dismiss_bar());
            self.focus_editor(window, cx);
        }
        cx.notify();
    }

    /// The `OpenCompleted` placement: a split-group target lands in the other
    /// group; everything else lands in the tab's own strip.
    pub(super) fn editor_show_open_target(
        &mut self,
        tab: TabId,
        group: Option<crate::ui::code_editor::GroupId>,
        id: BufferId,
        front: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(other) = self
            .tabs
            .iter_mut()
            .find(|tab_entry| tab_entry.id() == tab)
            .and_then(|tab_entry| tab_entry.code.as_mut())
            .and_then(|code| code.split.as_mut())
            .filter(|other| Some(other.id) == group)
        {
            let ix = other
                .files
                .iter()
                .position(|entry| *entry == id)
                .unwrap_or_else(|| {
                    other.files.push(id);
                    other.files.len() - 1
                });
            if front {
                other.active = ix;
            }
            cx.notify();
            return;
        }
        self.editor_show_in_tab(tab, id, front, window, cx);
    }

    /// A new, empty, never-saved buffer, on the window's own machine.
    pub(crate) fn editor_new_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor_new_untitled(BufferKind::Text, window, cx);
    }

    pub(crate) fn editor_new_markdown_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor_new_untitled(BufferKind::Markdown, window, cx);
    }

    fn editor_new_untitled(
        &mut self,
        kind: BufferKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = self.active_host(cx) else {
            return;
        };
        let Some(tab) = self.tabs.get(self.tabs.active_index()).map(|t| t.id()) else {
            return;
        };
        self.raise_code_overlay();
        let id = self
            .editor
            .update(cx, |editor, cx| editor.new_untitled(host, kind, window, cx));
        self.watch_editor_buffer_focus(id, window, cx);
        self.editor_show_in_tab(tab, id, true, window, cx);
    }

    /// A scrollback export is an unsaved document, never a filesystem path on
    /// the pane's host. Saving uses the ordinary Save As and close safeguards.
    pub(crate) fn editor_open_scrollback(
        &mut self,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(self.tabs.active_index()).map(|t| t.id()) else {
            return;
        };
        let host = lyrel_core::host::local::new();
        let id = self.editor.update(cx, |editor, cx| {
            editor.open_scrollback(text, host, window, cx)
        });
        self.watch_editor_buffer_focus(id, window, cx);
        self.raise_code_overlay();
        self.editor_show_in_tab(tab, id, true, window, cx);
    }

    /// The status line's document-focus watch for one buffer's input: a focus
    /// landing there tells the status bar what the front is.
    pub(in crate::ui) fn watch_editor_buffer_focus(
        &self,
        id: BufferId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(handle) = self
            .editor
            .read(cx)
            .buffers()
            .get(id)
            .map(|file| file.input.read(cx).focus_handle(cx))
        else {
            return;
        };
        self.watch_document_status_focus(&handle, window, cx);
    }

    // ---- Panel visibility and focus ----

    pub(crate) fn toggle_code_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.active_mut() else {
            return;
        };
        let buried = tab.reveal_code();
        if buried {
            self.focus_editor(window, cx);
            cx.notify();
            return;
        }
        let Some(tab) = self.tabs.active_mut() else {
            return;
        };
        let code = tab.code.get_or_insert_with(|| Box::new(TabCode::new()));
        if code.visible {
            code.visible = false;
            self.editor.update(cx, |editor, _| {
                editor.set_had_focus(None);
                editor.dismiss_bar();
            });
            self.file_tree.cancel_edit();
            self.focus_active(window, cx);
            cx.notify();
            return;
        }
        code.visible = true;
        self.file_tree_refresh_roots(window, cx);
        let has_file = self
            .tab_code()
            .and_then(|code| self.editor.read(cx).active_file(code))
            .is_some();
        if has_file {
            self.focus_editor(window, cx);
        } else {
            // With no file to show, the panel says "Open a file from the file
            // tree" and hands the tree the focus — but nothing was putting the
            // tree on screen, so ⌘⇧E on a fresh tab opened an empty editor
            // pointing at a panel the reader could not see or reach from
            // there. Reveal it, then focus it.
            if !self.file_tree_on_screen(cx) {
                self.set_right_panel_tab(crate::core::config::RightPanelTab::Files, window, cx);
            }
            self.file_tree.focus_handle.focus(window, cx);
        }
        cx.notify();
    }

    fn raise_code_overlay(&mut self) {
        if let Some(tab) = self.tabs.active_mut() {
            tab.reveal_code();
        }
    }

    pub(in crate::ui) fn focus_editor(&self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(tab), Some(code)) = (
            self.tabs.get(self.tabs.active_index()).map(|t| t.id()),
            self.tab_code(),
        ) else {
            return;
        };
        self.editor
            .update(cx, |editor, cx| editor.focus_editor(tab, code, window, cx));
    }

    /// Settings borrows focus, not the terminal's place in the workbench.
    /// Keep explicit terminal commands on `focus_active`; only an overlay's
    /// return path restores the foreground document or the last docked shell.
    pub(crate) fn focus_previous_content(&self, window: &mut Window, cx: &mut Context<Self>) {
        match self.status_document_context(window, cx) {
            Some(crate::ui::tab::OverlayTop::Code) => {
                let has_file = self
                    .tab_code()
                    .and_then(|code| self.editor.read(cx).active_file(code))
                    .is_some();
                if has_file {
                    self.focus_editor(window, cx);
                } else {
                    let empty = self.editor.read(cx).empty_focus().clone();
                    empty.focus(window, cx);
                }
            }
            Some(crate::ui::tab::OverlayTop::Diff) => {
                if let Some(diff) = self
                    .tabs
                    .get(self.tabs.active_index())
                    .and_then(|tab| tab.diff_overlay.as_ref())
                {
                    window.focus(&diff.focus_handle, cx);
                }
            }
            None => self.focus_active(window, cx),
        }
    }

    /// Brings the file at `pos` in the active tab's strip to the front.
    pub(crate) fn editor_activate(
        &mut self,
        pos: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(self.tabs.active_index()).map(|t| t.id()) else {
            return;
        };
        if self.tab_code().is_none_or(|code| pos >= code.files.len()) {
            return;
        }
        let editor = self.editor.clone();
        let Some(code) = self.tab_code_mut() else {
            return;
        };
        editor.update(cx, |editor, cx| editor.activate(pos, tab, code, window, cx));
    }

    /// Switch the active native Markdown presentation between live editing and source.
    /// `ToggleDocumentPreview` remains the action ID for existing custom keybindings.
    pub(crate) fn toggle_document_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.code_panel_visible() {
            return;
        }
        let (Some(tab), Some(code)) = (
            self.tabs.get(self.tabs.active_index()).map(|t| t.id()),
            self.tab_code(),
        ) else {
            return;
        };
        self.editor.update(cx, |editor, cx| {
            editor.toggle_document_preview(tab, code, window, cx)
        });
    }

    /// The status bar's Wrap button, and `ToggleDocumentWrap`. Same reach as
    /// [`Self::toggle_document_preview`], for any file; the new state becomes
    /// what the next file opens with.
    pub(crate) fn toggle_document_wrap(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.code_panel_visible() {
            return;
        }
        let Some(code) = self.tab_code() else {
            return;
        };
        let wrap = self.editor.update(cx, |editor, cx| {
            editor.toggle_document_wrap(code, window, cx)
        });
        if let Some(wrap) = wrap {
            self.update_config(cx, |cfg| cfg.editor_soft_wrap = wrap);
        }
        cx.notify();
    }

    /// The status bar's line-ending button: flips the file between LF and
    /// CRLF. The buffer itself always holds `\n`; this only changes what a
    /// save writes, so it marks the file unsaved without touching its text.
    pub(crate) fn toggle_line_ending(&mut self, cx: &mut Context<Self>) {
        let Some(code) = self.tab_code() else {
            return;
        };
        self.editor.update(cx, |editor, cx| {
            editor.toggle_line_ending(code, cx);
        });
    }

    // ---- Action reach ----

    /// Run one of gpui-component's editing actions on the active buffer, as
    /// if it had been pressed there. Returns false with no buffer to run it
    /// on. The palette closes before its row runs, so this focuses the
    /// buffer first rather than relying on where the focus happens to be.
    pub(crate) fn editor_dispatch(
        &mut self,
        action: &dyn gpui_kit::Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.code_panel_visible() {
            return false;
        }
        let (Some(tab), Some(code)) = (
            self.tabs.get(self.tabs.active_index()).map(|t| t.id()),
            self.tab_code(),
        ) else {
            return false;
        };
        let Some(input) = self.editor.read(cx).active_editor_input(tab, code) else {
            return false;
        };
        let handle = input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        handle.dispatch_action(action, window, cx);
        true
    }

    /// The window's listeners for the editor's text commands, when the user
    /// has bound one: each runs on the editor only while it has the focus.
    pub(crate) fn with_editor_text_commands(
        root: gpui_kit::Stateful<gpui_kit::Div>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let root = install_editor_commands(root, cx);
        use crate::core::actions::{
            EditorJoinLines, EditorRemoveSurroundingBrackets, EditorTransformLowercase,
            EditorTransformTitleCase, EditorTransformUppercase, EditorTrimTrailingWhitespace,
        };
        root.on_action(
            cx.listener(|this, _: &EditorTransformUppercase, window, cx| {
                if !this.editor_has_focus(window, cx) {
                    cx.propagate();
                    return;
                }
                this.editor_dispatch(
                    &crate::ui::code_editor::commands::TransformToUppercase,
                    window,
                    cx,
                );
            }),
        )
        .on_action(
            cx.listener(|this, _: &EditorTransformLowercase, window, cx| {
                if !this.editor_has_focus(window, cx) {
                    cx.propagate();
                    return;
                }
                this.editor_dispatch(
                    &crate::ui::code_editor::commands::TransformToLowercase,
                    window,
                    cx,
                );
            }),
        )
        .on_action(
            cx.listener(|this, _: &EditorTransformTitleCase, window, cx| {
                if !this.editor_has_focus(window, cx) {
                    cx.propagate();
                    return;
                }
                this.editor_dispatch(
                    &crate::ui::code_editor::commands::TransformToTitleCase,
                    window,
                    cx,
                );
            }),
        )
        .on_action(
            cx.listener(|this, _: &EditorTrimTrailingWhitespace, window, cx| {
                if !this.editor_has_focus(window, cx) {
                    cx.propagate();
                    return;
                }
                // This handler already owns LyrelApp. Redispatching to its own
                // command listener would try to update the same entity again.
                this.editor_apply_command(
                    crate::ui::code_editor::commands::Command::Trim,
                    window,
                    cx,
                );
            }),
        )
        .on_action(cx.listener(|this, _: &EditorJoinLines, window, cx| {
            if !this.editor_has_focus(window, cx) {
                cx.propagate();
                return;
            }
            this.editor_dispatch(&crate::ui::code_editor::commands::JoinLines, window, cx);
        }))
        .on_action(
            cx.listener(|this, _: &EditorRemoveSurroundingBrackets, window, cx| {
                if !this.editor_has_focus(window, cx) {
                    cx.propagate();
                    return;
                }
                this.editor_dispatch(
                    &crate::ui::code_editor::commands::RemoveSurroundingBrackets,
                    window,
                    cx,
                );
            }),
        )
    }

    pub(crate) fn editor_apply_command(
        &mut self,
        command: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let (Some(tab), Some(code)) = (
            self.tabs.get(self.tabs.active_index()).map(|tab| tab.id()),
            self.tab_code(),
        ) else {
            return false;
        };
        self.editor.update(cx, |editor, cx| {
            editor.apply_command(command, tab, code, window, cx)
        })
    }

    pub(crate) fn editor_markdown_search(
        &mut self,
        replace: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let (Some(tab), Some(code)) = (
            self.tabs.get(self.tabs.active_index()).map(|tab| tab.id()),
            self.tab_code(),
        ) else {
            return false;
        };
        let focused = self.editor_panel_has_focus(window, cx);
        self.editor.update(cx, |editor, cx| {
            editor.markdown_search(replace, focused, tab, code, window, cx)
        })
    }

    pub(crate) fn editor_markdown_find_step(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let (Some(tab), Some(code)) = (
            self.tabs.get(self.tabs.active_index()).map(|tab| tab.id()),
            self.tab_code(),
        ) else {
            return false;
        };
        let focused = self.editor_panel_has_focus(window, cx);
        self.editor.update(cx, |editor, cx| {
            editor.markdown_find_step(forward, focused, tab, code, window, cx)
        })
    }

    /// Whether an editor exists to run [`Self::editor_dispatch`] on.
    pub(crate) fn editor_can_dispatch(&self, cx: &gpui_kit::App) -> bool {
        self.code_panel_visible()
            && self
                .tab_code()
                .and_then(|code| self.editor.read(cx).active_file(code))
                .is_some()
    }

    pub(crate) fn active_editor_input(
        &self,
        cx: &App,
    ) -> Option<crate::ui::code_editor::EditorInput> {
        let tab = self.tabs.get(self.tabs.active_index())?.id();
        self.editor
            .read(cx)
            .active_editor_input(tab, self.tab_code()?)
    }

    pub(crate) fn editor_has_focus(&self, window: &Window, cx: &Context<Self>) -> bool {
        self.code_panel_visible()
            && self
                .active_editor_input(cx)
                .is_some_and(|input| input.read(cx).focus_handle(cx).contains_focused(window, cx))
    }

    /// Whether anything in the editor panel — the text, its search bar, the
    /// go-to-line prompt — has the focus. Wider than
    /// [`Self::editor_has_focus`], for keys that belong to the panel as a
    /// whole: ⌘S from the find box should still save.
    pub(crate) fn editor_panel_has_focus(&self, window: &Window, cx: &Context<Self>) -> bool {
        let editor = self.editor.read(cx);
        self.editor_has_focus(window, cx)
            // The strip and the empty panel are also document command targets.
            || (self.code_panel_visible()
                && (editor.strip_keyboard_focused(window) || editor.empty_focus().is_focused(window)))
            || editor.bar_has_focus(window, cx)
            || self.tabs.get(self.tabs.active_index()).zip(self.tab_code()).is_some_and(|(tab, code)| {
                editor.markdown_find_has_focus(tab.id(), code, window, cx)
            })
    }
}

impl LyrelApp {
    // ---- Saving ----

    pub(crate) fn editor_save_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.tab_code().and_then(TabCode::active_id) else {
            return;
        };
        self.editor_save_file(id, false, window, cx);
    }

    pub(crate) fn editor_save_as_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.tab_code().and_then(TabCode::active_id) else {
            return;
        };
        self.editor_save_as(id, window, cx);
    }

    pub(crate) fn editor_save_file(
        &mut self,
        id: BufferId,
        force: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let root = self.editor_save_root();
        self.editor
            .update(cx, |editor, cx| editor.save(id, force, &root, window, cx));
    }

    pub(crate) fn editor_save_as(
        &mut self,
        id: BufferId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let root = self.editor_save_root();
        self.editor
            .update(cx, |editor, cx| editor.save_as(id, &root, window, cx));
    }

    pub(crate) fn editor_go_to_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 行号是文本的位置。图片文档没有行可去，快捷键在这里就是空操作，
        // 而不是弹出一个永远落不到任何地方的行号框。
        if !self.code_panel_visible()
            || !self
                .tab_code()
                .and_then(|code| self.editor.read(cx).active_file(code))
                .is_some_and(|file| file.input.is_text())
        {
            return;
        }
        let id = self.tab_code().and_then(TabCode::active_id).unwrap();
        self.editor.update(cx, |editor, cx| {
            editor.open_bar(BarKind::GoToLine { id }, String::new(), window, cx)
        });
    }

    pub(crate) fn editor_close_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |editor, _| editor.dismiss_bar());
        self.focus_editor(window, cx);
        cx.notify();
    }

    pub(crate) fn editor_gutter_step(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.with_editor_gutter(window, cx, |editor, code, tab, window, cx| {
            editor.gutter_step(forward, code, tab, window, cx)
        })
    }

    pub(crate) fn editor_gutter_revert_at_cursor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.with_editor_gutter(window, cx, |editor, code, tab, window, cx| {
            editor.gutter_revert_at_cursor(code, tab, window, cx)
        })
    }

    pub(crate) fn editor_gutter_peek_at_cursor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.with_editor_gutter(window, cx, |editor, code, tab, window, cx| {
            editor.gutter_peek_at_cursor(code, tab, window, cx)
        })
    }

    fn with_editor_gutter(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
        apply: impl FnOnce(
            &mut EditorPanelState,
            &TabCode,
            TabId,
            &mut Window,
            &mut Context<EditorPanelState>,
        ) -> bool,
    ) -> bool {
        if !self.editor_panel_has_focus(window, cx) {
            return false;
        }
        let Some(tab) = self.tabs.get(self.tabs.active_index()) else {
            return false;
        };
        let Some(code) = tab.code.as_deref() else {
            return false;
        };
        self.editor
            .update(cx, |editor, cx| apply(editor, code, tab.id(), window, cx))
    }

    pub(super) fn toggle_editor_git_gutter(&mut self, cx: &mut Context<Self>) {
        let on = !cx.global::<Config>().editor_git_gutter;
        self.update_config(cx, |cfg| cfg.editor_git_gutter = on);
        self.apply_editor_git_gutter(on, cx);
    }

    pub(super) fn apply_editor_git_gutter(&mut self, on: bool, cx: &mut Context<Self>) {
        self.editor
            .update(cx, |editor, cx| editor.apply_git_gutter(on, cx));
    }

    // ---- Closing ----

    /// Closes the file at `pos` in the active tab's strip. Asks first only
    /// when this is the last tab showing a buffer with unsaved changes.
    pub(crate) fn editor_close_file(
        &mut self,
        pos: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab_ix = self.tabs.active_index();
        let Some(id) = self.tab_code().and_then(|c| c.files.get(pos).copied()) else {
            return;
        };
        let shared = self.buffer_refs(id) > 1;
        let group = self.tab_code().map(|c| c.focused_group()).unwrap();
        let tab = self.tabs.get(tab_ix).map(|t| t.id()).unwrap();
        self.editor.update(cx, |editor, cx| {
            editor.finish_close_composition(id, shared, tab, group, window, cx);
        });
        let dirty = self
            .editor
            .read(cx)
            .buffers()
            .get(id)
            .is_some_and(|file| file.dirty);
        if !dirty || shared {
            self.editor_remove_from_tab(tab_ix, id, cx);
            self.focus_editor(window, cx);
            return;
        }
        self.editor_guard_unsaved(
            vec![id],
            AfterUnsaved::CloseFiles {
                tab,
                ids: vec![id],
                group,
            },
            window,
            cx,
        );
    }

    /// Closes the file with this buffer in the front tab's strip, wherever
    /// it has moved to since a menu was built for it.
    pub(crate) fn editor_close_buffer(
        &mut self,
        id: BufferId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(pos) = self
            .tab_code()
            .and_then(|c| c.files.iter().position(|f| *f == id))
        {
            self.editor_close_file(pos, window, cx);
        }
    }

    /// Closes several files in the front tab's strip, asking once about the
    /// ones whose edits would be lost with them — not those another tab still
    /// shows.
    pub(crate) fn editor_close_buffers(
        &mut self,
        ids: Vec<BufferId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab_ix = self.tabs.active_index();
        let Some(tab) = self.tabs.get(tab_ix).map(|t| t.id()) else {
            return;
        };
        let unsaved: Vec<BufferId> = ids
            .iter()
            .copied()
            .filter(|id| {
                self.editor
                    .read(cx)
                    .buffers()
                    .get(*id)
                    .is_some_and(|f| f.dirty)
                    && self.buffer_refs(*id) == 1
            })
            .collect();
        let group = self.tab_code().map(|c| c.focused_group()).unwrap();
        if self.editor_guard_unsaved(
            unsaved,
            AfterUnsaved::CloseFiles {
                tab,
                ids: ids.clone(),
                group,
            },
            window,
            cx,
        ) {
            return;
        }
        for id in ids {
            self.editor_remove_from_tab(tab_ix, id, cx);
        }
    }

    pub(crate) fn editor_close_active_if_focused(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let empty = self
            .tab_code()
            .is_some_and(|c| c.visible && c.files.is_empty());
        // ⌘⇧E on an empty panel hands the focus to the file tree, so from
        // there ⌘W is the empty panel's too.
        let tree = empty && self.file_tree.focus_handle.contains_focused(window, cx);
        if !(tree || self.editor_panel_has_focus(window, cx)) {
            return false;
        }
        let Some(code) = self.tab_code_mut() else {
            return false;
        };
        if code.files.is_empty() {
            code.visible = false;
            self.editor.update(cx, |editor, _| {
                editor.set_had_focus(None);
                editor.dismiss_bar();
            });
            self.file_tree.cancel_edit();
            self.focus_active(window, cx);
            cx.notify();
            return true;
        }
        let active = code.active;
        self.editor_close_file(active, window, cx);
        true
    }

    /// Takes a buffer out of one tab's strip, and drops it once no tab shows
    /// it any more.
    fn editor_remove_from_tab(&mut self, tab_ix: usize, id: BufferId, cx: &mut Context<Self>) {
        if let Some(code) = self
            .tabs
            .get_mut(tab_ix)
            .and_then(|t| t.code.as_deref_mut())
        {
            code.close_in_focused(id);
        }
        if self.buffer_refs(id) == 0 {
            self.editor
                .update(cx, |editor, cx| editor.drop_buffer(id, cx));
        }
        cx.notify();
    }

    /// Forgets a buffer everywhere, edits and all.
    fn editor_drop_buffer(&mut self, id: BufferId, cx: &mut Context<Self>) {
        self.editor.update(cx, |editor, cx| {
            editor.drop_buffer(id, cx);
        });
        for code in self.tabs.iter_mut().filter_map(|t| t.code.as_deref_mut()) {
            code.forget_everywhere(id);
        }
        cx.notify();
    }

    // ---- Unsaved changes standing in the way of a close ----

    /// Asks about unsaved buffers before `then` goes ahead. Returns `true`
    /// when there was something to ask about: the caller stops there, and the
    /// answer carries the close on — after saving, after discarding, or not
    /// at all.
    pub(crate) fn editor_guard_unsaved(
        &mut self,
        ids: Vec<BufferId>,
        then: AfterUnsaved,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if ids.is_empty() {
            return false;
        }
        // Showing it is part of the question: the file being asked about may
        // be behind the terminal, in a tab that is not the one in front.
        if let Some(tab_ix) = ids.first().and_then(|id| {
            self.tabs
                .iter()
                .position(|t| t.code.as_deref().is_some_and(|c| c.shows(*id)))
        }) && tab_ix == self.tabs.active_index()
            && let Some(code) = self.tab_code_mut()
            && let Some(pos) = {
                if !code.files.contains(&ids[0]) {
                    code.swap_focus();
                }
                code.files.iter().position(|f| *f == ids[0])
            }
        {
            code.active = pos;
            code.visible = true;
        }
        let root = self.editor_save_root();
        self.editor.update(cx, |editor, cx| {
            editor.guard_unsaved(ids, then, &root, window, cx)
        })
    }

    fn editor_continue(&mut self, then: AfterUnsaved, window: &mut Window, cx: &mut Context<Self>) {
        match then {
            AfterUnsaved::CloseTab(tab) => {
                if let Some(ix) = self.tab_index_of(tab) {
                    self.close_tab(ix, window, cx);
                }
            }
            AfterUnsaved::CloseFiles { tab, ids, group } => {
                if let Some(ix) = self.tab_index_of(tab) {
                    for id in ids {
                        if let Some(code) = self
                            .tabs
                            .get_mut(ix)
                            .and_then(|tab| tab.code.as_deref_mut())
                        {
                            code.close_in_group(id, group);
                        }
                        if self.buffer_refs(id) == 0 {
                            self.editor
                                .update(cx, |editor, cx| editor.drop_buffer(id, cx));
                        }
                    }
                }
            }
            AfterUnsaved::ClosePane(pane) => self.close_pane_after_unsaved(pane, window, cx),
            AfterUnsaved::CloseWindow => self.close_window_after_unsaved(window, cx),
            AfterUnsaved::Quit => self.quit_after_unsaved(window, cx),
        }
    }

    // ---- The file tree moving files out from under their buffers ----

    /// A file or folder was renamed through lyrel: buffers at or under the old
    /// path follow it. Without this the buffer kept the old name, and the next
    /// save wrote a second copy there.
    pub(crate) fn editor_path_moved(
        &mut self,
        host: crate::ui::host_ops::HostId,
        from: &Path,
        to: &Path,
        cx: &mut Context<Self>,
    ) {
        self.editor.update(cx, |editor, cx| {
            editor.path_moved(host, from, to, cx);
        });
    }

    /// A file or folder was deleted through lyrel: its buffers say so rather
    /// than waiting for the watcher, which a remote host may not have.
    pub(crate) fn editor_path_removed(
        &mut self,
        host: crate::ui::host_ops::HostId,
        path: &Path,
        cx: &mut Context<Self>,
    ) {
        self.editor.update(cx, |editor, cx| {
            editor.path_removed(host, path, cx);
        });
    }

    // ---- Changes made by something else ----

    #[cfg(test)]
    pub(crate) fn editor_reload_from_disk(
        &mut self,
        id: BufferId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.update(cx, |editor, cx| {
            editor.reload_from_disk(id, window, cx);
        });
    }

    // ---- Keeping the registry, the tabs and the session store in step ----

    /// Once per frame: buffers no tab shows any more are dropped — or, if
    /// they hold unsaved work, handed to the tab in front rather than lost —
    /// the active tab gets back the files it had open last time, and any
    /// tab whose files changed is recorded for next time.
    ///
    /// Done here rather than at each place a tab can go — a close, a shell
    /// that exits, a tab dragged into another, a workspace switch, a server
    /// restart — because this sees all of them, including ones added later.
    pub(crate) fn editor_sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .editor
            .update(cx, |editor, _| editor.take_watch_refresh())
        {
            self.editor_rebuild_watcher(cx);
        }
        let live: std::collections::HashSet<TabId> = self.tabs.iter().map(|t| t.id()).collect();
        let root = self.editor_save_root();
        self.editor.update(cx, |editor, cx| {
            editor.sync(&live, &root, window, cx);
        });
        self.editor_adopt_orphans(window, cx);
        self.editor_restore_active(window, cx);
        let requests = self
            .editor
            .update(cx, |editor, cx| editor.take_drawio_save_requests(cx));
        for id in requests {
            // The engine's own Ctrl+S rides the same Host save path as the
            // toolbar action: conflicts, prompts and write identity stay
            // exactly one authority.
            self.editor_save_file(id, false, window, cx);
        }
        self.editor.update(cx, |editor, cx| {
            let shown = |id: BufferId, tab: TabId, group| {
                self.tabs
                    .iter()
                    .find(|entry| entry.id() == tab)
                    .and_then(|entry| entry.code.as_deref())
                    .is_some_and(|code| {
                        (code.group() == group && code.files.contains(&id))
                            || code
                                .split
                                .as_ref()
                                .is_some_and(|other| other.id == group && other.files.contains(&id))
                    })
            };
            editor.retain_presentations(&shown, window, cx);
        });
        self.editor_record_sessions(cx);
    }

    /// A deferred diagram save whose engine refused or never confirmed its
    /// model is a failed save: the editor says so itself, inside its sync.
    fn editor_adopt_orphans(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // No tabs at all is the gap in the middle of a server restart or a
        // workspace switch, not a verdict on the buffers: wait for the tabs.
        if self.editor.read(cx).buffers().is_empty() || self.tabs.is_empty() {
            return;
        }
        let shown: std::collections::HashSet<BufferId> = self
            .tabs
            .iter()
            .filter_map(|t| t.code.as_deref())
            .flat_map(TabCode::all_files)
            .collect();
        let orphans: Vec<(BufferId, bool)> = self
            .editor
            .read(cx)
            .buffers()
            .iter()
            .filter(|b| !shown.contains(&b.id()))
            .map(|b| (b.id(), b.dirty))
            .collect();
        if orphans.is_empty() {
            return;
        }
        let mut changed = false;
        for (id, dirty) in orphans {
            if !dirty {
                self.editor
                    .update(cx, |editor, cx| editor.drop_buffer(id, cx));
                changed = true;
                continue;
            }
            // Nowhere to put it (every tab gone mid-restart): keep it, and it
            // is adopted on a later frame.
            let Some(tab) = self.tabs.get(self.tabs.active_index()).map(|t| t.id()) else {
                continue;
            };
            let name = self
                .editor
                .read(cx)
                .buffers()
                .get(id)
                .map(crate::ui::code_editor::OpenFile::label)
                .unwrap_or_default();
            self.editor_show_in_tab(tab, id, false, window, cx);
            window.notify_app(t_fmt(L10nKey::EditorOrphanAdopted, &[("name", &name)]), cx);
            changed = true;
        }
        if changed {
            self.editor_rebuild_watcher(cx);
            cx.notify();
        }
    }

    pub(crate) fn editor_restore_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(self.tabs.active_index()) else {
            return;
        };
        let tab_id = tab.id();
        let has_files = tab.code.as_deref().is_some_and(|c| !c.files.is_empty());
        if !self.editor.read(cx).needs_restore(tab_id) {
            return;
        }
        let Some(host) = self.active_host(cx) else {
            return;
        };
        let workspace = self.workspace;
        let window_host = self.spawn_host(cx);
        self.editor.update(cx, |editor, cx| {
            editor.restore_tab(host, tab_id, has_files, workspace, window_host, window, cx);
        });
    }

    pub(crate) fn editor_record_sessions(&mut self, cx: &mut Context<Self>) {
        let spawn_host = self.spawn_host(cx);
        let mut changed: Vec<(TabId, crate::ui::editor_session::TabEditor)> = Vec::new();
        for tab in &self.tabs {
            let Some(code) = tab.code.as_deref() else {
                continue;
            };
            let tab_id = tab.id();
            // A tab still waiting for its files to come back has nothing to
            // say about them yet.
            if !self.editor.read(cx).is_restored(&tab_id) {
                continue;
            }
            // Only files on the window's own machine: an SFTP buffer's host
            // is a connection that will not exist next launch.
            let group = |ids: &[BufferId], front: usize| -> (Vec<PathBuf>, usize) {
                let mut files = Vec::new();
                let mut active = 0;
                for (pos, id) in ids.iter().enumerate() {
                    let Some(f) = self.editor.read(cx).buffers().get(*id) else {
                        continue;
                    };
                    if f.is_untitled() || f.host.id() != spawn_host {
                        continue;
                    }
                    if pos == front {
                        active = files.len();
                    }
                    files.push(f.path.clone());
                }
                (files, active)
            };
            let sides = code.sides();
            let (files, active) = group(&sides[0].files, sides[0].active);
            let split = sides.get(1).and_then(|right| {
                let (files, active) = group(&right.files, right.active);
                (!files.is_empty()).then_some(crate::ui::editor_session::SplitEditor {
                    files,
                    active,
                    focused: right.focused,
                })
            });
            let state = crate::ui::editor_session::TabEditor {
                files,
                active,
                visible: code.visible,
                split,
            };
            if !self.editor.read(cx).is_recorded(tab_id, &state) {
                changed.push((tab_id, state));
            }
        }
        for (tab_id, state) in changed {
            self.editor.update(cx, |editor, _| {
                editor.record_session(tab_id, state.clone());
            });
            crate::ui::editor_session::put(cx, tab_id, state);
        }
    }

    /// A tab is being closed: the files only it showed go with it. Unsaved
    /// ones were asked about before the close got here — except when a shell
    /// exiting took the tab, which cannot ask; those stay, and are handed to
    /// the tab in front.
    pub(crate) fn editor_close_tab_files(&mut self, tab_ix: usize, cx: &mut Context<Self>) {
        let Some(ids) = self
            .tabs
            .get(tab_ix)
            .and_then(|t| t.code.as_deref())
            .map(TabCode::all_files)
        else {
            return;
        };
        for id in ids {
            if self.buffer_tabs(id) == 1
                && self
                    .editor
                    .read(cx)
                    .buffers()
                    .get(id)
                    .is_some_and(|b| !b.dirty)
            {
                self.editor
                    .update(cx, |editor, cx| editor.drop_buffer(id, cx));
            }
        }
    }

    /// A tab was closed for good: forget what it had open.
    pub(crate) fn editor_forget_tab(&mut self, tab: TabId, cx: &mut Context<Self>) {
        self.editor.update(cx, |editor, _| {
            editor.forget_tab(tab);
        });
        crate::ui::editor_session::remove(cx, tab);
    }

    // ---- The panel's cross-panel intents ----

    /// The window-scoped subscription to the editor panel's intents,
    /// installed at bootstrap next to the other entity watches.
    pub(crate) fn watch_editor_panel(
        editor: &Entity<EditorPanelState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Subscription {
        cx.subscribe_in(editor, window, |app, _, event, window, cx| {
            app.on_editor_panel_event(event, window, cx);
        })
    }

    fn on_editor_panel_event(
        &mut self,
        event: &EditorPanelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            EditorPanelEvent::BufferEdited { id } => {
                self.lsp_buffer_edited(*id, cx);
            }
            EditorPanelEvent::BufferSaved {
                id,
                host,
                directory,
            } => {
                self.lsp_buffer_saved(*id, cx);
                if let Some(directory) = directory {
                    self.scm_invalidate_cwd(*host, directory, cx);
                }
            }
            EditorPanelEvent::IdentityChanged => self.editor_rebuild_watcher(cx),
            EditorPanelEvent::RenameRequested { id, offset, name } => {
                self.lsp_rename(*id, *offset, name.clone(), window, cx);
            }
            EditorPanelEvent::Continue(intent) => self.editor_continue(intent.clone(), window, cx),
            EditorPanelEvent::BuffersDiscarded { ids } => {
                for id in ids {
                    self.editor_drop_buffer(*id, cx);
                }
            }
            EditorPanelEvent::OpenCompleted {
                workspace,
                window_host,
                tab,
                group,
                id,
                created,
                front,
            } => {
                // A completion that outlived its tab or group, or arrived
                // after a workspace switch, has nothing to land in: the
                // buffer stays in the registry and orphan adoption reclaims
                // or adopts it.
                if *workspace != self.workspace || *window_host != self.spawn_host(cx) {
                    return;
                }
                let Some(tab_ix) = self.tab_index_of(*tab) else {
                    return;
                };
                let tab_exists = self
                    .tabs
                    .get(tab_ix)
                    .and_then(|t| t.code.as_deref())
                    .is_some_and(|code| {
                        group.is_none_or(|group| {
                            code.group() == group
                                || code.split.as_ref().is_some_and(|other| other.id == group)
                        })
                    });
                if !tab_exists {
                    return;
                }
                if *created {
                    self.watch_editor_buffer_focus(*id, window, cx);
                }
                self.editor_show_open_target(*tab, *group, *id, *front, window, cx);
                self.editor_rebuild_watcher(cx);
            }
            EditorPanelEvent::TabRestored {
                workspace,
                window_host,
                tab,
                open,
                snapshot,
            } => {
                if *workspace != self.workspace
                    || *window_host != self.spawn_host(cx)
                    || open.is_empty()
                {
                    return;
                }
                let Some(tab_ix) = self.tab_index_of(*tab) else {
                    return;
                };
                for (_, id, created) in open {
                    if *created {
                        self.watch_editor_buffer_focus(*id, window, cx);
                    }
                }
                let group = |paths: &[PathBuf], active: usize| -> (Vec<BufferId>, usize) {
                    let front = paths.get(active);
                    let mut ids: Vec<BufferId> = Vec::new();
                    let mut at = 0;
                    for path in paths {
                        let Some(&(_, id, _)) = open.iter().find(|(p, _, _)| p == path) else {
                            continue;
                        };
                        if ids.contains(&id) {
                            continue;
                        }
                        if Some(path) == front {
                            at = ids.len();
                        }
                        ids.push(id);
                    }
                    (ids, at)
                };
                let (left, left_active) = group(&snapshot.files, snapshot.active);
                let right = snapshot
                    .split
                    .as_ref()
                    .map(|s| (group(&s.files, s.active), s.focused));
                let code = self.tabs[tab_ix]
                    .code
                    .get_or_insert_with(|| Box::new(TabCode::new()));
                let restored: Vec<BufferId> = open.iter().map(|(_, id, _)| *id).collect();
                if code.is_split() {
                    // A split made while I/O was pending keeps its layout.
                    for id in &restored {
                        code.append(*id);
                    }
                }
                // Merged into what the tab has now, not written over it: a
                // file opened, or a split made, while these were loading stays.
                let untouched = code.restore_groups(
                    &restored,
                    (left, left_active),
                    right.map(|((files, active), focused)| (files, active, focused)),
                );
                let visible = snapshot.visible && !code.files.is_empty();
                code.visible |= visible;
                // Only a restore nobody has touched takes the keyboard; one the
                // reader has moved on from leaves it where they put it.
                if untouched && tab_ix == self.tabs.active_index() && visible {
                    self.focus_editor(window, cx);
                }
                self.editor_rebuild_watcher(cx);
                cx.notify();
            }
            EditorPanelEvent::AttachToAgent { path, suffix } => {
                self.attach_path_to_agent(path, suffix, cx)
            }
            EditorPanelEvent::OpenExternally { host, path } => {
                self.open_outside_the_editor(*host, path, window, cx)
            }
            EditorPanelEvent::FollowLink {
                buffer,
                source,
                host,
                path,
                anchor,
                revision,
            } => self.editor_follow_link(
                *buffer,
                *source,
                host.clone(),
                path,
                anchor.clone(),
                *revision,
                window,
                cx,
            ),
            EditorPanelEvent::FocusDocument => self.focus_editor(window, cx),
            EditorPanelEvent::StatusCommand(command) => {
                match command {
                    EditorStatusCommand::TogglePreview => self.toggle_document_preview(window, cx),
                    EditorStatusCommand::ToggleWrap => self.toggle_document_wrap(window, cx),
                    EditorStatusCommand::ToggleLineEnding => self.toggle_line_ending(cx),
                    EditorStatusCommand::GoToLine => {
                        self.editor_go_to_line(window, cx);
                        return;
                    }
                }
                self.focus_editor(window, cx);
            }
            EditorPanelEvent::GoToLineRequested { id, line, column } => {
                self.editor_go_to_line_landed(*id, *line, *column, window, cx);
            }
            EditorPanelEvent::RebuildWatcher => self.editor_rebuild_watcher(cx),
        }
    }

    /// The go-to-line bar's answer lands here: focus the front document, then
    /// put the cursor where the bar said, on the same bounded retry the old
    /// bar handler used.
    fn editor_go_to_line_landed(
        &mut self,
        id: BufferId,
        line: u32,
        column: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_editor(window, cx);
        let (Some(tab), Some(code)) = (
            self.tabs.get(self.tabs.active_index()).map(|t| t.id()),
            self.tab_code(),
        ) else {
            return;
        };
        let group = code.group();
        let Some(input) = self
            .editor
            .read(cx)
            .editor_input_for(id, tab, group)
            .filter(|i| i.is_text())
        else {
            return;
        };
        let text = input.text().expect("checked text input");
        let lines = text.read(cx).text().lines_len().max(1) as u32;
        crate::ui::code_editor::place_cursor(
            input,
            Position {
                line: line.min(lines) - 1,
                character: column - 1,
            },
            crate::ui::code_editor::CURSOR_SCROLL_ATTEMPTS,
            window,
            cx,
        );
        cx.notify();
    }

    /// What to do with a file the built-in editor cannot show.
    ///
    /// A click on a PNG or a `.zip` meant "open this", not "tell me it is not
    /// text", and on this machine the desktop knows how. A file on another
    /// machine has nobody here to hand it to, so that one gets the words.
    ///
    /// A program does not: `open` on a Mach-O binary runs it, and a build's
    /// output is full of paths to programs. Clicking a word in a terminal
    /// must not be a way to execute one, so those keep the words too.
    fn open_outside_the_editor(
        &mut self,
        host_id: crate::ui::host_ops::HostId,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !host_id.is_local()
            || !self.can_spawn_locally(cx)
            || crate::ui::code_editor::is_program(path)
        {
            window.notify_app(
                t_fmt(
                    L10nKey::EditorBinaryFile,
                    &[("path", &path.display().to_string())],
                ),
                cx,
            );
            return;
        }
        Self::open_with(path, crate::ui::desktop_file::open_file_path, window, cx);
    }

    /// Runs one of the desktop openers on a local file, and says so when it
    /// fails to spawn — as any opener can (#542).
    pub(super) fn open_with(
        path: &Path,
        opener: fn(&Path) -> std::io::Result<()>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(e) = opener(path) {
            log::warn!("failed to open {}: {e}", path.display());
            window.notify_app(
                crate::ui::host_ops::failure(
                    t_fmt(
                        L10nKey::LinkFileOpenFailed,
                        &[
                            ("path", &path.display().to_string()),
                            ("error", &e.to_string()),
                        ],
                    ),
                    &e,
                ),
                cx,
            );
        }
    }
}

impl LyrelApp {
    pub(crate) fn toggle_editor_problems(&mut self, cx: &mut Context<Self>) {
        self.editor
            .update(cx, |editor, cx| editor.toggle_problems(cx));
    }

    /// Where a file's name sits in the list: relative to the project the
    /// tab shows, when it is inside it.
    fn problems_label(&self, path: &Path, cx: &App) -> (String, String) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        let roots = self
            .tab_code()
            .map(|code| code.roots.as_slice())
            .unwrap_or(&[]);
        let spelled = self
            .file_tree
            .project_spelling(self.spawn_host(cx), roots, path);
        let dir = spelled.parent().map(|parent| {
            self.tab_code()
                .and_then(|c| {
                    c.roots
                        .iter()
                        .find_map(|r| parent.strip_prefix(r).ok().map(|p| (r, p)))
                })
                .map(|(root, rel)| {
                    let repo = root
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    match rel.as_os_str().is_empty() {
                        true => repo,
                        false => format!("{repo}/{}", rel.display()),
                    }
                })
                .unwrap_or_else(|| parent.display().to_string())
        });
        (name, dir.unwrap_or_default())
    }

    pub(crate) fn render_editor_problems(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.editor.read(cx).problems_open() {
            return None;
        }
        let files = collect(LspStore::diagnostics_snapshot(cx));
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let (errors, warnings) =
            files
                .iter()
                .flat_map(|f| &f.problems)
                .fold((0, 0), |(e, w), p| match p.severity {
                    Severity::Error => (e + 1, w),
                    Severity::Warning => (e, w + 1),
                    Severity::Info => (e, w),
                });
        let header = h_flex()
            .flex_none()
            .h(px(28.))
            .px(px(crate::ui::window_chrome::CONTENT_INSET))
            .gap_2()
            .items_center()
            .border_t(crate::ui::theme::hairline(window))
            .border_color(theme.sidebar_border)
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .child(t(L10nKey::EditorProblemsTitle)),
            )
            .child(div().text_xs().text_color(muted).child(t_fmt(
                L10nKey::LspProblemsTooltip,
                &[
                    ("errors", &errors.to_string()),
                    ("warnings", &warnings.to_string()),
                ],
            )))
            .child(div().flex_1())
            .child(
                Button::new("editor-problems-close")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.editor
                            .update(cx, |editor, cx| editor.toggle_problems(cx))
                    })),
            );

        let mut rows: Vec<AnyElement> = Vec::new();
        let mut left = 0usize;
        for (file_ix, file) in files.iter().enumerate() {
            if rows.len() >= MAX_ROWS {
                left += file.problems.len();
                continue;
            }
            let (name, dir) = self.problems_label(&file.path, cx);
            rows.push(
                h_flex()
                    .id(("editor-problems-file", file_ix))
                    .h(px(24.))
                    .px(px(crate::ui::window_chrome::CONTENT_INSET))
                    .gap_2()
                    .items_center()
                    .text_sm()
                    .child(
                        div()
                            .flex_none()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .child(name),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .text_ellipsis()
                            .text_xs()
                            .text_color(muted)
                            .child(dir),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(muted)
                            .child(file.problems.len().to_string()),
                    )
                    .into_any_element(),
            );
            for (ix, problem) in file.problems.iter().enumerate() {
                if rows.len() >= MAX_ROWS {
                    left += file.problems.len() - ix;
                    break;
                }
                let (icon, color) = match problem.severity {
                    Severity::Error => (IconName::CircleX, theme.danger),
                    Severity::Warning => (IconName::TriangleAlert, theme.warning),
                    Severity::Info => (IconName::Info, theme.info),
                };
                let at = t_fmt(
                    L10nKey::EditorLnCol,
                    &[
                        ("line", &(problem.line + 1).to_string()),
                        ("column", &(problem.column + 1).to_string()),
                    ],
                );
                let path = file.path.clone();
                let (line, column) = (problem.line + 1, problem.column + 1);
                rows.push(
                    h_flex()
                        .id(("editor-problem", rows.len()))
                        .h(px(22.))
                        .pl(px(crate::ui::window_chrome::CONTENT_INSET + 12.))
                        .pr(px(crate::ui::window_chrome::CONTENT_INSET))
                        .gap_2()
                        .items_center()
                        .text_sm()
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.list_hover))
                        .child(Icon::new(icon).xsmall().text_color(color))
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(headline(&problem.message).to_string()),
                        )
                        .children(problem.source.clone().map(|source| {
                            div().flex_none().text_xs().text_color(muted).child(source)
                        }))
                        .child(div().flex_none().text_xs().text_color(muted).child(at))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_file_in_editor_at(
                                &path,
                                Some(line),
                                Some(column),
                                window,
                                cx,
                            );
                        }))
                        .into_any_element(),
                );
            }
        }
        if left > 0 {
            rows.push(
                div()
                    .px(px(crate::ui::window_chrome::CONTENT_INSET))
                    .py_1()
                    .text_xs()
                    .text_color(muted)
                    .child(t_fmt(
                        L10nKey::EditorProblemsMore,
                        &[("n", &left.to_string())],
                    ))
                    .into_any_element(),
            );
        }
        let body: AnyElement = if files.is_empty() {
            div()
                .px(px(crate::ui::window_chrome::CONTENT_INSET))
                .py_2()
                .text_sm()
                .text_color(muted)
                .child(t(L10nKey::EditorProblemsNone))
                .into_any_element()
        } else {
            let scroll = self.editor.read(cx).problems_scroll();
            crate::ui::scrollbar::with_vertical_scrollbar(
                "editor-problems-scrollbar",
                v_flex()
                    .id("editor-problems-list")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .children(rows),
                &scroll,
            )
        };
        Some(
            v_flex()
                .id("editor-problems")
                .test_support()
                .flex_none()
                .h(px(PANE_HEIGHT))
                .w_full()
                .child(header)
                .child(div().flex_1().min_h_0().child(body))
                .into_any_element(),
        )
    }
}

fn install_editor_commands(
    mut root: gpui_kit::Stateful<gpui_kit::Div>,
    cx: &mut Context<LyrelApp>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    use commands::*;
    macro_rules! bind {
        ($($action:ty => $command:ident),* $(,)?) => { $(
            root = root.on_action(cx.listener(|app, _: &$action, window, cx| {
                if app.editor_has_focus(window, cx) {
                    app.editor_apply_command(Command::$command, window, cx);
                } else { cx.propagate(); }
            }));
        )* };
    }
    bind!(
        ToggleLineComment => Comment, ToggleBlockComment => BlockComment,
        MoveLineUp => MoveUp, MoveLineDown => MoveDown, CopyLineUp => CopyUp,
        CopyLineDown => CopyDown, DeleteLine => Delete, InsertLineAbove => InsertAbove,
        InsertLineBelow => InsertBelow, SelectLine => SelectLine, MoveToMatchingBracket => Bracket,
        SelectNextOccurrence => Next, SelectAllOccurrences => All, SkipOccurrence => Skip,
        TransformToUppercase => Upper, TransformToLowercase => Lower, TransformToTitleCase => Title,
        TrimTrailingWhitespace => Trim, JoinLines => Join, RemoveSurroundingBrackets => Unwrap,
    );
    root.on_action(
        cx.listener(|app, _: &gpui_kit::component::input::Search, window, cx| {
            if !app.editor_markdown_search(false, window, cx) {
                cx.propagate();
            }
        }),
    )
    .on_action(
        cx.listener(|app, _: &gpui_kit::component::input::Replace, window, cx| {
            if !app.editor_markdown_search(true, window, cx) {
                cx.propagate();
            }
        }),
    )
    .on_action(cx.listener(
        |app, _: &crate::core::actions::FindInTerminal, window, cx| {
            if !app.editor_markdown_search(false, window, cx) {
                cx.propagate();
            }
        },
    ))
    .on_action(
        cx.listener(|app, _: &crate::core::actions::FindNext, window, cx| {
            if !app.editor_markdown_find_step(true, window, cx) {
                cx.propagate();
            }
        }),
    )
    .on_action(
        cx.listener(|app, _: &crate::core::actions::FindPrevious, window, cx| {
            if !app.editor_markdown_find_step(false, window, cx) {
                cx.propagate();
            }
        }),
    )
}
