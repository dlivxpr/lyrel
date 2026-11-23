//! The General page: the session the app opens on — workspace, editor and
//! tab-bar rows. Its editor group moved here from a module of its own, so the
//! page a row renders on is the module that owns it.
use super::kit::Tk;
use super::shell::MenuEntry;
use super::*;

impl SettingsWindow {
    pub(crate) fn render_settings_general(&self, cx: &mut Context<Self>) -> AnyElement {
        let cfg = cx.global::<Config>();
        let current_lang = Self::normalize_gui_language(&cfg.gui_language);
        let language = self.settings_dropdown(
            "settings-language",
            t(lyrel_ui_base::i18n::language_mode_label(current_lang)),
            None,
            lyrel_ui_base::i18n::LANGUAGE_MODES
                .iter()
                .map(|&code| {
                    MenuEntry::item(
                        t(lyrel_ui_base::i18n::language_mode_label(code)),
                        code == current_lang,
                        move |this, window, cx| this.set_gui_language(code, window, cx),
                    )
                })
                .collect(),
            cx,
        );
        let mut first = vec![
            self.settings_row(
                t(L10nKey::SettingsLanguage),
                t(L10nKey::SettingsLanguageDesc),
                language,
                cx,
            )
            .into_any_element(),
        ];
        if cfg!(target_os = "macos") {
            let set_default = self
                .settings_button(
                    "set-default-terminal",
                    t(L10nKey::SettingsDefaultTerminalSet),
                    cx,
                    |_, window, cx| {
                        let message = match crate::core::default_terminal::set_as_default_terminal()
                        {
                            Ok(()) => t(L10nKey::SettingsDefaultTerminalSetSuccess).to_string(),
                            Err(error) => t_fmt(
                                L10nKey::SettingsDefaultTerminalSetFailed,
                                &[("error", &error)],
                            ),
                        };
                        window.notify_app(message, cx);
                    },
                )
                .into_any_element();
            first.push(
                self.settings_row(
                    t(L10nKey::SettingsDefaultTerminal),
                    t(L10nKey::SettingsDefaultTerminalDesc),
                    set_default,
                    cx,
                )
                .into_any_element(),
            );
        }

        let cfg = cx.global::<Config>();
        let startup_idx = match cfg.startup_mode {
            lyrel_core::core::config::StartupMode::Normal => 0,
            lyrel_core::core::config::StartupMode::Maximized => 1,
            lyrel_core::core::config::StartupMode::Fullscreen => 2,
        };
        let restore_session = cfg.restore_session;
        let remember_window_size = cfg.remember_window_size;
        let show_tray_icon = cfg.show_tray_icon;
        let notify_idx = match cfg.notify_on_command_finish {
            NotifyMode::Never => 0,
            NotifyMode::Unfocused => 1,
            NotifyMode::Always => 2,
        };
        // Exact-match highlight with a "Custom (Ns)" fallback: a hand-set 20s
        // used to light up "30s" (#550).
        let (threshold_sel, threshold_custom) = preset_choice(
            &NOTIFY_THRESHOLD_BUCKETS,
            cfg.notify_threshold_secs,
            |secs| format!("{secs}s"),
        );

        let startup = self.settings_choice(
            "wt-startup",
            &[
                t(L10nKey::SettingsStartupNormal),
                t(L10nKey::SettingsStartupMaximized),
                t(L10nKey::SettingsStartupFullscreen),
            ],
            startup_idx,
            cx,
            |this, ix, _w, cx| {
                let mode = match ix {
                    0 => lyrel_core::core::config::StartupMode::Normal,
                    1 => lyrel_core::core::config::StartupMode::Maximized,
                    _ => lyrel_core::core::config::StartupMode::Fullscreen,
                };
                this.set_startup_mode(mode, cx);
            },
        );
        let remember = self.settings_switch(
            "wt-remember-window",
            remember_window_size,
            cx,
            |this, on, _, cx| this.set_remember_window_size(on, cx),
        );
        let restore = self.settings_switch(
            "wt-restore-session",
            restore_session,
            cx,
            |this, on, _, cx| this.set_restore_session(on, cx),
        );
        let tray = self.settings_switch("wt-tray-icon", show_tray_icon, cx, |this, on, _, cx| {
            this.set_show_tray_icon(on, cx)
        });
        let notify = self.settings_choice(
            "wt-notify",
            &[
                t(L10nKey::NotifyModeNever),
                t(L10nKey::NotifyModeUnfocused),
                t(L10nKey::NotifyModeAlways),
            ],
            notify_idx,
            cx,
            |this, ix, _w, cx| {
                let mode = match ix {
                    0 => NotifyMode::Never,
                    1 => NotifyMode::Unfocused,
                    _ => NotifyMode::Always,
                };
                this.set_notify_mode(mode, cx);
            },
        );
        let threshold = self.settings_choice_valued(
            "wt-notify-threshold",
            &NOTIFY_THRESHOLD_LABELS,
            threshold_sel,
            threshold_custom,
            cx,
            |this, ix, _w, cx| {
                let secs = NOTIFY_THRESHOLD_BUCKETS
                    .get(ix)
                    .copied()
                    .unwrap_or(Config::default().notify_threshold_secs);
                this.set_notify_threshold(secs, cx);
            },
        );

        Self::settings_page([
            self.settings_group(None, None, first, cx),
            self.settings_group(
                Some(t(L10nKey::SettingsStartupRestore)),
                None,
                [
                    self.settings_row(
                        t(L10nKey::SettingsStartupWindow),
                        t(L10nKey::SettingsStartupWindowDesc),
                        startup,
                        cx,
                    ),
                    self.settings_row(
                        t(L10nKey::SettingsRememberWindowSize),
                        t(L10nKey::SettingsRememberWindowSizeDesc),
                        remember,
                        cx,
                    ),
                    self.settings_row(
                        t(L10nKey::SettingsRestoreLastLayout),
                        t(L10nKey::SettingsRestoreLastLayoutDesc),
                        restore,
                        cx,
                    ),
                    self.settings_row(
                        t(L10nKey::SettingsShowTrayIcon),
                        t(L10nKey::SettingsShowTrayIconDesc),
                        tray,
                        cx,
                    ),
                ]
                .map(IntoElement::into_any_element),
                cx,
            ),
            self.render_tabs_group(cx),
            self.render_editor_group(cx),
            self.settings_group(
                Some(t(L10nKey::SettingsNotifications)),
                None,
                [
                    self.settings_row(
                        t(L10nKey::SettingsNotifyOnCommandFinish),
                        t(L10nKey::SettingsNotifyOnCommandFinishDesc),
                        notify,
                        cx,
                    ),
                    self.settings_row(
                        t(L10nKey::SettingsNotifyThreshold),
                        t(L10nKey::SettingsNotifyThresholdDesc),
                        threshold,
                        cx,
                    ),
                    self.settings_row(
                        t(L10nKey::SettingsCommandDesktop),
                        t(L10nKey::SettingsCommandDesktopDesc),
                        {
                            let on = cx.global::<Config>().terminal_notifications.command_desktop;
                            self.settings_switch("settings-cmd-desktop", on, cx, |this, on, _, cx| {
                                this.set_terminal_notification(
                                    TerminalNotificationChannel::CommandDesktop,
                                    on,
                                    cx,
                                )
                            })
                        },
                        cx,
                    ),
                    self.settings_row(
                        t(L10nKey::SettingsOscDesktop),
                        t(L10nKey::SettingsOscDesktopDesc),
                        {
                            let on = cx.global::<Config>().terminal_notifications.osc_desktop;
                            self.settings_switch("settings-osc-desktop", on, cx, |this, on, _, cx| {
                                this.set_terminal_notification(
                                    TerminalNotificationChannel::OscDesktop,
                                    on,
                                    cx,
                                )
                            })
                        },
                        cx,
                    ),
                ]
                .map(IntoElement::into_any_element),
                cx,
            ),
            self.settings_group(
                Some(t(L10nKey::SettingsProgramActivity)),
                Some(t(L10nKey::SettingsProgramActivityDesc).to_string()),
                [
                    self.activity_matrix_header(cx),
                    self.activity_row(
                        t(L10nKey::ActivityRowRunning),
                        t(L10nKey::ActivityRowRunningDesc),
                        lyrel_core::core::config::ActivityState::Running,
                        lyrel_core::core::config::ActivityChannels {
                            tab: cx.global::<Config>().program_activity.running.tab,
                            ..Default::default()
                        },
                        cx,
                    ),
                    self.activity_row(
                        t(L10nKey::ActivityRowWaiting),
                        t(L10nKey::ActivityRowWaitingDesc),
                        lyrel_core::core::config::ActivityState::Waiting,
                        cx.global::<Config>().program_activity.waiting,
                        cx,
                    ),
                    self.activity_row(
                        t(L10nKey::ActivityRowEnded),
                        t(L10nKey::ActivityRowEndedDesc),
                        lyrel_core::core::config::ActivityState::Ended,
                        cx.global::<Config>().program_activity.ended,
                        cx,
                    ),
                    self.activity_row(
                        t(L10nKey::ActivityRowFailed),
                        t(L10nKey::ActivityRowFailedDesc),
                        lyrel_core::core::config::ActivityState::Failed,
                        cx.global::<Config>().program_activity.failed,
                        cx,
                    ),
                ]
                .map(IntoElement::into_any_element),
                cx,
            ),
        ])
    }

    /// The column heads over the activity matrix. They mirror the row's
    /// right-aligned control area, so each label sits over its switch
    /// column; the badge column lands with #397.
    fn activity_matrix_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let head = |key: L10nKey| {
            div()
                .w(px(56.))
                .flex()
                .justify_center()
                .text_xs()
                .text_color(Tk::of(&*cx).k5)
                .child(t(key))
        };
        h_flex()
            .w_full()
            .justify_end()
            .child(
                h_flex()
                    .gap(px(4.))
                    .child(head(L10nKey::ActivityChannelTab))
                    .child(head(L10nKey::ActivityChannelDesktop))
                    .child(head(L10nKey::ActivityChannelSound)),
            )
            .into_any_element()
    }

    /// One matrix row: the activity name as the row label, its channels as
    /// fixed-width cells. `Running` only owns the tab column — the others
    /// are placeholders so the grid still lines up.
    fn activity_row(
        &self,
        label: &str,
        desc: &str,
        activity: lyrel_core::core::config::ActivityState,
        channels: lyrel_core::core::config::ActivityChannels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut cell = |channel: ActivityChannel, on: bool| {
            div()
                .w(px(56.))
                .flex()
                .justify_center()
                .child(self.settings_switch(
                    match channel {
                        ActivityChannel::Tab => "settings-activity-tab",
                        ActivityChannel::Desktop => "settings-activity-desktop",
                        ActivityChannel::Sound => "settings-activity-sound",
                    },
                    on,
                    cx,
                    move |this, on, _, cx| this.set_activity_channel(activity, channel, on, cx),
                ))
        };
        let controls = if activity == lyrel_core::core::config::ActivityState::Running {
            h_flex()
                .gap(px(4.))
                .child(cell(ActivityChannel::Tab, channels.tab))
                .child(div().w(px(56.)))
                .child(div().w(px(56.)))
        } else {
            h_flex()
                .gap(px(4.))
                .child(cell(ActivityChannel::Tab, channels.tab))
                .child(cell(ActivityChannel::Desktop, channels.desktop))
                .child(cell(ActivityChannel::Sound, channels.sound))
        };
        self.settings_row(label, desc, controls.into_any_element(), cx)
            .into_any_element()
    }

    fn render_tabs_group(&self, cx: &mut Context<Self>) -> AnyElement {
        let cfg = cx.global::<Config>();
        let new_tab_idx = match cfg.new_tab_position {
            NewTabPosition::AfterCurrent => 0,
            NewTabPosition::End => 1,
        };
        let sidebar_auto_grouping = cfg.sidebar_auto_grouping;
        let confirm_idx = match cfg.confirm_close {
            ConfirmClose::Never => 0,
            ConfirmClose::WhenBusy => 1,
            ConfirmClose::Always => 2,
        };
        let confirm = self.settings_choice(
            "wt-confirm-close",
            &[
                t(L10nKey::ConfirmCloseNever),
                t(L10nKey::ConfirmCloseWhenBusy),
                t(L10nKey::ConfirmCloseAlways),
            ],
            confirm_idx,
            cx,
            |this, ix, _w, cx| {
                let mode = match ix {
                    0 => ConfirmClose::Never,
                    1 => ConfirmClose::WhenBusy,
                    _ => ConfirmClose::Always,
                };
                this.set_confirm_close(mode, cx);
            },
        );
        let new_tab = self.settings_choice(
            "wt-new-tab-pos",
            &[t(L10nKey::SettingsAfterCurrent), t(L10nKey::SettingsAtEnd)],
            new_tab_idx,
            cx,
            |this, ix, _w, cx| {
                this.set_new_tab_position(
                    if ix == 0 {
                        NewTabPosition::AfterCurrent
                    } else {
                        NewTabPosition::End
                    },
                    cx,
                );
            },
        );
        let grouping = self.settings_switch(
            "wt-sidebar-auto-grouping",
            sidebar_auto_grouping,
            cx,
            |this, on, _, cx| this.set_sidebar_auto_grouping(on, cx),
        );
        self.settings_group(
            Some(t(L10nKey::SettingsTabs)),
            None,
            [
                self.settings_row(
                    t(L10nKey::SettingsNewTabPosition),
                    t(L10nKey::SettingsNewTabPositionDesc),
                    new_tab,
                    cx,
                ),
                self.settings_row(
                    t(L10nKey::SettingsSidebarGrouping),
                    t(L10nKey::SettingsSidebarGroupingDesc),
                    grouping,
                    cx,
                ),
                self.settings_row(
                    t(L10nKey::SettingsConfirmClose),
                    t(L10nKey::SettingsConfirmCloseDesc),
                    confirm,
                    cx,
                ),
            ]
            .map(IntoElement::into_any_element),
            cx,
        )
    }
}

/// The Editor group on the General page: the code editor's switches that
/// used to live only in config.json or in a status-bar toggle.
impl SettingsWindow {
    pub(super) fn render_editor_group(&self, cx: &mut Context<Self>) -> AnyElement {
        let cfg = cx.global::<Config>();
        let (git_gutter, lsp, soft_wrap) =
            (cfg.editor_git_gutter, cfg.editor_lsp, cfg.editor_soft_wrap);
        let git_gutter =
            self.settings_switch("wt-editor-git-gutter", git_gutter, cx, |this, on, _, cx| {
                this.set_editor_git_gutter(on, cx)
            });
        let lsp = self.settings_switch("wt-editor-lsp", lsp, cx, |this, on, _, cx| {
            this.set_editor_lsp(on, cx)
        });
        let soft_wrap =
            self.settings_switch("wt-editor-soft-wrap", soft_wrap, cx, |this, on, _, cx| {
                this.set_editor_soft_wrap(on, cx)
            });
        self.settings_group(
            Some(t(L10nKey::SettingsEditor)),
            None,
            [
                self.settings_row(
                    t(L10nKey::SettingsEditorGitGutter),
                    t(L10nKey::SettingsEditorGitGutterDesc),
                    git_gutter,
                    cx,
                ),
                self.settings_row(
                    t(L10nKey::SettingsEditorLsp),
                    t(L10nKey::SettingsEditorLspDesc),
                    lsp,
                    cx,
                ),
                self.settings_row(
                    t(L10nKey::SettingsEditorSoftWrap),
                    t(L10nKey::SettingsEditorSoftWrapDesc),
                    soft_wrap,
                    cx,
                ),
            ]
            .map(IntoElement::into_any_element),
            cx,
        )
    }

    /// `LspStore` watches the config: off closes every document and lets
    /// its server go; on asks every window for the files it has open, which
    /// starts their servers again.
    pub(crate) fn set_editor_lsp(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.editor_lsp = on);
        cx.notify();
    }

    /// What new files open with; files already open keep their own Wrap.
    pub(crate) fn set_editor_soft_wrap(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.editor_soft_wrap = on);
    }
}
