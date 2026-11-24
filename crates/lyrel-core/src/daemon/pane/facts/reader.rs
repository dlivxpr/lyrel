use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::core::cli_agent::CLIAgent;
use crate::daemon::procinfo::MediaProcessRoot;
use crate::daemon::protocol::RemoteContext;

use super::output_marks::OscSniffer;
use super::{PaneFacts, SniffSignals, suppress_relayed_prompt_marks};

const REMOTE_CONTEXT_POLL_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Default)]
pub(in super::super) enum AgentProbe {
    #[default]
    Unavailable,
    #[cfg(any(unix, test))]
    Foreground(Option<(CLIAgent, Vec<String>)>),
    #[cfg(windows)]
    AntigravityChild(bool),
}

pub(in super::super) struct ForegroundProbes {
    /// Unix foreground PGID, or Windows shell-tree PID (ConPTY has no PGID).
    /// Cheap per-read identity; full probes run only on a hop or interval.
    pub group: Box<dyn Fn() -> Option<MediaProcessRoot> + Send>,
    pub remote: Box<dyn Fn() -> Option<RemoteContext> + Send>,
    pub agent: Box<dyn Fn() -> AgentProbe + Send>,
    pub cwd: Box<dyn Fn() -> Option<PathBuf> + Send>,
    pub local_group: fn(MediaProcessRoot) -> Option<bool>,
}

pub(in super::super) struct OutputObservation {
    signals: SniffSignals,
    remote: Option<Option<RemoteContext>>,
    agent: AgentProbe,
    cwd: Option<PathBuf>,
}

impl OutputObservation {
    /// Only OSC title/cwd are safe to publish before foreground observation.
    /// Shell/agent marks still need suppression and the original Host scope.
    pub(in super::super) fn take_metadata(&mut self) -> Option<Self> {
        if self.signals.cwd.is_none() && self.signals.title.is_none() {
            return None;
        }
        Some(Self {
            signals: SniffSignals {
                cwd: self.signals.cwd.take(),
                title: self.signals.title.take(),
                ..Default::default()
            },
            remote: None,
            agent: AgentProbe::Unavailable,
            cwd: None,
        })
    }

    pub(super) fn may_change_facts(&self) -> bool {
        self.signals.cwd.is_some()
            || self.signals.title.is_some()
            || !self.signals.agent_events.is_empty()
            || self.signals.notification.is_some()
            || !self.signals.shell.is_empty()
            || self.remote.is_some()
            || !matches!(self.agent, AgentProbe::Unavailable)
            || self.cwd.is_some()
    }

    /// Whether this read carried any OSC 133 prompt mark at all. The pane only
    /// probes foreground execution when one did, keeping the hot path free of
    /// an extra syscall.
    pub(in super::super) fn has_prompt_mark(&self) -> bool {
        self.signals.shell.iter().any(|shell| shell.at_prompt)
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
        let agent = match self.agent {
            AgentProbe::Unavailable => None,
            #[cfg(any(unix, test))]
            AgentProbe::Foreground(agent) => Some(agent),
            #[cfg(windows)]
            AgentProbe::AntigravityChild(present)
                if facts.remote.is_none()
                    && match facts.agent {
                        None => present,
                        Some(CLIAgent::Antigravity) => !present && facts.agent_argv.is_none(),
                        Some(_) => false,
                    } =>
            {
                Some(present.then(|| (CLIAgent::Antigravity, Vec::new())))
            }
            #[cfg(windows)]
            AgentProbe::AntigravityChild(_) => None,
        };
        if let Some(agent) = agent {
            // Process-name recognition can miss wrappers or a just-started
            // agent. Its absence cannot retire a hook-established rich turn
            // before the shell returns; positive detection still can switch
            // agents. Keep authoritative shell-mark handling unchanged.
            let keep_rich_turn = agent.is_none()
                && !facts.shell.at_prompt
                && facts
                    .agent_session
                    .as_ref()
                    .is_some_and(|session| session.rich);
            if !keep_rich_turn {
                super::apply_agent(facts, agent, emit);
            }
        }
        super::apply_probed_cwd(facts, self.cwd, emit);
    }
}

/// Ordered reader-local parsing and probe cadence; no locks or asynchronous consumers.
pub(in super::super) struct FactReader {
    sniffer: OscSniffer,
    probes: ForegroundProbes,
    next_remote_check: Instant,
    last_group: Option<MediaProcessRoot>,
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

    /// Cached process roots are facts, not permission: exec ssh can retain its group.
    /// Called afresh only when an indirect medium is accepted by the parser.
    pub(in super::super) fn indirect_media_locality(&self, before: &ReadProbe) -> Option<bool> {
        if before.managed {
            return Some(false);
        }
        let process_root = (self.probes.group)()?;
        if (self.probes.remote)().is_some() {
            Some(false)
        } else {
            (self.probes.local_group)(process_root)
        }
    }

    pub(in super::super) fn parse<'a>(
        &mut self,
        bytes: impl IntoIterator<Item = &'a [u8]>,
    ) -> OutputObservation {
        OutputObservation {
            signals: self.sniffer.feed_parts(bytes),
            remote: None,
            agent: AgentProbe::Unavailable,
            cwd: None,
        }
    }

    /// Output has already been published. No process probe delays its bytes.
    pub(in super::super) fn observe(
        &mut self,
        observation: OutputObservation,
        foreground_running: &impl Fn() -> bool,
        mut before: ReadProbe,
    ) -> OutputObservation {
        let mut signals = observation.signals;
        // Most reads contain no mark, so do not pay for a foreground syscall.
        if signals.shell.iter().any(|shell| shell.at_prompt) && foreground_running() {
            suppress_relayed_prompt_marks(&mut signals.shell);
            // A/B do not carry an exit code. Clear the parser's standing value
            // too, or the outer shell's next A/B would revive the relayed D.
            self.sniffer.shell.last_exit_code = None;
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
            agent: poll_now.then(&self.probes.agent).unwrap_or_default(),
            cwd: poll_now.then(&self.probes.cwd).flatten(),
        }
    }
}
