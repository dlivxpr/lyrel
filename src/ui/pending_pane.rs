use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    Animation, AnimationExt as _, Context, EventEmitter, FocusHandle, Focusable, SharedString,
    Window, div, prelude::*, px,
};

use crate::daemon::protocol::ShellSpec;
use crate::terminal::PaneWorkspace;
use crate::ui::i18n::{L10nKey, t_fmt};

#[derive(Clone)]
pub struct PendingSpawn {
    pub workspace: Option<PaneWorkspace>,
    pub working_directory: Option<std::path::PathBuf>,
    pub restore_pane: Option<u64>,
    pub shell: Option<ShellSpec>,
    pub agent: Option<crate::core::cli_agent::CLIAgent>,
    pub agent_session_id: Option<String>,
    pub agent_launch_argv: Option<Vec<String>>,
    pub agent_session_scope: Option<lyrel_core::host::session_operations::SessionScope>,
    /// A command line to type into the shell once the pane exists, for a pane
    /// opened to run something (a quick-launched agent). Typed only into this
    /// pane, and only when it is a fresh shell rather than a restored one.
    pub run_on_land: Option<String>,
    pub owner: Option<crate::core::session::WorkspaceId>,
    pub font_size: f32,
}

pub enum PendingState {
    Connecting,
    Failed(SharedString),
}

pub struct RetryRequested;

pub struct PendingPane {
    pub focus_handle: FocusHandle,
    pub machine: SharedString,
    pub state: PendingState,
    pub spawn: PendingSpawn,
    /// A native fork already exists; retry only opens its captured launch target.
    pub(crate) created_session: bool,
    /// Retries made without anyone pressing Try Again. See
    /// [`PendingPane::next_auto_retry`].
    auto_retries: u32,
    attach_only: bool,
    attach_spec: Option<Box<crate::daemon::protocol::NativeSshSpec>>,
}

/// How many times a pane standing in for one that may still be running asks
/// again on its own before it leaves the question to Try Again.
const AUTO_RETRY_LIMIT: u32 = 5;

/// 2s, 4s, 8s, 16s, 16s — about three quarters of a minute in all, which
/// covers a reconnect without keeping a machine that is really gone busy.
fn auto_retry_delay(restores: bool, made: u32) -> Option<Duration> {
    (restores && made < AUTO_RETRY_LIMIT).then(|| Duration::from_secs(1 << (made + 1).min(4)))
}

impl EventEmitter<RetryRequested> for PendingPane {}

impl PendingPane {
    pub fn new(
        machine: impl Into<SharedString>,
        spawn: PendingSpawn,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            machine: machine.into(),
            state: PendingState::Connecting,
            spawn,
            created_session: false,
            auto_retries: 0,
            attach_only: false,
            attach_spec: None,
        }
    }

    pub(crate) fn with_attach_only(mut self, spec: Option<Box<crate::daemon::protocol::NativeSshSpec>>) -> Self {
        self.attach_only = true;
        self.attach_spec = spec;
        self
    }
    pub(crate) fn is_attach_only(&self) -> bool { self.attach_only }
    pub(crate) fn attach_spec(&self) -> Option<Box<crate::daemon::protocol::NativeSshSpec>> { self.attach_spec.clone() }

    /// How long to wait before asking again on its own, or `None` once it has
    /// asked enough. Only a pane that stands in for one already running on its
    /// machine retries by itself: the failure that leaves it here is the link,
    /// not the pane, and the link usually comes back within seconds — the
    /// pane behind it has been running the whole time. A brand-new pane that
    /// failed has nothing waiting for it; Try Again is the user's call.
    pub fn next_auto_retry(&mut self) -> Option<Duration> {
        let delay = auto_retry_delay(self.spawn.restore_pane.is_some(), self.auto_retries)?;
        self.auto_retries += 1;
        Some(delay)
    }

    pub fn is_failed(&self) -> bool {
        matches!(self.state, PendingState::Failed(_))
    }

    pub fn fail(&mut self, reason: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.state = PendingState::Failed(reason.into());
        cx.notify();
    }

    pub fn retrying(&mut self, cx: &mut Context<Self>) {
        self.state = PendingState::Connecting;
        cx.notify();
    }

    fn failure_message(&self, reason: &str) -> String {
        match self
            .spawn
            .agent_session_id
            .as_deref()
            .filter(|_| self.created_session)
        {
            Some(id) => t_fmt(
                L10nKey::PendingSessionPreserved,
                &[("id", id), ("error", reason)],
            ),
            None => reason.to_string(),
        }
    }
}

impl Focusable for PendingPane {
    fn focus_handle(&self, _cx: &gpui_kit::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PendingPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, dim) = (theme.muted_foreground, theme.muted_foreground.opacity(0.75));

        let body = match &self.state {
            PendingState::Connecting => v_flex()
                .items_center()
                .gap(px(10.))
                .child(
                    Icon::new(IconName::LoaderCircle)
                        .size(px(18.))
                        .text_color(dim)
                        .with_animation(
                            "pending-pane-spin",
                            Animation::new(Duration::from_millis(900)).repeat(),
                            |icon, delta| {
                                icon.transform(gpui_kit::Transformation::rotate(
                                    gpui_kit::percentage(delta),
                                ))
                            },
                        ),
                )
                .child(div().text_sm().text_color(muted).child(t_fmt(
                    L10nKey::PendingConnecting,
                    &[("machine", &self.machine)],
                )))
                .into_any_element(),
            PendingState::Failed(reason) => v_flex()
                .items_center()
                .gap(px(10.))
                .max_w(px(420.))
                .child(div().text_sm().text_color(theme.foreground).child(t_fmt(
                    L10nKey::PendingUnreachable,
                    &[("machine", &self.machine)],
                )))
                .child(
                    div()
                        .text_xs()
                        .text_center()
                        .text_color(muted)
                        .child(self.failure_message(reason)),
                )
                .child(
                    Button::new("pending-pane-retry")
                        .label(crate::ui::i18n::t(if self.created_session {
                            L10nKey::PendingOpenSession
                        } else {
                            L10nKey::TryAgain
                        }))
                        .ghost()
                        .small()
                        .on_click(cx.listener(|this, _, _window, cx| {
                            this.retrying(cx);
                            cx.emit(RetryRequested);
                        })),
                )
                .into_any_element(),
        };

        h_flex()
            .track_focus(&self.focus_handle)
            .size_full()
            .items_center()
            .justify_center()
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_pane_standing_in_for_a_running_one_retries_by_itself() {
        assert_eq!(
            auto_retry_delay(false, 0),
            None,
            "a new pane has nothing waiting"
        );
        let delays: Vec<u64> = (0..)
            .map_while(|made| auto_retry_delay(true, made))
            .map(|d| d.as_secs())
            .collect();
        assert_eq!(
            delays,
            vec![2, 4, 8, 16, 16],
            "then Try Again is the user's"
        );
    }
}
