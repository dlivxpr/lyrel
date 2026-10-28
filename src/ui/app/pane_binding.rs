//! GPUI binding for pane creation and terminal events. PendingPane owns creation,
//! cancellation, and installation admission; this adapter owns cross-capability effects.
use super::*;

impl LyrelApp {
    pub(super) fn restore_agent_in_pane(
        agent: Option<crate::core::cli_agent::CLIAgent>,
        session_id: Option<String>,
        launch_argv: Option<Vec<String>>,
        scope: Option<lyrel_core::host::session_operations::SessionScope>,
        terminal: Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<LyrelApp>,
    ) {
        use lyrel_core::host::session_operations::{SessionOperation, SessionOperationKind};
        if !cx.global::<Config>().restore_agent_sessions {
            return;
        }
        let (Some(agent), Some(source_id)) = (agent, session_id) else {
            return;
        };
        let view = terminal.read(cx);
        let (Some(host), Some(cwd)) = (view.host(cx), view.host_cwd()) else {
            window.notify_app(t(L10nKey::AppPaneNoKnownDirectory), cx);
            return;
        };
        let mut scope = scope.unwrap_or_default();
        scope.shell_program = Self::pane_shell_program(view, cx);
        let request = SessionOperation {
            expected_host: view.host_id().0,
            agent,
            kind: SessionOperationKind::Resume,
            source_id,
            cwd,
            launch_argv: launch_argv.unwrap_or_else(|| vec![agent.binary().into()]),
            scope,
        };
        crate::ui::host_ops::HostOps::run_in(
            host,
            window,
            cx,
            move |host| host.session_operation(&request),
            move |app, result, window, cx| match result {
                Ok(prepared) => {
                    let still_present = app.tabs.iter().any(|tab| {
                        tab.pane
                            .terminals()
                            .iter()
                            .any(|leaf| leaf.entity_id() == terminal.entity_id())
                    });
                    if still_present && let Some(command) = prepared.command {
                        crate::ui::agent_launch::run_when_ready(
                            &PaneSlot::Ready(terminal),
                            command,
                            cx,
                        );
                    }
                }
                Err(error) => app.notify_session_operation_error(error, window, cx),
            },
        );
    }

    /// The shell `view` is running, for deciding what a line typed into it may
    /// use. A pane spawned without an explicit shell got the configured one or,
    /// failing that, the login shell — but only a local pane got this machine's;
    /// a workspace pane's default lives on its host, so it stays unknown.
    fn pane_shell_program(view: &TerminalView, cx: &App) -> Option<String> {
        if let Some(spec) = view.shell_spec() {
            return Some(spec.program);
        }
        if view.workspace().is_some()
            || view.ssh_spec().is_some()
            || view.remote_context().is_some()
        {
            return None;
        }
        Some(match &cx.global::<Config>().shell {
            Some(shell) if !shell.program.trim().is_empty() => shell.program.clone(),
            _ => crate::core::shells::login_shell(),
        })
    }

    /// Bind a retained connecting slot. Creation errors are asynchronous and
    /// remain on PendingPane; callers place this identity before it can land.
    pub(crate) fn new_terminal(
        workspace: Option<crate::terminal::PaneWorkspace>,
        owner: Option<WorkspaceId>,
        font_size: f32,
        working_directory: Option<std::path::PathBuf>,
        restore_pane: Option<u64>,
        shell: Option<ShellSpec>,
        window: &mut Window,
        cx: &mut Context<LyrelApp>,
    ) -> PaneSlot {
        Self::pending_terminal(
            workspace,
            owner,
            font_size,
            working_directory,
            restore_pane,
            shell,
            false,
            None,
            window,
            cx,
        )
    }

    /// Bind an existing attachment without permitting a fresh replacement shell.
    /// A missing or unreachable pane remains an attach-only pending failure.
    pub(crate) fn attach_existing_terminal(
        workspace: Option<crate::terminal::PaneWorkspace>,
        pane: u64,
        owner: WorkspaceId,
        font_size: f32,
        spec: Option<Box<crate::daemon::protocol::NativeSshSpec>>,
        window: &mut Window,
        cx: &mut Context<LyrelApp>,
    ) -> PaneSlot {
        Self::pending_terminal(
            workspace,
            Some(owner),
            font_size,
            None,
            Some(pane),
            None,
            true,
            spec,
            window,
            cx,
        )
    }

    fn pending_terminal(
        workspace: Option<crate::terminal::PaneWorkspace>,
        owner: Option<WorkspaceId>,
        font_size: f32,
        working_directory: Option<std::path::PathBuf>,
        restore_pane: Option<u64>,
        shell: Option<ShellSpec>,
        attach_only: bool,
        attach_spec: Option<Box<crate::daemon::protocol::NativeSshSpec>>,
        window: &mut Window,
        cx: &mut Context<LyrelApp>,
    ) -> PaneSlot {
        let pending = cx.new(|cx| {
            let pending = crate::ui::pending_pane::PendingPane::shell(
                workspace,
                owner,
                font_size,
                working_directory,
                restore_pane,
                shell,
                cx,
            );
            if attach_only {
                pending.with_attach_only(attach_spec)
            } else {
                pending
            }
        });
        Self::watch_pending(&pending, window, cx);
        PaneSlot::Connecting(pending)
    }

    fn watch_pending(
        pending: &Entity<crate::ui::pending_pane::PendingPane>,
        window: &mut Window,
        cx: &mut Context<LyrelApp>,
    ) {
        cx.subscribe_in(
            pending,
            window,
            |app, pending, _: &crate::ui::pending_pane::SpawnFinished, window, cx| {
                app.land_pane(&pending, window, cx);
            },
        )
        .detach();
        cx.subscribe_in(
            pending,
            window,
            |_app, pending, _: &crate::ui::pending_pane::RetryRequested, window, cx| {
                Self::start_pane_spawn(&pending, window, cx);
            },
        )
        .detach();
        let handle = pending.read(cx).focus_handle(cx);
        Self::watch_pane_focus(&handle, pending.entity_id(), window, cx);
        Self::start_pane_spawn(pending, window, cx);
    }

    fn start_pane_spawn(
        pending: &Entity<crate::ui::pending_pane::PendingPane>,
        window: &mut Window,
        cx: &mut Context<LyrelApp>,
    ) {
        let app = cx.weak_entity();
        let lease = std::rc::Rc::new(move |slot, owner: Option<WorkspaceId>, cx: &App| {
            app.upgrade().is_some_and(|app| {
                let app = app.read(cx);
                owner.is_none_or(|owner| owner == app.workspace) && app.tabs.contains_slot(slot)
            })
        });
        pending.update(cx, |pending, cx| pending.start(lease, window, cx));
    }

    pub(super) fn watch_terminal(
        view: &Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<LyrelApp>,
    ) {
        cx.subscribe_in(&view, window, |app, view, _: &ChildExited, window, cx| {
            app.on_child_exited(view.clone(), window, cx);
        })
        .detach();
        cx.subscribe_in(
            &view,
            window,
            |app, _view, _: &crate::terminal::view::AgentSessionChanged, _window, cx| {
                app.save_session(cx);
            },
        )
        .detach();
        cx.subscribe_in(
            &view,
            window,
            |app, _view, ev: &crate::terminal::view::AgentDetected, _window, cx| {
                app.note_agent_detected(ev.0, cx);
            },
        )
        .detach();
        cx.subscribe_in(
            &view,
            window,
            |app, view, _: &crate::terminal::view::AuthPromptReady, window, cx| {
                app.on_auth_prompt_ready(view.clone(), window, cx);
            },
        )
        .detach();
        Self::watch_open_file_requests(&view, window, cx);
        let handle = view.read(cx).focus_handle.clone();
        Self::watch_pane_focus(&handle, view.entity_id(), window, cx);
    }

    pub(super) fn kill_pane_off_thread(
        route: crate::terminal::PaneRoute,
        pane_id: u64,
        cx: &mut App,
    ) {
        cx.background_executor()
            .spawn(async move { crate::terminal::RemoteTerminal::kill_pane_on(&route, pane_id) })
            .detach();
    }

    /// Routes the file links clicked in a pane to whatever the app opens files
    /// with. Every pane needs this, however it was spawned — a link in an SSH pane
    /// is the same click as a link in a local one.
    fn watch_open_file_requests(
        view: &Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<LyrelApp>,
    ) {
        cx.subscribe_in(
            view,
            window,
            |app, view, ev: &crate::terminal::view::OpenFileRequested, window, cx| {
                // The popup can outlive focus changes. Resolve its retained pane,
                // never the workspace that happened to receive the later action.
                if view.read(cx).host_id() == ev.host_id
                    && app.focus_pane_command_target(view.entity_id(), window, cx)
                {
                    app.open_linked_file(&ev.path, ev.line, ev.column, ev.is_dir, window, cx);
                }
            },
        )
        .detach();
    }

    /// Repaint the chrome that marks the focused pane, and record the leaf as the
    /// one its tab returns to (#843).
    ///
    /// Every leaf is watched, connecting slots included: a pane can be focused
    /// while it is still coming up, and if the tab is left in that moment the
    /// answer has to already be written down.
    pub(super) fn watch_pane_focus(
        handle: &gpui_kit::FocusHandle,
        leaf: gpui_kit::EntityId,
        window: &mut Window,
        cx: &mut Context<LyrelApp>,
    ) {
        let app = cx.weak_entity();
        window
            .on_focus_in(handle, cx, move |_window, cx| {
                if let Some(app) = app.upgrade() {
                    app.update(cx, |app, cx| {
                        app.remember_focused_leaf(leaf);
                        cx.notify();
                    });
                }
            })
            .detach();
    }

    /// A pane dialling `spec`. `remote_start_dir` is a directory on the far host
    /// for its shell to start in — never a local path; see
    /// [`inherited_start_dir`] for which openings carry one.
    pub(crate) fn new_terminal_native(
        font_size: f32,
        owner: WorkspaceId,
        remote_start_dir: Option<std::path::PathBuf>,
        spec: Box<crate::daemon::protocol::NativeSshSpec>,
        window: &mut Window,
        cx: &mut Context<LyrelApp>,
    ) -> PaneSlot {
        let pending = cx.new(|cx| {
            crate::ui::pending_pane::PendingPane::native_ssh(
                owner,
                font_size,
                remote_start_dir,
                spec,
                cx,
            )
        });
        Self::watch_pending(&pending, window, cx);
        PaneSlot::Connecting(pending)
    }
}
