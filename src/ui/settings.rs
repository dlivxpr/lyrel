use crate::ui::notification_history::Notification;
use crate::ui::notification_history::NotificationWindowExt as _;
use gpui_kit::component::InteractiveElementExt as _;
use gpui_kit::component::color_picker::{ColorPicker, ColorPickerState};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::link::Link;
use gpui_kit::component::{Icon, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, Background, Context, Div, ElementId, Entity, Focusable as _, FontWeight,
    KeyDownEvent, MouseButton, SharedString, Stateful, Subscription, Window, div, prelude::*, px,
    rgb,
};
use std::cell::{Cell, RefCell};

use uuid::Uuid;

use crate::core::config::{
    BellMode, Config, ConfirmClose, CursorStyle, LinkFileOpen, MouseZoomModifier, NewTabPosition,
    NotifyMode, PromptCursorStyle, UI_FONT_SIZE_DEFAULT, WindowBackdrop,
};
use crate::ui::app::{FONT_SIZE_STEP, LINE_HEIGHT_STEP, UI_FONT_SIZE_STEP};
use crate::ui::host_ops::HostId;
use crate::ui::i18n::{L10nKey, t, t_fmt, t_plural};
use crate::ui::presets;
use crate::ui::settings_window::{SettingsEvent, SettingsWindow};
use crate::ui::window_chrome::TITLE_BAR_HEIGHT;
#[cfg(target_os = "windows")]
pub(crate) use theme_edit::window_backdrop_label_key;
pub(crate) use theme_edit::{ThemeEdit, theme_edit_label};

mod agents;
pub(crate) mod arguments;
mod configuration;
mod editor;
mod ssh;
mod inputs;
pub(crate) mod kit;
mod pages;
mod shell;
mod shortcuts;
pub(crate) mod skills;
mod state;
pub(crate) mod statistics;
mod theme_edit;
mod theme_picker;
pub(crate) mod usage;

#[cfg(test)]
pub(crate) mod test_window;

/// The settings nav, and the page's padding either side of its column.
const NAV_W: f32 = 220.;
const PAGE_PAD: f32 = 112.;

/// The narrowest the nav is still itself: an item that still shows its label
/// beside its icon.
///
/// Sized for both supported locales: at 140 the zh-CN "窗口与标签页"
/// lost the right half of its last character.
const NAV_W_MIN: f32 = 176.;

/// The reading column every page caps itself at. Wider than this and a
/// description stops being a paragraph and becomes a line to scan across.
const READING_COLUMN: f32 = 620.;

/// How far the content scrollbar stays clear of the top and bottom of the
/// window, so it does not run into the window's rounded corner.
const SCROLLBAR_WINDOW_INSET: f32 = 12.;

/// What the page gets before the nav gives anything back: a row's widest
/// control with a description beside it that still reads as a paragraph.
const CONTENT_W: f32 = 420.;

/// How wide the nav is at this window width: its full width while the page
/// beside it still gets `CONTENT_W`, then shrinking toward its floor.
fn nav_width(viewport: f32, scale: f32) -> f32 {
    let full = NAV_W * scale.max(1.);
    let floor = NAV_W_MIN * scale.max(1.);
    let short = (full + CONTENT_W + PAGE_PAD - viewport).max(0.);
    (full - short).clamp(floor, full).round()
}

/// What a row on the page really has to lay out in.
fn settings_row_width(viewport: f32, scale: f32) -> f32 {
    (viewport - nav_width(viewport, scale) - PAGE_PAD).clamp(0., READING_COLUMN * scale)
}

/// How much wider every piece of text on this page is than the px thresholds
/// below assume.
///
/// Those thresholds are widths a *label* needs, measured at the default
/// interface font. The interface has a font size of its own and it goes up to
/// 24 — half as wide again — while a slider or a text field beside that label
/// stays the px width it was built at. Without this the row that has to stack
/// first is the one that never does.
fn ui_scale(cx: &App) -> f32 {
    cx.global::<Config>().ui_font_size / UI_FONT_SIZE_DEFAULT
}

/// How wide a text field in the right-hand column is — the dropdowns' width,
/// so the column is one straight edge whichever control a row holds.
const FIELD_W: f32 = crate::ui::settings::kit::CONTROL_W;

/// A field for a path or a command line: the one kind of value unreadable cut
/// short, so the one kind allowed past [`FIELD_W`].
const PATH_FIELD_W: f32 = 240.;

/// The host editor's label column, and how wide a field beside one grows to.
const SSH_LABEL_W: f32 = 96.;
const FORM_FIELD_W: f32 = 300.;

/// Width a settings row needs before its label and its control fit side by
/// side: a wide control, the gap between them, and enough left for a
/// description to read as prose rather than as a column of words.
const STACK_ROW_BELOW: f32 = 500.;

/// The scrollback presets, and the labels their cells carry, in draw order.
/// One list each so the number a cell writes is the number it shows —
/// `preset_row_labels_name_the_value_they_write` holds the two together.
const SCROLLBACK_BUCKETS: [usize; 3] = [1_000, 10_000, 100_000];
const SCROLLBACK_LABELS: [&str; 3] = ["1,000", "10,000", "100,000"];

/// The notify-threshold presets. The last one is drawn in minutes, which is
/// why these labels are written out rather than derived.
const NOTIFY_THRESHOLD_BUCKETS: [u64; 4] = [5, 10, 30, 60];
const NOTIFY_THRESHOLD_LABELS: [&str; 4] = ["5s", "10s", "30s", "1m"];

/// Which preset a live value *is*, and — when it is none of them — the label
/// for the trailing cell that names it.
///
/// The match is exact on purpose. Matching a *range* is what made
/// `scrollback_limit: 5000` light up "10,000" and `notify_threshold_secs: 20`
/// light up "30s", with no digits anywhere on the row to correct the
/// impression, and clicking the cell that was wrongly lit overwrote the real
/// value with the bucket's (#550).
fn preset_choice<T: Copy + PartialEq>(
    buckets: &[T],
    value: T,
    name: impl FnOnce(T) -> String,
) -> (Option<usize>, Option<String>) {
    match buckets.iter().position(|&b| b == value) {
        Some(ix) => (Some(ix), None),
        None => (
            None,
            Some(t_fmt(
                L10nKey::SettingsCustomValue,
                &[("value", &name(value))],
            )),
        ),
    }
}

/// `50000` beside cells reading `10,000` and `100,000` looks like a different
/// kind of number, so the custom cell groups its digits the way the presets
/// next to it are written. Every locale lyrel ships writes these counts the
/// same way — the preset labels themselves are one set of literals for all
/// three.
fn group_thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.char_indices() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn settings_row_id(label: &str, _desc: &str) -> SharedString {
    let id = settings_search_entries()
        .iter()
        .find(|entry| t(entry.title) == label)
        .map(|entry| format!("{:?}", entry.title))
        .or_else(|| {
            L10nKey::ALL
                .iter()
                .find(|&&key| t(key) == label)
                .map(|key| format!("{key:?}"))
        })
        .unwrap_or_else(|| label.to_string());
    SharedString::from(format!("settings-row-{id}"))
}

fn settings_header_id(title: &str) -> SharedString {
    let id = L10nKey::ALL
        .iter()
        .find(|&&key| t(key) == title)
        .map(|key| format!("{key:?}"))
        .unwrap_or_else(|| title.to_string());
    SharedString::from(format!("settings-header-{id}"))
}

/// Whether the reset control has any effective override to clear on this
/// platform. A synchronized Windows backdrop remains stored elsewhere but is
/// inert here, so only platforms that expose it locally may count it.
fn window_overrides_active(config: &Config, backdrop_is_local: bool) -> bool {
    config.window_opacity.is_some()
        || config.window_blur.is_some()
        || (backdrop_is_local && config.window_backdrop != WindowBackdrop::Auto)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsSection {
    General,
    Appearance,
    Terminal,
    KeyboardMouse,
    Ssh,
    Agents,
    Skills,
    Statistics,
    Keybindings,
    Usage,
    About,
}

impl SettingsSection {
    pub(crate) const ALL: [SettingsSection; 10] = [
        SettingsSection::General,
        SettingsSection::Appearance,
        SettingsSection::Terminal,
        SettingsSection::KeyboardMouse,
        SettingsSection::Ssh,
        SettingsSection::Agents,
        SettingsSection::Skills,
        SettingsSection::Statistics,
        SettingsSection::Usage,
        SettingsSection::About,
    ];

    pub(crate) fn navigation_section(self) -> Self {
        match self {
            Self::Keybindings => Self::KeyboardMouse,
            other => other,
        }
    }

    fn title(self) -> L10nKey {
        match self {
            Self::General => L10nKey::SettingsNavGeneral,
            Self::Appearance => L10nKey::SettingsNavAppearance,
            Self::Terminal => L10nKey::SettingsNavTerminal,
            Self::KeyboardMouse => L10nKey::SettingsNavInput,
            Self::Ssh => L10nKey::SettingsNavSsh,
            Self::Agents => L10nKey::SettingsNavAgents,
            Self::Skills => L10nKey::SettingsNavSkills,
            Self::Statistics => L10nKey::StatisticsTitle,
            Self::Usage => L10nKey::SettingsNavUsage,
            Self::Keybindings => L10nKey::SettingsNavKeybindings,
            Self::About => L10nKey::SettingsNavAbout,
        }
    }

    fn icon_path(self) -> &'static str {
        match self {
            Self::General => "icons/settings/general.svg",
            Self::Appearance => "icons/settings/appearance.svg",
            Self::Terminal => "icons/settings/terminal.svg",
            Self::KeyboardMouse | Self::Keybindings => "icons/settings/keyboard.svg",
            Self::Ssh => "icons/settings/ssh.svg",
            Self::Agents => "icons/settings/integrations.svg",
            Self::Skills => "icons/settings/integrations.svg",
            Self::Statistics => "icons/settings/integrations.svg",
            Self::Usage => "icons/settings/integrations.svg",
            Self::About => "icons/settings/about.svg",
        }
    }

    fn profile_label(self) -> &'static str {
        match self {
            SettingsSection::General => "settings:general",
            SettingsSection::Appearance => "settings:appearance",
            SettingsSection::Terminal => "settings:terminal",
            SettingsSection::KeyboardMouse => "settings:keyboard-mouse",
            SettingsSection::Ssh => "settings:ssh",
            SettingsSection::Agents => "settings:agents",
            SettingsSection::Skills => "settings:skills",
            SettingsSection::Statistics => "settings:statistics",
            SettingsSection::Usage => "settings:usage",
            SettingsSection::Keybindings => "settings:keybindings",
            SettingsSection::About => "settings:about",
        }
    }
}

struct SearchEntry {
    section: SettingsSection,
    title: L10nKey,
    keywords: L10nKey,
}

use crate::core::update::{localized_update_install_hint, localized_update_phase};

fn settings_search_entries() -> &'static [SearchEntry] {
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
            title: SettingsEditorMarkdownPreview,
            keywords: SettingsSearchEditorMarkdownPreviewKeywords,
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

/// The cursor shapes' labels: block, bar, underline.
pub(crate) fn cursor_shape_labels() -> Vec<String> {
    [
        L10nKey::CursorShapeBlock,
        L10nKey::CursorShapeBar,
        L10nKey::CursorShapeUnderline,
    ]
    .into_iter()
    .map(|key| t(key).to_string())
    .collect()
}

/// The prompt cursor shape choices' labels, in `PROMPT_CURSOR_SHAPES` order:
/// follow, then the shapes.
pub(crate) fn prompt_cursor_shape_labels() -> Vec<String> {
    let mut rows = vec![t(L10nKey::PromptCursorShapeFollow).to_string()];
    rows.extend(cursor_shape_labels());
    rows
}

pub(crate) const PROMPT_CURSOR_SHAPES: [PromptCursorStyle; 4] = [
    PromptCursorStyle::Follow,
    PromptCursorStyle::Block,
    PromptCursorStyle::Bar,
    PromptCursorStyle::Underline,
];

impl SearchEntry {
    fn config_key(&self) -> &'static str {
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
            L10nKey::SettingsEditorMarkdownPreview => "editor_markdown_preview",
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
    fn description(&self) -> &'static str {
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
            L10nKey::SettingsEditorMarkdownPreview => t(L10nKey::SettingsEditorMarkdownPreviewDesc),
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
    fn rank(&self, query: &str) -> u8 {
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
    fn modified(&self, cfg: &Config) -> bool {
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
            L10nKey::SettingsEditorMarkdownPreview => {
                cfg.editor_markdown_preview != defaults.editor_markdown_preview
            }
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

fn entry_matches(entry: &SearchEntry, query: &str) -> bool {
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
fn row_matches_query(section: SettingsSection, label: &str, query: &str) -> bool {
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


pub(crate) struct SettingsState {
    pub(crate) focus_handle: gpui_kit::FocusHandle,
    pub(crate) section: SettingsSection,
    pub(crate) usage: Entity<usage::UsagePage>,
    pub(crate) search: Entity<InputState>,
    pub(crate) skills: skills::SkillsState,
    pub(crate) shortcut_search: Entity<InputState>,
    pub(crate) modified_only: bool,
    pub(crate) save_error: Option<String>,
    pub(crate) saved_config: Config,
    pub(crate) search_active: bool,
    pub(crate) search_return_offset: gpui_kit::Point<gpui_kit::Pixels>,
    pub(crate) search_selection: usize,
    pub(crate) search_rows: RefCell<Option<Vec<(L10nKey, AnyElement)>>>,
    pub(crate) focused_setting: Option<L10nKey>,
    /// The page's own scroll, and an anchor on it that the first matching row
    /// claims. Searching tells you "Appearance (2)"; these are what carry you
    /// to the two, which on a long page start well below the fold.
    pub(crate) content_scroll: gpui_kit::ScrollHandle,
    pub(crate) search_anchor: gpui_kit::ScrollAnchor,
    /// Set when the query or the section changes, and spent by the next render
    /// that has somewhere to go. A `Cell` because that render only holds `&self`.
    pub(crate) reveal_first_hit: Cell<bool>,
    /// The families the font menus list, read once when the page opens.
    pub(crate) font_names: std::rc::Rc<Vec<String>>,
    pub(crate) shell_program_input: Entity<InputState>,
    pub(crate) shell_args_input: Entity<InputState>,
    pub(crate) wd_path_input: Entity<InputState>,
    pub(crate) link_file_command_input: Entity<InputState>,
    pub(crate) http_proxy_input: Entity<InputState>,
    pub(crate) theme: theme_edit::ThemeEditing,
    /// The one popover open on the page — a dropdown, a theme list, a row's
    /// menu — by the id its trigger was drawn with.
    pub(crate) menu: Option<SharedString>,
    /// The query box inside a popover that has one, and the row the arrow
    /// keys have reached in it.
    pub(crate) menu_query: Entity<InputState>,
    pub(crate) menu_hi: usize,
    pub(crate) menu_scroll: gpui_kit::ScrollHandle,
    /// Whatever held keyboard focus when the page was last drawn, so a text
    /// field can draw its focus ring from a builder that has no `Window`.
    pub(crate) focused: RefCell<Option<gpui_kit::FocusHandle>>,
    /// A recorded shortcut that another action already answers to, held until
    /// the user says whether to take it over.
    pub(crate) kb_conflict: Option<KeyConflict>,
    pub(crate) recording: Option<Recording>,
    pub(crate) rebinding_note: Option<String>,
    pub(crate) ssh: ssh::SshSettings,
    pub(crate) agent_hooks_host: HostId,
    pub(crate) agent_hooks_states: AgentHooksView,
    pub(crate) agent_hooks_seq: u64,
    pub(crate) agent_hooks_note: Option<(crate::core::agent_hooks::HookAgent, String)>,
    pub(crate) agent_query: Entity<InputState>,
    /// Every agent, not just the ones installed on the selected machine.
    pub(crate) agent_show_all: bool,
    /// Agents acted on since the list was last narrowed, kept in view so a row
    /// does not vanish under the pointer the moment it is uninstalled.
    pub(crate) agent_touched: std::collections::HashSet<crate::core::agent_hooks::HookAgent>,
    /// Agents with an install or removal in flight.
    pub(crate) agent_busy: std::collections::HashSet<crate::core::agent_hooks::HookAgent>,
    pub(crate) statistics: statistics::StatisticsState,
    pub(crate) _subs: Vec<Subscription>,
}

/// A recorded chord and the action it would be taken from.
pub(crate) struct KeyConflict {
    pub(crate) action: String,
    pub(crate) spec: String,
    pub(crate) other: String,
}

#[derive(Clone)]
pub(crate) enum AgentHooksView {
    Loading,
    Ready(Vec<AgentHookRow>),
    Unavailable(String),
}

#[derive(Clone)]
pub(crate) struct AgentHookRow {
    pub(crate) agent: crate::core::agent_hooks::HookAgent,
    pub(crate) state: crate::core::agent_hooks::HooksState,
    pub(crate) target: String,
    pub(crate) diagnostic: Option<crate::core::agent_hooks::HooksDiagnostic>,
}

#[derive(Clone)]
pub(crate) struct AgentHooksMachine {
    pub(crate) host: HostId,
    pub(crate) label: String,
}

/// The Appearance page's three ways of choosing a theme.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ThemeMode {
    Light,
    Dark,
    System,
}


pub(crate) struct Recording {
    pub(crate) action: String,
    pub(crate) chords: Vec<String>,
    pub(crate) _intercept: Subscription,
}

pub(crate) fn font_default_label() -> &'static str {
    t(L10nKey::SettingsFontDefault)
}

/// The same first row for the interface face, spelled for what it actually
/// does. The bold and italic dropdowns fall back to the *terminal's* primary
/// family, which is what their label promises; the interface falls back to the
/// system UI font instead, so it cannot borrow that label without telling the
/// reader the chrome will come out in Hack.
pub(crate) fn ui_font_default_label() -> &'static str {
    t(L10nKey::SettingsUiFontDefault)
}

#[cfg(target_os = "macos")]
const LINK_MODIFIER_LABEL: &str = "⌘";
#[cfg(not(target_os = "macos"))]
const LINK_MODIFIER_LABEL: &str = "Ctrl";

pub(crate) fn humanize_action(action: &str) -> String {
    let mut out = String::new();
    for (i, ch) in action.chars().enumerate() {
        if i > 0 && ch.is_uppercase() {
            out.push(' ');
        }
        out.push(ch);
    }
    out
}


/// A picked path written the way a config file spells it. `~/.ssh/id_ed25519`
/// keeps meaning the right file on another machine, or after the account is
/// renamed; the absolute path the system picker hands back does not.
fn tildify(path: &str) -> String {
    #[cfg(windows)]
    let home = std::env::var("USERPROFILE").ok();
    #[cfg(not(windows))]
    let home = std::env::var("HOME").ok();
    tildify_with(path, home.as_deref().filter(|h| !h.is_empty()))
}

fn tildify_with(path: &str, home: Option<&str>) -> String {
    let Some(home) = home else {
        return path.to_string();
    };
    let home = home.trim_end_matches(['/', '\\']);
    match path.strip_prefix(home) {
        // A separator has to follow, or `/Users/adalovelace` would come back
        // as a file inside `/Users/ada`.
        Some(rest) if rest.starts_with('/') || rest.starts_with('\\') => format!(
            "~/{}",
            rest.trim_start_matches(['/', '\\']).replace('\\', "/")
        ),
        _ => path.to_string(),
    }
}


impl SettingsWindow {
    pub(crate) fn with_settings_edits_resolved(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        action: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let save_failed = self
            .active_settings()
            .is_some_and(|s| s.save_error.is_some());
        if !save_failed && !self.ssh_form_dirty(cx) && !self.theme_draft_dirty() {
            action(self, window, cx);
            return;
        }
        let answer = window.prompt::<gpui_kit::PromptButton>(
            gpui_kit::PromptLevel::Warning,
            t(L10nKey::SettingsUnsavedTitle),
            Some(t(L10nKey::SettingsUnsavedBody)),
            &[
                gpui_kit::PromptButton::ok(t(L10nKey::SettingsSaveChanges)),
                gpui_kit::PromptButton::new(t(L10nKey::EditorDiscard)),
                gpui_kit::PromptButton::cancel(t(L10nKey::SettingsKeepEditing)),
            ],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(choice) = answer.await else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                match choice {
                    0 => {
                        if this
                            .active_settings()
                            .is_some_and(|s| s.save_error.is_some())
                        {
                            this.persist_settings_config(cx);
                            if this
                                .active_settings()
                                .is_some_and(|s| s.save_error.is_some())
                            {
                                return;
                            }
                        }
                        if this.ssh_form_dirty(cx)
                            && this.save_editing_profile(window, cx).is_none()
                        {
                            return;
                        }
                        if !this.save_theme_draft(window, cx) {
                            return;
                        }
                    }
                    1 => {
                        this.discard_unsaved_settings(window, cx);
                        this.cancel_theme_draft(window, cx);
                        if let Some(s) = this.active_settings_mut() {
                            s.ssh.cancel();
                        }
                    }
                    _ => return,
                }
                action(this, window, cx);
            });
        })
        .detach();
    }

    pub(crate) fn navigate_settings(
        &mut self,
        target: SettingsSection,
        setting: Option<L10nKey>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.ssh_form_dirty(cx)
            || self.theme_draft_dirty()
            || self
                .active_settings()
                .is_some_and(|s| s.save_error.is_some())
        {
            self.with_settings_edits_resolved(window, cx, move |this, window, cx| {
                this.navigate_settings(target, setting, window, cx)
            });
            return;
        }
        if let Some(state) = self.active_settings() {
            state
                .search
                .clone()
                .update(cx, |search, cx| search.set_value("", window, cx));
        }
        let setting = match setting {
            Some(L10nKey::SettingsCustomPath)
                if cx.global::<Config>().working_directory.strategy
                    != crate::core::config::WdStrategy::Custom =>
            {
                Some(L10nKey::SettingsStartIn)
            }
            Some(L10nKey::SettingsOpenFilesCommand)
                if cx.global::<Config>().file_open_mode() != LinkFileOpen::Command =>
            {
                Some(L10nKey::OpenFilesWith)
            }
            other => other,
        };
        if let Some(state) = self.active_settings_mut() {
            state.modified_only = false;
            state.search_active = false;
            state.focused_setting = setting;
            state.reveal_first_hit.set(setting.is_some());
            state
                .content_scroll
                .set_offset(gpui_kit::point(px(0.), px(0.)));
            state.menu = None;
        }
        self.select_settings_section(target, cx);
    }

    /// Whether the row measured this render came out narrower than a threshold
    /// quoted at the default interface font — the only way those px thresholds
    /// mean anything to a reader who scaled the interface up.
    fn settings_row_under(&self, at_default_font: f32, cx: &App) -> bool {
        self.settings_row_width.get() < at_default_font * ui_scale(cx)
    }

    /// The scroll anchor for the first thing on the page the query matched,
    /// whatever kind of element that is. A section header can be the only
    /// match on its page — "ansi", "how shells work" — and while the dimming
    /// around it already picks it out, nothing was carrying the page to it:
    /// search "ansi" from the bottom of Appearance and every row greys out
    /// with the one answer left above the fold.
    fn first_hit_anchor(&self, label: &str, cx: &Context<Self>) -> Option<gpui_kit::ScrollAnchor> {
        let s = self.active_settings()?;
        if s.search_active || s.search_rows.borrow().is_some() {
            return None;
        }
        if let Some(target) = s.focused_setting {
            if t(target) == label && !self.settings_hit_anchored.replace(true) {
                return Some(s.search_anchor.clone());
            }
            return None;
        }
        let query = s.search.read(cx).value().trim().to_lowercase();
        if query.is_empty() || section_match_count(s.section, &query) == 0 {
            return None;
        }
        if !row_matches_query(s.section, label, &query) {
            return None;
        }
        match self.settings_hit_anchored.replace(true) {
            false => Some(s.search_anchor.clone()),
            true => None,
        }
    }


    /// Closing from Escape or the X is the user leaving; every other caller
    /// closes as the tail of something they explicitly chose, and has already
    /// saved or does not care.
    pub(crate) fn close_settings_checked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.with_settings_edits_resolved(window, cx, |this, window, cx| {
            this.close_settings(window, cx)
        });
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_has_unique_titles_and_default_values_are_unmodified() {
        let defaults = Config::default();
        for (i, entry) in settings_search_entries().iter().enumerate() {
            assert!(
                !settings_search_entries()[..i]
                    .iter()
                    .any(|e| e.title == entry.title),
                "duplicate {:?}",
                entry.title
            );
            assert!(
                !entry.modified(&defaults),
                "default {:?} is modified",
                entry.title
            );
            assert!(SettingsSection::ALL.contains(&entry.section));
        }
        assert!(!SettingsSection::ALL.contains(&SettingsSection::Keybindings));
    }

    #[test]
    fn config_keys_and_cross_language_names_reach_the_same_setting() {
        for locale in ["en", "zh-CN"] {
            crate::ui::i18n::set_locale(locale);
            for (query, title, section) in [
                (
                    "gui_language",
                    L10nKey::SettingsLanguage,
                    SettingsSection::General,
                ),
                (
                    "mouse_zoom_modifier",
                    L10nKey::SettingsMouseZoom,
                    SettingsSection::KeyboardMouse,
                ),
                (
                    "per_pane_history",
                    L10nKey::SettingsPerPaneHistory,
                    SettingsSection::Terminal,
                ),
                (
                    "ui_font_size",
                    L10nKey::SettingsUiFontSize,
                    SettingsSection::Appearance,
                ),
                (
                    "Shell program",
                    L10nKey::SettingsProgram,
                    SettingsSection::Terminal,
                ),
            ] {
                let entry = settings_search_entries()
                    .iter()
                    .find(|e| e.title == title)
                    .unwrap();
                assert!(entry_matches(entry, query), "{locale}: {query}");
                assert_eq!(
                    best_matching_section(query).unwrap().profile_label(),
                    section.profile_label()
                );
            }
        }
        crate::ui::i18n::set_locale("en");
    }

    #[test]
    fn language_search_keeps_system_mode_and_drops_retired_language_aliases() {
        let entry = settings_search_entries()
            .iter()
            .find(|entry| entry.title == L10nKey::SettingsLanguage)
            .unwrap();
        for locale in ["en", "zh-CN"] {
            crate::ui::i18n::set_locale(locale);
            for query in ["follow system", "跟随系统", "english", "chinese"] {
                assert!(entry_matches(entry, query), "{locale}: {query}");
            }
            for query in ["日本語", "Русский", "japanese", "russian"] {
                assert!(!entry_matches(entry, query), "retired alias: {query}");
            }
        }
    }

    #[test]
    fn settings_row_ids_survive_language_changes() {
        for key in [
            L10nKey::SettingsFontSize,
            L10nKey::SettingsLanguage,
            L10nKey::SettingsMouseZoom,
        ] {
            crate::ui::i18n::set_locale("en");
            let expected = settings_row_id(t(key), "");
            for locale in ["zh-CN"] {
                crate::ui::i18n::set_locale(locale);
                assert_eq!(settings_row_id(t(key), ""), expected);
            }
        }
        crate::ui::i18n::set_locale("en");
    }

    #[test]
    fn modified_settings_compare_their_own_values_only() {
        let mut cfg = Config::default();
        cfg.notify_threshold_secs += 1;
        let changed = settings_search_entries()
            .iter()
            .filter(|e| e.modified(&cfg))
            .map(|e| e.title)
            .collect::<Vec<_>>();
        assert_eq!(changed, vec![L10nKey::SettingsNotifyThreshold]);
    }

    /// A shortcut is the first thing someone searching a settings window for a
    /// feature by name is after, and the Keybindings page was the one page the
    /// search could not see into: searching "split" found the settings that
    /// merely mention splits and never the row labelled exactly that (#444).
    #[test]
    fn searching_for_a_feature_finds_its_shortcut() {
        crate::ui::i18n::set_locale("en");
        let kb = SettingsSection::Keybindings;

        // Split Right and Split Down, at least — the index carries no entry
        // for either, so before this every one of these counts was zero.
        assert!(
            section_match_count(kb, "split") >= 2,
            "got {}",
            section_match_count(kb, "split")
        );
        assert!(keybinding_matches_query("SplitRight", "split right"));

        // The action name is what the docs and `keybindings.json` spell, so a
        // reader arriving from either finds the row they read about.
        assert!(keybinding_matches_query("ScmSync", "scmsync"));

        // An empty query matches no row, or clearing the box would filter the
        // page down to nothing rather than back to every row. (The count above
        // it answers `contains("")` for the one indexed entry this section has
        // always carried, which is why the page gates on the query itself
        // before it consults either.)
        assert!(!keybinding_matches_query("SplitRight", ""));
        assert_eq!(keybinding_match_count(""), 0);

        // A query this page cannot answer leaves it alone — the page only
        // filters itself when it has something to show.
        assert_eq!(section_match_count(kb, "no such action anywhere"), 0);
    }


    /// A key picked from the system dialog arrives as an absolute path under
    /// the home directory. Written back that way it names the right file on
    /// this machine and the wrong one everywhere else — and `~` is how the
    /// rest of the field, and `~/.ssh/config` itself, spells it.
    #[test]
    fn a_picked_key_is_written_the_way_the_config_spells_it() {
        let home = Some("/Users/ada");
        assert_eq!(
            tildify_with("/Users/ada/.ssh/id_ed25519", home),
            "~/.ssh/id_ed25519"
        );
        // Outside the home directory there is nothing to shorten.
        assert_eq!(tildify_with("/etc/ssh/key", home), "/etc/ssh/key");
        // And a sibling that merely starts with the same letters is not
        // inside it.
        assert_eq!(
            tildify_with("/Users/adalovelace/key", home),
            "/Users/adalovelace/key"
        );
        // A trailing separator on the home directory changes nothing.
        assert_eq!(
            tildify_with("/Users/ada/.ssh/id_rsa", Some("/Users/ada/")),
            "~/.ssh/id_rsa"
        );
        // Nowhere to anchor against leaves the path as it came.
        assert_eq!(
            tildify_with("/Users/ada/.ssh/id_rsa", None),
            "/Users/ada/.ssh/id_rsa"
        );
    }


    #[test]
    fn a_row_stacks_once_its_label_and_control_stop_fitting() {
        assert!(settings_row_width(1440., 1.) >= STACK_ROW_BELOW);
        assert!(settings_row_width(900., 1.) >= STACK_ROW_BELOW);
        // At the narrowest window that turns up the nav gives back what it can
        // and the page is still under the width a label and a control share.
        assert_eq!(nav_width(640., 1.), NAV_W_MIN);
        assert!(settings_row_width(640., 1.) < STACK_ROW_BELOW);
        // Capped at the reading column, so a wider window never widens the row.
        assert_eq!(settings_row_width(4000., 1.), READING_COLUMN);
        // And never goes negative on a window narrower than its own chrome.
        assert_eq!(settings_row_width(100., 1.), 0.);
    }

    /// A preset row lights up the bucket the value *is*, and nothing when it
    /// is none of them — the range match it used to do labelled a hand-set
    /// value with a number the config did not hold, and the row carried no
    /// digits anywhere to correct it (#550).
    #[test]
    fn a_preset_row_highlights_only_the_bucket_the_value_actually_is() {
        // The default lands on a bucket, so the common case still reads as a
        // plain radio row.
        let (sel, custom) = preset_choice(
            &SCROLLBACK_BUCKETS,
            Config::default().scrollback_limit,
            group_thousands,
        );
        assert_eq!((sel, custom), (Some(1), None));

        // 50,000 is the value `docs/reference/configuration.mdx` puts in its
        // example config, so this is what following the documentation shows.
        let (sel, custom) = preset_choice(&SCROLLBACK_BUCKETS, 50_000, group_thousands);
        assert_eq!(sel, None, "50,000 is not one of the presets");
        let custom = custom.expect("a value off the presets names itself");
        assert!(
            custom.contains("50,000"),
            "the custom cell has to carry the real value, got {custom:?}"
        );

        // Boundaries: the old range match lit "10,000" for everything from
        // 1,001 up, and "100,000" for everything above that.
        assert_eq!(
            preset_choice(&SCROLLBACK_BUCKETS, 1_001, group_thousands).0,
            None
        );
        assert_eq!(
            preset_choice(&SCROLLBACK_BUCKETS, 100_000, group_thousands).0,
            Some(2)
        );

        // Same rule on the notify row, where 20s used to light up "30s".
        let (sel, custom) = preset_choice(&NOTIFY_THRESHOLD_BUCKETS, 20, |secs| format!("{secs}s"));
        assert_eq!(sel, None);
        assert!(custom.is_some_and(|c| c.contains("20s")));
        assert_eq!(
            preset_choice(&NOTIFY_THRESHOLD_BUCKETS, 60, |secs| format!("{secs}s")).0,
            Some(3),
            "60s is the '1m' cell, not a custom value"
        );
    }

    /// Each preset cell has to name the number clicking it writes, and the
    /// custom cell has to be written the same way as the cells beside it.
    #[test]
    fn preset_row_labels_name_the_value_they_write() {
        assert_eq!(SCROLLBACK_BUCKETS.len(), SCROLLBACK_LABELS.len());
        for (bucket, label) in SCROLLBACK_BUCKETS.iter().zip(SCROLLBACK_LABELS) {
            assert_eq!(group_thousands(*bucket), label);
        }
        assert_eq!(
            NOTIFY_THRESHOLD_BUCKETS.len(),
            NOTIFY_THRESHOLD_LABELS.len()
        );
        // Grouping starts at four digits and repeats every three.
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(999), "999");
        assert_eq!(group_thousands(1_000_000), "1,000,000");
    }

    /// The thresholds are widths a *label* needs, and a reader who scaled the
    /// interface up scaled every label with it while the slider beside it kept
    /// the px width it was built at. A window that reads fine at the default
    /// font is a starved label column at the largest one.
    #[test]
    fn the_stacking_width_follows_the_interface_font() {
        use crate::core::config::UI_FONT_SIZE_MAX;
        let large = UI_FONT_SIZE_MAX / UI_FONT_SIZE_DEFAULT;
        // Side by side at the default font...
        assert!(settings_row_width(900., 1.) >= STACK_ROW_BELOW);
        // ...and stacked at the largest, where the same row holds half as much.
        assert!(
            settings_row_width(900., large) < STACK_ROW_BELOW * large,
            "a 900pt window at the largest interface font has to stack"
        );
        // The nav keeps its labels readable at the larger font.
        assert!(nav_width(1440., large) > NAV_W);
        // The reading column grows with the font, so a wide window does not.
        assert!(settings_row_width(1600., large) >= STACK_ROW_BELOW * large);
    }

    #[test]
    fn a_row_is_marked_by_its_label_or_by_the_keywords_behind_it() {
        // Straight label hit.
        assert!(row_matches_query(
            SettingsSection::Appearance,
            "Blur",
            "blur"
        ));
        // Keyword hit: the label says "Theme" and nothing more, but the index
        // says that row answers "palette".
        assert!(row_matches_query(
            SettingsSection::Appearance,
            t(L10nKey::SettingsThemeIntroTitle),
            "palette"
        ));
        // A row on some other page is not a hit just because the query matches
        // an entry elsewhere.
        assert!(!row_matches_query(
            SettingsSection::Terminal,
            "Blur",
            "palette"
        ));
        // An empty query marks nothing at all, so no page ever renders greyed
        // out just because the field is focused.
        assert!(!row_matches_query(SettingsSection::Appearance, "Blur", ""));
    }

    #[test]
    fn a_query_that_matches_nothing_is_distinguishable_from_one_that_does() {
        assert_eq!(total_match_count("zzqqxx"), 0);
        assert!(total_match_count("blur") > 0);
        assert!(total_match_count("palette") > 0);
    }

    #[test]
    fn settings_row_identity_depends_only_on_its_stable_label() {
        assert_eq!(
            settings_row_id("Claude Code", "Installingâ€¦"),
            settings_row_id("Claude Code", "Installed in C:\\tools")
        );
        assert_ne!(
            settings_row_id("Claude Code", "Installed"),
            settings_row_id("Codex", "Installed")
        );
    }

    #[test]
    fn synced_windows_backdrop_is_only_a_local_override_on_windows() {
        let mut config = Config::default();
        config.window_backdrop = WindowBackdrop::MicaAlt;

        assert!(window_overrides_active(&config, true));
        assert!(!window_overrides_active(&config, false));
    }

    #[test]
    fn opacity_and_blur_are_local_overrides_on_every_platform() {
        let mut opacity = Config::default();
        opacity.window_opacity = Some(0.8);
        opacity.window_backdrop = WindowBackdrop::Mica;
        let mut blur = Config::default();
        blur.window_blur = Some(true);
        blur.window_backdrop = WindowBackdrop::Acrylic;

        assert!(window_overrides_active(&opacity, false));
        assert!(window_overrides_active(&blur, false));
    }

    #[test]
    fn every_section_has_search_entries() {
        for section in SettingsSection::ALL {
            let n = settings_search_entries()
                .iter()
                .filter(|e| e.section == section)
                .count();
            assert!(
                n > 0,
                "section {:?} has no search entries",
                section.profile_label()
            );
        }
    }

    #[test]
    fn best_matching_section_can_reach_every_section() {
        for section in SettingsSection::ALL {
            let entry = settings_search_entries()
                .iter()
                .find(|e| e.section == section)
                .expect("checked by every_section_has_search_entries");
            let query = t(entry.title).to_lowercase();
            let landed = best_matching_section(&query);
            assert!(
                landed.is_some(),
                "query {query:?} matched nothing at all (section {:?})",
                section.profile_label()
            );
        }
    }

    #[test]
    fn previously_unsearchable_settings_are_findable() {
        use SettingsSection::*;
        let cases = [
            ("opacity", Appearance),
            ("blur", Appearance),
            ("completion", Terminal),
            ("ctrl-r", Terminal),
            ("grouping", General),
            ("threshold", General),
            ("report mouse", KeyboardMouse),
            ("nushell", Terminal),
            ("open files with", Terminal),
            ("bell", Terminal),
            ("known_hosts", Ssh),
            ("symlink", Agents),
            // Rows the index had no entry for at all, so the query counted
            // nothing, no badge appeared and no row lit up: the whole Updates
            // group, and Smooth scrolling between two rows that were both
            // findable.
            ("smooth", Terminal),
            ("metered", About),
            ("automatic", About),
            // A headline feature the index had never heard of: "background
            // image" matched nothing, and typing it walked the page to About
            // because "background" alone hits Download updates in the
            // background.
            ("background image", Appearance),
            ("wallpaper", Appearance),
            ("image opacity", Appearance),
            #[cfg(target_os = "windows")]
            ("material", Appearance),
            #[cfg(target_os = "windows")]
            ("mica", Appearance),
            #[cfg(target_os = "windows")]
            ("acrylic", Appearance),
        ];
        for (query, expected) in cases {
            assert_eq!(
                best_matching_section(query).map(|s| s.profile_label()),
                Some(expected.profile_label()),
                "query {query:?} should land on {:?}",
                expected.profile_label()
            );
        }
    }

    /// The `lyrel` CLI exists so scripts and coding agents can drive lyrel, so it
    /// lives with the other agent integrations rather than under About.
    #[test]
    fn command_line_tool_is_searchable_under_agents() {
        let entry = settings_search_entries()
            .iter()
            .find(|entry| entry.title == L10nKey::SettingsInstallCliOnPath)
            .expect("the CLI setting should be searchable");

        assert_eq!(entry.section.profile_label(), "settings:agents");
    }

    #[test]
    fn hermes_integration_has_one_search_destination() {
        assert_eq!(section_match_count(SettingsSection::Agents, "hermes"), 1);
    }

    #[test]
    fn humanize_action_splits_on_capitals() {
        assert_eq!(humanize_action("NewTab"), "New Tab");
        assert_eq!(
            humanize_action("ToggleMaximizePane"),
            "Toggle Maximize Pane"
        );
        assert_eq!(humanize_action("Quit"), "Quit");
    }

}

#[cfg(test)]
mod gpui_tests {
    use super::SettingsSection;
    use crate::core::config::{Config, MouseZoomModifier};
    use crate::ui::settings::test_window::harness;
    use gpui_kit::{TestAppContext, px, size};


    #[gpui_kit::test]
    fn enter_on_a_shortcut_search_result_opens_its_local_filter(cx: &mut TestAppContext) {
        let (app, mut vcx) = harness(cx);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_settings_section(SettingsSection::General, window, cx);
            let input = app.active_settings().unwrap().search.clone();
            input.update(cx, |input, cx| input.set_value("SplitRight", window, cx));
            app.autoselect_settings_search(cx);
        });
        vcx.run_until_parked();
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        app.update_in(&mut vcx, |app, _, cx| {
            let state = app.active_settings().unwrap();
            assert!(state.section == SettingsSection::Keybindings);
            assert!(!state.search_active);
            assert_eq!(
                state.shortcut_search.read(cx).value().as_str(),
                "SplitRight"
            );
        });
    }


    #[gpui_kit::test]
    fn failed_settings_writes_remain_visible_until_retry_succeeds(cx: &mut TestAppContext) {
        crate::core::config::pin_test_config_dir();
        let (app, mut vcx) = harness(cx);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_settings_section(SettingsSection::General, window, cx);
            cx.global_mut::<Config>().quarantined = true;
            app.set_notify_threshold(73, cx);
            assert!(app.active_settings().unwrap().save_error.is_some());
            cx.global_mut::<Config>().quarantined = false;
            app.persist_settings_config(cx);
            assert!(app.active_settings().unwrap().save_error.is_none());
            assert_eq!(cx.global::<Config>().notify_threshold_secs, 73);
        });
        vcx.run_until_parked();
    }

    #[gpui_kit::test]
    fn search_reuses_rows_and_reset_changes_only_the_selected_setting(cx: &mut TestAppContext) {
        crate::core::config::pin_test_config_dir();
        let (app, mut vcx) = harness(cx);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_settings_section(SettingsSection::General, window, cx);
            app.set_notify_threshold(71, cx);
            app.set_copy_on_select(!Config::default().copy_on_select, cx);
            let search = app.active_settings().unwrap().search.clone();
            search.update(cx, |s, cx| s.set_value("notify_threshold_secs", window, cx));
            app.autoselect_settings_search(cx);
        });
        vcx.simulate_resize(size(px(720.), px(560.)));
        vcx.run_until_parked();
        app.update_in(&mut vcx, |app, window, cx| {
            assert!(app.active_settings().unwrap().search_active);
            assert!(
                app.active_settings()
                    .unwrap()
                    .search_rows
                    .borrow()
                    .is_none()
            );
            app.reset_settings_value(
                crate::ui::i18n::L10nKey::SettingsNotifyThreshold,
                window,
                cx,
            );
            assert_eq!(
                cx.global::<Config>().notify_threshold_secs,
                Config::default().notify_threshold_secs
            );
            assert_eq!(
                cx.global::<Config>().copy_on_select,
                !Config::default().copy_on_select
            );
        });
        vcx.run_until_parked();
    }

    #[gpui_kit::test]
    fn every_category_and_full_search_can_layout_at_minimum_width(cx: &mut TestAppContext) {
        let (app, mut vcx) = harness(cx);
        for section in SettingsSection::ALL {
            app.update_in(&mut vcx, |app, window, cx| {
                app.open_settings_section(section, window, cx)
            });
            vcx.simulate_resize(size(px(720.), px(560.)));
            vcx.run_until_parked();
        }
        app.update_in(&mut vcx, |app, _, cx| {
            app.active_settings_mut().unwrap().modified_only = true;
            app.autoselect_settings_search(cx);
        });
        vcx.run_until_parked();
    }

    #[gpui_kit::test]
    fn clearing_search_preserves_the_category_and_target_navigation_clears_search(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx) = harness(cx);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_settings_section(SettingsSection::Appearance, window, cx);
            let search = app.active_settings().unwrap().search.clone();
            search.update(cx, |s, cx| s.set_value("mouse", window, cx));
            app.autoselect_settings_search(cx);
            assert!(app.active_settings().unwrap().section == SettingsSection::Appearance);
            search.update(cx, |s, cx| s.set_value("", window, cx));
            app.autoselect_settings_search(cx);
            assert!(!app.active_settings().unwrap().search_active);
            app.navigate_settings(
                SettingsSection::KeyboardMouse,
                Some(crate::ui::i18n::L10nKey::SettingsMouseZoom),
                window,
                cx,
            );
            assert!(app.active_settings().unwrap().section == SettingsSection::KeyboardMouse);
        });
        vcx.run_until_parked();
    }

    #[gpui_kit::test]
    fn appearance_section_lays_out_with_its_rounded_controls(cx: &mut TestAppContext) {
        let (app, mut vcx) = harness(cx);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_settings_section(SettingsSection::Appearance, window, cx);
        });

        vcx.simulate_resize(size(px(1100.), px(800.)));
        vcx.run_until_parked();

        app.update_in(&mut vcx, |app, _, cx| {
            // The theme menu open: its popover has to lay out beside the page.
            if let Some(s) = app.active_settings_mut() {
                s.menu = Some("theme-slot-dark".into());
            }
            cx.notify();
        });
        vcx.simulate_resize(size(px(720.), px(560.)));
        vcx.run_until_parked();

        let section = vcx.update(|_, cx| app.read(cx).active_settings().map(|s| s.section));
        assert!(
            matches!(section, Some(SettingsSection::Appearance)),
            "the page should still be on Appearance after two paint passes",
        );
    }

    /// #668: the Terminal page carries the control that moves the zoom off the
    /// platform modifier, so the page has to paint with it, and the pick has to
    /// reach the config the wheel reads.
    #[gpui_kit::test]
    fn the_terminal_page_paints_the_zoom_modifier_row(cx: &mut TestAppContext) {
        crate::core::config::pin_test_config_dir();
        let (app, mut vcx) = harness(cx);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_settings_section(SettingsSection::Terminal, window, cx);
        });
        vcx.simulate_resize(size(px(1100.), px(800.)));
        vcx.run_until_parked();

        let modifier = vcx.update(|_, cx| cx.global::<Config>().mouse_zoom_modifier);
        assert_eq!(
            modifier,
            MouseZoomModifier::None,
            "the wheel only scrolls out of the box"
        );

        app.update_in(&mut vcx, |app, _, cx| {
            app.set_mouse_zoom_modifier(MouseZoomModifier::Platform, cx)
        });
        vcx.run_until_parked();
        let modifier = vcx.update(|_, cx| cx.global::<Config>().mouse_zoom_modifier);
        assert_eq!(modifier, MouseZoomModifier::Platform, "and the pick sticks");
    }

    /// The Input page paints with the prompt editor off — that is the state
    /// where two of its rows are greyed out and their switches disabled — and
    /// the cascade only *disables* those two. It must not rewrite what they
    /// hold, or turning the editor back on would hand the user a completion
    /// menu they had switched off.
    #[gpui_kit::test]
    fn the_prompt_editor_greys_its_dependants_without_rewriting_them(cx: &mut TestAppContext) {
        crate::core::config::pin_test_config_dir();
        let (app, mut vcx) = harness(cx);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_settings_section(SettingsSection::Terminal, window, cx);
            app.set_history_search(false, cx);
            app.set_prompt_editor(false, cx);
        });
        vcx.simulate_resize(size(px(1100.), px(800.)));
        vcx.run_until_parked();

        let (prompt_editor, tab_completion, history_search) = vcx.update(|_, cx| {
            let cfg = cx.global::<Config>();
            (cfg.prompt_editor, cfg.tab_completion, cfg.history_search)
        });
        assert!(!prompt_editor, "the switch stuck");
        assert!(
            tab_completion,
            "a greyed-out row keeps its value for when the editor comes back"
        );
        assert!(!history_search, "and one the user had turned off stays off");

        app.update_in(&mut vcx, |app, _, cx| app.set_prompt_editor(true, cx));
        vcx.run_until_parked();
        let (prompt_editor, tab_completion) = vcx.update(|_, cx| {
            let cfg = cx.global::<Config>();
            (cfg.prompt_editor, cfg.tab_completion)
        });
        assert!(prompt_editor);
        assert!(tab_completion, "the completion menu comes back with it");
    }
}
