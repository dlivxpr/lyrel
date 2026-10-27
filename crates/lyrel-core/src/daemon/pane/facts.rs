use std::path::{Path, PathBuf};

use crate::core::cli_agent::{AgentEvent, AgentSessionState, CLIAgent};
use crate::daemon::protocol::{DaemonMsg, RemoteContext, ShellSpec};

pub(crate) mod output_marks;
pub(super) mod probes;
mod reader;
use output_marks::{ShellState, SniffSignals};
pub(super) use reader::{FactReader, ForegroundProbes};

/// Foreground observations and the transition rules which give them meaning.
/// Kept under the existing PaneState mutex; never takes the PTY master lock.
#[derive(Default)]
pub(super) struct PaneFacts {
    cwd: Option<PathBuf>,
    osc_title: Option<String>,
    shell: ShellState,
    // A new hop must prove its own integrated far prompt.
    remote_prompt_seen: bool,
    shell_spec: Option<ShellSpec>,
    remote: Option<RemoteContext>,
    agent: Option<CLIAgent>,
    agent_argv: Option<Vec<String>>,
    agent_session: Option<AgentSessionState>,
    kimi_observer: Option<crate::daemon::kimi::Observer>,
}

/// An owned machine delta, published only after releasing the pane lock.
/// Comparing it is intentionally conditional: ordinary output allocates none.
pub(super) struct FactChange(Option<ObservedFacts>);

impl FactChange {
    pub(super) fn publish(self, pane: u64, alive: bool, shutting_down: bool) {
        if !shutting_down && let Some(after) = self.0 {
            publish_facts(pane, alive, after);
        }
    }
}

impl PaneFacts {
    pub(super) fn new(
        cwd: Option<PathBuf>,
        osc_title: Option<String>,
        shell_spec: Option<ShellSpec>,
        remote: Option<RemoteContext>,
    ) -> Self {
        Self {
            cwd,
            osc_title,
            shell_spec,
            remote,
            ..Self::default()
        }
    }

    pub(super) fn cwd(&self) -> Option<&Path> {
        self.cwd.as_deref()
    }
    pub(super) fn osc_title(&self) -> Option<&str> {
        self.osc_title.as_deref()
    }
    pub(super) fn remote(&self) -> Option<&RemoteContext> {
        self.remote.as_ref()
    }
    pub(super) fn locality(&self) -> (bool, bool) {
        (
            self.remote
                .as_ref()
                .is_some_and(|remote| remote.kind != crate::daemon::protocol::RemoteKind::Ssh),
            self.remote.is_none(),
        )
    }
    pub(super) fn agent_state(&self, pane: u64) -> Option<crate::daemon::control::PaneAgentState> {
        agent_state_snapshot(self, pane)
    }

    pub(super) fn context(
        &self,
        local_pty: bool,
        bracketed_paste: bool,
    ) -> crate::daemon::protocol::PaneContext {
        crate::daemon::protocol::PaneContext {
            remote: self.remote.clone(),
            local_pty,
            at_prompt: self.shell.active.then_some(self.shell.mark_at_prompt),
            remote_prompt_seen: self.remote_prompt_seen,
            bracketed_paste: Some(bracketed_paste),
        }
    }

    pub(super) fn prompt(&self, foreground_command: bool) -> Option<DaemonMsg> {
        self.shell.active.then_some(DaemonMsg::Prompt {
            active: self.shell.active,
            at_prompt: self.shell.at_prompt && !foreground_command,
            last_exit: self.shell.last_exit_code,
        })
    }

    pub(super) fn replay_context(&self, send: &mut impl FnMut(DaemonMsg)) {
        if self.remote.is_some() {
            send(DaemonMsg::RemoteContext(self.remote.clone()));
        }
        // The client clears cwd on RemoteContext. The far directory must follow.
        if let Some(cwd) = &self.cwd {
            send(DaemonMsg::Cwd(cwd.clone()));
        }
    }

    pub(super) fn replay_agent(&self, send: &mut impl FnMut(DaemonMsg)) {
        if self.agent.is_some() {
            send(DaemonMsg::Agent(self.agent));
        }
        if self.agent_session.is_some() {
            send(DaemonMsg::AgentStatus(self.agent_session.clone()));
        }
    }

    pub(super) fn apply_output(
        &mut self,
        observation: reader::OutputObservation,
        emit: &mut dyn FnMut(DaemonMsg),
    ) -> FactChange {
        let before = observation.may_change_facts().then(|| observed_facts(self));
        observation.apply(self, emit);
        self.change_since(before)
    }

    fn change_since(&self, before: Option<ObservedFacts>) -> FactChange {
        FactChange(before.and_then(|before| {
            let after = observed_facts(self);
            facts_changed(&before, &after).then_some(after)
        }))
    }

    pub(super) fn apply_events(
        &mut self,
        events: Vec<AgentEvent>,
        notification: Option<String>,
        emit: &mut dyn FnMut(DaemonMsg),
    ) -> FactChange {
        let before = observed_facts(self);
        apply_agent_signals(self, events, notification, emit);
        self.change_since(Some(before))
    }

    pub(super) fn take_durable_observer(&mut self) -> Option<crate::daemon::kimi::Observer> {
        if self.remote.is_none() && self.agent == Some(CLIAgent::Kimi) {
            self.kimi_observer.take()
        } else {
            None
        }
    }

    pub(super) fn accept_durable_observation(
        &mut self,
        observer: crate::daemon::kimi::Observer,
        events: Result<Vec<AgentEvent>, std::io::Error>,
        emit: &mut dyn FnMut(DaemonMsg),
    ) -> FactChange {
        let matches = self.agent == Some(CLIAgent::Kimi)
            && self
                .agent_session
                .as_ref()
                .and_then(|session| session.session_id.as_deref())
                == Some(observer.session_id.as_str())
            && self
                .agent_session
                .as_ref()
                .and_then(|session| session.readout.session_scope.as_ref())
                .and_then(|scope| scope.session_file.as_ref())
                .and_then(|file| file.parent())
                .map(|dir| dir.join("agents/main/wire.jsonl"))
                == Some(observer.file.clone());
        if !matches || self.kimi_observer.is_some() {
            return FactChange(None);
        }
        let before = observed_facts(self);
        self.kimi_observer = Some(observer);
        match events {
            Ok(events) => apply_agent_signals(self, events, None, emit),
            Err(error) => {
                let message =
                    format!("Kimi durable state unavailable: {error}; completion is unknown");
                if let Some(session) = &mut self.agent_session
                    && session.message.as_deref() != Some(message.as_str())
                {
                    session.message = Some(message);
                    emit(DaemonMsg::AgentStatus(self.agent_session.clone()));
                }
            }
        }
        self.change_since(Some(before))
    }

    #[cfg(unix)]
    pub(super) fn from_carried(carried: &mut super::Carried) -> Self {
        let agent_session = carried.agent_session.take();
        let kimi_observer = if carried.agent == Some(CLIAgent::Kimi) {
            agent_session.as_ref().and_then(kimi_observer)
        } else {
            None
        };
        Self {
            cwd: carried.cwd.take(),
            osc_title: carried.osc_title.take(),
            shell_spec: carried.shell_spec.take(),
            shell: ShellState {
                active: carried.shell_active,
                at_prompt: carried.at_prompt,
                last_exit_code: carried.last_exit,
                command: None,
                mark_at_prompt: false,
            },
            // The handoff wire format does not carry proof of a far prompt.
            remote_prompt_seen: false,
            remote: carried.remote.take(),
            agent: carried.agent,
            agent_argv: carried.agent_argv.take(),
            agent_session,
            kimi_observer,
        }
    }

    #[cfg(unix)]
    pub(super) fn write_carried(&self, mut carried: super::Carried) -> super::Carried {
        carried.cwd = self.cwd.clone();
        carried.osc_title = self.osc_title.clone();
        carried.shell_spec = self.shell_spec.clone();
        carried.shell_active = self.shell.active;
        carried.at_prompt = self.shell.at_prompt;
        carried.last_exit = self.shell.last_exit_code;
        carried.remote = self.remote.clone();
        carried.agent = self.agent;
        carried.agent_argv = self.agent_argv.clone();
        carried.agent_session = self.agent_session.clone();
        carried
    }
}

fn agent_state_snapshot(
    st: &PaneFacts,
    pane: u64,
) -> Option<crate::daemon::control::PaneAgentState> {
    st.agent_session
        .clone()
        .map(|state| crate::daemon::control::PaneAgentState {
            pane_id: pane,
            agent: st.agent,
            state,
        })
}

/// What the daemon has learned about a pane that the machine tree wants to
/// hold. Compared before and after each read, and written out only when it
/// moved — every field here costs a `PaneFacts` delta to every attached client.
#[derive(Debug, PartialEq)]
struct ObservedFacts {
    cwd: Option<String>,
    osc_title: Option<String>,
    agent: Option<crate::core::machine::AgentFacts>,
    shell: Option<ShellSpec>,
}

fn observed_facts(st: &PaneFacts) -> ObservedFacts {
    let cwd = st.cwd.as_ref().map(|p| p.to_string_lossy().into_owned());
    let agent = st.agent.map(|agent| crate::core::machine::AgentFacts {
        agent,
        session_id: st.agent_session.as_ref().and_then(|s| s.session_id.clone()),
        launch_argv: st
            .agent_session
            .as_ref()
            .and_then(|s| s.launch_argv.clone())
            .or_else(|| st.agent_argv.clone()),
        session_scope: st
            .agent_session
            .as_ref()
            .and_then(|s| s.readout.session_scope.as_ref())
            .map(crate::host::session_operations::durable_scope),
        status: st.agent_session.as_ref().map(|s| s.status),
    });
    ObservedFacts {
        cwd,
        osc_title: st.osc_title.clone(),
        agent,
        shell: st.shell_spec.clone(),
    }
}

fn publish_facts(pane: u64, alive: bool, after: ObservedFacts) {
    crate::core::machine::observe_pane(pane, |p| {
        if after.cwd.is_some() {
            p.cwd = after.cwd;
        }
        // Unlike the others this one is also cleared by a reset, so it is
        // assigned either way.
        p.osc_title = after.osc_title;
        p.agent = after.agent;
        if after.shell.is_some() {
            p.shell = after.shell;
        }
        if alive {
            p.live = true;
        }
    });
}

fn facts_changed(before: &ObservedFacts, after: &ObservedFacts) -> bool {
    before.cwd != after.cwd
        || before.osc_title != after.osc_title
        || agent_facts_changed(before.agent.as_ref(), after.agent.as_ref())
        || before.shell != after.shell
}

fn agent_facts_changed(
    before: Option<&crate::core::machine::AgentFacts>,
    after: Option<&crate::core::machine::AgentFacts>,
) -> bool {
    match (before, after) {
        (None, None) => false,
        (Some(a), Some(b)) => {
            a.agent != b.agent
                || a.session_id != b.session_id
                || a.launch_argv != b.launch_argv
                || a.session_scope != b.session_scope
                || a.status != b.status
        }
        _ => true,
    }
}

fn apply_signals(st: &mut PaneFacts, signals: SniffSignals, emit: &mut dyn FnMut(DaemonMsg)) {
    if let Some(cwd) = signals.cwd {
        if st.cwd.as_ref() != Some(&cwd) {
            emit(DaemonMsg::Cwd(cwd.clone()));
            st.cwd = Some(cwd);
        }
    }
    if let Some(title) = signals.title {
        // No `notify`: a window renders its own tabs from its own terminal,
        // which parsed the same sequence. This is only for the tree.
        st.osc_title = (!title.is_empty()).then_some(title);
    }
    for shell in signals.shell {
        #[cfg(windows)]
        if shell_mark_capture_changed(&st.shell, &shell) {
            apply_agent(
                st,
                agent_from_shell_mark(&shell, &crate::core::config::agent_detection_aliases()),
                emit,
            );
        }
        st.shell = shell.clone();
        emit(DaemonMsg::Prompt {
            active: shell.active,
            at_prompt: shell.at_prompt,
            last_exit: shell.last_exit_code,
        });
    }
    apply_agent_signals(st, signals.agent_events, signals.notification, emit);
}

#[cfg_attr(not(windows), allow(dead_code))]
fn shell_mark_capture_changed(prev: &ShellState, next: &ShellState) -> bool {
    prev.command != next.command
}

#[cfg_attr(not(windows), allow(dead_code))]
fn agent_from_shell_mark(
    shell: &ShellState,
    custom: &std::collections::HashMap<String, String>,
) -> Option<(crate::core::cli_agent::CLIAgent, Vec<String>)> {
    let cmd = shell.command.as_deref()?;
    let agent = crate::core::cli_agent::CLIAgent::detect_from_command_with(cmd, custom)?;
    Some((agent, crate::core::cli_agent::command_argv(cmd)))
}

fn kimi_observer(
    session: &crate::core::cli_agent::AgentSessionState,
) -> Option<crate::daemon::kimi::Observer> {
    let id = session.session_id.clone()?;
    let file = session
        .readout
        .session_scope
        .as_ref()?
        .session_file
        .as_ref()?
        .parent()?
        .join("agents/main/wire.jsonl");
    Some(crate::daemon::kimi::Observer::new(
        id,
        file,
        session.mid_turn(),
    ))
}

fn apply_agent_signals(
    st: &mut PaneFacts,
    events: Vec<crate::core::cli_agent::AgentEvent>,
    notification: Option<String>,
    emit: &mut dyn FnMut(DaemonMsg),
) {
    use crate::core::cli_agent::{AgentEventKind, AgentSessionState, AgentStatus};

    if events.is_empty() && notification.is_none() {
        return;
    }
    let before = st.agent_session.clone();
    let mut ended_session = false;

    for event in &events {
        if event.agent == Some(crate::core::cli_agent::CLIAgent::Kimi)
            && !matches!(
                event.kind,
                crate::core::cli_agent::AgentEventKind::SessionStart
                    | crate::core::cli_agent::AgentEventKind::SessionEnd
            )
            && event.source.as_deref() != Some("kimi-durable-wire")
            && st
                .agent_session
                .as_ref()
                .and_then(|session| session.readout.session_scope.as_ref())
                .is_some_and(|scope| scope.session_file.is_some())
        {
            // Native hooks run fire-and-forget and may arrive after turn.ended.
            // Once bound, only the ordered main wire owns this session's state.
            continue;
        }
        // A hook names the agent that emitted it. Once this pane has a
        // foreground agent, a different one's session id must not land here:
        // omp forking with a Claude UUID is "session not found".
        if let (Some(current), Some(from)) = (st.agent, event.agent)
            && current != from
        {
            continue;
        }
        if event.kind == AgentEventKind::SessionEnd {
            if let (Some(current), Some(ended)) = (
                st.agent_session
                    .as_ref()
                    .and_then(|s| s.session_id.as_ref()),
                event.session_id.as_ref(),
            ) && current != ended
            {
                continue;
            }
            // Shutdown releases ownership, not just the turn status. Handle it
            // before a hook can claim the pane, including after a process probe
            // has already observed the foreground agent leaving.
            st.agent_session = None;
            st.agent_argv = None;
            st.kimi_observer = None;
            if st.agent.take().is_some() {
                emit(DaemonMsg::Agent(None));
            }
            ended_session = true;
            continue;
        }
        if st.agent.is_none() && event.agent.is_some() {
            st.agent = event.agent;
            emit(DaemonMsg::Agent(st.agent));
        }
        st.agent_session
            .get_or_insert_with(AgentSessionState::default)
            .apply_event(event);
        if event.agent == Some(crate::core::cli_agent::CLIAgent::Kimi)
            && event.kind == AgentEventKind::SessionStart
        {
            st.kimi_observer = st.agent_session.as_ref().and_then(kimi_observer);
        }
    }

    if let Some(body) = notification
        && st.agent.is_some()
        && !st.agent_session.as_ref().is_some_and(|s| s.rich)
    {
        let sess = st
            .agent_session
            .get_or_insert_with(AgentSessionState::default);
        sess.status = AgentStatus::Waiting;
        sess.message = Some(body);
    }

    if let (Some(sess), Some(argv)) = (&mut st.agent_session, &st.agent_argv)
        && sess.launch_argv.is_none()
    {
        sess.launch_argv = Some(argv.clone());
    }

    if ended_session || st.agent_session != before {
        emit(DaemonMsg::AgentStatus(st.agent_session.clone()));
    }
}

fn apply_probed_cwd(st: &mut PaneFacts, probed: Option<PathBuf>, emit: &mut dyn FnMut(DaemonMsg)) {
    let Some(probed) = probed else {
        return;
    };
    if st.remote.is_some() {
        return;
    }
    if st.cwd.as_deref().is_some_and(|cur| same_dir(cur, &probed)) {
        return;
    }
    emit(DaemonMsg::Cwd(probed.clone()));
    st.cwd = Some(probed);
}

fn same_dir(a: &Path, b: &Path) -> bool {
    a == b
        || match (a.canonicalize(), b.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
}

/// Take a batch of prompt marks out of the local line editor's reach.
///
/// Called when a foreground program owns the pty, which means the marks are
/// being relayed — an `ssh` passing the far shell's prompt through, a nested
/// shell drawing its own. The local editor must not engage on those, so
/// `at_prompt` is cleared across the whole batch.
///
/// [`ShellState::mark_at_prompt`] is deliberately left standing: it is the
/// same marks read for a different question, and on a remote pane it is the
/// only thing this machine knows about the far shell (#840). Because of it,
/// dedup has to compare what is actually emitted rather than the whole struct
/// — two marks that used to collapse into one `Prompt` message must still
/// collapse when only their unsuppressed twin tells them apart, and the
/// survivor has to carry the *newest* twin: a whole turn can arrive in one read
/// (`D`, the prompt's `A`/`B`, then the next command's `C`), and dropping the
/// last entry's reading would hand the pane back a prompt it has already left.
fn suppress_relayed_prompt_marks(shell: &mut Vec<ShellState>) {
    for s in shell.iter_mut() {
        s.at_prompt = false;
    }
    shell.dedup_by(|a, b| {
        let same = a.active == b.active
            && a.at_prompt == b.at_prompt
            && a.last_exit_code == b.last_exit_code
            && a.command == b.command;
        // `dedup_by` keeps `b`, the earlier of the pair, and drops `a`. Move
        // the reading over first so the collapse costs a `Prompt` message and
        // nothing else.
        if same {
            b.mark_at_prompt = a.mark_at_prompt;
        }
        same
    });
}

fn apply_remote_context(
    st: &mut PaneFacts,
    remote: Option<RemoteContext>,
    emit: &mut dyn FnMut(DaemonMsg),
) {
    if st.remote == remote {
        return;
    }
    st.cwd = None;
    // A new far side has to prove its own shell integration. The mark that is
    // standing right now belongs to whatever the pane was before this hop —
    // the near shell's "I started `ssh`", or the previous host's prompt.
    st.remote_prompt_seen = false;
    emit(DaemonMsg::RemoteContext(remote.clone()));
    st.remote = remote;
}

/// Record that this read carried a prompt mark while the pane was remote.
///
/// Called *after* [`apply_remote_context`], not with the signals: the read that
/// first sees a pane as remote is very often the one carrying the far shell's
/// opening prompt, and latching before the hop was applied would throw exactly
/// that mark away — the reset above would clear it a line later. Ordering it
/// here means the far side's first prompt counts, which is what makes a later
/// "not at a prompt" mean "the remote is busy" rather than "we never heard
/// from it" (#840).
fn latch_remote_prompt(st: &mut PaneFacts, saw_prompt_mark: bool) {
    st.remote_prompt_seen |= saw_prompt_mark && st.remote.is_some();
}

fn apply_agent(
    st: &mut PaneFacts,
    detected: Option<(crate::core::cli_agent::CLIAgent, Vec<String>)>,
    emit: &mut dyn FnMut(DaemonMsg),
) {
    let (agent, argv) = match detected {
        Some((agent, argv)) => (Some(agent), Some(argv)),
        None => (None, None),
    };
    if st.agent == agent {
        stamp_launch_argv(st, argv, emit);
        return;
    }
    // The session belongs to whoever was in the foreground. Switching from
    // Claude to omp (or back to the shell) must not keep the previous id.
    if st.agent_session.is_some() {
        st.agent_session = None;
        emit(DaemonMsg::AgentStatus(None));
    }
    if agent.is_none() {
        st.agent_argv = None;
    }
    emit(DaemonMsg::Agent(agent));
    st.agent = agent;
    stamp_launch_argv(st, argv, emit);
}

fn stamp_launch_argv(
    st: &mut PaneFacts,
    argv: Option<Vec<String>>,
    emit: &mut dyn FnMut(DaemonMsg),
) {
    let Some(argv) = argv else { return };
    if argv.is_empty() {
        return;
    }
    if st.agent_argv.as_ref() == Some(&argv) {
        return;
    }
    st.agent_argv = Some(argv.clone());
    if let Some(sess) = &mut st.agent_session
        && sess.launch_argv.as_ref() != Some(&argv)
    {
        sess.launch_argv = Some(argv);
        emit(DaemonMsg::AgentStatus(st.agent_session.clone()));
    }
}

#[cfg(test)]
mod agent_tests;
#[cfg(test)]
mod marks_tests;
#[cfg(test)]
mod probes_tests;
#[cfg(test)]
mod publication_tests;
