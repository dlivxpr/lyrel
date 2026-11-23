//! The app icon's system badge: distinct terminal tabs that need attention
//! (#397).
//!
//! The bell counts unread records; this counts tabs. A tab contributes once
//! no matter how many of its panes are blocked or how many failed results it
//! holds, and the aggregate spans every open desktop window. Which rows count
//! is the notification activity matrix's `badge` column: Waiting and Failed
//! are on by default, Ended is off, and Running has no badge channel at all.
//!
//! macOS shows the number on the Dock tile. Windows shows a `1`–`9` or `9+`
//! overlay on every top-level taskbar button, so ungrouped buttons and a
//! grouped button agree. Windows has no window-procedure hook here for
//! `TaskbarButtonCreated`; the one-second reconcile below re-applies the same
//! aggregate, which restores it after Explorer recreates the buttons and
//! clears it when the aggregate empties. The tray's own attention indicator
//! is untouched.

#[cfg(all(target_os = "windows", not(test)))]
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
#[cfg(all(target_os = "windows", not(test)))]
use windows::Win32::Foundation::HWND;

use crate::ui::app::LyrelApp;
use crate::ui::program_attention::TerminalViewProgramSource as _;
use crate::ui::windows::WindowRegistry;
use gpui_kit::{App, Global};
use lyrel_core::core::config::{ActivityState, ProgramActivity};
use lyrel_ui_base::config::Config;
use lyrel_ui_base::i18n::{L10nKey, t_fmt};

mod native;

/// How often the process re-applies the current aggregate. The Windows
/// taskbar button can be recreated behind the app's back (Explorer restart);
/// re-applying an unchanged icon is idempotent, and an empty aggregate costs
/// nothing because there is nothing to restore.
const RECONCILE_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

/// The process-wide badge state. `count` is the last aggregate read,
/// `applied` the label the native surface accepted, and `error` the platform's
/// most recent refusal (kept until a later apply succeeds).
#[derive(Default)]
pub(crate) struct SystemBadgeState {
    count: usize,
    applied: Option<String>,
    error: Option<String>,
}

impl Global for SystemBadgeState {}

impl SystemBadgeState {
    /// Start the reconcile loop once per process. Tests drive
    /// [`Self::reconcile`] directly instead of waiting on a timer.
    pub(crate) fn init(cx: &mut App) {
        static STARTED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        if STARTED.set(()).is_err() {
            return;
        }
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(RECONCILE_EVERY).await;
                cx.update(Self::reconcile);
            }
        })
        .detach();
    }

    #[cfg(test)]
    pub(crate) fn count(&self) -> usize {
        self.count
    }

    /// Re-read the aggregate and apply it when the label changed. Called on
    /// every transition that can reshape attention: an agent status, a
    /// program record, a closed tab or window, and the matrix itself.
    pub(crate) fn refresh(cx: &mut App) {
        let count = aggregate(cx);
        let label = label_for(count);
        let state = cx.default_global::<Self>();
        let previous = state.count;
        state.count = count;
        if previous == count && state.applied == label && state.error.is_none() {
            return;
        }
        Self::apply(cx, label, count);
    }

    /// The reconcile tick: re-read the aggregate and re-apply the one label
    /// even when it did not change, because the native surface may have been
    /// recreated. An empty aggregate that was already empty is a no-op.
    pub(crate) fn reconcile(cx: &mut App) {
        let count = aggregate(cx);
        let label = label_for(count);
        let state = cx.default_global::<Self>();
        let previous = state.count;
        state.count = count;
        if count == 0
            && previous == 0
            && state.applied.is_none()
            && state.error.is_none()
        {
            return;
        }
        Self::apply(cx, label, count);
    }

    /// Ask for a refresh after the current update unwinds, so reading every
    /// window cannot observe the one whose event triggered it mid-lease.
    pub(crate) fn refresh_deferred(cx: &mut App) {
        cx.defer(Self::refresh);
    }

    fn apply(cx: &mut App, label: Option<String>, count: usize) {
        let description = if label.is_some() {
            t_fmt(
                L10nKey::SystemBadgeTabsNeedAttention,
                &[("count", &count.to_string())],
            )
        } else {
            String::new()
        };
        #[cfg(all(target_os = "windows", not(test)))]
        let windows = app_window_handles(cx);
        #[cfg(all(target_os = "windows", not(test)))]
        if label.is_some() && windows.is_empty() {
            // No taskbar button exists while the app is tray-retired. Keep
            // the last applied label unchanged so the next reconcile applies
            // the aggregate once a window is back.
            return;
        }
        #[cfg(any(not(target_os = "windows"), test))]
        let windows = ();
        let result = native::apply(label.as_deref(), &description, &windows);
        let state = cx.default_global::<Self>();
        match result {
            Ok(()) => {
                state.applied = label;
                state.error = None;
            }
            Err(error) => {
                if state.error.as_deref() != Some(error.as_str()) {
                    log::warn!("system badge: {error}");
                }
                state.error = Some(error);
            }
        }
    }
}

/// The number of distinct terminal tabs across every open window that need
/// attention under the matrix's badge column.
pub(crate) fn aggregate(cx: &App) -> usize {
    WindowRegistry::live_apps(cx)
        .map(|app| app.read(cx).badge_tab_count(cx))
        .sum()
}

/// The platform's label for an aggregate. Zero is no badge at all; ten or
/// more is the documented `9+` cap.
pub(crate) fn label_for(count: usize) -> Option<String> {
    match count {
        0 => None,
        1..=9 => Some(count.to_string()),
        _ => Some("9+".to_owned()),
    }
}

impl LyrelApp {
    /// This window's tabs that qualify for the system badge. Each tab counts
    /// once even when several of its panes qualify.
    pub(crate) fn badge_tab_count(&self, cx: &App) -> usize {
        let activity = cx.global::<Config>().program_activity;
        self.tabs
            .iter()
            .filter(|tab| {
                tab.pane.terminals().into_iter().any(|leaf| {
                    let view = leaf.read(cx);
                    if view.terminal.exited {
                        return false;
                    }
                    pane_needs_badge(view, &activity)
                        || self
                            .program_attention
                            .badge_attention(view.program_source())
                            .any(&activity)
                })
            })
            .count()
    }
}

/// Whether one pane's agent session is a live wait or an unread result whose
/// row keeps its badge channel. A settled result only needs attention while
/// it is unread; a wait is live until the pane stops waiting.
fn pane_needs_badge(
    view: &lyrel_terminal::view::TerminalView,
    activity: &ProgramActivity,
) -> bool {
    let Some(session) = view.agent_session() else {
        return false;
    };
    let outcome = session.readout.result.as_ref().map(|result| result.outcome);
    let Some(row) = ActivityState::of_agent(session.status, outcome) else {
        return false;
    };
    let channels = activity.channels(row);
    match row {
        ActivityState::Running => false,
        ActivityState::Waiting => channels.badge,
        ActivityState::Ended | ActivityState::Failed => channels.badge && view.agent_result_unread(),
    }
}

#[cfg(all(target_os = "windows", not(test)))]
fn app_window_handles(cx: &mut App) -> Vec<HWND> {
    // Every top-level app window gets the same overlay, including Settings:
    // ungrouped taskbar buttons each show it, and a grouped button follows
    // the one Windows picks for the group.
    cx.windows()
        .into_iter()
        .filter_map(|handle| {
            handle
                .update(cx, |_, window, _| {
                    let handle = HasWindowHandle::window_handle(window).ok()?;
                    match handle.as_raw() {
                        RawWindowHandle::Win32(win32) => Some(HWND(win32.hwnd.get() as _)),
                        _ => None,
                    }
                })
                .ok()
                .flatten()
        })
        .collect()
}

#[cfg(test)]
pub(crate) fn take_test_badges() -> Vec<Option<String>> {
    native::take_test_badges()
}
