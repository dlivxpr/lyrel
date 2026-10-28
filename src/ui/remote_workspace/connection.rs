use crate::core::session::RemoteTarget;
use crate::ui::remote_connect::{Connected, HostChoice};

/// One window's manual attempt, not the shared machine connection authority.
#[derive(Default)]
pub(crate) struct WindowConnection {
    generation: u64,
    state: Option<ConnectionState>,
}

pub(crate) enum ConnectionState {
    Connecting { choice: HostChoice },
    Failed { choice: HostChoice, error: String },
}

impl ConnectionState {
    pub(crate) fn choice(&self) -> Option<&HostChoice> {
        match self {
            Self::Connecting { choice } | Self::Failed { choice, .. } => Some(choice),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AttemptId(u64);

pub(crate) enum ConnectionOutcome {
    Connected {
        choice: HostChoice,
        connected: Connected,
    },
    Failed {
        choice: HostChoice,
        error: String,
    },
}

impl WindowConnection {
    pub(crate) fn state(&self) -> Option<&ConnectionState> {
        self.state.as_ref()
    }

    pub(crate) fn choice(&self) -> Option<&HostChoice> {
        self.state.as_ref().and_then(ConnectionState::choice)
    }

    pub(crate) fn begin(&mut self, choice: HostChoice) -> AttemptId {
        self.generation = self
            .generation
            .checked_add(1)
            .expect("connection generation exhausted");
        self.state = Some(ConnectionState::Connecting { choice });
        AttemptId(self.generation)
    }

    pub(crate) fn is_connecting(&self, attempt: AttemptId) -> bool {
        attempt.0 == self.generation
            && matches!(self.state, Some(ConnectionState::Connecting { .. }))
    }

    /// Retiring the window flow never disconnects a machine or destroys panes.
    pub(crate) fn retire(&mut self) {
        self.state = None;
    }

    pub(crate) fn retire_target(&mut self, target: &RemoteTarget) {
        if self.choice().is_some_and(|choice| &choice.target == target) {
            self.retire();
        }
    }

    pub(crate) fn dismiss_failure(&mut self, target: Option<&RemoteTarget>) -> bool {
        if let Some(ConnectionState::Failed { choice, .. }) = &self.state
            && target.is_none_or(|target| &choice.target == target)
        {
            self.retire();
            return true;
        }
        false
    }

    /// Only the attempt that still owns this slot may apply its result. A late
    /// success drops its handle; it cannot redirect a replacement window flow.
    pub(crate) fn complete(
        &mut self,
        attempt: AttemptId,
        result: Result<Connected, String>,
    ) -> Option<ConnectionOutcome> {
        if !self.is_connecting(attempt) {
            return None;
        }
        let Some(ConnectionState::Connecting { choice }) = self.state.take() else {
            return None;
        };
        match result {
            Ok(connected) => {
                Some(ConnectionOutcome::Connected { choice, connected })
            }
            Err(error) => {
                self.state = Some(ConnectionState::Failed {
                    choice: choice.clone(),
                    error: error.clone(),
                });
                Some(ConnectionOutcome::Failed { choice, error })
            }
        }
    }
}

// Framework scheduling stays on the existing window entity. The owner above
// accepts only an attempt identity and semantic outcomes, never a mutable root.
use gpui_kit::{Context, Window};
use lyrel_core::host::HostId;

use super::{RemoteLinks, pump_auth_sheets};
use crate::core::config::Config;
use crate::core::session::{WorkspaceId, WorkspaceStore};
use crate::ui::app::LyrelApp;
use crate::ui::remote_connect;

impl LyrelApp {
    pub(crate) fn connect_to_host(&mut self, choice: HostChoice, cx: &mut Context<Self>) {
        remote_connect::register(cx);
        // Whatever went wrong last time is about to be answered by this attempt.
        self.remote_host_errors.remove(&choice.target.to_string());
        let attempt = self.connect.begin(choice.clone());
        let header = match remote_connect::control_route(&choice.target, cx) {
            Ok(header) => header,
            Err(e) => {
                self.connect.complete(attempt, Err(e));
                cx.notify();
                return;
            }
        };
        let target = choice.target.clone();
        let label = choice.label.clone();
        cx.default_global::<RemoteLinks>()
            .suspended
            .remove(&choice.target.host_id());
        cx.notify();

        let host_id = choice.target.host_id();
        remote_connect::clear_install_progress(host_id);
        self.watch_for_install_consent(host_id, attempt, cx);
        let launch_commands = cx.global::<Config>().agent_launch.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    remote_connect::connect_blocking(&target, header, &label, &launch_commands)
                })
                .await;
            // Retired here rather than in `finish_connect`, which bows out
            // early whenever `connect` has moved on — a disconnect from the
            // switcher, or entering another workspace, both of which can land
            // mid-install. The strip reads this entry with no link state to
            // temper it, so one left behind freezes a progress bar on every
            // window pointed at this machine *and* takes away the Update
            // Server button, which is the one thing that could have fixed it.
            remote_connect::clear_install_progress(host_id);
            let _ = this.update_in(cx, |this, window, cx| {
                this.finish_connect(attempt, result, window, cx)
            });
        })
        .detach();
    }

    fn watch_for_install_consent(&self, host: HostId, attempt: AttemptId, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let mut painted: Option<crate::daemon::install::InstallPhase> = None;
            loop {
                let connecting = this
                    .update(cx, |this, _| {
                        this.connect.is_connecting(attempt)
                    })
                    .unwrap_or(false);
                if !connecting {
                    return;
                }
                if let Some(pending) = remote_connect::take_pending_install() {
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.prompt_install_consent(pending, window, cx)
                    });
                }
                let reported = remote_connect::install_progress_for(host);
                if reported != painted {
                    painted = reported;
                    let _ = this.update(cx, |_, cx| cx.notify());
                }
                cx.update(pump_auth_sheets);
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(100))
                    .await;
            }
        })
        .detach();
    }
    pub(super) fn finish_connect(
        &mut self,
        attempt: AttemptId,
        result: Result<remote_connect::Connected, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(outcome) = self.connect.complete(attempt, result) else {
            return;
        };
        match outcome {
            ConnectionOutcome::Connected { choice, connected } => {
                let home = connected.home.clone();
                let rows = connected.rows.clone();
                self.host_snapshots.insert(
                    choice.target.host_id(),
                    crate::ui::switcher::HostSnapshot {
                        target: choice.target.clone(),
                        rows: rows.clone(),
                    },
                );
                remote_connect::HostLinks::insert(cx, connected.host, home.clone());
                // The listing outlives the link: mirrored into the store so
                // the switcher still knows this machine's workspaces after a
                // restart, without a connection.
                let listing: Vec<(WorkspaceId, String, u64)> = rows
                    .iter()
                    .map(|r| (r.id, r.name.clone(), r.last_active))
                    .collect();
                WorkspaceStore::sync_remote(cx, &choice.target, &listing);
                self.prompt_remote_daemon_mismatch_later(cx);
                self.connection_ready_for_create(&choice.target, home, window, cx);
            }
            ConnectionOutcome::Failed { choice, error } => {
                log::warn!("connect to {} failed: {error}", choice.label);
                self.connection_failed_for_create(&choice.target, &error);
            }
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_flow_state_names_its_machine() {
        let choice = HostChoice {
            target: RemoteTarget::Alias {
                alias: "build-box".into(),
            },
            label: "build-box".into(),
            detail: "me@build-box".into(),
        };
        let flow = ConnectionState::Connecting {
            choice: choice.clone(),
        };
        assert_eq!(flow.choice(), Some(&choice));
        let flow = ConnectionState::Failed {
            choice: choice.clone(),
            error: "no route to host".into(),
        };
        assert_eq!(flow.choice(), Some(&choice));
    }

    fn choice(alias: &str) -> HostChoice {
        HostChoice {
            target: RemoteTarget::Alias {
                alias: alias.into(),
            },
            label: alias.into(),
            detail: String::new(),
        }
    }

    #[test]
    fn retired_and_replaced_attempts_cannot_apply_outcomes() {
        let mut flow = WindowConnection::default();
        let first = flow.begin(choice("build"));
        flow.retire_target(&choice("other").target);
        assert!(flow.is_connecting(first));
        flow.retire_target(&choice("build").target);
        assert!(flow.complete(first, Err("cancelled".into())).is_none());

        // Re-entering the same machine is a new window interaction too.
        let replacement = flow.begin(choice("build"));
        assert!(flow.complete(first, Err("old failure".into())).is_none());
        assert!(flow.is_connecting(replacement));
        let outcome = flow.complete(replacement, Err("new failure".into()));
        assert!(matches!(
            outcome,
            Some(ConnectionOutcome::Failed { error, .. }) if error == "new failure"
        ));
        assert!(flow.complete(replacement, Err("duplicate".into())).is_none());
        assert!(!flow.dismiss_failure(Some(&choice("other").target)));
        assert!(flow.dismiss_failure(Some(&choice("build").target)));
        assert!(flow.choice().is_none());
    }

    #[gpui_kit::test]
    fn a_late_window_result_cannot_redirect_a_replacement_or_its_create(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::VisualContext as _;

        let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
        app.update_in(&mut vcx, |app, window, cx| {
            let old = app.connect.begin(choice("old-machine"));
            app.connect.retire();
            let new = app.connect.begin(choice("new-machine"));
            app.pending_create = Some(crate::ui::switcher::PendingCreate {
                target: choice("new-machine").target,
                name: Some("keep-me".into()),
            });
            app.finish_connect(old, Err("old failure".into()), window, cx);
            assert!(app.connect.is_connecting(new));
            assert_eq!(
                app.pending_create.as_ref().unwrap().name.as_deref(),
                Some("keep-me")
            );
            assert!(app.host_snapshots.is_empty());
            app.finish_connect(new, Err("new failure".into()), window, cx);
            assert!(app.pending_create.is_none());
            assert!(matches!(app.connect.state(), Some(ConnectionState::Failed { choice, error })
                if choice.target == self::choice("new-machine").target && error == "new failure"));
        });
    }
}
