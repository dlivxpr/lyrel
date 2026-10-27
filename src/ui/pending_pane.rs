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
mod creation;
pub(crate) use creation::{PendingLaunch, SpawnFinished};


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

enum PendingState {
    Connecting,
    Failed(SharedString),
}

pub struct RetryRequested;

pub struct PendingPane {
    focus_handle: FocusHandle,
    machine: SharedString,
    state: PendingState,
    spawn: PendingSpawn,
    /// A native fork already exists; retry only opens its captured launch target.
    created_session: bool,
    creation: creation::Creation,
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
    pub(crate) fn shell(
        workspace: Option<PaneWorkspace>,
        owner: Option<crate::core::session::WorkspaceId>,
        font_size: f32,
        working_directory: Option<std::path::PathBuf>,
        restore_pane: Option<u64>,
        shell: Option<ShellSpec>,
        cx: &mut Context<Self>,
    ) -> Self {
        let machine = workspace.as_ref()
            .map(|workspace| crate::ui::remote_connect::target_label(cx, &workspace.target))
            .unwrap_or_else(|| crate::ui::i18n::t(L10nKey::AppLocalServerName).to_string());
        Self::new(machine, PendingSpawn {
            workspace, owner, font_size, working_directory, restore_pane, shell,
            agent: None, agent_session_id: None, agent_launch_argv: None,
            agent_session_scope: None, run_on_land: None,
        }, cx)
    }

    pub(crate) fn native_ssh(
        owner: crate::core::session::WorkspaceId,
        font_size: f32,
        remote_start_dir: Option<std::path::PathBuf>,
        spec: Box<crate::daemon::protocol::NativeSshSpec>,
        cx: &mut Context<Self>,
    ) -> Self {
        let machine = spec.display_name.clone().unwrap_or_else(|| spec.host.clone());
        Self::new(machine, PendingSpawn {
            workspace: None, owner: Some(owner), font_size,
            working_directory: remote_start_dir, restore_pane: None, shell: None,
            agent: None, agent_session_id: None, agent_launch_argv: None,
            agent_session_scope: None, run_on_land: None,
        }, cx).native(spec)
    }

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
            creation: creation::Creation::default(),
            auto_retries: 0,
            attach_only: false,
            attach_spec: None,
        }
    }

    pub(crate) fn with_attach_only(
        mut self,
        spec: Option<Box<crate::daemon::protocol::NativeSshSpec>>,
    ) -> Self {
        self.attach_only = true;
        self.attach_spec = spec;
        self
    }

    pub(crate) fn is_attach_only(&self) -> bool { self.attach_only }

    pub(crate) fn parked_card(&self, slot: gpui_kit::EntityId) -> crate::ui::parked_deck::ParkedCard {
        let status = match &self.state {
            PendingState::Connecting => t_fmt(L10nKey::PendingConnecting, &[("machine", &self.machine)]),
            PendingState::Failed(reason) => reason.to_string(),
        };
        crate::ui::parked_deck::ParkedCard::new(
            slot.as_u64(), self.machine.clone(),
            self.spawn.working_directory.as_ref().map(|path| path.display().to_string()).unwrap_or_default(),
            false,
        ).with_status(status)
    }

    /// How long to wait before asking again on its own, or `None` once it has
    /// asked enough. Only a pane that stands in for one already running on its
    /// machine retries by itself: the failure that leaves it here is the link,
    /// not the pane, and the link usually comes back within seconds — the
    /// pane behind it has been running the whole time. A brand-new pane that
    /// failed has nothing waiting for it; Try Again is the user's call.
    fn next_auto_retry(&mut self) -> Option<Duration> {
        let delay = auto_retry_delay(self.spawn.restore_pane.is_some(), self.auto_retries)?;
        self.auto_retries += 1;
        Some(delay)
    }

    pub(crate) fn restore_pane(&self) -> Option<u64> { self.spawn.restore_pane }
    pub(crate) fn workspace(&self) -> Option<&PaneWorkspace> { self.spawn.workspace.as_ref() }
    pub(crate) fn run_when_ready(&mut self, command: String) { self.spawn.run_on_land = Some(command); }
    pub(crate) fn resume_agent(
        &mut self,
        agent: Option<crate::core::cli_agent::CLIAgent>,
        session: Option<String>,
        argv: Option<Vec<String>>,
        scope: Option<lyrel_core::host::session_operations::SessionScope>,
        created: bool,
    ) {
        self.spawn.agent = agent;
        self.spawn.agent_session_id = session;
        self.spawn.agent_launch_argv = argv;
        self.spawn.agent_session_scope = scope.as_ref().map(lyrel_core::host::session_operations::durable_scope);
        self.created_session = created && self.spawn.agent_session_id.is_some();
    }

    pub(crate) fn session(&self) -> crate::core::session::SessionPane {
        crate::core::session::SessionPane::Leaf {
            cwd: self.spawn.working_directory.clone(),
            pane_id: self.spawn.restore_pane,
            shell: self.spawn.shell.clone(),
            ssh_spec: self.creation.ssh_spec().or_else(|| self.attach_spec.clone()),
            agent: self.spawn.agent,
            agent_session_id: self.spawn.agent_session_id.clone(),
            agent_launch_argv: self.spawn.agent_launch_argv.clone(),
            agent_session_scope: self.spawn.agent_session_scope.as_ref().map(lyrel_core::host::session_operations::durable_scope),
        }
    }

    pub fn is_failed(&self) -> bool {
        matches!(self.state, PendingState::Failed(_))
    }

    fn fail(&mut self, reason: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.state = PendingState::Failed(reason.into());
        cx.notify();
    }

    fn retrying(&mut self, cx: &mut Context<Self>) {
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
