//! Caffeine: the status bar's keep-awake control (#363, #379).
//!
//! One process-wide mode shared by every desktop window: Off, Always On, or
//! While Agents Work. Automatic mode holds the native idle-sleep request while
//! any pane in any Lyrel window — local or an attached managed remote — has an
//! agent Working, and releases it the moment none is, including when all are
//! waiting for input. Display sleep and screen lock are never touched: macOS
//! gets an idle *system sleep* assertion, Windows gets
//! `ES_CONTINUOUS | ES_SYSTEM_REQUIRED` with no display override (see
//! `native`).
//!
//! The mode is deliberately in-memory: the desktop starts in Off on every
//! launch (#363). What the menu and the coffee cup show is the request's real
//! outcome — held, released, or the platform's own error — never the mode
//! alone.
//!
//! Toolbar contract with #380: hiding the Caffeine switch in Toolbar settings
//! calls [`CaffeineState::reset`], which releases the request and returns the
//! mode to Off; showing the switch again renders that Off state. There is no
//! separate persisted visibility state on this side.

mod native;

#[cfg(test)]
pub(crate) use native::force_failure_for_test;

use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, button::Button};
use gpui_kit::{
    Anchor, AnyElement, App, Context, TestSupportExt as _, Window, div, prelude::*, px,
};
use lyrel_ui_base::i18n::{L10nKey, t, t_fmt};

use crate::ui::app::LyrelApp;
use crate::ui::windows::WindowRegistry;

/// What the user asked for. Process-wide; never persisted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CaffeineMode {
    #[default]
    Off,
    AlwaysOn,
    WhileAgentsWork,
}

impl CaffeineMode {
    fn label(self) -> &'static str {
        match self {
            CaffeineMode::Off => t(L10nKey::StatusCaffeineModeOff),
            CaffeineMode::AlwaysOn => t(L10nKey::StatusCaffeineModeAlwaysOn),
            CaffeineMode::WhileAgentsWork => t(L10nKey::StatusCaffeineModeWhileAgentsWork),
        }
    }
}

/// The single process-wide Caffeine record. `guard.is_some()` is the truth of
/// "a native idle-sleep request is live right now"; `last_error` is the truth
/// of the most recent failed acquire or release.
#[derive(Default)]
pub(crate) struct CaffeineState {
    mode: CaffeineMode,
    guard: Option<native::SleepHold>,
    last_error: Option<String>,
}

impl gpui_kit::Global for CaffeineState {}

impl CaffeineState {
    pub(crate) fn mode(&self) -> CaffeineMode {
        self.mode
    }

    /// Whether the native idle-sleep request is held this moment.
    pub(crate) fn holding(&self) -> bool {
        self.guard.is_some()
    }

    /// Automatic mode with the request not held and no error: waiting for an
    /// agent to start working. What the status-bar cup's badge expresses.
    pub(crate) fn waiting(&self) -> bool {
        self.mode == CaffeineMode::WhileAgentsWork
            && self.guard.is_none()
            && self.last_error.is_none()
    }

    pub(crate) fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// The menu's footer line: the effective hold, the wait, or the error.
    pub(crate) fn footer_text(&self) -> String {
        if let Some(error) = self.last_error() {
            return t_fmt(L10nKey::StatusCaffeineFooterError, &[("error", error)]);
        }
        match (self.mode, self.holding()) {
            (_, true) => t(L10nKey::StatusCaffeineFooterHolding).to_string(),
            (CaffeineMode::WhileAgentsWork, false) => {
                t(L10nKey::StatusCaffeineFooterWaiting).to_string()
            }
            _ => t(L10nKey::StatusCaffeineFooterOff).to_string(),
        }
    }

    /// The menu's choice. Changing the mode re-evaluates the hold at once.
    pub(crate) fn set_mode(mode: CaffeineMode, cx: &mut App) {
        cx.global_mut::<Self>().mode = mode;
        Self::refresh(cx);
    }

    /// The #380 contract: hiding the Caffeine component stops its effect —
    /// release the request and return to Off. Re-showing renders this Off.
    pub(crate) fn reset(cx: &mut App) {
        cx.global_mut::<Self>().mode = CaffeineMode::Off;
        Self::refresh(cx);
    }

    /// App exit: the power request's lifetime ends with the desktop. The mode
    /// goes Off with it, so an agent-status deferred effect landing during
    /// teardown cannot re-acquire a request the exit just released.
    pub(crate) fn release_for_quit(cx: &mut App) {
        cx.global_mut::<Self>().mode = CaffeineMode::Off;
        Self::release_hold(cx);
    }

    /// Re-evaluate whether the native request should be live. Called on mode
    /// changes and on every Working/Waiting transition in any window; cheap
    /// and idempotent, so callers need not debounce.
    pub(crate) fn refresh(cx: &mut App) {
        let (mode, held) = {
            let state = cx.global::<Self>();
            (state.mode, state.holding())
        };
        let want = mode != CaffeineMode::Off
            && (mode == CaffeineMode::AlwaysOn || any_window_agent_working(cx));
        if want == held {
            // Converged. A stored error can only belong to a failed acquire
            // (want, not held) or a failed release (not want, still held) —
            // neither is this state — so whatever it blames no longer holds.
            if cx.global::<Self>().last_error.is_some() {
                cx.global_mut::<Self>().last_error = None;
            }
            return;
        }
        if want {
            match native::SleepHold::acquire(t(L10nKey::StatusCaffeineTitle)) {
                Ok(guard) => {
                    let state = cx.global_mut::<Self>();
                    state.guard = Some(guard);
                    state.last_error = None;
                }
                Err(error) => {
                    // No guard is installed; the failure is the visible state
                    // and the next transition retries the acquire.
                    cx.global_mut::<Self>().last_error = Some(error);
                }
            }
        } else {
            Self::release_hold(cx);
        }
    }

    /// An agent's session state changed somewhere; re-evaluate after the
    /// current update unwinds, so aggregation can read every window without
    /// this one being leased.
    pub(crate) fn agents_changed(cx: &mut App) {
        cx.defer(|cx| Self::refresh(cx));
    }

    fn release_hold(cx: &mut App) {
        let Some(mut guard) = cx.global_mut::<Self>().guard.take() else {
            return;
        };
        if let Err(error) = guard.release() {
            // The platform still holds the request: put the guard back and
            // show the failure instead of pretending the release happened.
            let state = cx.global_mut::<Self>();
            state.guard = Some(guard);
            state.last_error = Some(error);
        } else {
            // A successful release retires any failure it once reported.
            cx.global_mut::<Self>().last_error = None;
        }
    }
}

/// Whether any pane in any open Lyrel window has an agent Working — local
/// windows and attached managed remote workspaces alike. The computer being
/// kept awake is always this one, no matter which host the agent runs on.
fn any_window_agent_working(cx: &mut App) -> bool {
    for (_, weak) in WindowRegistry::open_windows(cx) {
        if weak
            .upgrade()
            .is_some_and(|app| app.read(cx).any_agent_working(cx))
        {
            return true;
        }
    }
    false
}

/// Which ink the cup is painted in; resolved against the live theme in
/// [`LyrelApp::render_caffeine`], where the accent ink arrives as the
/// `ActiveAccentInk` global.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CupInk {
    /// The theme accent cleared as ink: a live hold.
    AccentInk,
    /// The danger role: the last request failed.
    Danger,
    /// The muted caption role: Off, and automatic mode's wait.
    Muted,
}

/// The cup's silhouette and ink for the request's real outcome — the one
/// place the four presentations are decided (#401). A live hold is the filled
/// cup in the contrast-checked theme accent; Off and the wait keep the
/// muted outline; failure keeps the outline with the danger role. The hold
/// wins the silhouette even while a stale release error is still reported:
/// the request is genuinely live, and the failure shows in the menu footer
/// and the accessible name. Automatic waiting adds its dot in
/// `render_caffeine`, the badge the approved figure puts on the outline cup.
pub(crate) fn cup_look(holding: bool, errored: bool) -> (&'static str, CupInk) {
    match (holding, errored) {
        (true, _) => ("icons/coffee-filled.svg", CupInk::AccentInk),
        (false, true) => ("icons/coffee.svg", CupInk::Danger),
        (false, false) => ("icons/coffee.svg", CupInk::Muted),
    }
}

impl LyrelApp {
    /// Whether any pane — anywhere in this window's tree — has an agent
    /// Working right now. Read leaf by leaf: `Tab::agent_status` answers the
    /// sidebar badge, which ranks Waiting above Working, and one waiting pane
    /// must not mask another pane's live work. A pane whose link or child has
    /// exited is not working either: a hard disconnect leaves the last
    /// session state behind and it must not keep this computer awake.
    fn any_agent_working(&self, cx: &App) -> bool {
        use crate::core::cli_agent::AgentStatus;
        self.tabs.iter().any(|tab| {
            tab.pane.terminals().into_iter().any(|leaf| {
                let view = leaf.read(cx);
                !view.terminal.exited
                    && view.agent_session().map(|s| s.status) == Some(AgentStatus::Working)
            })
        })
    }

    /// The status bar's coffee cup and its mode menu. A live hold is the
    /// filled cup in the theme accent cleared as ink (`ActiveAccentInk`); Off
    /// and automatic waiting keep the muted outline, waiting adds the dot, and
    /// a failed request keeps the outline with the danger role. The accessible
    /// name carries what the cup only shows: the selected mode and the
    /// request's real outcome, the failure's explanation included (#401).
    pub(crate) fn render_caffeine(&self, _window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let state = cx.global::<CaffeineState>();
        let mode = state.mode();
        let holding = state.holding();
        let waiting = state.waiting();
        let errored = state.last_error().is_some();
        let footer = state.footer_text();
        let title = t(L10nKey::StatusCaffeineTitle);

        // The control exposes the mode and the actual state accessibly; the
        // failure footer is itself the explanation.
        let access_name = t_fmt(
            L10nKey::StatusCaffeineAccessName,
            &[("title", title), ("mode", mode.label()), ("state", footer.as_str())],
        );

        let (cup_icon, cup_ink) = cup_look(holding, errored);
        let theme = cx.theme();
        let cup_color = match cup_ink {
            CupInk::AccentInk => {
                gpui_kit::rgb(cx.global::<lyrel_ui_base::presets::ActiveAccentInk>().0).into()
            }
            CupInk::Danger => theme.danger,
            CupInk::Muted => theme.muted_foreground,
        };

        let cup = Button::new("status-caffeine")
            .ghost()
            .xsmall()
            .icon(Icon::empty().path(cup_icon).text_color(cup_color))
            .accessibility_label(access_name)
            .tooltip(title)
            .dropdown_menu_with_anchor(Anchor::BottomRight, {
                move |menu, window, cx| caffeine_menu(menu, window, cx, mode, footer.clone())
            });

        // Waiting is a state, not a colour: the badge is what the approved
        // figure puts on the cup while automatic mode has nothing to hold yet.
        if waiting {
            div()
                .id("status-caffeine-waiting")
                .test_support()
                .relative()
                .flex_none()
                .child(cup)
                .child(
                    div()
                        .id("status-caffeine-waiting-dot")
                        .test_support()
                        .absolute()
                        .bottom(px(1.))
                        .right(px(1.))
                        .size(px(5.))
                        .rounded_full()
                        .bg(theme.muted_foreground),
                )
                .into_any_element()
        } else {
            cup.into_any_element()
        }
    }
}

fn caffeine_menu(
    menu: PopupMenu,
    window: &Window,
    _cx: &mut Context<PopupMenu>,
    current: CaffeineMode,
    footer: String,
) -> PopupMenu {
    menu.min_w(window.rem_size() * 14.)
        .label(t(L10nKey::StatusCaffeineTitle))
        .item(mode_item(CaffeineMode::Off, current))
        .item(mode_item(CaffeineMode::AlwaysOn, current))
        .item(mode_item(CaffeineMode::WhileAgentsWork, current))
        .separator()
        .item(PopupMenuItem::new(footer).disabled(true))
}

fn mode_item(mode: CaffeineMode, current: CaffeineMode) -> PopupMenuItem {
    PopupMenuItem::new(mode.label())
        .checked(mode == current)
        .on_click(move |_, _, cx| CaffeineState::set_mode(mode, cx))
}
