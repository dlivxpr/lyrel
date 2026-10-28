//! Window bootstrap: compose existing capability owners and bind window lifetimes.
use super::*;

impl LyrelApp {
    pub fn for_workspace(
        id: Option<WorkspaceId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::for_workspace_at(id, None, window, cx)
    }

    pub fn for_workspace_at(
        id: Option<WorkspaceId>,
        mut initial_cwd: Option<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let restore = cx.global::<Config>().restore_session;
        let known = id.is_some_and(|id| WorkspaceStore::all(cx).get(id).is_some());
        let workspace = WorkspaceStore::claim(cx, id);
        let is_remote = WorkspaceStore::all(cx)
            .get(workspace)
            .is_some_and(|w| w.is_remote());
        // Tabs that exist on the machine are shown whatever the restore
        // setting says: that setting decides whether a window comes back at
        // launch, not whether an open one shows what is really in it. The
        // `else` arm below saves this window's session, and saving an empty
        // one over a live tree would erase it.
        let on_machine = id.is_some_and(|id| crate::ui::machine_mirror::machine_holds_tabs(cx, id));
        let hydrate = on_machine || (known && (restore || is_remote));
        let session = hydrate.then(Session::default);
        // A window that is about to pull its layout cannot also open the folder
        // the launch asked for as its first terminal — the pull would find a
        // window that already has a tab, decline to adopt into it, and push
        // that one tab back as the whole workspace. So the folder travels with
        // the hydration and becomes a tab once the layout is up.
        let open_after_hydrate = hydrate.then(|| initial_cwd.take()).flatten();
        let app = Self::with_session_at(Some(workspace), session, initial_cwd, window, cx);
        if hydrate {
            match open_after_hydrate {
                Some(cwd) => {
                    crate::ui::tree_sync::hydrate_window_then_open(cx, workspace, cwd);
                }
                None => crate::ui::tree_sync::hydrate_window_from_tree(cx, workspace),
            }
        } else {
            if !is_remote {
                crate::ui::tree_sync::mark_window_informed(cx, workspace);
            }
            app.save_session(cx);
        }
        Self::prompt_daemon_version_mismatch(window, cx);
        crate::ui::remote_connect::register(cx);
        crate::ui::remote_connect::sweep_wsl(cx);
        Self::prompt_remote_daemon_mismatch(window, cx);
        app.reopen_remote_at_startup(cx);
        app
    }

    /// Put a version-mismatched local server to the user, with no third way out.
    ///
    /// Both handshakes compare their version for equality and hang up on
    /// anything else — the pane protocol in `daemon::spawn::ensure_running`, the
    /// control dialect in `host::server`'s hello. So a server whose number
    /// disagrees cannot be talked round, and carrying on beside it is not a
    /// degraded mode but a broken one: panes still spawn while every
    /// machine-tree call is refused, which is how a window comes to open with no
    /// tabs and save none of the ones you make. This used to be offered as "Keep
    /// Shells", and taking it was indistinguishable from the bug.
    ///
    /// Restart or quit, then. Quitting is the half that destroys nothing: the
    /// server and every shell under it keep running, which is what makes it a
    /// real answer for someone who would rather go install the matching build
    /// than lose a session mid-flight.
    fn prompt_daemon_version_mismatch(window: &mut Window, cx: &mut Context<Self>) {
        let Some(mismatch) = crate::daemon::spawn::take_mismatched_daemon() else {
            return;
        };
        let ours = crate::daemon::protocol::PROTOCOL_VERSION;
        let detail = match &mismatch {
            DaemonMismatch::Protocol(Some(v)) => t_fmt(
                L10nKey::AppRestartServerMismatchDetail,
                &[
                    ("build", &v.build.to_string()),
                    ("protocol", &v.protocol.to_string()),
                    ("ours", &ours.to_string()),
                ],
            ),
            DaemonMismatch::Protocol(None) => t(L10nKey::AppRestartServerOldDetail).to_string(),
            // The handshake only reports disagreement, not direction, and a
            // daemon left behind by a newer build is as much a mismatch as one
            // left behind by an older. Calling that one "older" would be the
            // same wrong guess the remote path stopped making.
            DaemonMismatch::Dialect(refusal) => t_fmt(
                if refusal.peer < refusal.ours {
                    L10nKey::AppRestartServerDialectDetail
                } else {
                    L10nKey::AppRestartServerDialectNewerDetail
                },
                &[
                    ("build", &refusal.peer_build),
                    ("dialect", &refusal.peer.to_string()),
                    ("ours", &refusal.ours.to_string()),
                ],
            ),
        };
        // The one prompt here that does not use `confirm_answers`, because
        // neither answer is "leave it alone" — the app cannot carry on beside a
        // server it cannot speak to. With nothing safe to give Escape, this
        // keeps Quit at index 0 where Return lands: it arrives unasked at
        // launch, the moment a stray Return is most likely, and quitting loses
        // no sessions while restarting the server ends every one of them.
        let answer = window.prompt(
            PromptLevel::Warning,
            t(L10nKey::AppRestartServerTitle),
            Some(&detail),
            &[t(L10nKey::CmdQuitLyrel), t(L10nKey::RestartServer)],
            cx,
        );
        cx.spawn(async move |this, cx| match answer.await {
            Ok(1) => {
                let _ = this.update_in(cx, |this, _window, cx| this.restart_daemon_confirmed(cx));
            }
            Ok(_) => {
                let _ = cx.update(|cx| cx.quit());
            }
            // Dismissed without an answer: the window went away before the
            // question was settled. Arm it again so the next window asks, rather
            // than letting the state this prompt exists to prevent slip through
            // the gap.
            Err(_) => crate::daemon::spawn::note_daemon_mismatch(mismatch),
        })
        .detach();
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn with_session(
        workspace: Option<WorkspaceId>,
        session: Option<Session>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_session_at(workspace, session, None, window, cx)
    }

    fn with_session_at(
        workspace: Option<WorkspaceId>,
        session: Option<Session>,
        initial_cwd: Option<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let workspace = workspace.unwrap_or_default();
        let pane_ws = crate::ui::remote_workspace::pane_workspace_for(cx, workspace);
        let (
            font_size,
            line_height,
            font_family,
            font_family_bold,
            font_family_italic,
            font_fallbacks,
            font_features,
            terminal_cursor_style,
            terminal_scrollback_limit,
        ) = {
            let cfg = cx.global::<Config>();
            (
                cfg.font_size,
                cfg.line_height,
                cfg.font_family.clone(),
                cfg.font_family_bold.clone(),
                cfg.font_family_italic.clone(),
                cfg.font_fallbacks.clone(),
                cfg.font_features
                    .as_ref()
                    .map(crate::core::config::gpui_font_features),
                cfg.cursor_style,
                cfg.scrollback_limit,
            )
        };
        let sftp_panel = crate::ui::sftp::SftpPanelState::new(window, cx);
        let file_tree = crate::ui::file_tree::FileTreeState::new(window, cx);
        let editor = crate::ui::code_editor::EditorPanelState::new(window, cx);
        let loopback_panel = Forwarding::new(window, cx);
        let sidebar_width = cx.global::<Config>().sidebar_width;
        let right_panel_width = cx.global::<Config>().right_panel_width;
        let document_ratio = cx.global::<Config>().document_ratio;
        let right_panel_visible = cx.global::<Config>().right_panel_visible;
        let right_panel_tab =
            crate::ui::right_panel::shown_tab(cx.global::<Config>().right_panel_tab);
        let scm_graph_expanded = cx.global::<Config>().scm_graph_expanded;
        let sidebar_collapsed = cx.global::<Config>().sidebar_collapsed;
        let config_watch = cx.observe_global_in::<Config>(window, |this, window, cx| {
            this.reload_from_config(window, cx)
        });
        cx.default_global::<crate::terminal::git_status::GitStatusCache>();
        let git_status_watch =
            cx.observe_global::<crate::terminal::git_status::GitStatusCache>(|this, cx| {
                this.maybe_refresh_diff_overlay(cx);
                this.right_panel_refresh_changes(cx);
                cx.notify();
            });
        cx.default_global::<crate::terminal::pane_liveness::PaneLivenessCache>();
        let pane_liveness_watch = cx
            .observe_global::<crate::terminal::pane_liveness::PaneLivenessCache>(|_this, cx| {
                cx.notify();
            });
        let this = cx.weak_entity();
        let keystroke_watch = cx.intercept_keystrokes(move |_ev, _window, cx| {
            let _ = this.update(cx, |this, cx| this.dismiss_mod_hint(cx));
        });
        let activation_watch = cx.observe_window_activation(window, |this, window, cx| {
            // DWM can drop the window's native material state while focus is
            // elsewhere (composition restart, sleep/resume). The resolved
            // value did not change, so the change-gated paths would skip —
            // replay it unconditionally on both activation and deactivation.
            crate::ui::theme::replay_window_backdrop(window, cx);
            this.dismiss_mod_hint(cx);
            this.set_link_modifier(false, cx);
            // A modifier released over another window never reports here, so a
            // Ctrl+Tab panel would hang waiting for a commit that cannot come.
            this.switcher_release_hold(cx);
            if !window.is_window_active()
                && (crate::ui::pane_drag::lifted(&this.pane_drag).is_some()
                    || this
                        .tabs
                        .get(this.tabs.active_index())
                        .is_some_and(|tab| tab.pip_interaction.has_gesture()))
            {
                this.cancel_pane_drag(window, cx);
            }
            if window.is_window_active() {
                WorkspaceStore::focus(cx, this.workspace);
                this.refresh_git_status_all(cx);
            }
        });
        // A DPI change — including a move onto a display with a different one,
        // which arrives as WM_DPICHANGED — recreates the window's frame and
        // can drop the native material. This observer also fires for plain
        // moves, resizes and activation changes; the scale gate inside keeps
        // those from re-running the DWM writes.
        let bounds_watch = cx.observe_window_bounds(window, |_, window, cx| {
            crate::ui::theme::replay_window_backdrop_on_scale_change(window, cx);
        });
        let this = cx.weak_entity();
        let appearance_watch = window.observe_window_appearance(move |window, cx| {
            crate::ui::theme::note_system_appearance(window, cx);
            if !cx.global::<Config>().theme_follow_system {
                return;
            }
            apply_theme(Some(window), cx);
            let _ = this.update(cx, |this, cx| {
                this.refresh_settings_theme(cx);
                cx.notify();
            });
        });
        apply_theme(Some(window), cx);
        set_menus(cx);
        let mut startup_error: Option<gpui_kit::SharedString> = None;
        let (tabs, active) = match session {
            None => {
                let first = Self::new_terminal(
                    pane_ws.clone(),
                    Some(workspace),
                    font_size,
                    initial_cwd,
                    None,
                    None,
                    window,
                    cx,
                );
                (vec![Tab::new(Pane::leaf(first))], 0)
            }
            some => {
                let (tabs, active, dropped) =
                    SessionRestore::tabs(pane_ws.clone(), workspace, some, font_size, window, cx);
                if dropped > 0 {
                    startup_error = Some(gpui_kit::SharedString::from(t_plural(
                        L10nKey::AppTabsNotRestored,
                        dropped,
                        &[],
                    )));
                }
                (tabs, active)
            }
        };
        let parked_deck = cx.new(crate::ui::parked_deck::ParkedDeck::new);
        let parked_deck_sub = cx.subscribe_in(
            &parked_deck,
            window,
            |this, _, event, window, cx| match event {
                crate::ui::parked_deck::ParkedDeckEvent::Restore(token) => {
                    let slot = this.tabs.get(this.tabs.active_index()).and_then(|tab| {
                        tab.pane
                            .leaves()
                            .into_iter()
                            .find(|slot| slot.entity_id().as_u64() == *token)
                    });
                    if let Some(slot) = slot {
                        this.restore_parked(slot.entity_id(), window, cx);
                    }
                }
                crate::ui::parked_deck::ParkedDeckEvent::Dismiss => {
                    this.refocus_parked_pane(window, cx);
                    if window.focused(cx).is_none() {
                        this.focus_active(window, cx);
                    }
                }
            },
        );
        let file_search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t(crate::ui::i18n::L10nKey::SearchFiles))
        });
        let file_search_sub = cx.subscribe_in(&file_search, window, |_this, _i, ev, _w, cx| {
            if matches!(ev, InputEvent::Change) {
                cx.notify();
            }
        });
        // The Files tab's one field drives both of its searches: names here,
        // and what the files say through the content search.
        let panel_search =
            crate::ui::panel_search::PanelSearchState::new(file_search.clone(), window, cx);
        let mut app = Self {
            tabs: TabSet::new(tabs, active),
            pending_notification_source: None,
            font_size,
            line_height,
            font_family,
            font_family_bold,
            font_family_italic,
            font_fallbacks,
            font_features,
            terminal_cursor_style,
            terminal_scrollback_limit,
            _config_watch: config_watch,
            _keystroke_watch: keystroke_watch,
            _activation_watch: activation_watch,
            _bounds_watch: bounds_watch,
            _git_status_watch: git_status_watch,
            _pane_liveness_watch: pane_liveness_watch,
            _appearance_watch: appearance_watch,
            search: SearchState::default(),
            theme_preview_restore: None,
            session: SessionLifecycle::default(),
            renaming: None,
            worktree_prompt: None,
            maximized: None,
            mod_hint_badges: false,
            mod_hint_gen: 0,
            home_focus: cx.focus_handle(),
            home_cursor_on: true,
            working_dot_on: true,
            shells: ShellInventory::default(),
            shells_host: HostId::LOCAL,
            loopback_panel,
            sftp_panel,
            right_panel: Default::default(),
            scm: crate::ui::scm::ScmPanelState::new(scm_graph_expanded),
            github: Default::default(),
            diff_probes_inflight: Default::default(),
            diff_probes_restale: Default::default(),
            file_tree,
            panel_search,
            editor,
            sidebar_width: Rc::new(Cell::new(sidebar_width)),
            sidebar_dragging: Rc::new(Cell::new(false)),
            last_settings_location: (SettingsSection::General, gpui_kit::point(px(0.), px(0.))),
            right_panel_width: Rc::new(Cell::new(right_panel_width)),
            right_panel_dragging: Rc::new(Cell::new(false)),
            document_ratio: Rc::new(Cell::new(document_ratio)),
            document_dragging: Rc::new(Cell::new(false)),
            right_panel_visible,
            right_panel_tab,
            sidebar_collapsed,
            sidebar_scroll: gpui_kit::ScrollHandle::new(),
            sidebar_reveal: Rc::new(Cell::new(None)),
            reorder: Rc::new(RefCell::new(None)),
            pane_drag: Rc::new(RefCell::new(None)),
            parked_deck,
            _parked_deck_sub: parked_deck_sub,
            tab_merge: Cell::new(None),
            sidebar_slots: Rc::new(RefCell::new(Vec::new())),
            sidebar_group_slots: Rc::new(RefCell::new(Vec::new())),
            sidebar_divider: Rc::new(Cell::new(None)),
            sidebar_groups: Default::default(),
            pane_area: Rc::new(Cell::new(None)),
            file_search,
            _file_search_sub: file_search_sub,
            settings: None,
            settings_events: None,
            settings_window: None,
            ssh_prompt: crate::ui::ssh_prompt::SshPromptState::new(cx),
            close_prompt_open: false,
            window_bounds: window_bounds_to_remember(window),
            workspace,
            workspace_rename: None,
            group_rename: None,
            window_title: std::cell::RefCell::new(String::new()),
            status_clock: cx.new(crate::ui::status_bar::Clock::new),
            status_metrics: None,
            notification_bell: cx.new(crate::ui::notification_history::NotificationBell::new),
            status_document_focus: None,
            connect: Default::default(),
            switcher: None,
            host_snapshots: std::collections::HashMap::new(),
            remote_host_errors: std::collections::HashMap::new(),
            parked_dismissed: std::collections::HashSet::new(),
            startup_error,
        };
        if !cfg!(test) && crate::ui::windows::WindowRegistry::count(cx) == 0 {
            crate::ui::tray::init(cx);
        }
        app.refresh_shells(cx);
        // Backstop for every other way a window goes away (a workspace
        // closed from elsewhere): its language-server documents go with it.
        let app_id = cx.entity_id();
        cx.on_release(move |_, cx| crate::ui::lsp::LspStore::sync_window(app_id, Vec::new(), cx))
            .detach();
        cx.on_release(move |_, cx| {
            if cx.has_global::<crate::terminal::git_data::ScmData>() {
                cx.global_mut::<crate::terminal::git_data::ScmData>()
                    .release_window(app_id.as_u64());
            }
        })
        .detach();
        cx.on_app_quit(|app, cx| {
            app.save_session(cx);
            crate::core::window_state::WindowState::from_bounds(app.window_bounds).save();
            async move {}
        })
        .detach();

        cx.observe_window_bounds(window, |this, window, _cx| {
            this.window_bounds = window_bounds_to_remember(window);
        })
        .detach();

        // With focus on nothing, or on a handle whose element is gone (a panel
        // closed under it, a context menu that dismissed), gpui dispatches keys
        // on the window root alone, one level above every listener on
        // `lyrel-root`, and ⌘P, ⌘T, ⌘W and the rest go dead.
        cx.on_focus_lost(window, |this, window, cx| this.focus_active(window, cx))
            .detach();

        // The home page's cursor, on the terminal's own schedule. It ticks
        // whether or not the page is up — a timer that wakes twice a second to
        // compare a `Vec`'s length against zero costs nothing — but only asks
        // for a frame when the page is the thing on screen.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(HOME_CURSOR_BLINK_MS))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        if this.tabs.is_empty() {
                            this.home_cursor_on = !this.home_cursor_on;
                            cx.notify();
                        }
                        let working = this.tabs.iter().any(|tab| {
                            tab.agent_status(cx)
                                == Some(crate::core::cli_agent::AgentStatus::Working)
                        });
                        // Keep blinking while an agent works; once none does,
                        // settle on "on" with one last frame and go quiet.
                        if working || !this.working_dot_on {
                            this.working_dot_on = !this.working_dot_on || !working;
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        let weak_app = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            let Some(app) = weak_app.upgrade() else {
                return true;
            };
            // Unsaved editor buffers hold the window open until the question
            // about them is answered; the answer closes it.
            let asked = app.update(cx, |app, cx| {
                let unsaved = app.editor_unsaved();
                app.editor_guard_unsaved(
                    unsaved,
                    crate::ui::code_editor::saving::AfterUnsaved::CloseWindow,
                    window,
                    cx,
                )
            });
            if asked {
                return false;
            }
            app.update(cx, |app, cx| {
                app.prepare_window_close(cx);
                app.lsp_window_closed(cx);
            });
            true
        });

        app.focus_active(window, cx);
        app
    }
}
