//! Agent observations, read marks and notification intent for one retained pane view.
//!
//! Read marks belong to the app lifetime and are keyed by Host and daemon pane;
//! delivery still targets the retained GPUI entity, never that daemon id.

use crate::core::cli_agent::{AgentSessionState, AgentStatus, CLIAgent};
use crate::ui::host_ops::HostId;
use crate::ui::i18n::{L10nKey, t, t_fmt};
use gpui_kit::{App, Task};
use std::time::{Duration, Instant};

type PaneKey = (HostId, u64);

/// A queued message or Stop hook can resume work before Done is stable.
pub(super) const DONE_SETTLE: Duration = Duration::from_millis(1500);

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

pub(super) struct AgentActivity {
    seen_agent: Option<CLIAgent>,
    detection_armed: bool,
    last_status: Option<AgentStatus>,
    last_session: (Option<String>, Option<Vec<String>>),
    turn_started: Option<Instant>,
    pending_finish_notice: Option<Task<()>>,
    was_rich: bool,
    unread: bool,
    keep_unread_on_focus: bool,
    status_seen: bool,
    last_tool_activity: u64,
}

#[non_exhaustive]
pub(super) struct AgentNotice {
    pub agent: &'static str,
    pub body: String,
    pub delayed: bool,
}

#[derive(Default)]
#[non_exhaustive]
pub(super) struct AgentUpdate {
    pub session_changed: bool,
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

    pub fn observe(
        &mut self,
        key: PaneKey,
        agent: Option<CLIAgent>,
        session: Option<&AgentSessionState>,
        replayed: Option<Option<AgentStatus>>,
        focused: bool,
        notify_allowed: bool,
        cx: &mut App,
    ) -> AgentUpdate {
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
            self.last_session = identity;
            update.session_changed = true;
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
            Some(AgentStatus::Waiting) if rich && notify_allowed => {
                update.notice = Some(AgentNotice {
                    agent: agent_name,
                    body: session
                        .and_then(|s| s.message.clone())
                        .unwrap_or_else(|| t(L10nKey::NotifyAgentWaiting).to_string()),
                    delayed: false,
                });
            }
            Some(AgentStatus::Done)
                if rich
                    && notify_allowed
                    && matches!(
                        prev,
                        Some(AgentStatus::Working) | Some(AgentStatus::Waiting)
                    ) =>
            {
                let elapsed = self.turn_started.take().map(|start| start.elapsed());
                let body = match session.and_then(|s| s.readout.result.as_ref()) {
                    Some(result) => {
                        let result = crate::ui::i18n::agent_result_text(result);
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
                    body,
                    delayed: true,
                });
            }
            _ => {}
        }
        update
    }
}
