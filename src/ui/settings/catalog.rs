//! 本地化设置目录、匹配、排序与 modified 判定；不持有窗口或编辑草稿。
use super::*;

pub(super) struct SearchEntry {
    pub(super) section: SettingsSection,
    pub(super) title: L10nKey,
    pub(super) keywords: L10nKey,
}

pub(super) fn settings_search_entries() -> &'static [SearchEntry] {
    use L10nKey::*;
    use SettingsSection::*;
    &[
        SearchEntry {
            section: Skills,
            title: SettingsNavSkills,
            keywords: SkillsPageDescription,
        },
        SearchEntry {
            section: Statistics,
            title: StatisticsTitle,
            keywords: StatisticsKeywords,
        },
        SearchEntry {
            section: Usage,
            title: SettingsNavUsage,
            keywords: UsageSearchKeywords,
        },
        SearchEntry {
            section: Appearance,
            title: SettingsUiFontSize,
            keywords: SettingsSearchFontSizeKeywords,
        },
        SearchEntry {
            section: Terminal,
            title: SettingsPerPaneHistory,
            keywords: SettingsSearchHistorySearchKeywords,
        },
        SearchEntry {
            section: KeyboardMouse,
            title: SettingsMouseZoom,
            keywords: SettingsSearchScrollSpeedKeywords,
        },
        SearchEntry {
            section: Terminal,
            title: SettingsCustomPath,
            keywords: SettingsSearchStartInKeywords,
        },
        SearchEntry {
            section: Terminal,
            title: SettingsOpenFilesCommand,
            keywords: SettingsSearchOpenFilesWithKeywords,
        },
        #[cfg(target_os = "macos")]
        SearchEntry {
            section: General,
            title: SettingsDefaultTerminal,
            keywords: SettingsSearchAboutKeywords,
        },
        SearchEntry {
            section: About,
            title: SettingsServer,
            keywords: SettingsSearchAboutKeywords,
        },
        SearchEntry {
            section: General,
            title: SettingsLanguage,
            keywords: SettingsSearchLanguageKeywords,
        },
        SearchEntry {
            section: Appearance,
            title: SettingsThemeIntroTitle,
            keywords: SettingsSearchThemeKeywords,
        },
        SearchEntry {
            section: Appearance,
            title: SettingsSyncWithSystem,
            keywords: SettingsSearchSyncWithSystemKeywords,
        },
        SearchEntry {
            section: Appearance,
            title: SettingsLegiblePalette,
            keywords: SettingsSearchLegiblePaletteKeywords,
        },
        SearchEntry {
            section: Appearance,
            title: SettingsCustomThemes,
            keywords: SettingsSearchCustomThemesKeywords,
        },
        SearchEntry {
            section: Appearance,
            title: SettingsOpacity,
            keywords: SettingsSearchOpacityKeywords,
        },
        SearchEntry {
            section: Appearance,
            title: SettingsBlur,
            keywords: SettingsSearchBlurKeywords,
        },
        #[cfg(target_os = "windows")]
        SearchEntry {
            section: Appearance,
            title: SettingsBackdrop,
            keywords: SettingsSearchBackdropKeywords,
        },
        SearchEntry {
            section: Appearance,
            title: SettingsDimInactivePanes,
            keywords: SettingsSearchDimInactivePanesKeywords,
        },
        SearchEntry {
            section: Appearance,
            title: SettingsFontSize,
            keywords: SettingsSearchFontSizeKeywords,
        },
        SearchEntry {
            section: Appearance,
            title: SettingsUiFontFamily,
            keywords: SettingsSearchUiFontFamilyKeywords,
        },
        SearchEntry {
            section: Appearance,
            title: SettingsLineHeight,
            keywords: SettingsSearchLineHeightKeywords,
        },
        SearchEntry {
            section: Appearance,
            title: SettingsFontFamily,
            keywords: SettingsSearchFontFamilyKeywords,
        },
        SearchEntry {
            section: Appearance,
            title: SettingsBoldFont,
            keywords: SettingsSearchBoldFontKeywords,
        },
        SearchEntry {
            section: Appearance,
            title: SettingsItalicFont,
            keywords: SettingsSearchItalicFontKeywords,
        },
        SearchEntry {
            section: Appearance,
            title: SettingsFontLigatures,
            keywords: SettingsSearchFontLigaturesKeywords,
        },
        #[cfg(target_os = "macos")]
        SearchEntry {
            section: Appearance,
            title: SettingsFontThicken,
            keywords: SettingsSearchFontThickenKeywords,
        },
        SearchEntry {
            section: Appearance,
            title: SettingsCursorShape,
            keywords: SettingsSearchCursorShapeKeywords,
        },
        SearchEntry {
            section: Appearance,
            title: SettingsPromptCursorShape,
            keywords: SettingsSearchPromptCursorShapeKeywords,
        },
        SearchEntry {
            section: Appearance,
            title: SettingsCursorBlink,
            keywords: SettingsSearchCursorBlinkKeywords,
        },
        SearchEntry {
            section: Appearance,
            title: SettingsAnsiColors,
            keywords: SettingsSearchAnsiColorsKeywords,
        },
        SearchEntry {
            section: Appearance,
            title: SettingsBackgroundImage,
            keywords: SettingsSearchBackgroundImageKeywords,
        },
        SearchEntry {
            section: Appearance,
            title: SettingsImageOpacity,
            keywords: SettingsSearchImageOpacityKeywords,
        },
        SearchEntry {
            section: Terminal,
            title: SettingsProgram,
            keywords: SettingsSearchProgramKeywords,
        },
        SearchEntry {
            section: Terminal,
            title: SettingsArguments,
            keywords: SettingsSearchArgumentsKeywords,
        },
        SearchEntry {
            section: Terminal,
            title: SettingsStartIn,
            keywords: SettingsSearchStartInKeywords,
        },
        SearchEntry {
            section: Terminal,
            title: SettingsScrollback,
            keywords: SettingsSearchScrollbackKeywords,
        },
        SearchEntry {
            section: Terminal,
            title: SettingsScrollSpeed,
            keywords: SettingsSearchScrollSpeedKeywords,
        },
        SearchEntry {
            section: Terminal,
            title: SettingsSmoothScroll,
            keywords: SettingsSearchSmoothScrollKeywords,
        },
        SearchEntry {
            section: KeyboardMouse,
            title: SettingsFocusFollowsMouse,
            keywords: SettingsSearchFocusFollowsMouseKeywords,
        },
        SearchEntry {
            section: KeyboardMouse,
            title: SettingsHideMouseWhileTyping,
            keywords: SettingsSearchHideMouseWhileTypingKeywords,
        },
        SearchEntry {
            section: KeyboardMouse,
            title: SettingsReportMouseToApps,
            keywords: SettingsSearchReportMouseToAppsKeywords,
        },
        SearchEntry {
            section: Terminal,
            title: SettingsTerminalBell,
            keywords: SettingsSearchTerminalBellKeywords,
        },
        SearchEntry {
            section: Terminal,
            title: DetectUrls,
            keywords: SettingsSearchDetectUrlsKeywords,
        },
        SearchEntry {
            section: Terminal,
            title: ForwardSshLoopbackLinks,
            keywords: SettingsSearchForwardSshLoopbackLinksKeywords,
        },
        SearchEntry {
            section: Terminal,
            title: OpenFilesWith,
            keywords: SettingsSearchOpenFilesWithKeywords,
        },
        SearchEntry {
            section: Terminal,
            title: SettingsPromptEditor,
            keywords: SettingsSearchPromptEditorKeywords,
        },
        SearchEntry {
            section: Terminal,
            title: SettingsTabCompletion,
            keywords: SettingsSearchTabCompletionKeywords,
        },
        SearchEntry {
            section: Terminal,
            title: SettingsHistorySearch,
            keywords: SettingsSearchHistorySearchKeywords,
        },
        #[cfg(target_os = "macos")]
        SearchEntry {
            section: KeyboardMouse,
            title: SettingsOptionAsMeta,
            keywords: SettingsSearchOptionAsMetaKeywords,
        },
        SearchEntry {
            section: KeyboardMouse,
            title: SettingsSmartSelection,
            keywords: SettingsSearchSmartSelectionKeywords,
        },
        SearchEntry {
            section: KeyboardMouse,
            title: SettingsCopyOnSelect,
            keywords: SettingsSearchCopyOnSelectKeywords,
        },
        SearchEntry {
            section: KeyboardMouse,
            title: SettingsTrimTrailingSpaces,
            keywords: SettingsSearchTrimTrailingSpacesKeywords,
        },
        SearchEntry {
            section: Ssh,
            title: SettingsHosts,
            keywords: SettingsSearchHostsKeywords,
        },
        SearchEntry {
            section: Ssh,
            title: SettingsVerifyHostKeys,
            keywords: SettingsSearchVerifyHostKeysKeywords,
        },
        SearchEntry {
            section: Ssh,
            title: WarnBeforeClosing,
            keywords: SettingsSearchWarnBeforeClosingKeywords,
        },
        SearchEntry {
            section: Agents,
            title: SettingsAgentOpencode,
            keywords: SettingsSearchOpencodeKeywords,
        },
        SearchEntry {
            section: Agents,
            title: SettingsAgentPi,
            keywords: SettingsSearchPiKeywords,
        },
        SearchEntry {
            section: Agents,
            title: SettingsAgentOhMyPi,
            keywords: SettingsSearchOhMyPiKeywords,
        },
        SearchEntry {
            section: Agents,
            title: SettingsAgentKimiCode,
            keywords: SettingsSearchKimiCodeKeywords,
        },
        SearchEntry {
            section: Agents,
            title: SettingsAgentHermes,
            keywords: SettingsSearchHermesKeywords,
        },
        SearchEntry {
            section: General,
            title: SettingsStartupWindow,
            keywords: SettingsSearchStartupWindowKeywords,
        },
        SearchEntry {
            section: General,
            title: SettingsRememberWindowSize,
            keywords: SettingsSearchRememberWindowSizeKeywords,
        },
        SearchEntry {
            section: General,
            title: SettingsRestoreLastLayout,
            keywords: SettingsSearchRestoreLastLayoutKeywords,
        },
        SearchEntry {
            section: General,
            title: SettingsShowTrayIcon,
            keywords: SettingsSearchShowTrayIconKeywords,
        },
        SearchEntry {
            section: General,
            title: SettingsNewTabPosition,
            keywords: SettingsSearchNewTabPositionKeywords,
        },
        SearchEntry {
            section: General,
            title: SettingsConfirmClose,
            keywords: SettingsSearchConfirmCloseKeywords,
        },
        SearchEntry {
            section: General,
            title: SettingsSidebarGrouping,
            keywords: SettingsSearchSidebarGroupingKeywords,
        },
        SearchEntry {
            section: General,
            title: SettingsEditorGitGutter,
            keywords: SettingsSearchEditorGitGutterKeywords,
        },
        SearchEntry {
            section: General,
            title: SettingsEditorLsp,
            keywords: SettingsSearchEditorLspKeywords,
        },
        SearchEntry {
            section: General,
            title: SettingsEditorSoftWrap,
            keywords: SettingsSearchEditorSoftWrapKeywords,
        },
        SearchEntry {
            section: General,
            title: SettingsNotifyOnCommandFinish,
            keywords: SettingsSearchNotifyOnCommandFinishKeywords,
        },
        SearchEntry {
            section: General,
            title: SettingsNotifyThreshold,
            keywords: SettingsSearchNotifyThresholdKeywords,
        },
        SearchEntry {
            section: KeyboardMouse,
            title: SettingsSearchKeybindingsTitle,
            keywords: SettingsSearchKeybindingsKeywords,
        },
        SearchEntry {
            section: About,
            title: SettingsNavAbout,
            keywords: SettingsSearchAboutKeywords,
        },
        SearchEntry {
            section: About,
            title: SettingsAppHttpProxy,
            keywords: SettingsSearchAppHttpProxyKeywords,
        },
        SearchEntry {
            section: About,
            title: SettingsCheckUpdatesOnLaunch,
            keywords: SettingsSearchCheckUpdatesOnLaunchKeywords,
        },
        SearchEntry {
            section: About,
            title: SettingsAutoDownload,
            keywords: SettingsSearchAutoDownloadKeywords,
        },
        SearchEntry {
            section: Agents,
            title: SettingsInstallCliOnPath,
            keywords: SettingsSearchCommandLineToolKeywords,
        },
    ]
}

impl SearchEntry {
    pub(super) fn config_key(&self) -> &'static str {
        match self.title {
            L10nKey::SettingsDimInactivePanes => "dim_inactive_panes",
            L10nKey::SettingsCursorBlink => "cursor_blink",
            L10nKey::SettingsCursorShape => "cursor_style",
            L10nKey::SettingsPromptCursorShape => "prompt_cursor_style",
            L10nKey::SettingsScrollback => "scrollback_limit",
            L10nKey::SettingsNewTabPosition => "new_tab_position",
            L10nKey::SettingsConfirmClose => "confirm_close",
            L10nKey::SettingsSidebarGrouping => "sidebar_auto_grouping",
            L10nKey::SettingsEditorGitGutter => "editor_git_gutter",
            L10nKey::SettingsEditorLsp => "editor_lsp",
            L10nKey::SettingsEditorSoftWrap => "editor_soft_wrap",
            L10nKey::SettingsNotifyOnCommandFinish => "notify_on_command_finish",
            L10nKey::SettingsNotifyThreshold => "notify_threshold_secs",
            L10nKey::SettingsTerminalBell => "bell",
            L10nKey::SettingsRestoreLastLayout => "restore_session",
            L10nKey::SettingsPerPaneHistory => "per_pane_history",
            L10nKey::SettingsShowTrayIcon => "show_tray_icon",
            L10nKey::SettingsOptionAsMeta => "macos_option_as_alt",
            L10nKey::SettingsHideMouseWhileTyping => "mouse_hide_while_typing",
            L10nKey::SettingsFocusFollowsMouse => "focus_follows_mouse",
            L10nKey::SettingsReportMouseToApps => "mouse_reporting",
            L10nKey::SettingsScrollSpeed => "mouse_scroll_multiplier",
            L10nKey::SettingsSmoothScroll => "smooth_scroll",
            L10nKey::SettingsMouseZoom => "mouse_zoom_modifier",
            L10nKey::SettingsTrimTrailingSpaces => "clipboard_trim_trailing_spaces",
            L10nKey::SettingsCopyOnSelect => "copy_on_select",
            L10nKey::SettingsSmartSelection => "smart_select",
            L10nKey::SettingsPromptEditor => "prompt_editor",
            L10nKey::SettingsTabCompletion => "tab_completion",
            L10nKey::SettingsHistorySearch => "history_search",
            L10nKey::SettingsStartupWindow => "startup_mode",
            L10nKey::SettingsRememberWindowSize => "remember_window_size",
            L10nKey::SettingsCheckUpdatesOnLaunch => "check_for_updates",
            L10nKey::SettingsAutoDownload => "auto_download_updates",
            L10nKey::DetectUrls => "link_url",
            L10nKey::ForwardSshLoopbackLinks => "ssh_loopback_forward",
            L10nKey::SettingsVerifyHostKeys => "verify_host_keys",
            L10nKey::WarnBeforeClosing => "ssh_warn_on_close",
            L10nKey::SettingsLanguage => "gui_language",
            L10nKey::SettingsProgram => "shell.program",
            L10nKey::SettingsArguments => "shell.args",
            L10nKey::SettingsStartIn => "working_directory.strategy",
            L10nKey::SettingsCustomPath => "working_directory.path",
            L10nKey::SettingsFontSize => "font_size",
            L10nKey::SettingsUiFontSize => "ui_font_size",
            L10nKey::SettingsLineHeight => "line_height",
            L10nKey::SettingsFontFamily => "font_family",
            L10nKey::SettingsBoldFont => "font_family_bold",
            L10nKey::SettingsItalicFont => "font_family_italic",
            L10nKey::SettingsUiFontFamily => "ui_font_family",
            L10nKey::SettingsFontLigatures => "font_features",
            L10nKey::SettingsFontThicken => "font_thicken",
            L10nKey::SettingsOpacity => "window_opacity",
            L10nKey::SettingsBlur => "window_blur",
            L10nKey::SettingsBackdrop => "window_backdrop",
            L10nKey::SettingsSyncWithSystem => "theme_follow_system",
            L10nKey::SettingsLegiblePalette => "theme_legible_palette",
            L10nKey::SettingsThemeIntroTitle => "theme_preset",
            L10nKey::SettingsAppHttpProxy => "http_proxy",
            L10nKey::SettingsOpenFilesCommand => "link_file_command",
            L10nKey::OpenFilesWith => "link_file_open",
            L10nKey::SettingsInstallCliOnPath => "install_cli_on_path",
            L10nKey::SettingsSearchKeybindingsTitle => "keybindings",
            L10nKey::SettingsHosts => "ssh_profiles",
            _ => "",
        }
    }
    pub(super) fn description(&self) -> &'static str {
        match self.title {
            L10nKey::SettingsNavSkills => t(L10nKey::SkillsPageDescription),
            L10nKey::SettingsUiFontSize => t(L10nKey::SettingsUiFontSizeDesc),
            L10nKey::SettingsMouseZoom => t(L10nKey::SettingsMouseZoomDesc),
            L10nKey::SettingsCustomPath => t(L10nKey::SettingsCustomPathDesc),
            L10nKey::SettingsDefaultTerminal => t(L10nKey::SettingsDefaultTerminalDesc),
            L10nKey::SettingsServer => t(L10nKey::SettingsServerDesc),
            L10nKey::SettingsLanguage => t(L10nKey::SettingsLanguageDesc),
            L10nKey::SettingsSyncWithSystem => t(L10nKey::SettingsSyncWithSystemDesc),
            L10nKey::SettingsLegiblePalette => t(L10nKey::SettingsLegiblePaletteDesc),
            L10nKey::SettingsOpacity => t(L10nKey::SettingsOpacityDesc),
            L10nKey::SettingsBlur => t(L10nKey::SettingsBlurDesc),
            L10nKey::SettingsBackdrop => t(L10nKey::SettingsBackdropDesc),
            L10nKey::SettingsDimInactivePanes => t(L10nKey::SettingsDimInactivePanesDesc),
            L10nKey::SettingsFontSize => t(L10nKey::SettingsFontSizeDesc),
            L10nKey::SettingsUiFontFamily => t(L10nKey::SettingsUiFontFamilyDesc),
            L10nKey::SettingsLineHeight => t(L10nKey::SettingsLineHeightDesc),
            L10nKey::SettingsFontFamily => t(L10nKey::SettingsFontFamilyDesc),
            L10nKey::SettingsBoldFont => t(L10nKey::SettingsBoldFontDesc),
            L10nKey::SettingsItalicFont => t(L10nKey::SettingsItalicFontDesc),
            L10nKey::SettingsFontLigatures => t(L10nKey::SettingsFontLigaturesDesc),
            L10nKey::SettingsFontThicken => t(L10nKey::SettingsFontThickenDesc),
            L10nKey::SettingsCursorShape => t(L10nKey::SettingsCursorShapeDesc),
            L10nKey::SettingsPromptCursorShape => t(L10nKey::SettingsPromptCursorShapeDesc),
            L10nKey::SettingsCursorBlink => t(L10nKey::SettingsCursorBlinkDesc),
            L10nKey::SettingsBackgroundImage => t(L10nKey::SettingsBackgroundImageDesc),
            L10nKey::SettingsImageOpacity => t(L10nKey::SettingsImageOpacityDesc),
            L10nKey::SettingsProgram => t(L10nKey::SettingsProgramDesc),
            L10nKey::SettingsArguments => t(L10nKey::SettingsArgumentsDesc),
            L10nKey::SettingsStartIn => t(L10nKey::SettingsStartInDesc),
            L10nKey::SettingsScrollback => t(L10nKey::SettingsScrollbackDesc),
            L10nKey::SettingsScrollSpeed => t(L10nKey::SettingsScrollSpeedDesc),
            L10nKey::SettingsSmoothScroll => t(L10nKey::SettingsSmoothScrollDesc),
            L10nKey::SettingsFocusFollowsMouse => t(L10nKey::SettingsFocusFollowsMouseDesc),
            L10nKey::SettingsHideMouseWhileTyping => t(L10nKey::SettingsHideMouseWhileTypingDesc),
            L10nKey::SettingsReportMouseToApps => t(L10nKey::SettingsReportMouseToAppsDesc),
            L10nKey::SettingsTerminalBell => t(L10nKey::SettingsTerminalBellDesc),
            L10nKey::SettingsPromptEditor => t(L10nKey::SettingsPromptEditorDesc),
            L10nKey::SettingsTabCompletion => t(L10nKey::SettingsTabCompletionDesc),
            L10nKey::SettingsHistorySearch => t(L10nKey::SettingsHistorySearchDesc),
            L10nKey::SettingsOptionAsMeta => t(L10nKey::SettingsOptionAsMetaDesc),
            L10nKey::SettingsSmartSelection => t(L10nKey::SettingsSmartSelectionDesc),
            L10nKey::SettingsCopyOnSelect => t(L10nKey::SettingsCopyOnSelectDesc),
            L10nKey::SettingsTrimTrailingSpaces => t(L10nKey::SettingsTrimTrailingSpacesDesc),
            L10nKey::SettingsVerifyHostKeys => t(L10nKey::SettingsVerifyHostKeysDesc),
            L10nKey::SettingsStartupWindow => t(L10nKey::SettingsStartupWindowDesc),
            L10nKey::SettingsRememberWindowSize => t(L10nKey::SettingsRememberWindowSizeDesc),
            L10nKey::SettingsRestoreLastLayout => t(L10nKey::SettingsRestoreLastLayoutDesc),
            L10nKey::SettingsShowTrayIcon => t(L10nKey::SettingsShowTrayIconDesc),
            L10nKey::SettingsNewTabPosition => t(L10nKey::SettingsNewTabPositionDesc),
            L10nKey::SettingsConfirmClose => t(L10nKey::SettingsConfirmCloseDesc),
            L10nKey::SettingsSidebarGrouping => t(L10nKey::SettingsSidebarGroupingDesc),
            L10nKey::SettingsEditorGitGutter => t(L10nKey::SettingsEditorGitGutterDesc),
            L10nKey::SettingsEditorLsp => t(L10nKey::SettingsEditorLspDesc),
            L10nKey::SettingsEditorSoftWrap => t(L10nKey::SettingsEditorSoftWrapDesc),
            L10nKey::SettingsNotifyOnCommandFinish => t(L10nKey::SettingsNotifyOnCommandFinishDesc),
            L10nKey::SettingsNotifyThreshold => t(L10nKey::SettingsNotifyThresholdDesc),
            L10nKey::SettingsAppHttpProxy => t(L10nKey::SettingsAppHttpProxyDesc),
            L10nKey::SettingsAutoDownload => t(L10nKey::SettingsAutoDownloadDesc),
            L10nKey::SettingsPerPaneHistory => t(L10nKey::SettingsPerPaneHistoryDescription),
            L10nKey::DetectUrls => t(L10nKey::SettingsDetectUrlsDesc),
            L10nKey::ForwardSshLoopbackLinks => t(L10nKey::SettingsForwardSshLoopbackLinksDesc),
            L10nKey::OpenFilesWith => t(L10nKey::SettingsOpenFilesModeDesc),
            _ => "",
        }
    }
    pub(super) fn rank(&self, query: &str) -> u8 {
        let label = t(self.title).to_lowercase();
        let key = self.config_key();
        if label == query || key == query {
            0
        } else if label.starts_with(query) || (!key.is_empty() && key.starts_with(query)) {
            1
        } else if t(self.keywords)
            .split_whitespace()
            .any(|word| word.eq_ignore_ascii_case(query))
        {
            2
        } else {
            3
        }
    }
    pub(super) fn modified(&self, cfg: &Config) -> bool {
        let defaults = Config::default();
        match self.title {
            L10nKey::SettingsDimInactivePanes => {
                cfg.dim_inactive_panes != defaults.dim_inactive_panes
            }
            L10nKey::SettingsCursorBlink => cfg.cursor_blink != defaults.cursor_blink,
            L10nKey::SettingsCursorShape => cfg.cursor_style != defaults.cursor_style,
            L10nKey::SettingsPromptCursorShape => {
                cfg.prompt_cursor_style != defaults.prompt_cursor_style
            }
            L10nKey::SettingsScrollback => cfg.scrollback_limit != defaults.scrollback_limit,
            L10nKey::SettingsNewTabPosition => cfg.new_tab_position != defaults.new_tab_position,
            L10nKey::SettingsConfirmClose => cfg.confirm_close != defaults.confirm_close,
            L10nKey::SettingsSidebarGrouping => {
                cfg.sidebar_auto_grouping != defaults.sidebar_auto_grouping
            }
            L10nKey::SettingsEditorGitGutter => cfg.editor_git_gutter != defaults.editor_git_gutter,
            L10nKey::SettingsEditorLsp => cfg.editor_lsp != defaults.editor_lsp,
            L10nKey::SettingsEditorSoftWrap => cfg.editor_soft_wrap != defaults.editor_soft_wrap,
            L10nKey::SettingsNotifyOnCommandFinish => {
                cfg.notify_on_command_finish != defaults.notify_on_command_finish
            }
            L10nKey::SettingsNotifyThreshold => {
                cfg.notify_threshold_secs != defaults.notify_threshold_secs
            }
            L10nKey::SettingsTerminalBell => cfg.bell != defaults.bell,
            L10nKey::SettingsRestoreLastLayout => cfg.restore_session != defaults.restore_session,
            L10nKey::SettingsPerPaneHistory => cfg.per_pane_history != defaults.per_pane_history,
            L10nKey::SettingsShowTrayIcon => cfg.show_tray_icon != defaults.show_tray_icon,
            L10nKey::SettingsOptionAsMeta => {
                cfg.macos_option_as_alt != defaults.macos_option_as_alt
            }
            L10nKey::SettingsHideMouseWhileTyping => {
                cfg.mouse_hide_while_typing != defaults.mouse_hide_while_typing
            }
            L10nKey::SettingsFocusFollowsMouse => {
                cfg.focus_follows_mouse != defaults.focus_follows_mouse
            }
            L10nKey::SettingsReportMouseToApps => cfg.mouse_reporting != defaults.mouse_reporting,
            L10nKey::SettingsScrollSpeed => {
                cfg.mouse_scroll_multiplier != defaults.mouse_scroll_multiplier
            }
            L10nKey::SettingsSmoothScroll => cfg.smooth_scroll != defaults.smooth_scroll,
            L10nKey::SettingsMouseZoom => cfg.mouse_zoom_modifier != defaults.mouse_zoom_modifier,
            L10nKey::SettingsTrimTrailingSpaces => {
                cfg.clipboard_trim_trailing_spaces != defaults.clipboard_trim_trailing_spaces
            }
            L10nKey::SettingsCopyOnSelect => cfg.copy_on_select != defaults.copy_on_select,
            L10nKey::SettingsSmartSelection => cfg.smart_select != defaults.smart_select,
            L10nKey::SettingsPromptEditor => cfg.prompt_editor != defaults.prompt_editor,
            L10nKey::SettingsTabCompletion => cfg.tab_completion != defaults.tab_completion,
            L10nKey::SettingsHistorySearch => cfg.history_search != defaults.history_search,
            L10nKey::SettingsStartupWindow => cfg.startup_mode != defaults.startup_mode,
            L10nKey::SettingsRememberWindowSize => {
                cfg.remember_window_size != defaults.remember_window_size
            }
            L10nKey::SettingsCheckUpdatesOnLaunch => {
                cfg.check_for_updates != defaults.check_for_updates
            }
            L10nKey::SettingsAutoDownload => {
                cfg.auto_download_updates != defaults.auto_download_updates
            }
            L10nKey::DetectUrls => cfg.link_url != defaults.link_url,
            L10nKey::ForwardSshLoopbackLinks => {
                cfg.ssh_loopback_forward != defaults.ssh_loopback_forward
            }
            L10nKey::SettingsVerifyHostKeys => cfg.verify_host_keys != defaults.verify_host_keys,
            L10nKey::WarnBeforeClosing => cfg.ssh_warn_on_close != defaults.ssh_warn_on_close,
            L10nKey::SettingsLanguage => cfg.gui_language != defaults.gui_language,
            L10nKey::SettingsFontSize => cfg.font_size != defaults.font_size,
            L10nKey::SettingsUiFontSize => cfg.ui_font_size != defaults.ui_font_size,
            L10nKey::SettingsLineHeight => cfg.line_height != defaults.line_height,
            L10nKey::SettingsFontFamily => cfg.font_family != defaults.font_family,
            L10nKey::SettingsBoldFont => cfg.font_family_bold != defaults.font_family_bold,
            L10nKey::SettingsItalicFont => cfg.font_family_italic != defaults.font_family_italic,
            L10nKey::SettingsUiFontFamily => cfg.ui_font_family != defaults.ui_font_family,
            L10nKey::SettingsOpacity => cfg.window_opacity != defaults.window_opacity,
            L10nKey::SettingsBlur => cfg.window_blur != defaults.window_blur,
            L10nKey::SettingsBackdrop => cfg.window_backdrop != defaults.window_backdrop,
            L10nKey::SettingsSyncWithSystem => {
                cfg.theme_follow_system != defaults.theme_follow_system
            }
            L10nKey::SettingsLegiblePalette => {
                cfg.theme_legible_palette != defaults.theme_legible_palette
            }
            L10nKey::SettingsThemeIntroTitle => {
                cfg.theme_preset != defaults.theme_preset
                    || cfg.theme_preset_light != defaults.theme_preset_light
                    || cfg.theme_preset_dark != defaults.theme_preset_dark
            }
            L10nKey::SettingsAppHttpProxy => cfg.http_proxy != defaults.http_proxy,
            L10nKey::SettingsOpenFilesCommand => {
                cfg.link_file_command != defaults.link_file_command
            }
            L10nKey::OpenFilesWith => cfg.link_file_open != defaults.link_file_open,
            L10nKey::SettingsSearchKeybindingsTitle => {
                cfg.keybindings != defaults.keybindings
                    || cfg.keybinding_preset != defaults.keybinding_preset
                    || cfg.prefix != defaults.prefix
            }
            L10nKey::SettingsProgram => {
                cfg.shell.as_ref().map(|s| &s.program)
                    != defaults.shell.as_ref().map(|s| &s.program)
            }
            L10nKey::SettingsArguments => cfg.shell.as_ref().is_some_and(|s| !s.args.is_empty()),
            L10nKey::SettingsStartIn => {
                cfg.working_directory.strategy != defaults.working_directory.strategy
            }
            L10nKey::SettingsCustomPath => {
                cfg.working_directory.path != defaults.working_directory.path
            }
            L10nKey::SettingsFontLigatures => cfg.font_features != defaults.font_features,
            L10nKey::SettingsFontThicken => cfg.font_thicken != defaults.font_thicken,
            _ => false,
        }
    }
}

pub(super) fn entry_matches(entry: &SearchEntry, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    query.split_whitespace().all(|word| {
        t(entry.title).to_lowercase().contains(word)
            || match entry.title {
                L10nKey::SettingsMouseZoom => {
                    "zoom modifier scroll wheel 缩放 滚轮 修饰键".contains(word)
                }
                L10nKey::SettingsPerPaneHistory => {
                    "per pane shell history independent 独立 命令历史".contains(word)
                }
                L10nKey::SettingsServer => {
                    "daemon server background service 后台 服务".contains(word)
                }
                _ => false,
            }
            || t(entry.keywords).to_lowercase().contains(word)
            || entry.description().to_lowercase().contains(word)
            || entry.config_key().contains(word)
            || crate::ui::i18n::alias_translations(entry.title)
                .iter()
                .any(|s| s.to_lowercase().contains(word))
            || crate::ui::i18n::alias_translations(entry.keywords)
                .iter()
                .any(|s| s.to_lowercase().contains(word))
    })
}

/// Whether one keybinding row answers the query.
///
/// The label is what the page shows and what someone searching for a feature
/// will type; the action name is what the docs and `keybindings.json` spell, so
/// `ScmSync` finds the row a reader arrived from the configuration page with.
pub(crate) fn keybinding_matches_query(action: &str, query: &str) -> bool {
    if query.is_empty() {
        return false;
    }
    let (_, label) = crate::ui::keymap::action_entry(action);
    label.to_lowercase().contains(query) || action.to_lowercase().contains(query)
}

/// The Keybindings page is the one page whose rows are not in the search index
/// above: there are eighty-odd of them, they are generated from the binding
/// table, and their labels are already localized there. Counting them here is
/// what puts an `(n)` on the nav item and lets `best_matching_section` land on
/// the page — without it, searching for a feature by name found the settings
/// that mention it and never the shortcut named exactly that (#444).
fn keybinding_match_count(query: &str) -> usize {
    if query.is_empty() {
        return 0;
    }
    crate::ui::keymap::default_bindings()
        .into_iter()
        .filter(|(action, _)| keybinding_matches_query(action, query))
        .count()
}

pub(crate) fn section_match_count(section: SettingsSection, query: &str) -> usize {
    let indexed = settings_search_entries()
        .iter()
        .filter(|e| e.section == section && entry_matches(e, query))
        .count();
    match section {
        SettingsSection::KeyboardMouse | SettingsSection::Keybindings => {
            indexed + keybinding_match_count(query)
        }
        _ => indexed,
    }
}

/// Whether a rendered row is one of the ones the section's `(n)` badge counted.
/// A row can match on its own label, or through the keyword list the search
/// index carries for it — "palette" finds "Theme" and nothing in that label
/// contains the word.
pub(super) fn row_matches_query(section: SettingsSection, label: &str, query: &str) -> bool {
    if query.is_empty() {
        return false;
    }
    if label.to_lowercase().contains(query) {
        return true;
    }
    settings_search_entries()
        .iter()
        .any(|e| e.section == section && t(e.title) == label && entry_matches(e, query))
}

#[cfg(test)]
pub(crate) fn total_match_count(query: &str) -> usize {
    SettingsSection::ALL
        .into_iter()
        .map(|s| section_match_count(s, query))
        .sum()
}

#[cfg(test)]
pub(crate) fn best_matching_section(query: &str) -> Option<SettingsSection> {
    settings_search_entries()
        .iter()
        .filter(|entry| entry_matches(entry, query))
        .min_by_key(|entry| entry.rank(query))
        .map(|entry| entry.section)
        .or_else(|| (keybinding_match_count(query) > 0).then_some(SettingsSection::KeyboardMouse))
}

/// The search results in the order the page draws them: grouped by page in
/// nav order, best match first within each.
pub(super) fn search_result_order(
    query: &str,
    modified_only: bool,
    cfg: &Config,
) -> Vec<&'static SearchEntry> {
    let mut matches: Vec<&SearchEntry> = settings_search_entries()
        .iter()
        .filter(|entry| entry_matches(entry, query) && (!modified_only || entry.modified(cfg)))
        .collect();
    matches.sort_by_key(|entry| {
        (
            SettingsSection::ALL
                .iter()
                .position(|&s| s == entry.section)
                .unwrap_or(0),
            entry.rank(query),
        )
    });
    matches
}

/// The shortcut actions a query names, after the settings it matched.
pub(super) fn search_result_actions(query: &str, modified_only: bool) -> Vec<String> {
    if query.is_empty() || modified_only {
        return Vec::new();
    }
    crate::ui::keymap::default_bindings()
        .into_iter()
        .filter(|(action, _)| keybinding_matches_query(action, query))
        .map(|(action, _)| action.to_string())
        .collect()
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
