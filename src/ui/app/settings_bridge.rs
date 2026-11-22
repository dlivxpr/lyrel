//! 工作区与独立设置窗口的接线：工作区保留 pane、文件和焦点操作，设置只发送明确意图。
use super::*;
use crate::ui::settings_window::{SessionSources, SettingsContext, SettingsEvent, SettingsWindow};

type SettingsFollowUp =
    Box<dyn FnOnce(&mut SettingsWindow, &mut Window, &mut Context<SettingsWindow>)>;

impl LyrelApp {
    fn settings_context(
        &self,
        owner: gpui_kit::AnyWindowHandle,
        cx: &Context<Self>,
    ) -> SettingsContext {
        let shells = cx.weak_entity();
        let profiles = shells.clone();
        let sources = shells.clone();
        let active_host = shells.clone();
        let live_sessions = shells.clone();
        let session_scan = shells.clone();
        // Production hands the real keychain to the settings pages; GUI tests
        // take the in-memory store so no settings test can touch the OS one.
        #[cfg(not(test))]
        let credentials = std::sync::Arc::new(crate::core::keychain::OsCredentialStore);
        #[cfg(test)]
        let credentials =
            std::sync::Arc::new(crate::core::keychain::InMemoryCredentialStore::new());
        let usage_vault = credentials.clone();
        SettingsContext {
            owner,
            credentials,
            usage_vault,
            shells: Rc::new(move |cx| {
                shells
                    .upgrade()
                    .map(|app| {
                        app.read(cx)
                            .shells
                            .shells
                            .iter()
                            .filter(|shell| !shell.user_authored)
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default()
            }),
            live_profiles: Rc::new(move |cx| {
                profiles
                    .upgrade()
                    .map(|app| {
                        let app = app.read(cx);
                        app.tabs
                            .iter()
                            .flat_map(|tab| tab.pane.terminals())
                            .filter_map(|leaf| {
                                let pane = leaf.read(cx);
                                if !matches!(
                                    pane.ssh_phase(),
                                    Some(crate::daemon::protocol::SshPhase::Connected)
                                ) || pane.terminal.exited
                                {
                                    return None;
                                }
                                pane.ssh_spec()?
                                    .profile_id
                                    .as_deref()
                                    .and_then(|id| uuid::Uuid::parse_str(id).ok())
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            }),
            session_sources: Rc::new(move |host, cx| {
                sources
                    .upgrade()
                    .map(|app| app.read(cx).settings_session_sources(host, cx))
                    .unwrap_or_default()
            }),
            active_host: Rc::new(move |cx| {
                active_host
                    .upgrade()
                    .map(|app| app.read(cx).spawn_host(cx))
                    .unwrap_or(HostId::LOCAL)
            }),
            live_sessions: Rc::new(move |cx| {
                live_sessions
                    .upgrade()
                    .map(|app| app.read(cx).settings_live_sessions(cx))
                    .unwrap_or_default()
            }),
            session_scan: Rc::new(move |host, cx| {
                session_scan
                    .upgrade()
                    .map(|app| app.read(cx).settings_session_scan(host, cx))
            }),
        }
    }

    fn settings_session_sources(&self, host_id: HostId, cx: &App) -> SessionSources {
        use lyrel_core::host::session_operations::{
            SessionOperation, SessionOperationKind, launch_context,
        };
        let native = crate::core::cli_agent::CLIAgent::OpenCode;
        let mut sources = Vec::new();
        for leaf in self.tabs.iter().flat_map(|tab| tab.pane.terminals()) {
            let pane = leaf.read(cx);
            if pane.host_id() != host_id || pane.agent() != Some(native) {
                continue;
            }
            let Some(session) = pane.agent_session() else {
                continue;
            };
            let (Some(scope), Some(cwd), Some(launch_argv)) = (
                session.readout.session_scope.clone(),
                pane.host_cwd(),
                launch_context(native, &session),
            ) else {
                continue;
            };
            let selector = format!(
                "opencode:client:{}",
                scope
                    .opencode
                    .as_ref()
                    .and_then(|context| context.bridge.as_ref())
                    .map_or("disconnected", |bridge| bridge.client_id.as_str())
            );
            if sources
                .iter()
                .any(|(key, _): &(String, SessionOperation)| *key == selector)
            {
                continue;
            }
            sources.push((
                selector,
                SessionOperation {
                    expected_host: host_id.0,
                    agent: native,
                    kind: SessionOperationKind::Resume,
                    source_id: String::new(),
                    cwd,
                    launch_argv,
                    scope,
                },
            ));
        }
        sources
    }

    /// The agent sessions open in this workspace's panes, named by the same
    /// identity the session listings use — the settings Sessions page marks a
    /// listed session "running" through this, never by reading the pane tree.
    fn settings_live_sessions(&self, cx: &App) -> Vec<crate::ui::session_history::SessionIdentity> {
        use crate::ui::session_history::SessionIdentity;
        let mut out = Vec::new();
        for leaf in self.tabs.iter().flat_map(|tab| tab.pane.terminals()) {
            let pane = leaf.read(cx);
            let Some(agent) = pane.agent() else {
                continue;
            };
            let Some(state) = pane.agent_session() else {
                continue;
            };
            let Some(id) = state.session_id.clone() else {
                continue;
            };
            out.push(SessionIdentity {
                host: pane.host_id(),
                agent,
                id,
                cwd: state.cwd.clone(),
                scope: state.readout.session_scope.clone(),
            });
        }
        out
    }

    /// The host-scoped scan plan for the settings Sessions page: the same
    /// plan the Session History tool and Search Everywhere run, over this
    /// workspace's panes and offered agents. The settings window only ever
    /// holds the plan — the scan itself runs on the owning Host.
    fn settings_session_scan(
        &self,
        host: HostId,
        cx: &App,
    ) -> crate::ui::session_history::SessionScan {
        let panes: Vec<Entity<TerminalView>> = self
            .tabs
            .iter()
            .flat_map(|tab| tab.pane.terminals())
            .collect();
        let agents = self.offered_agents(cx);
        // No context-pane access from the settings window: the default scope
        // validates against the host's home, the same fallback Search's
        // session query uses.
        let default_cwd = crate::ui::desktop_lookups::home_for_host(cx, host).unwrap_or_default();
        crate::ui::session_history::plan_scan(
            host,
            &panes,
            &agents,
            &default_cwd,
            &cx.global::<Config>().agent_launch.clone(),
            cx,
        )
    }

    fn attach_settings(
        &mut self,
        view: Entity<SettingsWindow>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.settings_events = Some(cx.subscribe_in(
            &view,
            window,
            |this, source, event, window, cx| {
                if !this
                    .settings
                    .as_ref()
                    .is_some_and(|view| view.entity_id() == source.entity_id())
                {
                    return;
                }
                match event {
                    SettingsEvent::Closed(location) => {
                        this.last_settings_location = *location;
                        this.settings = None;
                        if let Some((settings, owner)) = this.settings_window.take() {
                            let app = cx.weak_entity();
                            cx.defer(move |cx| {
                                let _ = settings.update(cx, |_, window, _| window.remove_window());
                                let _ = owner.update(cx, |_, window, cx| {
                                    let _ = app.update(cx, |this, cx| {
                                        if this.settings.is_none() {
                                            window.activate_window();
                                            this.focus_previous_content(window, cx);
                                        }
                                    });
                                });
                            });
                        } else {
                            this.focus_previous_content(window, cx);
                        }
                        cx.notify();
                    }
                    SettingsEvent::ConnectSsh(id) => {
                        let id = *id;
                        cx.defer_in(window, move |this, window, cx| {
                            this.connect_ssh_profile(id, window, cx)
                        });
                    }
                    SettingsEvent::OpenFile(host, path) => {
                        let host = host.clone();
                        let path = path.clone();
                        cx.defer_in(window, move |this, window, cx| {
                            this.editor_open_on_host(host, &path, window, cx)
                        });
                    }
                    SettingsEvent::RestartDaemon => this.restart_daemon(window, cx),
                    SettingsEvent::RefreshShells => this.refresh_shells(cx),
                    SettingsEvent::TerminalCursorChanged { force_visible } => {
                        for leaf in this.tabs.iter().flat_map(|tab| tab.pane.terminals()) {
                            leaf.update(cx, |pane, cx| {
                                if *force_visible {
                                    pane.show_cursor();
                                }
                                cx.notify();
                            });
                        }
                    }
                    SettingsEvent::EditorGitGutter(on) => this.apply_editor_git_gutter(*on, cx),
                    SettingsEvent::LocaleChanged => {
                        // Menus and the explorer's context-menu wording read
                        // their labels from the registry, so a language change
                        // does not reach them on its own. No-op unless the
                        // user installed the context menu, and off Windows
                        // entirely.
                        set_menus(cx);
                        crate::core::explorer_context_menu::refresh_labels();
                        this.refresh_locale_state(window, cx);
                        crate::ui::windows::WindowRegistry::refresh_locale(
                            cx,
                            Some(this.workspace),
                        );
                    }
                    SettingsEvent::RebindKeymap => crate::ui::keymap::rebind(cx),
                    SettingsEvent::SessionsDeleted { host, agent, ids } => {
                        let host = *host;
                        let agent = *agent;
                        let ids = ids.clone();
                        cx.defer_in(window, move |this, _window, cx| {
                            this.note_sessions_deleted(host, agent, &ids, cx);
                        });
                    }
                    SettingsEvent::ApplyTheme {
                        persist,
                        refresh_windows,
                        rebuild_editor,
                    } => {
                        this.apply_theme_change(*persist, window, cx);
                        if *rebuild_editor {
                            this.refresh_settings_theme(cx);
                        }
                        if *refresh_windows {
                            cx.refresh_windows();
                        }
                    }
                }
            },
        ));
        let app = cx.entity();
        view.update(cx, |view, cx| {
            view.subscriptions
                .push(cx.observe(&app, |_, _, cx| cx.notify()));
            view.subscriptions
                .push(cx.observe_release(&app, move |view, _, cx| {
                    if let Some(state) = view.settings.take() {
                        state
                            .usage
                            .update(cx, |page, cx| page.set_visible(false, cx));
                    }
                    if view.window != view.owner() {
                        let window = view.window;
                        cx.defer(move |cx| {
                            let _ = window.update(cx, |_, window, _| window.remove_window());
                        });
                    }
                }));
        });
        self.settings = Some(view);
    }

    pub(super) fn toggle_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((handle, _)) = self.settings_window {
            if handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
            {
                return;
            }
            self.settings_window = None;
            self.settings = None;
        }
        if let Some(view) = self.settings.clone() {
            view.update(cx, |view, cx| view.close_settings_checked(window, cx));
            return;
        }
        self.open_settings_then(window, cx, |_, _, _| {});
    }

    pub(crate) fn open_settings_window_then(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: Option<SettingsFollowUp>,
    ) {
        let owner = window.window_handle();
        let app = cx.entity();
        let context = self.settings_context(owner, cx);
        let location = self.last_settings_location;
        cx.defer(move |cx| {
            let options = crate::ui::settings_window::window_options(cx);
            let opened = cx.open_window(options, |window, cx| {
                let view = cx.new(|cx| SettingsWindow::new(context, location, window, cx));
                let _ = owner.update(cx, |_, window, cx| {
                    app.update(cx, |this, cx| {
                        this.attach_settings(view.clone(), window, cx)
                    });
                });
                if let Some(then) = then {
                    view.update(cx, |view, cx| then(view, window, cx));
                }
                let root = cx.new(|cx| gpui_kit::component::Root::new(view, window, cx));
                crate::ui::theme::sync_window_backdrop(window, cx);
                root
            });
            match opened {
                Ok(handle) => app.update(cx, |this, _| {
                    this.settings_window = Some((handle.into(), owner))
                }),
                Err(error) => log::error!("failed to open the settings window: {error}"),
            }
        });
    }

    pub(crate) fn open_settings_section(
        &mut self,
        section: SettingsSection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings_then(window, cx, move |view, window, cx| {
            view.navigate_settings(section, None, window, cx)
        });
    }
    pub(crate) fn open_settings_then(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut SettingsWindow, &mut Window, &mut Context<SettingsWindow>) + 'static,
    ) {
        if let Some(view) = self.settings.clone() {
            let session = view
                .read(cx)
                .active_settings()
                .map(|s| s.navigation.session());
            let handle = view.read(cx).window;
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, cx| {
                    view.update(cx, |view, cx| {
                        if session.is_some_and(|session| {
                            view.active_settings()
                                .is_some_and(|s| s.navigation.accepts(session))
                        }) {
                            window.activate_window();
                            then(view, window, cx);
                        }
                    });
                });
            });
            return;
        }
        self.remember_active_pane(window, cx);
        if cfg!(test) {
            let owner = window.window_handle();
            let context = self.settings_context(owner, cx);
            let location = self.last_settings_location;
            let app = cx.entity();
            cx.defer(move |cx| {
                let _ = owner.update(cx, |_, window, cx| {
                    let view = cx.new(|cx| SettingsWindow::new(context, location, window, cx));
                    app.update(cx, |this, cx| {
                        this.attach_settings(view.clone(), window, cx)
                    });
                    view.update(cx, |view, cx| then(view, window, cx));
                });
            });
        } else {
            self.open_settings_window_then(window, cx, Some(Box::new(then)));
        }
    }
    pub(crate) fn open_ssh_profile_form(
        &mut self,
        profile: crate::core::ssh_profile::SshProfile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings_then(window, cx, move |view, window, cx| {
            view.with_settings_edits_resolved(window, cx, move |view, window, cx| {
                view.navigate_settings(SettingsSection::Ssh, None, window, cx);
                view.ssh_form_load(&profile, window, cx);
            });
        });
    }
    pub(crate) fn open_ssh_profile_in_settings(
        &mut self,
        id: uuid::Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings_then(window, cx, move |view, window, cx| {
            view.with_settings_edits_resolved(window, cx, move |view, window, cx| {
                let profile = cx
                    .global::<Config>()
                    .ssh_profiles
                    .iter()
                    .find(|p| p.id == id)
                    .cloned();
                view.navigate_settings(SettingsSection::Ssh, None, window, cx);
                if let Some(profile) = profile {
                    view.ssh_form_load(&profile, window, cx);
                }
            });
        });
    }

    pub(crate) fn persist_settings_config(&mut self, cx: &mut Context<Self>) {
        let config = cx.global::<Config>().clone();
        let error = config.try_save().err().map(|error| error.to_string());
        if let Some(message) = &error {
            log::warn!("failed to save settings: {message}");
        }
        if let Some(view) = &self.settings {
            view.update(cx, |view, cx| {
                if let Some(state) = view.active_settings_mut() {
                    if error.is_none() {
                        state.saved_config = config;
                    }
                    state.save_error = error;
                }
                cx.notify();
            });
        }
        cx.notify();
    }
    pub(crate) fn update_config(
        &mut self,
        cx: &mut Context<Self>,
        mutate: impl FnOnce(&mut Config),
    ) {
        mutate(cx.global_mut::<Config>());
        self.persist_settings_config(cx);
    }

    pub(super) fn apply_theme_change(
        &mut self,
        persist: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        apply_theme(Some(window), cx);
        set_menus(cx);
        if persist {
            self.persist_settings_config(cx);
        }
        cx.notify();
    }
    pub(super) fn refresh_settings_theme(&self, cx: &mut Context<Self>) {
        if let Some(view) = self.settings.clone() {
            let handle = view.read(cx).window;
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, cx| {
                    view.update(cx, |view, cx| view.rebuild_theme_editor(window, cx));
                });
            });
        }
    }
}
