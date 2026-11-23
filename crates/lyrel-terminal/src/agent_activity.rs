//! Agent observations, read marks and notification intent for one retained pane view.
//!
//! Read marks belong to the app lifetime and are keyed by Host and daemon pane;
//! delivery still targets the retained GPUI entity, never that daemon id.

use gpui_kit::{App, Task};
use lyrel_core::core::cli_agent::{AgentSessionState, AgentStatus, CLIAgent};
use lyrel_ui_base::host_ops::HostId;
use lyrel_ui_base::i18n::{L10nKey, t, t_fmt};
use lyrel_ui_base::notification_history::Severity;
use std::time::{Duration, Instant};

type PaneKey = (HostId, u64);

/// A queued message or Stop hook can resume work before Done is stable.
pub const DONE_SETTLE: Duration = Duration::from_millis(1500);

#[derive(Default)]
struct AgentReadMarks(std::collections::HashMap<PaneKey, AgentReadMark>);
impl gpui_kit::Global for AgentReadMarks {}

#[derive(Clone)]
struct AgentReadMark {
    session: (Option<String>, Option<Vec<String>>),
    // A non-Done mark still proves this app watched the pane before rebuilding.
    status: Option<AgentStatus>,
    turns: u64,
    unread: bool,
}

pub struct AgentActivity {
    seen_agent: Option<CLIAgent>,
    detection_armed: bool,
    last_status: Option<AgentStatus>,
    last_session: (Option<String>, Option<Vec<String>>),
    last_session_title: Option<String>,
    turn_started: Option<Instant>,
    pending_finish_notice: Option<Task<()>>,
    was_rich: bool,
    unread: bool,
    keep_unread_on_focus: bool,
    status_seen: bool,
    last_tool_activity: u64,
}

#[non_exhaustive]
pub struct AgentNotice {
    pub agent: &'static str,
    pub body: String,
    pub delayed: bool,
    /// How the settled result reads in the shared history.
    pub severity: Severity,
    /// The session-and-transition half of the stable event identity. The view
    /// prepends its source (Host, pane, ownership); what remains here names
    /// the session and the transition, so the same turn settling is recorded
    /// once no matter how many views observed it. Durations never belong in
    /// it — two viewers' clocks must not split one event into two records.
    pub identity: String,
}

/// What the terminal reports about the pane's agent right now: the detected
/// CLI agent, its live session state, and the status a relink replayed when
/// this attach is replaying one.
pub struct AgentObservation<'a> {
    pub agent: Option<CLIAgent>,
    pub session: Option<&'a AgentSessionState>,
    pub replayed: Option<Option<AgentStatus>>,
}

#[derive(Default)]
#[non_exhaustive]
pub struct AgentUpdate {
    pub session_changed: bool,
    pub session_id_changed: bool,
    pub changed: bool,
    pub turn_finished: bool,
    pub notice: Option<AgentNotice>,
}

impl AgentActivity {
    pub fn new(reattached: bool) -> Self {
        Self {
            seen_agent: None,
            detection_armed: !reattached,
            last_status: None,
            last_session: (None, None),
            last_session_title: None,
            turn_started: None,
            pending_finish_notice: None,
            was_rich: false,
            unread: false,
            keep_unread_on_focus: false,
            status_seen: false,
            last_tool_activity: 0,
        }
    }

    /// A reattached running agent is not a new launch. Arm after seeing a prompt.
    pub fn detect(&mut self, agent: Option<CLIAgent>, at_prompt: bool) -> Option<CLIAgent> {
        let detected = if agent != self.seen_agent {
            self.seen_agent = agent;
            agent.filter(|_| self.detection_armed)
        } else {
            None
        };
        if agent.is_none() && at_prompt {
            self.detection_armed = true;
        }
        detected
    }

    /// Tool activity is a semantic edge even when status stays Working.
    pub fn tool_activity_changed(&mut self, session: Option<&AgentSessionState>) -> bool {
        match session.map(|s| s.activity) {
            Some(n) => std::mem::replace(&mut self.last_tool_activity, n) != n,
            None => {
                self.last_tool_activity = 0;
                false
            }
        }
    }

    pub fn was_rich(&self) -> bool {
        self.was_rich
    }

    pub fn unread(&self) -> bool {
        self.unread
    }

    pub fn mark_unread(&mut self, key: PaneKey, refocus_incoming: bool, cx: &mut App) {
        self.unread = true;
        self.keep_unread_on_focus = refocus_incoming;
        self.note_unread(key, cx);
    }

    pub fn focused(&mut self, key: PaneKey, cx: &mut App) {
        if self.keep_unread_on_focus {
            self.keep_unread_on_focus = false;
        } else {
            self.unread = false;
            self.note_unread(key, cx);
        }
    }

    fn record(&self, key: PaneKey, turns: u64, cx: &mut App) {
        cx.default_global::<AgentReadMarks>().0.insert(
            key,
            AgentReadMark {
                session: self.last_session.clone(),
                status: self.last_status,
                turns,
                unread: self.unread,
            },
        );
    }

    fn note_unread(&self, key: PaneKey, cx: &mut App) {
        if !cx.has_global::<AgentReadMarks>() {
            return;
        }
        if let Some(mark) = cx.global_mut::<AgentReadMarks>().0.get_mut(&key) {
            mark.unread = self.unread;
        }
    }

    /// The adapter owns scheduling; retaining its task makes every new status
    /// cancel a still-settling Done notice without another timer or allocation.
    pub fn await_notice(&mut self, task: Task<()>) {
        self.pending_finish_notice = Some(task);
    }

    pub fn notice_delivered(&mut self) {
        self.pending_finish_notice = None;
    }

    /// Observe the pane's current agent state. Eligibility for a notice —
    /// a rich session's Waiting edge or a settled Done — does not depend on
    /// the desktop delivery mode: a held-back note still belongs in the
    /// history, and the caller decides delivery separately.
    pub fn observe(
        &mut self,
        key: PaneKey,
        observation: AgentObservation<'_>,
        focused: bool,
        cx: &mut App,
    ) -> AgentUpdate {
        let AgentObservation {
            agent,
            session,
            replayed,
        } = observation;
        // A cold attach adopts stored work as its baseline. A mark from an
        // earlier view takes precedence; relink's reader does not offer replay.
        let adopted_baseline = match replayed {
            Some(restored)
                if !cx
                    .try_global::<AgentReadMarks>()
                    .is_some_and(|marks| marks.0.contains_key(&key)) =>
            {
                self.last_status = restored;
                self.status_seen = true;
                true
            }
            _ => false,
        };
        if session.is_some_and(|s| s.rich) {
            self.was_rich = true;
        }
        if agent.is_none() && session.is_none() {
            self.was_rich = false;
        }
        let identity = (
            session.and_then(|s| s.session_id.clone()),
            session.and_then(|s| s.launch_argv.clone()),
        );
        let mut update = AgentUpdate::default();
        if identity != self.last_session {
            update.session_id_changed = self.last_session.0.is_some()
                && identity.0.is_some()
                && identity.0 != self.last_session.0;
            self.last_session = identity;
            update.session_changed = true;
        }
        let session_title = session.and_then(|s| s.readout.session_title.clone());
        if session_title != self.last_session_title {
            self.last_session_title = session_title;
            update.session_changed = true;
            update.changed = true;
        }
        let status = session.map(|s| s.status);
        let turns = session.map_or(0, |s| s.turns);
        if adopted_baseline {
            self.record(key, turns, cx);
        }
        if status == self.last_status {
            return update;
        }
        let prev = std::mem::replace(&mut self.last_status, status);
        let first_sight = !std::mem::replace(&mut self.status_seen, true);
        self.pending_finish_notice = None;
        update.changed = true;

        // Rebuilding is not reading. Restore only the exact session and turn
        // the previous view recorded, using live focus rather than a cached bit.
        if first_sight
            && status == Some(AgentStatus::Done)
            && let Some(mark) = cx
                .try_global::<AgentReadMarks>()
                .and_then(|marks| marks.0.get(&key))
                .filter(|mark| {
                    mark.status == Some(AgentStatus::Done)
                        && mark.session == self.last_session
                        && mark.turns == turns
                })
                .cloned()
        {
            self.unread = mark.unread && !focused;
            self.keep_unread_on_focus = false;
            self.record(key, turns, cx);
            return update;
        }
        update.turn_finished = status == Some(AgentStatus::Done) && prev != Some(AgentStatus::Done);
        match status {
            Some(AgentStatus::Done) if prev != Some(AgentStatus::Done) => {
                self.unread = !focused;
                self.keep_unread_on_focus = false;
            }
            Some(AgentStatus::Done) => {}
            _ => {
                self.unread = false;
                self.keep_unread_on_focus = false;
            }
        }
        self.record(key, turns, cx);
        let rich = session.is_some_and(|s| s.rich);
        let agent_name = agent.map(|a| a.display_name()).unwrap_or("Agent");
        match status {
            Some(AgentStatus::Working) => self.turn_started = Some(Instant::now()),
            Some(AgentStatus::Waiting) if rich => {
                let message = session.and_then(|s| s.message.clone());
                update.notice = Some(AgentNotice {
                    agent: agent_name,
                    body: message
                        .clone()
                        .unwrap_or_else(|| t(L10nKey::NotifyAgentWaiting).to_string()),
                    delayed: false,
                    severity: Severity::Warning,
                    identity: format!(
                        "agent-wait|{}|{}|{}|{}",
                        session_identity(session),
                        turns,
                        session.map_or(0, |s| s.activity),
                        message.unwrap_or_default()
                    ),
                });
            }
            Some(AgentStatus::Done)
                if rich
                    && matches!(
                        prev,
                        Some(AgentStatus::Working) | Some(AgentStatus::Waiting)
                    ) =>
            {
                let elapsed = self.turn_started.take().map(|start| start.elapsed());
                let result = session.and_then(|s| s.readout.result.clone());
                let body = match result.as_ref() {
                    Some(result) => {
                        let result = lyrel_ui_base::i18n::agent_result_text(result);
                        match elapsed {
                            Some(elapsed) => t_fmt(
                                L10nKey::AgentResultWithDuration,
                                &[
                                    ("result", &result),
                                    ("secs", &elapsed.as_secs().to_string()),
                                ],
                            ),
                            None => result,
                        }
                    }
                    None => match elapsed {
                        Some(elapsed) => t_fmt(
                            L10nKey::NotifyAgentFinished,
                            &[("secs", &elapsed.as_secs().to_string())],
                        ),
                        None => t(L10nKey::NotifyTurnFinished).to_string(),
                    },
                };
                update.notice = Some(AgentNotice {
                    agent: agent_name,
                    severity: result.as_ref().map_or(Severity::Info, |result| {
                        use lyrel_core::core::cli_agent::AgentOutcome;
                        match result.outcome {
                            AgentOutcome::Succeeded => Severity::Success,
                            AgentOutcome::Failed | AgentOutcome::Rejected => Severity::Error,
                            AgentOutcome::Interrupted => Severity::Warning,
                        }
                    }),
                    identity: format!(
                        "agent-done|{}|{}|{}|{result:?}",
                        session_identity(session),
                        turns,
                        session.map_or(0, |s| s.activity),
                    ),
                    body,
                    delayed: true,
                });
            }
            _ => {}
        }
        update
    }
}

/// The session half of a stable event identity: what the agent itself
/// reported, nothing inferred. Two views of the same daemon pane hash the
/// same string.
fn session_identity(session: Option<&AgentSessionState>) -> String {
    match session {
        Some(session) => format!(
            "{}|{:?}",
            session.session_id.as_deref().unwrap_or(""),
            session.launch_argv.as_deref().unwrap_or(&[])
        ),
        None => "|".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::TestAppContext;
    use lyrel_core::core::cli_agent::{AgentSessionState, AgentStatus, CLIAgent};

    #[gpui_kit::test]
    fn session_id_changed_only_fires_between_two_active_sessions(cx: &mut TestAppContext) {
        let mut activity = AgentActivity::new(false);
        let key = (HostId(0), 1);
        let state = |sid: &str| AgentSessionState {
            status: AgentStatus::Done,
            message: None,
            session_id: Some(sid.into()),
            launch_argv: Some(vec!["omp".into()]),
            rich: true,
            cwd: None,
            activity: 0,
            turns: 0,
            pending_requests: Vec::new(),
            request_resume_status: Default::default(),
            pending_stop: false,
            readout: Default::default(),
            since_ms: None,
        };

        cx.update(|cx| {
            let s1 = state("sid-1");
            let first = activity.observe(
                key,
                AgentObservation {
                    agent: Some(CLIAgent::OhMyPi),
                    session: Some(&s1),
                    replayed: None,
                },
                true,
                cx,
            );
            assert!(!first.session_id_changed);

            let s2 = state("sid-2");
            let resumed = activity.observe(
                key,
                AgentObservation {
                    agent: Some(CLIAgent::OhMyPi),
                    session: Some(&s2),
                    replayed: None,
                },
                true,
                cx,
            );
            assert!(resumed.session_id_changed);

            let exited = activity.observe(
                key,
                AgentObservation {
                    agent: None,
                    session: None,
                    replayed: None,
                },
                true,
                cx,
            );
            assert!(
                !exited.session_id_changed,
                "returning to shell must not clear a fresh OSC title"
            );
        });
    }
}
