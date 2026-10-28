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

mod agents;
pub(crate) mod arguments;
mod catalog;
use catalog::*;
mod configuration;
mod editor;
mod inputs;
pub(crate) mod kit;
mod navigation;
mod pages;
mod shell;
mod shortcuts;
pub(crate) mod skills;
mod ssh;
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

use crate::core::update::{localized_update_install_hint, localized_update_phase};

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

pub(crate) struct SettingsState {
    pub(crate) focus_handle: gpui_kit::FocusHandle,
    pub(crate) navigation: navigation::SettingsNavigation,
    pub(crate) usage: Entity<usage::UsagePage>,
    pub(crate) skills: skills::SkillsState,
    pub(super) shortcuts: shortcuts::ShortcutEditor,
    pub(crate) save_error: Option<String>,
    pub(crate) saved_config: Config,
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
        let Some(session) = self.active_settings().map(|s| s.navigation.session()) else {
            return;
        };
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
                if !this
                    .active_settings()
                    .is_some_and(|s| s.navigation.accepts(session))
                {
                    return;
                }
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
                .navigation
                .search()
                .clone()
                .update(cx, |search, cx| search.set_value("", window, cx));
        }
        if let Some(state) = self.active_settings_mut() {
            state.navigation.navigate(setting, cx.global::<Config>());
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
        self.active_settings()?
            .navigation
            .first_hit_anchor(label, cx)
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

        let section = vcx.update(|_, cx| {
            app.read(cx)
                .active_settings()
                .map(|s| s.navigation.section())
        });
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
