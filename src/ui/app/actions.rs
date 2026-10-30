//! Window Action adaptation only; capability state transitions remain with their owners.
use super::*;

impl LyrelApp {
    pub(super) fn bind_window_actions(
        root: gpui_kit::Stateful<gpui_kit::Div>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let root = Self::bind_workspace_actions(root, cx);
        let root = Self::bind_panes_actions(root, cx);
        let root = Self::bind_navigation_actions(root, cx);
        let root = Self::bind_documents_actions(root, cx);
        let root = Self::bind_sessions_actions(root, cx);
        let root = Self::bind_application_actions(root, cx);
        root
    }

    fn bind_workspace_actions(
        root: gpui_kit::Stateful<gpui_kit::Div>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        root.on_action(cx.listener(|this, _: &NewTab, window, cx| this.new_tab(window, cx)))
            .on_action(cx.listener(|this, _: &SelectWorkspace1, window, cx| {
                this.select_workspace_slot(0, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectWorkspace2, window, cx| {
                this.select_workspace_slot(1, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectWorkspace3, window, cx| {
                this.select_workspace_slot(2, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectWorkspace4, window, cx| {
                this.select_workspace_slot(3, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectWorkspace5, window, cx| {
                this.select_workspace_slot(4, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectWorkspace6, window, cx| {
                this.select_workspace_slot(5, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectWorkspace7, window, cx| {
                this.select_workspace_slot(6, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectWorkspace8, window, cx| {
                this.select_workspace_slot(7, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectWorkspace9, window, cx| {
                this.select_workspace_slot(8, window, cx)
            }))
            .on_action(cx.listener(|this, _: &RenameWorkspace, window, cx| {
                this.start_workspace_rename(window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ToggleSwitcher, window, cx| {
                    this.toggle_switcher(window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &StopWorkspace, window, cx| {
                let id = this.workspace;
                this.stop_workspace(id, window, cx);
            }))
            .on_action(cx.listener(|this, _: &DeleteWorkspace, window, cx| {
                let id = this.workspace;
                this.delete_workspace(id, window, cx);
            }))
            .on_action(cx.listener(|this, _: &NewWorkspace, window, cx| {
                this.open_workspace_form(window, cx);
            }))
            .on_action(cx.listener(|this, _: &NewWindow, _window, cx| {
                this.new_window(cx);
            }))
            .on_action(
                cx.listener(|this, _: &CloseWindow, window, cx| this.close_window(window, cx)),
            )
    }

    fn bind_panes_actions(
        root: gpui_kit::Stateful<gpui_kit::Div>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        root.on_action(cx.listener(|this, _: &CloseActiveTab, window, cx| {
            if !this.editor_close_active_if_focused(window, cx) {
                this.close_pane(window, cx)
            }
        }))
        .on_action(
            cx.listener(|this, _: &SplitRight, window, cx| {
                this.split(Axis::Horizontal, window, cx)
            }),
        )
        .on_action(
            cx.listener(|this, _: &SplitDown, window, cx| this.split(Axis::Vertical, window, cx)),
        )
        .on_action(
            cx.listener(|this, _: &MovePaneToTab, window, cx| this.move_pane_to_tab(window, cx)),
        )
        .on_action(
            cx.listener(|this, _: &TogglePanePip, window, cx| this.toggle_pane_pip(window, cx)),
        )
        .on_action(
            cx.listener(|this, _: &RestorePanePip, window, cx| this.restore_pane_pip(window, cx)),
        )
        .on_action(cx.listener(|this, _: &ParkPane, window, cx| this.park_active_pane(window, cx)))
        .on_action(cx.listener(|this, _: &RestoreParkedPane, window, cx| {
            this.preview_parked_pane(window, cx)
        }))
        .on_action(cx.listener(|this, _: &OpenScrollbackInEditor, window, cx| {
            this.open_scrollback_in_editor(window, cx)
        }))
        .on_action(cx.listener(|this, _: &ClearPaneScreen, window, cx| {
            if let Some(view) = this.focused_leaf(window, cx) {
                view.update(cx, |view, cx| view.clear_screen(cx));
            }
        }))
        .on_action(
            cx.listener(|this, _: &RestartPaneShell, window, cx| {
                this.restart_pane_shell(window, cx)
            }),
        )
        .on_action(
            cx.listener(|this, _: &FocusNextPane, window, cx| this.cycle_pane(true, window, cx)),
        )
        .on_action(
            cx.listener(|this, _: &FocusPrevPane, window, cx| this.cycle_pane(false, window, cx)),
        )
        .on_action(cx.listener(|this, _: &FocusPaneLeft, window, cx| {
            this.focus_pane_dir(Dir::Left, window, cx)
        }))
        .on_action(cx.listener(|this, _: &FocusPaneRight, window, cx| {
            this.focus_pane_dir(Dir::Right, window, cx)
        }))
        .on_action(
            cx.listener(|this, _: &FocusPaneUp, window, cx| {
                this.focus_pane_dir(Dir::Up, window, cx)
            }),
        )
        .on_action(cx.listener(|this, _: &FocusPaneDown, window, cx| {
            this.focus_pane_dir(Dir::Down, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ResizePaneLeft, window, cx| {
            this.resize_pane(Dir::Left, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ResizePaneRight, window, cx| {
            this.resize_pane(Dir::Right, window, cx)
        }))
        .on_action(
            cx.listener(|this, _: &ResizePaneUp, window, cx| this.resize_pane(Dir::Up, window, cx)),
        )
        .on_action(cx.listener(|this, _: &ResizePaneDown, window, cx| {
            this.resize_pane(Dir::Down, window, cx)
        }))
        .on_action(
            cx.listener(|this, _: &SwapPaneNext, window, cx| this.swap_pane(true, window, cx)),
        )
        .on_action(
            cx.listener(|this, _: &SwapPanePrev, window, cx| this.swap_pane(false, window, cx)),
        )
        .on_action(cx.listener(|this, _: &NextTab, window, cx| this.tab_switch(true, window, cx)))
        .on_action(cx.listener(|this, _: &PrevTab, window, cx| this.tab_switch(false, window, cx)))
        .on_action(
            cx.listener(|this, _: &SelectNextTab, window, cx| this.cycle_tab(true, window, cx)),
        )
        .on_action(
            cx.listener(|this, _: &SelectPrevTab, window, cx| this.cycle_tab(false, window, cx)),
        )
        .on_action(cx.listener(|this, _: &MoveTabLeft, _window, cx| this.move_tab(false, cx)))
        .on_action(cx.listener(|this, _: &MoveTabRight, _window, cx| this.move_tab(true, cx)))
        .on_action(
            cx.listener(|this, _: &ActivateTab1, window, cx| this.activate_visual(0, window, cx)),
        )
        .on_action(
            cx.listener(|this, _: &ActivateTab2, window, cx| this.activate_visual(1, window, cx)),
        )
        .on_action(
            cx.listener(|this, _: &ActivateTab3, window, cx| this.activate_visual(2, window, cx)),
        )
        .on_action(
            cx.listener(|this, _: &ActivateTab4, window, cx| this.activate_visual(3, window, cx)),
        )
        .on_action(
            cx.listener(|this, _: &ActivateTab5, window, cx| this.activate_visual(4, window, cx)),
        )
        .on_action(
            cx.listener(|this, _: &ActivateTab6, window, cx| this.activate_visual(5, window, cx)),
        )
        .on_action(
            cx.listener(|this, _: &ActivateTab7, window, cx| this.activate_visual(6, window, cx)),
        )
        .on_action(
            cx.listener(|this, _: &ActivateTab8, window, cx| this.activate_visual(7, window, cx)),
        )
        .on_action(
            cx.listener(|this, _: &ActivateTab9, window, cx| this.activate_visual(8, window, cx)),
        )
    }

    fn bind_navigation_actions(
        root: gpui_kit::Stateful<gpui_kit::Div>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        root.on_action(cx.listener(|this, _: &IncreaseFontSize, _window, cx| {
            this.change_font_size(FONT_SIZE_STEP, cx)
        }))
        .on_action(cx.listener(|this, _: &DecreaseFontSize, _window, cx| {
            this.change_font_size(-FONT_SIZE_STEP, cx)
        }))
        .on_action(cx.listener(|this, _: &ResetFontSize, _window, cx| this.reset_font_size(cx)))
        .on_action(
            cx.listener(|this, _: &TogglePalette, window, cx| this.toggle_search(window, cx)),
        )
        .on_action(
            cx.listener(|this, _: &QuickOpenFile, window, cx| this.quick_open_file(window, cx)),
        )
        .on_action(
            cx.listener(|this, _: &ReopenClosedTab, window, cx| this.reopen_closed_tab(window, cx)),
        )
        .on_action(
            cx.listener(|this, _: &ToggleMaximizePane, window, cx| {
                this.toggle_maximize(window, cx)
            }),
        )
        .on_action(
            cx.listener(|this, _: &ToggleFullscreen, window, cx| {
                this.toggle_fullscreen(window, cx)
            }),
        )
        .on_action(cx.listener(|this, _: &ToggleLeftPanel, _window, cx| this.toggle_left_panel(cx)))
        .on_action(
            cx.listener(|this, _: &ToggleRightPanel, _window, cx| this.toggle_right_panel(cx)),
        )
        .on_action(cx.listener(|this, _: &ShowRightPanelInfo, _window, cx| {
            this.set_right_panel_tab(crate::core::config::RightPanelTab::Info, cx)
        }))
        .on_action(cx.listener(|this, _: &ShowRightPanelChanges, _window, cx| {
            this.set_right_panel_tab(crate::core::config::RightPanelTab::Scm, cx)
        }))
        .on_action(cx.listener(|this, _: &ShowRightPanelFiles, _window, cx| {
            this.set_right_panel_tab(crate::core::config::RightPanelTab::Files, cx)
        }))
        .on_action(cx.listener(|this, _: &ShowRightPanelSearch, _window, cx| {
            this.set_right_panel_tab(crate::core::config::RightPanelTab::Search, cx)
        }))
        .on_action(cx.listener(|this, _: &ShowRightPanelGitHub, _window, cx| {
            this.set_right_panel_tab(crate::core::config::RightPanelTab::GitHub, cx)
        }))
        .on_action(cx.listener(|this, _: &ScmToggleGraph, _window, cx| this.scm_toggle_graph(cx)))
        .on_action(
            cx.listener(|this, _: &ToggleDiffViewMode, _window, cx| this.toggle_diff_view_mode(cx)),
        )
        .on_action(
            cx.listener(|this, _: &ToggleDocumentFill, _window, cx| this.toggle_document_fill(cx)),
        )
        .on_action(cx.listener(|this, _: &DocumentWidthThird, _window, cx| {
            this.set_document_ratio(crate::core::config::DOCUMENT_RATIO_THIRD, cx)
        }))
        .on_action(cx.listener(|this, _: &DocumentWidthHalf, _window, cx| {
            this.set_document_ratio(crate::core::config::DOCUMENT_RATIO_HALF, cx)
        }))
        .on_action(
            cx.listener(|this, _: &DocumentWidthTwoThirds, _window, cx| {
                this.set_document_ratio(crate::core::config::DOCUMENT_RATIO_TWO_THIRDS, cx)
            }),
        )
        .on_action(cx.listener(|this, _: &ToggleDocumentPreview, window, cx| {
            this.toggle_document_preview(window, cx)
        }))
        .on_action(cx.listener(|this, _: &ToggleDocumentWrap, window, cx| {
            this.toggle_document_wrap(window, cx)
        }))
        .on_action(cx.listener(|this, _: &ScmCommit, window, cx| {
            this.run_scm_action(ScmIntent::Commit, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ScmCommitAmend, window, cx| {
            this.run_scm_action(ScmIntent::CommitAmend, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ScmStageAll, window, cx| {
            this.run_scm_action(ScmIntent::StageAll, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ScmUnstageAll, window, cx| {
            this.run_scm_action(ScmIntent::UnstageAll, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ScmDiscardAll, window, cx| {
            this.run_scm_action(ScmIntent::DiscardAll, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ScmRefresh, window, cx| {
            this.run_scm_action(ScmIntent::Refresh, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ScmSync, window, cx| {
            this.run_scm_action(ScmIntent::Sync, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ScmPush, window, cx| {
            this.run_scm_action(ScmIntent::Push, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ScmPull, window, cx| {
            this.run_scm_action(ScmIntent::Pull, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ScmFetch, window, cx| {
            this.run_scm_action(ScmIntent::Fetch, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ScmCheckoutBranch, window, cx| {
            this.run_scm_action(ScmIntent::CheckoutBranch, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ScmCreateBranch, window, cx| {
            this.run_scm_action(ScmIntent::CreateBranch, window, cx)
        }))
    }

    fn bind_documents_actions(
        root: gpui_kit::Stateful<gpui_kit::Div>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        root.on_action(
            cx.listener(|this, _: &OpenSettings, window, cx| this.toggle_settings(window, cx)),
        )
        .on_action(
            cx.listener(|this, _: &RestartDaemon, window, cx| {
                this.restart_window_daemon(window, cx)
            }),
        )
        .on_action(cx.listener(|this, _: &ToggleSftp, window, cx| this.toggle_sftp(window, cx)))
        .on_action(
            cx.listener(|this, _: &ShowSshForwards, window, cx| this.show_ssh_forwards(window, cx)),
        )
        .on_action(
            cx.listener(|this, _: &ToggleCodePanel, window, cx| this.toggle_code_panel(window, cx)),
        )
        .on_action(cx.listener(|this, _: &EditorSave, window, cx| {
            if !this.editor_panel_has_focus(window, cx) {
                cx.propagate();
                return;
            }
            this.editor_save_active(window, cx)
        }))
        // Same shape as `EditorSave`: the chord belongs to the editor
        // only while it has the focus, and reaches the terminal
        // otherwise — Ctrl+G is BEL to a shell.
        .on_action(cx.listener(|this, _: &EditorSaveAs, window, cx| {
            if !this.editor_panel_has_focus(window, cx) {
                cx.propagate();
                return;
            }
            this.editor_save_as_active(window, cx)
        }))
        .on_action(cx.listener(|this, _: &EditorGoToLine, window, cx| {
            if !this.editor_panel_has_focus(window, cx) {
                cx.propagate();
                return;
            }
            this.editor_go_to_line(window, cx)
        }))
        // The language-server commands (`ui::lsp`) belong to the
        // editor only while it has the focus; F2 and F12 mean
        // something to programs in the terminal too.
        .on_action(cx.listener(|this, _: &EditorGoToDefinition, window, cx| {
            if !this.editor_has_focus(window, cx) {
                cx.propagate();
                return;
            }
            this.lsp_go_to_definition(window, cx)
        }))
        .on_action(cx.listener(|this, _: &EditorQuickFix, window, cx| {
            if !this.editor_has_focus(window, cx) {
                cx.propagate();
                return;
            }
            this.lsp_code_actions(window, cx)
        }))
        .on_action(cx.listener(|this, _: &EditorRenameSymbol, window, cx| {
            if !this.editor_has_focus(window, cx) {
                cx.propagate();
                return;
            }
            this.lsp_rename_start(window, cx)
        }))
        .on_action(cx.listener(|this, _: &EditorFormatDocument, window, cx| {
            if !this.editor_has_focus(window, cx) {
                cx.propagate();
                return;
            }
            this.lsp_format_document(window, cx)
        }))
        .on_action(cx.listener(|this, _: &EditorFindReferences, window, cx| {
            if !this.editor_has_focus(window, cx) {
                cx.propagate();
                return;
            }
            this.lsp_find_references(window, cx)
        }))
        .on_action(
            cx.listener(|this, _: &EditorNewFile, window, cx| this.editor_new_file(window, cx)),
        )
        .on_action(cx.listener(|this, _: &EditorNewMarkdownFile, window, cx| {
            this.editor_new_markdown_file(window, cx)
        }))
        // The gutter's change markers. Each one lets the chord through
        // when the editor is not focused or has nothing to do.
        .on_action(cx.listener(|this, _: &EditorNextChange, window, cx| {
            if !this.editor_gutter_step(true, window, cx) {
                cx.propagate();
            }
        }))
        .on_action(cx.listener(|this, _: &EditorPrevChange, window, cx| {
            if !this.editor_gutter_step(false, window, cx) {
                cx.propagate();
            }
        }))
        .on_action(cx.listener(|this, _: &EditorRevertChange, window, cx| {
            if !this.editor_gutter_revert_at_cursor(window, cx) {
                cx.propagate();
            }
        }))
        .on_action(
            cx.listener(|this, _: &ToggleEditorGitGutter, _, cx| this.toggle_editor_git_gutter(cx)),
        )
        .on_action(cx.listener(|this, _: &EditorPeekChange, window, cx| {
            if !this.editor_gutter_peek_at_cursor(window, cx) {
                cx.propagate();
            }
        }))
        .on_action(cx.listener(|this, _: &ToggleEditorProblems, _, cx| {
            if !this.code_panel_visible() {
                cx.propagate();
                return;
            }
            this.toggle_editor_problems(cx)
        }))
    }

    fn bind_sessions_actions(
        root: gpui_kit::Stateful<gpui_kit::Div>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        root.on_action(
            cx.listener(|this, _: &Quit, window, cx| this.quit_stop_sessions(window, cx)),
        )
        .on_action(cx.listener(|this, _: &OpenSshProfiles, window, cx| {
            this.open_settings_section(SettingsSection::Ssh, window, cx)
        }))
        .on_action(cx.listener(|this, _: &RestartSshSession, window, cx| {
            this.restart_ssh_session(window, cx)
        }))
        .on_action(cx.listener(|this, _: &RenameTab, window, cx| {
            this.start_rename(this.tabs.active_index(), window, cx)
        }))
        .on_action(cx.listener(|this, _: &NewWorktreeTab, window, cx| {
            this.new_worktree_tab(this.tabs.active_index(), window, cx)
        }))
        .on_action(cx.listener(|this, _: &CloseOtherTabs, window, cx| {
            this.close_other_tabs(this.tabs.active_index(), window, cx)
        }))
        .on_action(cx.listener(|this, _: &CloseTabsToTheRight, window, cx| {
            this.close_tabs_right_of(this.tabs.active_index(), window, cx)
        }))
        .on_action(cx.listener(|this, _: &CopyWorkingDirectory, window, cx| {
            this.copy_active_cwd(window, cx)
        }))
        .on_action(cx.listener(|this, _: &MarkTabUnread, _window, cx| {
            this.mark_tab_unread(this.tabs.active_index(), cx)
        }))
        .on_action(cx.listener(|this, _: &HibernateTab, window, cx| {
            this.hibernate_tab(this.tabs.active_index(), window, cx)
        }))
        .on_action(cx.listener(|this, _: &ForkAgentSession, window, cx| {
            this.fork_active_pane_session(ForkPlacement::NewTab, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ForkAgentSessionRight, window, cx| {
            this.fork_focused_pane_session(Axis::Horizontal, false, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ForkAgentSessionLeft, window, cx| {
            this.fork_focused_pane_session(Axis::Horizontal, true, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ForkAgentSessionDown, window, cx| {
            this.fork_focused_pane_session(Axis::Vertical, false, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ForkAgentSessionUp, window, cx| {
            this.fork_focused_pane_session(Axis::Vertical, true, window, cx)
        }))
        .on_action(cx.listener(|this, _: &CopyAgentSessionId, window, cx| {
            this.copy_agent_session_id(this.tabs.active_index(), window, cx)
        }))
        .on_action(cx.listener(|this, _: &NewAgentTab, window, cx| this.new_agent_tab(window, cx)))
        .on_action(cx.listener(|this, action: &LaunchAgent, window, cx| {
            this.launch_agent(action.agent, SpawnWhere::NewTab, window, cx)
        }))
        .on_action(cx.listener(|this, _: &SaveAgentLaunchArgs, window, cx| {
            this.save_agent_launch_args(window, cx)
        }))
    }

    fn bind_application_actions(
        root: gpui_kit::Stateful<gpui_kit::Div>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        root.on_action(cx.listener(|this, _: &ShowKeyboardShortcuts, window, cx| {
            this.open_settings_section(SettingsSection::Keybindings, window, cx)
        }))
        .on_action(cx.listener(|this, _: &About, window, cx| {
            this.open_settings_section(SettingsSection::About, window, cx)
        }))
        .on_action(cx.listener(|this, _: &CheckForUpdates, window, cx| {
            this.check_for_updates_now(window, cx)
        }))
        .on_action(cx.listener(|_, _: &HideApp, _window, cx| cx.hide()))
        .on_action(cx.listener(|_, _: &HideOthers, _window, cx| cx.hide_other_apps()))
        .on_action(cx.listener(|_, _: &ShowAll, _window, cx| cx.unhide_other_apps()))
        .on_action(cx.listener(|_, _: &MinimizeWindow, window, _cx| window.minimize_window()))
        .on_action(cx.listener(|_, _: &ZoomWindow, window, _cx| window.zoom_window()))
        .on_action(cx.listener(|_, _: &OpenDocumentation, _window, cx| cx.open_url(DOCS_URL)))
        .on_action(cx.listener(|_, _: &OpenProject, _window, cx| cx.open_url(PROJECT_URL)))
        .on_action(cx.listener(|_, _: &ReportIssue, _window, cx| cx.open_url(ISSUES_URL)))
    }
}

#[cfg(test)]
mod new_window_action_tests {
    use crate::core::actions::NewWindow;
    use crate::core::config::Config;
    use crate::core::session::Session;
    use crate::ui::app::LyrelApp;
    use crate::ui::windows::WindowRegistry;
    use gpui_kit::{AppContext as _, TestAppContext, VisualTestContext};

    /// `NewWindow` has to open a window, not merely exist.
    ///
    /// Everything else about the action is a table entry — the `actions!`
    /// row, the keymap slot, the palette command — and every one of those can
    /// be there while the action reaches nothing. This drives the real
    /// dispatch path and then asks the registry, so the assertion is "a second
    /// window is open, on a workspace of its own, and the first one is still
    /// here": the same `windows::open` the switcher calls for "Open in New
    /// Window", with no workspace named.
    #[gpui_kit::test]
    fn dispatching_new_window_opens_a_second_window_beside_the_first(cx: &mut TestAppContext) {
        crate::core::config::pin_test_config_dir();
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            cx.set_global(Config::default());
            crate::ui::keymap::init(cx);
            WindowRegistry::init(cx);
        });
        let window = cx.add_window(|window, cx| {
            let app =
                cx.new(|cx| LyrelApp::with_session(None, Some(Session::default()), window, cx));
            gpui_kit::component::Root::new(app, window, cx)
        });
        let app = window
            .update(cx, |root, _, _| {
                root.view()
                    .clone()
                    .downcast::<LyrelApp>()
                    .ok()
                    .expect("window root wraps a LyrelApp")
            })
            .unwrap();
        // Registered the way an opened window registers itself; without it the
        // registry cannot tell the two windows apart afterwards.
        let handle = window.into();
        let weak = app.downgrade();
        app.update(cx, |app, cx| {
            WindowRegistry::register(cx, app.workspace, handle, weak);
        });

        let mut vcx = VisualTestContext::from_window(handle, cx);
        vcx.background_executor.run_until_parked();
        let first = app.update(&mut vcx, |app, _| app.workspace);
        assert_eq!(
            vcx.update(|_, cx| WindowRegistry::count(cx)),
            1,
            "the harness starts with exactly the one window"
        );

        vcx.dispatch_action(NewWindow);
        vcx.background_executor.run_until_parked();

        let open = vcx.update(|_, cx| WindowRegistry::open_windows(cx));
        assert_eq!(
            open.len(),
            2,
            "NewWindow has to reach windows::open; it opened {} window(s)",
            open.len()
        );
        assert!(
            open.iter().any(|(id, _)| *id == first),
            "the window the action was fired from must survive it"
        );
        // The registry is keyed by workspace, so a second window on the
        // current one is not a thing it could tell apart from the first.
        assert!(
            open.iter().any(|(id, _)| *id != first),
            "the new window belongs on a workspace of its own"
        );
    }

    /// The windowless state is the one `NewWindow` exists for.
    ///
    /// `show_tray_icon` is on by default, so closing the last window retires
    /// lyrel to the tray rather than quitting it: the process is alive, the
    /// menu bar is still lyrel's, and there is nothing on screen. A listener
    /// that lives only on `LyrelApp`'s render root reaches nothing there, and
    /// the chord that means "give me a window" is the one chord that has to
    /// answer. `App::dispatch_action` falls through to the global listeners
    /// when no window is active, which is where `keymap::init` puts this one.
    #[gpui_kit::test]
    fn new_window_answers_with_no_window_to_dispatch_it(cx: &mut TestAppContext) {
        crate::core::config::pin_test_config_dir();
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            cx.set_global(Config::default());
            crate::ui::keymap::init(cx);
            WindowRegistry::init(cx);
            assert_eq!(
                WindowRegistry::count(cx),
                0,
                "the retired-to-tray state this covers has no window in it"
            );
            cx.dispatch_action(&NewWindow);
        });
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(
                WindowRegistry::count(cx),
                1,
                "NewWindow has to reach windows::open with no window to bubble through"
            );
        });
    }
}

#[cfg(test)]
mod unfocused_shortcut_tests {
    use crate::core::config::Config;
    use crate::core::session::Session;
    use crate::ui::app::LyrelApp;
    use crate::ui::windows::WindowRegistry;
    use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext};

    fn open(cx: &mut TestAppContext) -> (Entity<LyrelApp>, VisualTestContext) {
        crate::core::config::pin_test_config_dir();
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            cx.set_global(Config::default());
            crate::ui::keymap::init(cx);
            WindowRegistry::init(cx);
        });
        let window = cx.add_window(|window, cx| {
            let app =
                cx.new(|cx| LyrelApp::with_session(None, Some(Session::default()), window, cx));
            gpui_kit::component::Root::new(app, window, cx)
        });
        let app = window
            .update(cx, |root, _, _| {
                root.view().clone().downcast::<LyrelApp>().ok().unwrap()
            })
            .unwrap();
        let vcx = VisualTestContext::from_window(window.into(), cx);
        vcx.run_until_parked();
        (app, vcx)
    }

    /// Focus on a handle no element tracks — a panel that closed under it, a
    /// field that went away — leaves gpui dispatching keys on the window root
    /// alone, and `LyrelApp`'s listeners sit one level below that. The
    /// shortcuts have to answer anyway.
    fn answers_with_focus_off_the_tree(
        action: &str,
        opened: fn(&LyrelApp) -> bool,
        blur: bool,
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx) = open(cx);
        let key = vcx
            .update(|_, cx| crate::ui::keymap::effective_key(action, cx))
            .unwrap();
        vcx.update(|window, cx| {
            if blur {
                window.blur(cx);
            } else {
                let orphan = cx.focus_handle();
                window.focus(&orphan, cx);
                std::mem::forget(orphan);
            }
            window.refresh();
        });
        vcx.run_until_parked();
        assert!(!app.read_with(&vcx, |app, _| opened(app)));
        vcx.simulate_keystrokes(&key);
        vcx.run_until_parked();
        assert!(
            app.read_with(&vcx, |app, _| opened(app)),
            "{action} ({key}) did nothing with focus off LyrelApp's tree (blur: {blur})"
        );
    }

    #[gpui_kit::test]
    fn palette_with_app_focus(cx: &mut TestAppContext) {
        let (app, mut vcx) = open(cx);
        let key = vcx
            .update(|_, cx| crate::ui::keymap::effective_key("TogglePalette", cx))
            .unwrap();
        app.update_in(&mut vcx, |app, window, cx| app.focus_active(window, cx));
        vcx.run_until_parked();
        vcx.simulate_keystrokes(&key);
        vcx.run_until_parked();
        assert!(app.read_with(&vcx, |app, _| app.search.is_open()));
    }

    #[gpui_kit::test]
    fn palette_after_blur(cx: &mut TestAppContext) {
        answers_with_focus_off_the_tree("TogglePalette", |a| a.search.is_open(), true, cx);
    }

    #[gpui_kit::test]
    fn palette_with_orphan_focus(cx: &mut TestAppContext) {
        answers_with_focus_off_the_tree("TogglePalette", |a| a.search.is_open(), false, cx);
    }

    #[gpui_kit::test]
    fn switcher_with_orphan_focus(cx: &mut TestAppContext) {
        answers_with_focus_off_the_tree("ToggleSwitcher", |a| a.switcher.is_some(), false, cx);
    }

    #[gpui_kit::test]
    fn settings_with_orphan_focus(cx: &mut TestAppContext) {
        answers_with_focus_off_the_tree("OpenSettings", |a| a.settings.is_some(), false, cx);
    }

    /// With settings up in a window of its own, the workspace window's focus
    /// comes back to its own panes — the page's handle is drawn elsewhere.
    #[gpui_kit::test]
    fn settings_in_its_own_window_leaves_the_workspace_its_focus(cx: &mut TestAppContext) {
        let (app, mut vcx) = open(cx);
        let elsewhere = cx.add_window(|_, _| gpui_kit::EmptyView);
        let _settings = crate::ui::settings::test_window::open(&app, &mut vcx);
        app.update_in(&mut vcx, |app, window, cx| {
            // Tests build the page in place; point it at the other window, as
            // a real ⌘, does.
            app.settings_window = Some((elsewhere.into(), window.window_handle()));
            let orphan = cx.focus_handle();
            window.focus(&orphan, cx);
            std::mem::forget(orphan);
            window.refresh();
        });
        vcx.run_until_parked();
        app.update_in(&mut vcx, |app, window, cx| {
            let expected = match app.tabs.get(app.tabs.active_index()) {
                Some(tab) => tab.focus_target(cx).unwrap().focus_handle(cx),
                None => app.home_focus.clone(),
            };
            assert!(
                !app.settings
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .active_settings()
                    .unwrap()
                    .focus_handle
                    .is_focused(window),
                "the workspace window must not park focus on a page it does not draw"
            );
            assert!(expected.is_focused(window));
        });
    }

    #[gpui_kit::test]
    fn separate_settings_escape_returns_focus_to_the_filled_document(cx: &mut TestAppContext) {
        use crate::core::actions::{EditorNewFile, ToggleDocumentFill};
        use gpui_kit::test::TestWindowExt as _;
        let (app, mut vcx, _streams) = crate::ui::app::test_window::harness_with_tabs(cx, 1);
        vcx.dispatch_action(EditorNewFile);
        vcx.dispatch_action(ToggleDocumentFill);
        vcx.run_until_parked();
        app.update_in(&mut vcx, |app, window, cx| {
            assert!(app.editor_has_focus(window, cx));
            // Use the production separate-window constructor: normal test
            // Actions intentionally keep Settings inline in the harness.
            app.open_settings_window_then(window, cx, None);
        });
        vcx.run_until_parked();
        let settings = app.read_with(&vcx, |app, _| app.settings_window.unwrap().0);
        let mut settings_vcx = VisualTestContext::from_window(settings, cx);
        settings_vcx.update(|window, cx| window.render_frame(cx));
        settings_vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        vcx.update(|window, cx| window.render_frame(cx));
        assert!(app.update_in(&mut vcx, |app, window, cx| {
            app.settings.is_none() && app.editor_has_focus(window, cx)
        }));
        vcx.update(|window, cx| window.input("returns to document", cx));
        assert!(app.update_in(&mut vcx, |app, window, cx| app.editor_has_focus(window, cx)));
    }
}
