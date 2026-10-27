use super::*;
use crate::core::config::{FONT_SIZE_MAX, FONT_SIZE_MIN, LINE_HEIGHT_MAX, LINE_HEIGHT_MIN};
use crate::ui::i18n::set_locale;
use crate::ui::theme::{apply_theme, set_menus};
use std::sync::Arc;

impl SettingsWindow {
    fn set_ui_font_size(&mut self, size: f32, cx: &mut Context<Self>) {
        use crate::core::config::{UI_FONT_SIZE_MAX, UI_FONT_SIZE_MIN};
        let size = size.clamp(UI_FONT_SIZE_MIN, UI_FONT_SIZE_MAX);
        let cfg = cx.global_mut::<Config>();
        if cfg.ui_font_size == size {
            return;
        }
        cfg.ui_font_size = size;
        self.persist_settings_config(cx);
        gpui_kit::component::Theme::update(cx, |theme| theme.font_size = px(size));
        // Unlike the settings that only redraw the window they were changed
        // in, this one re-lays-out every open window, and each reads the new
        // rem from the global on its own next frame.
        cx.notify();
    }

    pub(crate) fn ui_font_size(&self, cx: &gpui_kit::App) -> f32 {
        cx.global::<Config>().ui_font_size
    }

    pub(crate) fn change_ui_font_size(&mut self, delta: f32, cx: &mut Context<Self>) {
        self.set_ui_font_size(self.ui_font_size(cx) + delta, cx);
    }

    pub(crate) fn reset_ui_font_size(&mut self, cx: &mut Context<Self>) {
        self.set_ui_font_size(Config::default().ui_font_size, cx);
    }
    pub(crate) fn set_font_ligatures(&mut self, on: bool, cx: &mut Context<Self>) {
        let features = on.then(|| {
            crate::core::config::FontFeatures(Arc::new(vec![
                ("calt".to_string(), 1),
                ("liga".to_string(), 1),
            ]))
        });
        let cfg = cx.global_mut::<Config>();
        cfg.font_features = features;
        self.persist_settings_config(cx);
        cx.notify();
    }
    pub(crate) fn set_cursor_style(&mut self, style: CursorStyle, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.cursor_style = style);
    }

    pub(crate) fn set_prompt_cursor_style(
        &mut self,
        style: crate::core::config::PromptCursorStyle,
        cx: &mut Context<Self>,
    ) {
        self.update_config(cx, |cfg| cfg.prompt_cursor_style = style);
        cx.emit(SettingsEvent::TerminalCursorChanged {
            force_visible: false,
        });
    }

    pub(crate) fn persist_settings_config(&mut self, cx: &mut Context<Self>) {
        let config = cx.global::<Config>().clone();
        let error = config.try_save().err().map(|error| error.to_string());
        if let Some(message) = &error {
            log::warn!("failed to save settings: {message}");
        }
        if let Some(s) = self.active_settings_mut() {
            if error.is_none() {
                s.saved_config = config;
            }
            s.save_error = error;
        }
        cx.notify();
    }

    pub(crate) fn discard_unsaved_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let snapshot = self
            .active_settings()
            .filter(|s| s.save_error.is_some())
            .map(|s| s.saved_config.clone());
        if let Some(snapshot) = snapshot {
            cx.set_global(snapshot);
            if let Some(s) = self.active_settings_mut() {
                s.save_error = None;
            }
            apply_theme(Some(window), cx);
        }
    }

    pub(crate) fn update_config(
        &mut self,
        cx: &mut Context<Self>,
        mutate: impl FnOnce(&mut Config),
    ) {
        mutate(cx.global_mut::<Config>());
        self.persist_settings_config(cx);
    }

    pub(crate) fn set_link_url(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.link_url = on);
    }

    pub(crate) fn set_link_file_open(
        &mut self,
        mode: crate::core::config::LinkFileOpen,
        cx: &mut Context<Self>,
    ) {
        self.update_config(cx, |cfg| cfg.link_file_open = Some(mode));
    }

    pub(crate) fn set_ssh_loopback_forward(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.ssh_loopback_forward = on);
    }

    pub(crate) fn set_verify_host_keys(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.verify_host_keys = on);
    }

    pub(crate) fn set_ssh_warn_on_close(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.ssh_warn_on_close = on);
    }
    pub(crate) fn set_check_for_updates(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.check_for_updates = on);
    }

    pub(crate) fn set_auto_download_updates(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.auto_download_updates = on);
    }

    /// Takes effect at next launch: `core::cli_install` runs once from `main`,
    /// before there is a window to flip this in. Turning it off does not remove
    /// a symlink already placed — the install is idempotent, not reversible.
    pub(crate) fn set_install_cli_on_path(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.install_cli_on_path = on);
    }

    pub(crate) fn set_dim_inactive_panes(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.dim_inactive_panes = on);
    }

    pub(crate) fn set_cursor_blink(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.cursor_blink = on);
        if !on {
            cx.emit(SettingsEvent::TerminalCursorChanged {
                force_visible: true,
            });
        }
    }

    pub(crate) fn set_scrollback_limit(&mut self, lines: usize, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| {
            cfg.scrollback_limit = lines.clamp(100, crate::core::config::MAX_SCROLLBACK)
        });
    }

    pub(crate) fn set_new_tab_position(&mut self, pos: NewTabPosition, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.new_tab_position = pos);
    }

    pub(crate) fn set_confirm_close(
        &mut self,
        mode: crate::core::config::ConfirmClose,
        cx: &mut Context<Self>,
    ) {
        self.update_config(cx, |cfg| cfg.confirm_close = mode);
    }

    pub(crate) fn set_sidebar_auto_grouping(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.sidebar_auto_grouping = on);
    }
    pub(crate) fn set_notify_mode(
        &mut self,
        mode: crate::core::config::NotifyMode,
        cx: &mut Context<Self>,
    ) {
        self.update_config(cx, |cfg| cfg.notify_on_command_finish = mode);
    }

    pub(crate) fn set_notify_threshold(&mut self, secs: u64, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.notify_threshold_secs = secs.clamp(1, 3600));
    }

    pub(crate) fn set_bell_mode(
        &mut self,
        mode: crate::core::config::BellMode,
        cx: &mut Context<Self>,
    ) {
        self.update_config(cx, |cfg| cfg.bell = mode);
    }

    pub(crate) fn set_restore_session(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.restore_session = on);
    }

    /// Takes effect on the next pane: a shell is told where its history lives
    /// when it starts, and nothing can move it afterwards.
    pub(crate) fn set_per_pane_history(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.per_pane_history = on);
    }

    pub(crate) fn set_show_tray_icon(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.show_tray_icon = on);
    }

    /// Saved only: it is handed to the platform wrapper once, at launch, so
    /// it takes hold at the next one (see `platform_application` in main.rs).
    pub(crate) fn set_font_thicken(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.font_thicken = on);
    }

    pub(crate) fn set_macos_option_as_alt(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.macos_option_as_alt = on);
    }

    pub(crate) fn set_mouse_hide_while_typing(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.mouse_hide_while_typing = on);
        crate::ui::theme::apply_cursor_hide_mode(cx);
    }

    pub(crate) fn set_focus_follows_mouse(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.focus_follows_mouse = on);
    }

    pub(crate) fn set_mouse_reporting(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.mouse_reporting = on);
    }

    pub(crate) fn set_mouse_scroll_multiplier(&mut self, mult: f32, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| {
            cfg.mouse_scroll_multiplier = mult.clamp(0.1, 10.0)
        });
    }

    pub(crate) fn set_smooth_scroll(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.smooth_scroll = on);
    }

    /// Panes read the modifier off the global config as each wheel event
    /// arrives, so there is nothing to push at them here.
    pub(crate) fn set_mouse_zoom_modifier(
        &mut self,
        modifier: MouseZoomModifier,
        cx: &mut Context<Self>,
    ) {
        self.update_config(cx, |cfg| cfg.mouse_zoom_modifier = modifier);
    }

    pub(crate) fn set_clipboard_trim(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.clipboard_trim_trailing_spaces = on);
    }

    pub(crate) fn set_copy_on_select(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.copy_on_select = on);
    }

    pub(crate) fn set_smart_select(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.smart_select = on);
    }

    /// Hands the prompt to the shell's own line editor, or takes it back.
    ///
    /// Live panes carry a cached copy of the flag (see
    /// [`crate::terminal::view::TerminalView::prompt_editor`]), so the switch
    /// has to reach them: without this, only panes opened afterwards would
    /// change hands.
    pub(crate) fn set_prompt_editor(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.prompt_editor = on);
    }

    pub(crate) fn set_tab_completion(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.tab_completion = on);
    }

    pub(crate) fn set_history_search(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.history_search = on);
    }

    pub(crate) fn set_startup_mode(
        &mut self,
        mode: crate::core::config::StartupMode,
        cx: &mut Context<Self>,
    ) {
        self.update_config(cx, |cfg| cfg.startup_mode = mode);
    }

    pub(crate) fn set_remember_window_size(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.remember_window_size = on);
    }
    pub(crate) fn reset_settings_value(
        &mut self,
        title: L10nKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let defaults = Config::default();
        match title {
            L10nKey::SettingsDimInactivePanes => {
                self.set_dim_inactive_panes(defaults.dim_inactive_panes, cx)
            }
            L10nKey::SettingsCursorBlink => self.set_cursor_blink(defaults.cursor_blink, cx),
            L10nKey::SettingsCursorShape => self.set_cursor_style(defaults.cursor_style, cx),
            L10nKey::SettingsPromptCursorShape => {
                self.set_prompt_cursor_style(defaults.prompt_cursor_style, cx)
            }
            L10nKey::SettingsScrollback => self.set_scrollback_limit(defaults.scrollback_limit, cx),
            L10nKey::SettingsNewTabPosition => {
                self.set_new_tab_position(defaults.new_tab_position, cx)
            }
            L10nKey::SettingsConfirmClose => self.set_confirm_close(defaults.confirm_close, cx),
            L10nKey::SettingsSidebarGrouping => {
                self.set_sidebar_auto_grouping(defaults.sidebar_auto_grouping, cx)
            }
            L10nKey::SettingsEditorGitGutter => {
                self.set_editor_git_gutter(defaults.editor_git_gutter, cx)
            }
            L10nKey::SettingsEditorLsp => self.set_editor_lsp(defaults.editor_lsp, cx),
            L10nKey::SettingsEditorSoftWrap => {
                self.set_editor_soft_wrap(defaults.editor_soft_wrap, cx)
            }
            L10nKey::SettingsEditorMarkdownPreview => {
                self.set_editor_markdown_preview(defaults.editor_markdown_preview, cx)
            }
            L10nKey::SettingsNotifyOnCommandFinish => {
                self.set_notify_mode(defaults.notify_on_command_finish, cx)
            }
            L10nKey::SettingsNotifyThreshold => {
                self.set_notify_threshold(defaults.notify_threshold_secs, cx)
            }
            L10nKey::SettingsTerminalBell => self.set_bell_mode(defaults.bell, cx),
            L10nKey::SettingsRestoreLastLayout => {
                self.set_restore_session(defaults.restore_session, cx)
            }
            L10nKey::SettingsPerPaneHistory => {
                self.set_per_pane_history(defaults.per_pane_history, cx)
            }
            L10nKey::SettingsShowTrayIcon => self.set_show_tray_icon(defaults.show_tray_icon, cx),
            L10nKey::SettingsOptionAsMeta => {
                self.set_macos_option_as_alt(defaults.macos_option_as_alt, cx)
            }
            L10nKey::SettingsHideMouseWhileTyping => {
                self.set_mouse_hide_while_typing(defaults.mouse_hide_while_typing, cx)
            }
            L10nKey::SettingsFocusFollowsMouse => {
                self.set_focus_follows_mouse(defaults.focus_follows_mouse, cx)
            }
            L10nKey::SettingsReportMouseToApps => {
                self.set_mouse_reporting(defaults.mouse_reporting, cx)
            }
            L10nKey::SettingsScrollSpeed => {
                self.set_mouse_scroll_multiplier(defaults.mouse_scroll_multiplier, cx)
            }
            L10nKey::SettingsSmoothScroll => self.set_smooth_scroll(defaults.smooth_scroll, cx),
            L10nKey::SettingsMouseZoom => {
                self.set_mouse_zoom_modifier(defaults.mouse_zoom_modifier, cx)
            }
            L10nKey::SettingsTrimTrailingSpaces => {
                self.set_clipboard_trim(defaults.clipboard_trim_trailing_spaces, cx)
            }
            L10nKey::SettingsCopyOnSelect => self.set_copy_on_select(defaults.copy_on_select, cx),
            L10nKey::SettingsSmartSelection => self.set_smart_select(defaults.smart_select, cx),
            L10nKey::SettingsPromptEditor => self.set_prompt_editor(defaults.prompt_editor, cx),
            L10nKey::SettingsTabCompletion => self.set_tab_completion(defaults.tab_completion, cx),
            L10nKey::SettingsHistorySearch => self.set_history_search(defaults.history_search, cx),
            L10nKey::SettingsStartupWindow => self.set_startup_mode(defaults.startup_mode, cx),
            L10nKey::SettingsRememberWindowSize => {
                self.set_remember_window_size(defaults.remember_window_size, cx)
            }
            L10nKey::SettingsCheckUpdatesOnLaunch => {
                self.set_check_for_updates(defaults.check_for_updates, cx)
            }
            L10nKey::SettingsAutoDownload => {
                self.set_auto_download_updates(defaults.auto_download_updates, cx)
            }
            L10nKey::DetectUrls => self.set_link_url(defaults.link_url, cx),
            L10nKey::ForwardSshLoopbackLinks => {
                self.set_ssh_loopback_forward(defaults.ssh_loopback_forward, cx)
            }
            L10nKey::SettingsVerifyHostKeys => {
                self.set_verify_host_keys(defaults.verify_host_keys, cx)
            }
            L10nKey::WarnBeforeClosing => {
                self.set_ssh_warn_on_close(defaults.ssh_warn_on_close, cx)
            }
            L10nKey::SettingsLanguage => self.set_gui_language(
                Self::normalize_gui_language(&defaults.gui_language),
                window,
                cx,
            ),
            L10nKey::SettingsFontSize => self.reset_font_size(cx),
            L10nKey::SettingsUiFontSize => self.reset_ui_font_size(cx),
            L10nKey::SettingsLineHeight => self.reset_line_height(cx),
            L10nKey::SettingsFontFamily => {
                self.commit_font_family(defaults.font_family.clone(), cx)
            }
            L10nKey::SettingsBoldFont => self.commit_font_family_emphasis(
                true,
                crate::ui::settings::font_default_label().to_string(),
                cx,
            ),
            L10nKey::SettingsItalicFont => self.commit_font_family_emphasis(
                false,
                crate::ui::settings::font_default_label().to_string(),
                cx,
            ),
            L10nKey::SettingsUiFontFamily => self.commit_ui_font_family(
                crate::ui::settings::ui_font_default_label().to_string(),
                window,
                cx,
            ),
            L10nKey::SettingsSyncWithSystem => {
                self.set_theme_follow_system(defaults.theme_follow_system, window, cx)
            }
            L10nKey::SettingsLegiblePalette => {
                self.set_theme_legible_palette(defaults.theme_legible_palette, window, cx)
            }
            L10nKey::SettingsOpacity => {
                self.update_config(cx, |cfg| cfg.window_opacity = None);
                apply_theme(Some(window), cx);
            }
            L10nKey::SettingsBlur => {
                self.update_config(cx, |cfg| cfg.window_blur = None);
                apply_theme(Some(window), cx);
            }
            #[cfg(target_os = "windows")]
            L10nKey::SettingsBackdrop => {
                self.set_window_backdrop(defaults.window_backdrop, window, cx)
            }
            L10nKey::SettingsFontLigatures => self.set_font_ligatures(
                defaults
                    .font_features
                    .as_ref()
                    .is_some_and(|f| f.is_calt_enabled() == Some(true)),
                cx,
            ),
            L10nKey::OpenFilesWith => {
                self.update_config(cx, |cfg| cfg.link_file_open = defaults.link_file_open);
            }
            L10nKey::SettingsProgram => {
                self.update_config(cx, |cfg| cfg.shell = defaults.shell.clone())
            }
            L10nKey::SettingsArguments => self.update_config(cx, |cfg| {
                if let Some(shell) = &mut cfg.shell {
                    shell.args.clear();
                }
            }),
            L10nKey::SettingsStartIn => self.update_config(cx, |cfg| {
                cfg.working_directory.strategy = defaults.working_directory.strategy
            }),
            L10nKey::SettingsCustomPath => self.update_config(cx, |cfg| {
                cfg.working_directory.path = defaults.working_directory.path.clone()
            }),
            L10nKey::SettingsAppHttpProxy => {
                self.update_config(cx, |cfg| cfg.http_proxy = defaults.http_proxy.clone())
            }
            L10nKey::SettingsOpenFilesCommand => self.update_config(cx, |cfg| {
                cfg.link_file_command = defaults.link_file_command.clone()
            }),
            _ => return,
        }
        self.refresh_settings_controls(title, window, cx);
    }

    fn refresh_settings_controls(
        &mut self,
        title: L10nKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut subs = Vec::new();
        match title {
            L10nKey::SettingsProgram | L10nKey::SettingsArguments => {
                let (program, args, _) = self.build_shell_inputs(&mut subs, window, cx);
                if let Some(s) = self.active_settings_mut() {
                    s.shell_program_input = program;
                    s.shell_args_input = args;
                }
            }
            L10nKey::SettingsCustomPath => {
                let value = cx.global::<Config>().working_directory.path.clone();
                if let Some(s) = self.active_settings() {
                    s.wd_path_input
                        .clone()
                        .update(cx, |s, cx| s.set_value(value, window, cx));
                }
            }
            L10nKey::SettingsOpenFilesCommand => {
                let value = self.build_link_file_command_input(&mut subs, window, cx);
                if let Some(s) = self.active_settings_mut() {
                    s.link_file_command_input = value;
                }
            }
            L10nKey::SettingsAppHttpProxy => {
                let value = self.build_http_proxy_input(&mut subs, window, cx);
                if let Some(s) = self.active_settings_mut() {
                    s.http_proxy_input = value;
                }
            }
            _ => {}
        }
        if let Some(s) = self.active_settings_mut() {
            s._subs.extend(subs);
        }
        cx.notify();
    }
    pub(crate) fn normalize_gui_language(code: &str) -> &'static str {
        crate::core::config::normalize_gui_language(code)
    }

    pub(crate) fn set_gui_language(
        &mut self,
        code: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let code = Self::normalize_gui_language(code);
        {
            let cfg = cx.global_mut::<Config>();
            cfg.gui_language = code.to_string();
        }
        set_locale(code);
        self.persist_settings_config(cx);
        set_menus(cx);
        // Explorer reads its menu wording from the registry, so it is the one
        // surface a language change does not reach on its own. No-op unless
        // the user installed the context menu, and off Windows entirely.
        crate::core::explorer_context_menu::refresh_labels();
        self.refresh_locale_state(window, cx);
        cx.emit(SettingsEvent::LocaleChanged);
    }
    pub(crate) fn commit_font_family(&mut self, family: String, cx: &mut Context<Self>) {
        let cfg = cx.global_mut::<Config>();
        cfg.font_family = family;
        self.persist_settings_config(cx);
        cx.notify();
    }

    pub(crate) fn commit_font_family_emphasis(
        &mut self,
        bold: bool,
        name: String,
        cx: &mut Context<Self>,
    ) {
        let family = (name != crate::ui::settings::font_default_label()).then_some(name);
        let cfg = cx.global_mut::<Config>();
        if bold {
            cfg.font_family_bold = family;
        } else {
            cfg.font_family_italic = family;
        }
        self.persist_settings_config(cx);
        cx.notify();
    }

    pub(crate) fn commit_ui_font_family(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let family = (name != crate::ui::settings::ui_font_default_label()).then_some(name);
        let cfg = cx.global_mut::<Config>();
        if cfg.ui_font_family == family {
            return;
        }
        cfg.ui_font_family = family;
        self.persist_settings_config(cx);
        apply_theme(Some(window), cx);
        cx.refresh_windows();
        cx.notify();
    }
    pub(crate) fn change_font_size(&mut self, delta: f32, cx: &mut Context<Self>) {
        let size = (cx.global::<Config>().font_size + delta).clamp(FONT_SIZE_MIN, FONT_SIZE_MAX);
        self.update_config(cx, |cfg| cfg.font_size = size);
    }
    fn reset_font_size(&mut self, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.font_size = Config::default().font_size);
    }
    pub(crate) fn change_line_height(&mut self, delta: f32, cx: &mut Context<Self>) {
        let height =
            (cx.global::<Config>().line_height + delta).clamp(LINE_HEIGHT_MIN, LINE_HEIGHT_MAX);
        self.update_config(cx, |cfg| cfg.line_height = height);
    }
    fn reset_line_height(&mut self, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.line_height = Config::default().line_height);
    }
    pub(crate) fn set_editor_git_gutter(&mut self, on: bool, cx: &mut Context<Self>) {
        if on == cx.global::<Config>().editor_git_gutter {
            return;
        }
        self.update_config(cx, |cfg| cfg.editor_git_gutter = on);
        cx.emit(SettingsEvent::EditorGitGutter(on));
    }
    pub(crate) fn restart_daemon(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(SettingsEvent::RestartDaemon);
    }
}
