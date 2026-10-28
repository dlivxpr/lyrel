//! Integrations: the agent hooks that put each agent's live status in the
//! sidebar, per machine, and the `lyrel` command on PATH.

use crate::core::agent_hooks::{HookAgent, HooksState};

use super::kit::{self, BtnKind, Tk, fs};
use super::shell::MenuEntry;
use super::*;

/// The agent the tab bar draws for this hook, so the two show the same mark.
fn cli_agent(agent: HookAgent) -> crate::core::cli_agent::CLIAgent {
    use crate::core::cli_agent::CLIAgent as C;
    match agent {
        HookAgent::OpenCode => C::OpenCode,
        HookAgent::Pi => C::Pi,
        HookAgent::OhMyPi => C::OhMyPi,
        HookAgent::Kimi => C::Kimi,
        HookAgent::Hermes => C::Hermes,
    }
}

impl SettingsWindow {
    pub(crate) fn render_settings_agents(&self, cx: &mut Context<Self>) -> AnyElement {
        Self::settings_page([
            self.render_agent_hooks_group(cx),
            self.render_command_line_rows(cx),
        ])
    }

    pub(crate) fn render_command_line_rows(&self, cx: &mut Context<Self>) -> AnyElement {
        let on = cx.global::<Config>().install_cli_on_path;
        let control = self.settings_switch("install-cli-on-path", on, cx, |this, on, _, cx| {
            this.set_install_cli_on_path(on, cx)
        });
        self.settings_group(
            Some(t(L10nKey::SettingsCommandLine)),
            None,
            [self
                .settings_row(
                    t(L10nKey::SettingsInstallCliOnPath),
                    t(L10nKey::SettingsCommandLineDesc),
                    control,
                    cx,
                )
                .into_any_element()],
            cx,
        )
    }

    fn render_agent_hooks_group(&self, cx: &mut Context<Self>) -> AnyElement {
        let tk = Tk::of(cx);
        let Some(s) = self.active_settings() else {
            return div().into_any_element();
        };
        let view = s.agent_hooks_states.clone();
        let note = s.agent_hooks_note.clone();
        let selected = s.agent_hooks_host;
        let query_input = s.agent_query.clone();
        let query = query_input.read(cx).value().trim().to_lowercase();
        let show_all = s.agent_show_all;
        let touched = s.agent_touched.clone();
        let busy = s.agent_busy.clone();
        let focused = s.navigation.focused_setting();
        let menu_open = s.menu.clone();

        // The machine the hooks below belong to.
        let machines = self.agent_hooks_machines(cx);
        let offline = self.agent_hooks_offline_count(cx);
        let current = machines
            .iter()
            .find(|m| m.host == selected)
            .map(|m| m.label.clone())
            .unwrap_or_default();
        let rows: Vec<AgentHookRow> = match &view {
            AgentHooksView::Ready(rows) => rows.clone(),
            _ => Vec::new(),
        };
        let installed = rows
            .iter()
            .filter(|r| r.state != HooksState::NotInstalled)
            .count();
        let mut entries = vec![MenuEntry::Heading(
            t(L10nKey::SettingsConnectedMachines).into(),
        )];
        for m in &machines {
            let host = m.host;
            entries.push(MenuEntry::Item {
                label: m.label.clone().into(),
                checked: host == selected,
                font: None,
                danger: false,
                trailing: (host == selected && matches!(view, AgentHooksView::Ready(_))).then(
                    || {
                        t_fmt(
                            L10nKey::SettingsInstalledCount,
                            &[("count", &installed.to_string())],
                        )
                        .into()
                    },
                ),
                on_pick: std::rc::Rc::new(move |this, _w, cx| {
                    if let Some(s) = this.active_settings_mut() {
                        s.agent_touched.clear();
                    }
                    this.select_agent_hooks_host(host, cx)
                }),
            });
        }
        if offline > 0 {
            entries.push(MenuEntry::Separator);
            entries.push(MenuEntry::Note(
                t_plural(L10nKey::SettingsOfflineMachines, offline, &[]).into(),
            ));
        }
        let machine_icon = Icon::empty()
            .path(if selected.is_local() {
                "icons/settings/machine-local.svg"
            } else {
                "icons/settings/machine-remote.svg"
            })
            .size(px(12.))
            .text_color(tk.k5)
            .into_any_element();
        let machine = self.settings_dropdown(
            "agent-hooks-machine",
            current.clone(),
            Some(machine_icon),
            entries,
            cx,
        );
        let machine_desc = if selected.is_local() {
            t(L10nKey::SettingsMachineLocalDesc).to_string()
        } else {
            t_fmt(L10nKey::SettingsMachineRemoteDesc, &[("name", &current)])
        };
        let machine_row = h_flex()
            .min_h(px(52.))
            .py(px(12.))
            .gap(px(32.))
            .items_center()
            .justify_between()
            .child(
                v_flex()
                    .min_w_0()
                    .gap(px(2.))
                    .child(div().text_size(fs(13.)).child(t(L10nKey::SettingsMachine)))
                    .child(
                        div()
                            .text_size(fs(12.))
                            .line_height(px(17.))
                            .text_color(tk.k5)
                            .child(machine_desc),
                    ),
            )
            .child(machine)
            .into_any_element();

        let total = HookAgent::ALL.len();
        let toolbar = h_flex()
            .gap(px(12.))
            .items_center()
            .pt(px(10.))
            .pb(px(8.))
            .child(
                kit::search_field(&query_input, &tk)
                    .w(px(240.))
                    .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                        if ev.keystroke.key.as_str() == "escape" {
                            let input = this.active_settings().map(|s| s.agent_query.clone());
                            if let Some(input) = input
                                && !input.read(cx).value().is_empty()
                            {
                                input.update(cx, |i, cx| i.set_value("", window, cx));
                                cx.stop_propagation();
                            }
                        }
                    })),
            )
            .child(div().flex_1())
            .when(matches!(view, AgentHooksView::Ready(_)), |r| {
                r.child(div().text_size(fs(12.)).text_color(tk.k45).child(t_fmt(
                    L10nKey::SettingsAgentsInstalledSummary,
                    &[
                        ("count", &installed.to_string()),
                        ("total", &total.to_string()),
                    ],
                )))
            })
            .into_any_element();

        let mut body: Vec<AnyElement> = vec![machine_row, toolbar];
        match &view {
            AgentHooksView::Loading => body.push(
                div()
                    .h(px(44.))
                    .flex()
                    .items_center()
                    .text_color(tk.k35)
                    .child(t(L10nKey::SettingsReadingAgentConfig))
                    .into_any_element(),
            ),
            AgentHooksView::Unavailable(reason) => body.push(
                div()
                    .py(px(12.))
                    .text_size(fs(12.5))
                    .text_color(tk.warn_text)
                    .child(reason.clone())
                    .into_any_element(),
            ),
            AgentHooksView::Ready(rows) => {
                let has = |r: &AgentHookRow| r.state != HooksState::NotInstalled;
                let is_focused =
                    |r: &AgentHookRow| focused.is_some_and(|k| t(k) == r.agent.display_name());
                let mut list: Vec<&AgentHookRow> = rows
                    .iter()
                    .filter(|r| {
                        if !query.is_empty() {
                            return format!("{} {}", r.agent.display_name(), r.target)
                                .to_lowercase()
                                .contains(&query);
                        }
                        show_all || has(r) || touched.contains(&r.agent) || is_focused(r)
                    })
                    .collect();
                let rank = |r: &AgentHookRow| match r.state {
                    HooksState::Outdated => 0,
                    HooksState::Installed => 1,
                    HooksState::NotInstalled => 2,
                };
                list.sort_by_key(|r| {
                    (
                        rank(r),
                        HookAgent::ALL
                            .iter()
                            .position(|a| *a == r.agent)
                            .unwrap_or(0),
                    )
                });
                let none = list.is_empty();
                let mut col = v_flex()
                    .id("agent-hook-list")
                    .when(!query.is_empty() || show_all, |c| {
                        c.max_h(px(416.)).overflow_y_scroll()
                    });
                for (i, row) in list.into_iter().enumerate() {
                    let row_note = note
                        .as_ref()
                        .filter(|(a, _)| *a == row.agent)
                        .map(|(_, text)| text.clone());
                    col = col.child(self.render_agent_row(
                        i,
                        row,
                        busy.contains(&row.agent),
                        row_note,
                        menu_open.as_deref(),
                        cx,
                    ));
                }
                body.push(col.into_any_element());
                if none {
                    body.push(
                        div()
                            .h(px(44.))
                            .flex()
                            .items_center()
                            .text_color(tk.k35)
                            .child(if query.is_empty() {
                                t(L10nKey::SettingsNoAgentsInstalled).to_string()
                            } else {
                                t_fmt(L10nKey::SettingsNoAgentsMatch, &[("query", &query)])
                            })
                            .into_any_element(),
                    );
                }
                let rest = total - installed;
                if query.is_empty() && rest > 0 {
                    body.push(
                        h_flex()
                            .h(px(40.))
                            .gap(px(8.))
                            .items_center()
                            .text_size(fs(12.))
                            .text_color(tk.k45)
                            .when(!show_all, |r| {
                                r.child(t_plural(L10nKey::SettingsMoreAgents, rest, &[]))
                            })
                            .child(
                                kit::button(
                                    "agent-show-all",
                                    if show_all {
                                        t(L10nKey::SettingsShowInstalledOnly)
                                    } else {
                                        t(L10nKey::SettingsShowAll)
                                    },
                                    BtnKind::Link,
                                )
                                .on_click(cx.listener(
                                    |this, _, _w, cx| {
                                        if let Some(s) = this.active_settings_mut() {
                                            s.agent_show_all = !s.agent_show_all;
                                            s.agent_touched.clear();
                                        }
                                        cx.notify();
                                    },
                                )),
                            )
                            .into_any_element(),
                    );
                }
            }
        }

        self.settings_group(
            Some(t(L10nKey::SettingsAgentsIntro)),
            Some(t(L10nKey::SettingsAgentsIntroDesc).to_string()),
            body,
            cx,
        )
    }

    fn render_agent_row(
        &self,
        i: usize,
        row: &AgentHookRow,
        busy: bool,
        note: Option<String>,
        menu_open: Option<&str>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tk = Tk::of(cx);
        let note = note.or_else(|| row.diagnostic.as_ref().map(integration_diagnostic));
        let agent = row.agent;
        let mark = cli_agent(agent);
        let (status, dot) = if busy {
            (Some(t(L10nKey::SettingsWorking)), tk.k3)
        } else {
            match row.state {
                HooksState::Installed => (Some(t(L10nKey::SettingsStatusInstalled)), tk.ok),
                HooksState::Outdated => (Some(t(L10nKey::SettingsUpdateAvailable)), tk.warn),
                HooksState::NotInstalled => (None, tk.k3),
            }
        };
        let action = (!busy)
            .then(|| match row.state {
                HooksState::NotInstalled => Some(t(L10nKey::SettingsInstall)),
                HooksState::Outdated => Some(t(L10nKey::SettingsUpdate)),
                HooksState::Installed => None,
            })
            .flatten();
        let has_menu = row.state != HooksState::NotInstalled && !busy;
        let menu_id: SharedString = format!("agent-menu-{i}").into();
        let open = menu_open == Some(menu_id.as_ref());
        let local = self
            .active_settings()
            .is_some_and(|s| s.agent_hooks_host.is_local());
        let target = row.target.clone();
        let menu = has_menu.then(|| {
            let mut entries = vec![MenuEntry::item(
                t(L10nKey::SettingsReinstall),
                false,
                move |this, _w, cx| this.settings_install_agent_hooks(agent, cx),
            )];
            if local {
                entries.push(MenuEntry::item(
                    t(L10nKey::SettingsRevealHookFile),
                    false,
                    move |_this, _w, cx| {
                        let path = crate::core::ssh_profile::expand_tilde(&target);
                        cx.reveal_path(std::path::Path::new(&path));
                    },
                ));
            }
            entries.push(MenuEntry::Item {
                label: t(L10nKey::SettingsUninstall).into(),
                checked: false,
                font: None,
                danger: true,
                trailing: None,
                on_pick: std::rc::Rc::new(move |this, _w, cx| {
                    this.settings_uninstall_agent_hooks(agent, cx)
                }),
            });
            let toggle = menu_id.clone();
            div()
                .relative()
                .child(
                    div()
                        .id(ElementId::Name(SharedString::from(format!(
                            "{menu_id}-trigger"
                        ))))
                        .size(px(26.))
                        .rounded(px(6.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .when(open, |d| d.bg(tk.k05))
                        .hover(move |s| s.bg(tk.k05))
                        .child(
                            Icon::empty()
                                .path("icons/settings/more.svg")
                                .size(px(12.))
                                .text_color(tk.k5),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.toggle_settings_menu(toggle.clone(), 0, false, window, cx)
                        })),
                )
                .when(open, |d| {
                    // Built only while open: the entries hold callbacks, and
                    // the panel is where they are wired to the rows.
                    d.child(kit::popover_below(
                        true,
                        4.,
                        self.settings_menu_panel(&menu_id, entries, cx)
                            .min_w(px(170.)),
                    ))
                })
        });

        v_flex()
            .id(SharedString::from(format!("agent-row-{i}")))
            .anchor_scroll(self.first_hit_anchor(agent.display_name(), cx))
            .child(
                h_flex()
                    .min_h(px(52.))
                    .py(px(8.))
                    .gap(px(12.))
                    .items_center()
                    .child(
                        // The tab bar's agent disc: the brand field, the mark
                        // on it, and a hairline where the field is the page.
                        div()
                            .size(px(20.))
                            .flex_shrink_0()
                            .rounded_full()
                            .bg(rgb(mark.accent_rgb()))
                            .when(presets::needs_edge(mark.accent_rgb(), tk.page), |d| {
                                d.shadow(vec![kit::ring(tk.k15, 0.5, false)])
                            })
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                gpui_kit::svg()
                                    .path(mark.icon_path())
                                    .size(px(11.))
                                    .text_color(rgb(mark.icon_rgb())),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(2.))
                            .child(agent.display_name())
                            .child(
                                div()
                                    .font_family(Tk::mono(cx))
                                    .text_size(fs(11.5))
                                    .text_color(tk.k45)
                                    .truncate()
                                    .child(row.target.clone()),
                            ),
                    )
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .gap(px(10.))
                            .items_center()
                            .when_some(status, |r, text| {
                                r.child(
                                    h_flex()
                                        .gap(px(6.))
                                        .items_center()
                                        .text_size(fs(12.))
                                        .text_color(tk.k5)
                                        .whitespace_nowrap()
                                        .child(div().size(px(6.)).rounded_full().bg(dot))
                                        .child(text),
                                )
                            })
                            .when_some(action, |r, label| {
                                r.child(
                                    kit::button(
                                        ("agent-hooks-install", i),
                                        label,
                                        BtnKind::Secondary,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _w, cx| {
                                            this.settings_install_agent_hooks(agent, cx)
                                        },
                                    )),
                                )
                            })
                            .children(menu),
                    ),
            )
            .when_some(note, |col, text| {
                col.child(
                    div()
                        .pl(px(32.))
                        .pb(px(8.))
                        .mt(px(-4.))
                        .text_size(fs(12.))
                        .line_height(px(17.))
                        .text_color(tk.k45)
                        .child(text),
                )
            })
            .into_any_element()
    }
}

fn integration_diagnostic(diagnostic: &crate::core::agent_hooks::HooksDiagnostic) -> String {
    use crate::core::agent_hooks::HooksDiagnostic;
    match diagnostic {
        HooksDiagnostic::PiExtension => t(L10nKey::IntegrationPiExtension).to_string(),
        HooksDiagnostic::OmpExtension => t(L10nKey::IntegrationOmpExtension).to_string(),
        HooksDiagnostic::KimiHistory => t(L10nKey::IntegrationKimiHistory).to_string(),
        HooksDiagnostic::HermesObserver { home } => t_fmt(
            L10nKey::IntegrationHermesObserver,
            &[("home", &home.display().to_string())],
        ),
        HooksDiagnostic::ScopeUnavailable { error } => {
            t_fmt(L10nKey::IntegrationScopeUnavailable, &[("error", error)])
        }
        HooksDiagnostic::OpenCodeUnreadable { path } => t_fmt(
            L10nKey::IntegrationOpenCodeUnreadable,
            &[("path", &path.display().to_string())],
        ),
        HooksDiagnostic::OpenCodeRuntimeDiagnosis { path } => t_fmt(
            L10nKey::IntegrationOpenCodeRuntimeDiagnosis,
            &[("path", &path.display().to_string())],
        ),
        HooksDiagnostic::OpenCodeDisabled { path, pattern } => t_fmt(
            L10nKey::IntegrationOpenCodeDisabled,
            &[("path", &path.display().to_string()), ("pattern", pattern)],
        ),
    }
}

impl SettingsWindow {
    pub(crate) fn agent_hooks_machines(
        &self,
        cx: &mut App,
    ) -> Vec<crate::ui::settings::AgentHooksMachine> {
        use crate::ui::settings::AgentHooksMachine;
        let mut out = vec![AgentHooksMachine {
            host: crate::ui::host_ops::HostId::LOCAL,
            label: t(L10nKey::AppAgentHooksThisComputer).to_string(),
        }];
        let configured = crate::ui::remote_connect::available_hosts(cx);
        for id in crate::ui::host_registry::HostRegistry::ids(cx) {
            if id.is_local() {
                continue;
            }
            let label = configured
                .iter()
                .find(|h| h.target.host_id() == id)
                .map(|h| h.label.clone())
                .unwrap_or_else(|| t(L10nKey::AppAgentHooksRemoteMachine).to_string());
            out.push(AgentHooksMachine { host: id, label });
        }
        out
    }

    pub(crate) fn agent_hooks_offline_count(&self, cx: &mut App) -> usize {
        let connected = crate::ui::host_registry::HostRegistry::ids(cx);
        cx.global::<Config>()
            .ssh_profiles
            .iter()
            .filter(|p| {
                !connected
                    .contains(&crate::core::session::RemoteTarget::Profile { id: p.id }.host_id())
            })
            .count()
    }

    pub(crate) fn select_agent_hooks_host(
        &mut self,
        host: crate::ui::host_ops::HostId,
        cx: &mut Context<Self>,
    ) {
        if let Some(s) = self.settings.as_mut() {
            if s.agent_hooks_host == host {
                return;
            }
            s.agent_hooks_host = host;
            s.agent_hooks_note = None;
            s.agent_hooks_states = crate::ui::settings::AgentHooksView::Loading;
        }
        self.load_agent_hooks_states(cx);
        cx.notify();
    }

    pub(super) fn load_agent_hooks_states(&mut self, cx: &mut Context<Self>) {
        use crate::core::agent_hooks::{HookAgent, HookTarget};
        use crate::ui::settings::{AgentHookRow, AgentHooksView};

        let Some(host_id) = self.settings.as_ref().map(|s| s.agent_hooks_host) else {
            return;
        };
        let seq = match self.settings.as_mut() {
            Some(s) => {
                s.agent_hooks_seq += 1;
                s.agent_hooks_seq
            }
            None => return,
        };
        let Some((host, home)) = self.agent_hooks_link(host_id, cx) else {
            if let Some(s) = self.settings.as_mut() {
                s.agent_hooks_states = AgentHooksView::Unavailable(Self::agent_hooks_offline_msg());
            }
            cx.notify();
            return;
        };
        let launch_commands = cx.global::<Config>().agent_launch.clone();

        crate::ui::host_ops::HostOps::run(
            host,
            cx,
            move |h| {
                let target = match &home {
                    Some(home) => HookTarget::remote(h, home.clone()),
                    None => HookTarget::local(h)?,
                }
                .with_launch_commands(&launch_commands);
                Some(
                    HookAgent::ALL
                        .into_iter()
                        .map(|agent| AgentHookRow {
                            agent,
                            state: crate::core::agent_hooks::hooks_state(&target, agent),
                            target: agent.target_display(&target),
                            diagnostic: crate::core::agent_hooks::hooks_diagnostic(&target, agent),
                        })
                        .collect::<Vec<_>>(),
                )
            },
            move |this, rows, cx| {
                if let Some(s) = this.settings.as_mut()
                    && s.agent_hooks_seq == seq
                {
                    s.agent_hooks_states = match rows {
                        Some(rows) => AgentHooksView::Ready(rows),
                        None => AgentHooksView::Unavailable(
                            t(L10nKey::AppAgentHooksNoHomeDir).to_string(),
                        ),
                    };
                    cx.notify();
                }
            },
        );
    }

    fn agent_hooks_offline_msg() -> String {
        t(L10nKey::AppAgentHooksOffline).to_string()
    }

    fn agent_hooks_link(
        &self,
        host_id: crate::ui::host_ops::HostId,
        cx: &mut App,
    ) -> Option<(crate::ui::host_ops::SharedHost, Option<std::path::PathBuf>)> {
        let host = crate::ui::host_registry::HostRegistry::get(cx, host_id)?;
        if host_id.is_local() {
            return Some((host, None));
        }
        if !host.is_connected() {
            return None;
        }
        let home = crate::ui::remote_connect::HostLinks::home(cx, host_id)?;
        Some((host, Some(home)))
    }

    pub(crate) fn settings_install_agent_hooks(
        &mut self,
        agent: crate::core::agent_hooks::HookAgent,
        cx: &mut Context<Self>,
    ) {
        self.run_agent_hooks_action(agent, true, cx);
    }

    pub(crate) fn settings_uninstall_agent_hooks(
        &mut self,
        agent: crate::core::agent_hooks::HookAgent,
        cx: &mut Context<Self>,
    ) {
        self.run_agent_hooks_action(agent, false, cx);
    }

    /// Words a hook install or removal for the note in Settings.
    ///
    /// `agent_hooks` returns what it did rather than a sentence, because it
    /// lives in `lyrel-core` and cannot reach `src/ui/i18n` — it used to hand
    /// back English prose, which a Chinese or Japanese UI then showed as-is.
    fn agent_hooks_outcome_msg(outcome: &crate::core::agent_hooks::HookOutcome) -> String {
        use crate::core::agent_hooks::HookOutcome as O;
        match outcome {
            O::Installed => t(L10nKey::AppAgentHooksInstalled).to_string(),
            O::Removed => t(L10nKey::AppAgentHooksRemoved).to_string(),
            O::NothingInstalled => t(L10nKey::AppAgentHooksNothingInstalled).to_string(),
            O::NoLyrelHooks => t(L10nKey::AppAgentHooksNoLyrelHooks).to_string(),
        }
    }

    fn run_agent_hooks_action(
        &mut self,
        agent: crate::core::agent_hooks::HookAgent,
        install: bool,
        cx: &mut Context<Self>,
    ) {
        use crate::core::agent_hooks::HookTarget;

        let Some(host_id) = self.settings.as_ref().map(|s| s.agent_hooks_host) else {
            return;
        };
        let Some((host, home)) = self.agent_hooks_link(host_id, cx) else {
            if let Some(s) = self.settings.as_mut() {
                s.agent_hooks_note = Some((agent, Self::agent_hooks_offline_msg()));
                s.agent_hooks_states = crate::ui::settings::AgentHooksView::Unavailable(
                    Self::agent_hooks_offline_msg(),
                );
            }
            cx.notify();
            return;
        };

        if let Some(s) = self.settings.as_mut() {
            s.agent_busy.insert(agent);
            s.agent_touched.insert(agent);
        }
        cx.notify();
        let launch_commands = cx.global::<Config>().agent_launch.clone();
        crate::ui::host_ops::HostOps::run(
            host,
            cx,
            move |h| {
                let target = match &home {
                    Some(home) => HookTarget::remote(h, home.clone()),
                    None => HookTarget::local(h).ok_or_else(|| {
                        anyhow::anyhow!("{}", t(L10nKey::AppAgentHooksHomeDirUnresolved))
                    })?,
                }
                .with_launch_commands(&launch_commands);
                if install {
                    crate::core::agent_hooks::install_hooks(&target, agent)
                } else {
                    crate::core::agent_hooks::uninstall_hooks(&target, agent)
                }
            },
            move |this, result, cx| {
                if let Some(s) = this.settings.as_mut() {
                    s.agent_busy.remove(&agent);
                    s.agent_hooks_note = Some((
                        agent,
                        match result {
                            Ok(outcome) => Self::agent_hooks_outcome_msg(&outcome),
                            // Every sibling error names its action; this one
                            // said only "Failed:", leaving the note that
                            // reports it silent about which half of the
                            // toggle had not happened.
                            Err(e) => t_fmt(
                                match install {
                                    true => L10nKey::AppAgentHooksInstallFailed,
                                    false => L10nKey::AppAgentHooksRemoveFailed,
                                },
                                &[("error", &e.to_string())],
                            ),
                        },
                    ));
                }
                this.load_agent_hooks_states(cx);
                cx.notify();
            },
        );
    }
}
