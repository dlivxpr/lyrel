use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::core::cli_agent::CLIAgent;
use crate::daemon::protocol::RemoteContext;

use super::output_marks::OscSniffer;
use super::{PaneFacts, SniffSignals, suppress_relayed_prompt_marks};

const REMOTE_CONTEXT_POLL_INTERVAL: Duration = Duration::from_millis(500);

pub(in super::super) struct ForegroundProbes {
    /// One cheap ioctl per read; full process probes run only on a hop or interval.
    pub group: Box<dyn Fn() -> Option<i32> + Send>,
    pub remote: Box<dyn Fn() -> Option<RemoteContext> + Send>,
    pub agent: Box<dyn Fn() -> Option<Option<(CLIAgent, Vec<String>)>> + Send>,
    pub cwd: Box<dyn Fn() -> Option<PathBuf> + Send>,
    pub local_group: fn(i32) -> Option<bool>,
}

pub(in super::super) struct OutputObservation {
    signals: SniffSignals,
    remote: Option<Option<RemoteContext>>,
    agent: Option<Option<(CLIAgent, Vec<String>)>>,
    cwd: Option<PathBuf>,
}

impl OutputObservation {
    pub(super) fn may_change_facts(&self) -> bool {
        self.signals.cwd.is_some()
            || self.signals.title.is_some()
            || !self.signals.agent_events.is_empty()
            || self.signals.notification.is_some()
            || !self.signals.shell.is_empty()
            || self.remote.is_some()
            || self.agent.is_some()
            || self.cwd.is_some()
    }

    pub(super) fn apply(
        self,
        facts: &mut PaneFacts,
        emit: &mut dyn FnMut(crate::daemon::protocol::DaemonMsg),
    ) {
        let saw_prompt_mark = self.signals.shell.iter().any(|shell| shell.mark_at_prompt);
        super::apply_signals(facts, self.signals, emit);
        if let Some(remote) = self.remote {
            super::apply_remote_context(facts, remote, emit);
        }
        // A hop must land before its first integrated remote prompt is latched.
        super::latch_remote_prompt(facts, saw_prompt_mark);
        if let Some(agent) = self.agent {
            super::apply_agent(facts, agent, emit);
        }
        super::apply_probed_cwd(facts, self.cwd, emit);
    }
}

/// Ordered reader-local parsing and probe cadence; no locks or asynchronous consumers.
pub(in super::super) struct FactReader {
    sniffer: OscSniffer,
    probes: ForegroundProbes,
    next_remote_check: Instant,
    last_group: Option<i32>,
}

pub(in super::super) struct ReadProbe {
    managed: bool,
    poll_before: bool,
    remote: Option<Option<RemoteContext>>,
    media_local: bool,
}

impl ReadProbe {
    pub(in super::super) fn media_local(&self) -> bool {
        self.media_local
    }
}

impl FactReader {
    pub(in super::super) fn new(probes: ForegroundProbes) -> Self {
        Self {
            sniffer: OscSniffer::new(),
            probes,
            next_remote_check: Instant::now(),
            last_group: None,
        }
    }

    /// Called without PaneState locked: probes may take the existing PTY lock.
    pub(in super::super) fn before_read(
        &mut self,
        (managed, cached_local): (bool, bool),
    ) -> ReadProbe {
        let group = (self.probes.group)();
        let poll_before = group != self.last_group || Instant::now() >= self.next_remote_check;
        self.last_group = group;
        let remote = (poll_before && !managed).then(&self.probes.remote);
        let media_local = group.is_some() && remote.as_ref().map_or(cached_local, Option::is_none);
        ReadProbe {
            managed,
            poll_before,
            remote,
            media_local,
        }
    }

    /// Cached pgids are facts, not permission: exec ssh can retain its group.
    /// Called afresh only when an indirect medium is accepted by the parser.
    pub(in super::super) fn indirect_media_locality(&self, before: &ReadProbe) -> Option<bool> {
        if before.managed {
            return Some(false);
        }
        let pgid = (self.probes.group)()?;
        if (self.probes.remote)().is_some() {
            Some(false)
        } else {
            (self.probes.local_group)(pgid)
        }
    }

    pub(in super::super) fn observe(
        &mut self,
        bytes: &[u8],
        foreground_running: &impl Fn() -> bool,
        mut before: ReadProbe,
    ) -> OutputObservation {
        let mut signals = self.sniffer.feed(bytes);
        // Most reads contain no mark, so do not pay for a foreground syscall.
        if signals.shell.iter().any(|shell| shell.at_prompt) && foreground_running() {
            suppress_relayed_prompt_marks(&mut signals.shell);
        }
        // Probe on the final near-shell prompt even inside the interval: it may
        // be the last output after ssh exits, and must clear remote history scope.
        let back_at_prompt = signals.shell.iter().any(|shell| shell.at_prompt);
        let poll_now = back_at_prompt || before.poll_before;
        if poll_now {
            self.next_remote_check = Instant::now() + REMOTE_CONTEXT_POLL_INTERVAL;
        }
        if back_at_prompt && !before.poll_before && !before.managed {
            before.remote = Some((self.probes.remote)());
        }
        OutputObservation {
            signals,
            remote: before.remote,
            agent: poll_now.then(&self.probes.agent).flatten(),
            cwd: poll_now.then(&self.probes.cwd).flatten(),
        }
    }
}
