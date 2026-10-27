use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
#[cfg(test)]
use std::sync::mpsc;
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::Duration;


use crate::core::clipboard::ClipboardSniffer;
use crate::core::kitty_graphics::{GraphicsEffect, GraphicsSniffer, GraphicsState};
use crate::core::term_modes::TerminalModes;
use crate::daemon::protocol::{
    AuthResponse, DaemonMsg, NativeSshSpec, PaneInfo, RemoteContext, RemoteKind, ShellSpec, WinSize,
};

#[path = "graphics_admission.rs"]
mod graphics_admission;
#[path = "graphics_reader.rs"]
mod graphics_reader;
pub use graphics_admission::GraphicsAdmissionBroker;
mod launch;
mod lifecycle;
use lifecycle::{DeathReporter, PaneBackend, PtyLaunch, PtyParts, join_bounded};
mod replay;
pub(crate) mod facts;
use facts::{FactReader, ForegroundProbes, PaneFacts};
pub mod subscriptions;
#[cfg(test)]
mod test_support;
use subscriptions::{GraphicsReplayAdmission, OutputGate, ReplayState, Subscriptions};
pub(crate) use launch::LYREL_PANE_ENV;
use launch::build_spawn_config;
use replay::ReplayRing;
#[cfg(test)]
use replay::RING_CAP;






struct PaneState {
    id: u64,
    ring: ReplayRing,
    /// Authoritative resources live with the pane, not its current controller.
    /// Taken on exit even if a client keeps the dead pane's text attached.
    graphics: Option<GraphicsState>,
    subscriptions: Subscriptions,
    facts: PaneFacts,
    /// A native SSH pane's connection phase, as last sent. Status frames go
    /// only to whoever is attached at the time, so a window reattaching to a
    /// live session learned nothing and drew it as an unknown remote — no
    /// "connected" dot, and no warning before closing it.
    ssh_phase: Option<crate::daemon::protocol::SshPhase>,
    /// The private modes the pane's output has switched on — the alternate
    /// screen and mouse reporting above all. Folded from the same bytes the
    /// ring gets, because the ring cannot be trusted to still hold them: a
    /// full-screen tool sets them once at startup and the ring drops its front
    /// (#774). See [`TerminalModes`].
    modes: TerminalModes,
    alive: bool,
    exit_code: Option<i32>,
    /// When a reattach last made the program on this pane repaint; see
    /// [`redraw_nudge`].
    last_redraw_nudge: Option<std::time::Instant>,
}

fn notify(st: &mut PaneState, msg: DaemonMsg) {
    st.subscriptions.notify(msg, &mut st.graphics);
}

impl PaneState {
    fn with_facts<R>(&mut self, apply: impl FnOnce(&mut PaneFacts, &mut dyn FnMut(DaemonMsg)) -> R) -> R {
        let Self { facts, subscriptions, graphics, .. } = self;
        apply(facts, &mut |msg| subscriptions.notify(msg, graphics))
    }
}

/// Apply a resize to the pane's shared state: seal the replay ring's segment
/// and tell every connected client the new geometry.
///
/// The controller's `Size` echo has to be sent while the state lock is held —
/// output is fanned out under the same lock, so the frame lands in the stream
/// after every byte produced at the old size and before the repaint the PTY
/// emits once it is actually resized (which happens after the lock is
/// released). A client that speaks `FEATURE_RESIZE_ECHO` defers its grid
/// reflow to this marker instead of reflowing the moment its window changed:
/// the channel can hold megabytes of old-width output (the pane gate allows
/// 16 MiB), and reflowing ahead of it parsed old-width bytes into a new-width
/// grid, garbling the pane on maximize during a burst of output.
///
/// The marker is deliberately not airtight. Old-width bytes the reader thread
/// has read but not yet fanned out — and bytes still sitting in the kernel
/// pipe, which conhost keeps producing until it processes the resize — land
/// after the echo and parse at the new width. That residue is bounded by one
/// 64 KiB read plus the pipe, cannot be attributed to a geometry from here
/// (the bytes carry no tags and ConPTY emits no sync marker of its own), and
/// is the same ambiguity every ConPTY terminal accepts on a mid-output
/// resize. The echo exists to close the 16 MiB window, not that one.
fn resize_state(st: &mut PaneState, size: WinSize, gate: &OutputGate) {
    st.subscriptions.admission().cancel();
    st.ring.resize(size);
    let effects = st
        .graphics
        .as_mut()
        .map(|graphics| graphics.resize(size))
        .unwrap_or_default();
    st.subscriptions.resize(size);
    publish_graphics_effects(st, effects, gate);
}

fn publish_graphics_effects(st: &mut PaneState, effects: Vec<GraphicsEffect>, gate: &OutputGate) {
    st.subscriptions.publish_graphics(effects, gate, &mut st.graphics);
}

fn fan_out_output(st: &mut PaneState, bytes: &[u8], frames: Vec<GraphicsFrame>, gate: &OutputGate) {
    st.subscriptions.output(bytes, frames, gate, &mut st.graphics);
}


pub struct DaemonPane {
    pub id: u64,
    owner: Option<String>,
    backend: PaneBackend,
    /// The input side (keyboard input / pasted text): the PTY writer, or the
    /// native-SSH channel writer. Behind a `Mutex` because writes can arrive from
    /// different connection threads. `Arc`-shared so the reader thread can write
    /// kitty-graphics query replies (`\x1b_G…;OK\x1b\\`) back to the PTY inline
    /// with the sniff, without routing through `write_input`.
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    /// Set during teardown so the reader doesn't emit a spurious exit.
    shutting_down: Arc<AtomicBool>,
    gate: Arc<OutputGate>,
    state: Arc<Mutex<PaneState>>,
    reader: Mutex<Option<JoinHandle<()>>>,
    broker: Option<Arc<crate::daemon::ssh::PromptBroker>>,
}


/// Out-of-band frames remain interleaved with plain output so placements
/// reach the consumer at the same cursor position as in the PTY stream.
enum GraphicsFrame {
    Output(Vec<u8>),
    Graphics(crate::daemon::protocol::GraphicsPacket),
    TransportFull,
    ClipboardWrite(Vec<u8>),
}

fn append_graphics_effects(
    effects: Vec<GraphicsEffect>,
    pass: &mut Vec<u8>,
    frames: &mut Vec<GraphicsFrame>,
    writer: &Arc<Mutex<Box<dyn Write + Send>>>,
) {
    for effect in effects {
        match effect {
            GraphicsEffect::Update(update) => {
                frames.push(
                    update
                        .encode_packet()
                        .map_or(GraphicsFrame::TransportFull, GraphicsFrame::Graphics),
                );
            }
            GraphicsEffect::Frame(packet) => frames.push(GraphicsFrame::Graphics(packet)),
            GraphicsEffect::Reply(reply) => write_graphics_reply(writer, &reply),
            GraphicsEffect::Output(bytes) => {
                pass.extend_from_slice(&bytes);
                frames.push(GraphicsFrame::Output(bytes));
            }
        }
    }
}

fn write_graphics_reply(writer: &Arc<Mutex<Box<dyn Write + Send>>>, reply: &[u8]) {
    if let Ok(mut writer) = writer.lock() {
        let _ = writer.write_all(reply);
        let _ = writer.flush();
    }
}

/// A live pane, reduced to what survives an `exec` plus what has to be written
/// down because it does not.
///
/// The descriptor number and the child pid are the half the kernel keeps for
/// us. Everything else here was in memory, and memory is exactly what `exec`
/// replaces — so a pane's screen, its cwd, its prompt state and its agent are
/// carried across by hand or not at all. See `daemon::handoff`.
#[cfg(unix)]
pub struct Carried {
    pub id: u64,
    pub owner: Option<String>,
    pub master_fd: std::os::fd::RawFd,
    pub child_pid: u32,
    pub integration_dir: Option<PathBuf>,
    pub size: WinSize,
    pub ring: Vec<crate::daemon::scrollback::Segment>,
    pub cwd: Option<PathBuf>,
    pub osc_title: Option<String>,
    /// What the pane is running. Nothing on the other side of the exec can work
    /// it out again — the command line belongs to a child this image never
    /// spawned — so a handoff that dropped it would leave the tree naming no
    /// shell for a pane that plainly has one.
    pub shell_spec: Option<ShellSpec>,
    pub shell_active: bool,
    pub at_prompt: bool,
    pub last_exit: Option<i32>,
    pub remote: Option<RemoteContext>,
    pub agent: Option<crate::core::cli_agent::CLIAgent>,
    pub agent_argv: Option<Vec<String>>,
    pub agent_session: Option<crate::core::cli_agent::AgentSessionState>,
    /// The modes the ring dropped off its front, as bytes that switch them
    /// back on. A program sets them once at startup, so after an `exec` a
    /// long-running TUI would otherwise lose its alternate screen, mouse, 2031
    /// and kitty keyboard flags for good.
    pub head_modes: Option<Vec<u8>>,
}


/// The screen a restored pane opens with.
///
/// `segments` is what some earlier pane — the one this one replaces after a
/// daemon died without handing off — last had on it. `banner` is the sentence
/// that says so, supplied by the client because the daemon has no locale of its
/// own. Both are decoration over a shell that is unambiguously new: nothing
/// here revives a process.
pub struct Restore {
    pub segments: Vec<crate::daemon::scrollback::Segment>,
    pub banner: Option<String>,
    /// The pane's last OSC title, carried beside the bytes rather than in them:
    /// the OSC that set it was usually emitted screens ago and trimmed out of
    /// the capped snapshot, so replaying the segments alone brings the screen
    /// back under the default name.
    pub title: Option<String>,
}

/// The OSC that puts a restored pane's title back.
///
/// BEL-terminated rather than ST so the sequence contains no ESC of its own,
/// and the title is stripped of control bytes — a BEL or ESC inside it would
/// end the sequence early and leak the rest into the terminal as input.
pub fn retitle(title: &str) -> Vec<u8> {
    let mut out = b"\x1b]0;".to_vec();
    out.extend(
        title
            .chars()
            .filter(|c| !c.is_control())
            .collect::<String>()
            .as_bytes(),
    );
    out.push(0x07);
    out
}

/// The bytes that separate restored output from the new shell's own.
///
/// The resets are not cosmetic. A snapshot is trimmed at the front, so it can
/// begin in the middle of anything: an SGR run whose reset was cut, a hidden
/// cursor, a disabled autowrap, an alternate screen whose `?1049h` survived but
/// whose `?1049l` never came because the daemon died while `vim` was open.
/// Replaying that leaves the emulator in a state the incoming shell did not ask
/// for and cannot see. Leaving the alternate screen also does the useful thing
/// in the common case: the primary buffer still holds the pre-`vim` scrollback
/// from earlier in the same snapshot.
///
/// The same argument reaches further than the four resets it started with, and
/// [`INPUT_MODE_RESETS`] is the rest of it: a snapshot that ends inside a
/// full-screen program replays that program's `?1002h` and nothing to undo it,
/// and the emulator then reports every pointer move into a shell that never
/// asked (#850).
///
/// On Windows it ends by scrolling the restored screen out of the viewport
/// ([`SCROLL_RESTORED_AWAY`]), which is a correctness requirement rather than a
/// matter of taste — see that constant.
pub fn restore_preamble(banner: Option<&str>) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"\x1b[?1049l\x1b[?25h\x1b[?7h\x1b[0m");
    out.extend_from_slice(INPUT_MODE_RESETS);
    if let Some(banner) = banner.map(str::trim).filter(|b| !b.is_empty()) {
        out.extend_from_slice(b"\r\n\x1b[2m\xe2\x94\x80\xe2\x94\x80 ");
        // A newline inside the banner would be a client writing multiple lines
        // through a one-line hole; the rest of the sequence assumes one line.
        out.extend_from_slice(banner.replace(['\r', '\n'], " ").as_bytes());
        out.extend_from_slice(b" \xe2\x94\x80\xe2\x94\x80\x1b[0m\r\n");
    }
    if cfg!(windows) {
        out.extend_from_slice(SCROLL_RESTORED_AWAY);
    }
    out
}

/// Turn off every mode that makes a terminal *send* bytes on its own, or encode
/// keys in a way the incoming shell cannot read.
///
/// A restored pane is a snapshot of a dead process replayed at a brand-new
/// shell. The process that switched these on was killed by a hangup with no
/// grace period, so it never emitted its own `l` counterparts, and the snapshot
/// was photographed before it could have anyway. What is left is a byte stream
/// whose last word on mouse reporting is "on", replayed verbatim into an
/// emulator that obeys it: the pointer crossing the pane types SGR reports into
/// the shell's line, and the line grows for as long as the pointer is there
/// (#850).
///
/// Unconditional on purpose. The state to land in is not "whatever the dead
/// program had" but a constant — the new shell asked for none of these, and it
/// is not running yet, so there is nothing here to preserve. Switching off a
/// mode that is already off is a no-op in every emulator, which makes the blunt
/// version the one that cannot fail: a fold over the snapshot's bytes that
/// misreads one sequence leaves the mode on and the bug exactly as it is today.
///
/// - `9`/`1000`/`1002`/`1003` are the mouse reporting level, `1005`/`1006`/
///   `1015`/`1016` its encodings. Our own emulator knows `1000`, `1002`, `1003`,
///   `1005` and `1006` and ignores the rest; `9`, `1015` and `1016` are here for
///   the terminals downstream of a CLI consumer replaying the same ring, which
///   do know them.
/// - `1004` is focus reporting — the other mode that makes the terminal write
///   into the pty unprompted, on every window activation. `2031` is the same
///   for a theme change.
/// - `2004` is bracketed paste: a paste into a shell that does not know the
///   protocol arrives with `ESC[200~` typed around it.
/// - `1` is DECCKM, which sends the arrow keys as `ESC O A` instead of `ESC[A`.
/// - `CSI = 0 ; 1 u` sets the kitty keyboard flags to none, the same reset
///   `stale_mode_resets` in the client uses.
///
/// Deliberately absent: `1007` (alternate scroll), which this emulator has *on*
/// by default, so clearing it would walk away from the default rather than back
/// to it; and `2026` (synchronised update), which the client's processor closes
/// out itself the moment a replayed frame ends inside one.
pub const INPUT_MODE_RESETS: &[u8] = b"\x1b[?9l\x1b[?1000l\x1b[?1002l\x1b[?1003l\
\x1b[?1005l\x1b[?1006l\x1b[?1015l\x1b[?1016l\x1b[?1004l\x1b[?2031l\x1b[?2004l\x1b[?1l\x1b[=0;1u";

/// Push the restored screen into the client's scrollback and put the cursor
/// back at the top-left, so the incoming shell starts on a blank viewport.
///
/// A pty on unix hands the terminal a stream; a ConPTY hands it a *rendering of
/// a screen buffer it owns*. That buffer starts blank with its cursor at the
/// top-left, and conhost addresses it absolutely: PSReadLine redrawing the line
/// being typed emits `ESC[6;20H`, meaning row 6 of conhost's buffer, and every
/// frame conhost paints is positioned the same way. Those row numbers are only
/// correct if the client's viewport is conhost's buffer, row for row.
///
/// Restored output breaks exactly that. It is output conhost never produced and
/// knows nothing about, so leaving it on screen puts the shell's first prompt
/// some rows below where conhost believes it is, and the first keystroke
/// repaints the input line *over the restored text* — the prompt stops
/// responding and the old screen fills with fragments of what is being typed.
/// Nothing the client can do fixes that after the fact: the offset is not a
/// constant (the screen scrolls) and it would have to be unpicked from every
/// absolute address in the stream.
///
/// So the restored screen goes where it can be kept without claiming a row:
/// `ESC[2J` on the primary screen scrolls the viewport into history rather than
/// erasing it, so it is a scroll away, and `ESC[H` leaves the cursor where a
/// fresh ConPTY expects to find it.
///
/// Not done on unix, where the shell positions itself relatively and the
/// restored screen can simply stay where the user can see it.
pub const SCROLL_RESTORED_AWAY: &[u8] = b"\x1b[2J\x1b[H";

impl DaemonPane {
    /// Desktop source/decoded reservations constrain future admission. A
    /// smaller consumer must not attach by silently losing accepted content.
    pub fn set_graphics_capacity(&self, encoded: u64, decoded: u64) -> bool {
        let mut state = self.state.lock().unwrap();
        let PaneState { subscriptions, graphics, .. } = &mut *state;
        subscriptions.set_graphics_capacity(graphics, encoded, decoded)
    }

    pub fn graphics_admission_broker(&self) -> Arc<GraphicsAdmissionBroker> {
        self.state.lock().unwrap().subscriptions.admission().clone()
    }

    pub(crate) fn graphics_viewport(
        &self,
        endpoint: u64,
        owner: &std::sync::Weak<Sender<DaemonMsg>>,
        viewport: super::protocol::GraphicsViewport,
    ) {
        let mut state = self.state.lock().unwrap();
        let Some(sender) = owner.upgrade() else {
            return;
        };
        let attached = state.subscriptions.contains(&sender);
        if attached && let Some(graphics) = &mut state.graphics {
            graphics.set_viewport(endpoint, owner.clone(), viewport);
        }
    }

    pub(crate) fn remove_graphics_viewport(&self, endpoint: u64) {
        if let Some(graphics) = &mut self.state.lock().unwrap().graphics {
            graphics.remove_viewport(endpoint);
        }
    }

    pub(crate) fn pressure_graphics(&self, key: u64) -> bool {
        let mut state = self.state.lock().unwrap();
        let (reclaimed, effects) = state
            .graphics
            .as_mut()
            .map(|graphics| graphics.pressure_resource(key))
            .unwrap_or_default();
        publish_graphics_effects(&mut state, effects, &self.gate);
        reclaimed
    }

    pub fn prepare_graphics_replay(
        &self,
        endpoint: u64,
    ) -> Result<Option<GraphicsReplayAdmission>, String> {
        let (broker, resources, work) = {
            let mut state = self.state.lock().unwrap();
            let PaneState { subscriptions, graphics, .. } = &mut *state;
            let (resources, work) = subscriptions.replay_offers(graphics, &self.gate)?;
            (subscriptions.admission().clone(), resources, work)
        };
        GraphicsReplayAdmission::prepare(endpoint, broker, resources, work, |resources| {
            self.state.lock().unwrap().graphics.as_ref()
                .map_or(resources.is_empty(), |graphics| graphics.matches_offers(resources))
        })
    }

    pub fn attach_admitted(
        &self,
        subscriber: Arc<Sender<DaemonMsg>>,
        allow_clipboard: bool,
        admission: GraphicsReplayAdmission,
    ) -> Result<Option<u64>, String> {
        let foreground = self.backend.foreground_command_running();
        let mut state = self.state.lock().unwrap();
        let (subscriptions, graphics, replay) = state.subscription_parts();
        subscriptions.attach_admitted(replay, graphics, subscriber, &self.gate, allow_clipboard, admission, foreground)
    }

    pub fn observe_admitted(
        &self,
        subscriber: Arc<Sender<DaemonMsg>>,
        gate: Arc<OutputGate>,
        admission: GraphicsReplayAdmission,
    ) -> Result<Option<u64>, String> {
        let foreground = self.backend.foreground_command_running();
        let mut state = self.state.lock().unwrap();
        let (subscriptions, graphics, replay) = state.subscription_parts();
        subscriptions.observe_admitted(replay, graphics, subscriber, gate, &self.gate, admission, foreground)
    }

    pub fn spawn(
        id: u64,
        cwd: Option<PathBuf>,
        size: WinSize,
        shell: Option<ShellSpec>,
        owner: Option<String>,
        workspace: Option<String>,
        restore: Option<Restore>,
        allow_remote_clipboard_write: bool,
        on_dead: impl FnOnce() + Send + 'static,
    ) -> anyhow::Result<Arc<Self>> {
        let pty = PtyLaunch::open(size)?;
        let spawn = build_spawn_config(id, cwd, shell, workspace.as_deref())?;

        let parts = pty.spawn(id, spawn.cmd, spawn.integration_dir)?;

        let (ring, restored_title) = match restore {
            Some(restore) => {
                let mut ring = ReplayRing::seeded(restore.segments, size);
                if let Some(title) = restore.title.as_deref() {
                    ring.append(&retitle(title));
                }
                ring.append(&restore_preamble(restore.banner.as_deref()));
                (ring, restore.title)
            }
            None => (ReplayRing::new(size), None),
        };

        Ok(Self::over_pty(
            parts,
            PaneState {
                id,
                ring,
                graphics: Some(GraphicsState::with_resize_mode(
                    size,
                    if cfg!(windows) {
                        crate::core::terminal_resize::ResizeMode::LocalConpty
                    } else {
                        crate::core::terminal_resize::ResizeMode::Raw
                    },
                )),
                subscriptions: Subscriptions::new(allow_remote_clipboard_write, None),
                facts: PaneFacts::new(spawn.initial_cwd, restored_title, spawn.shell.clone(), spawn.remote.clone()),
                ssh_phase: None,
                modes: TerminalModes::default(),
                alive: true,
                exit_code: None,
                last_redraw_nudge: None,
            },
            owner,
            on_dead,
        ))
    }

    /// Wire a pty, a child and a starting state into a running pane.
    ///
    /// Shared by the two ways a pty-backed pane comes to exist — one that opens
    /// a pty and starts a shell in it, and one that inherits both from the
    /// image it replaced. Everything below this line is identical for them, and
    /// it is the part where getting it subtly wrong shows up as a pane that
    /// never reports its death or never sees its own output.
    fn over_pty(
        parts: PtyParts,
        state: PaneState,
        owner: Option<String>,
        on_dead: impl FnOnce() + Send + 'static,
    ) -> Arc<Self> {
        let (backend, reader_handle, writer) = parts.connect();

        let id = state.id;
        let state = Arc::new(Mutex::new(state));
        let shutting_down = Arc::new(AtomicBool::new(false));
        let gate = Arc::new(OutputGate::new());

        let pane = Arc::new(Self {
            id,
            owner,
            backend,
            writer: writer.clone(),
            shutting_down: shutting_down.clone(),
            gate: gate.clone(),
            state: state.clone(),
            reader: Mutex::new(None),
            broker: None,
        });

        let death = Arc::new(DeathReporter::new(on_dead));
        pane.backend.monitor_exit(&state, &shutting_down, &death);
        let (foreground_running, probes) = pane.backend.reader_probes();
        let reader = Self::spawn_reader(
            state,
            shutting_down,
            gate,
            reader_handle,
            writer.clone(),
            foreground_running,
            probes,
            death,
        );
        *pane.reader.lock().unwrap() = Some(reader);

        pane
    }

    /// Reduce a live pane to what can be handed to another program image.
    ///
    /// Nothing is taken: the descriptor number is copied out, not the
    /// descriptor, and the pane keeps serving exactly as before. That is what
    /// lets the caller stage a whole handoff and then abandon it — an `exec`
    /// that fails costs a log line, not a machine's worth of shells.
    ///
    /// `None` for a native-SSH pane. Its session lives in this process's
    /// memory, not in a descriptor, and no amount of copying descriptor numbers
    /// would let the new image speak on the wire the old one had encrypted.
    #[cfg(unix)]
    pub fn carry(&self) -> Option<crate::daemon::pane::Carried> {
        let (master_fd, child_pid, integration_dir) = self.backend.carry()?;
        let st = self.state.lock().unwrap();
        Some(st.facts.write_carried(Carried {
            id: self.id,
            owner: self.owner.clone(),
            master_fd,
            child_pid,
            integration_dir,
            size: st.ring.tail_size(),
            ring: st.ring.snapshot(),
            cwd: None,
            osc_title: None,
            shell_spec: None,
            shell_active: false,
            at_prompt: false,
            last_exit: None,
            remote: None,
            agent: None,
            agent_argv: None,
            agent_session: None,
            head_modes: st.ring.head_restore_bytes(),
        }))
    }

    /// Rebuild a pane around a pty this process already holds.
    ///
    /// The descriptor and the child pid came through an `exec`, so both are
    /// still ours in the only sense that matters: the kernel still has us down
    /// as the pty's owner and the shell's parent. What is gone is everything
    /// that was in memory, which is why the ring and the pane's status come
    /// back from the handoff blob rather than from the shell.
    #[cfg(unix)]
    pub fn adopt(
        mut carried: crate::daemon::pane::Carried,
        on_dead: impl FnOnce() + Send + 'static,
    ) -> anyhow::Result<Arc<Self>> {
        let (parts, size) = PtyParts::adopt(&mut carried)?;
        let facts = PaneFacts::from_carried(&mut carried);

        let (mut ring, modes) =
            ReplayRing::adopted(carried.ring, size, carried.head_modes.as_deref());
        // Not a restore banner: nothing was lost, so there is nothing to
        // announce and nothing to reset. The shell below these bytes is the
        // same shell that wrote them.
        ring.resize(size);

        Ok(Self::over_pty(
            parts,
            PaneState {
                id: carried.id,
                ring,
                graphics: Some(GraphicsState::new(size)),
                subscriptions: Subscriptions::new(false, None),
                facts,
                ssh_phase: None,
                modes,
                alive: true,
                exit_code: None,
                last_redraw_nudge: None,
            },
            carried.owner,
            on_dead,
        ))
    }

    /// A pane bridged to a shell channel on `spec`'s host.
    ///
    /// `remote_start_dir` is a directory on that host for the shell to start
    /// in. The pane's own `cwd` is deliberately left unknown until the far
    /// shell reports one: the `cd` that takes it there can quietly fail, and a
    /// guess recorded now would be persisted as fact.
    pub fn spawn_native_ssh(
        id: u64,
        size: WinSize,
        spec: Box<NativeSshSpec>,
        remote_start_dir: Option<String>,
        on_dead: impl FnOnce() + Send + 'static,
    ) -> anyhow::Result<Arc<Self>> {
        let allow_remote_clipboard_write = spec.remote_clipboard_write;
        let bridge = crate::daemon::ssh::session::make_bridge();
        let reader_handle: Box<dyn Read + Send> = Box::new(bridge.reader);
        let writer: Arc<Mutex<Box<dyn Write + Send>>> =
            Arc::new(Mutex::new(Box::new(bridge.writer)));
        // The connect task fills this once authenticated; the pane exposes it to
        // WS4/WS5 via `ssh_connection()`.
        let connection: crate::daemon::ssh::SharedConnection = Arc::new(Mutex::new(Weak::new()));

        let target = spec
            .display_name
            .clone()
            .unwrap_or_else(|| format!("{}@{}", spec.user, spec.host));
        let remote = RemoteContext {
            kind: RemoteKind::NativeSsh,
            argv: Vec::new(),
            target,
        };

        let state = Arc::new(Mutex::new(PaneState {
            id,
            ring: ReplayRing::new(size),
            graphics: Some(GraphicsState::new(size)),
            subscriptions: Subscriptions::new(allow_remote_clipboard_write, Some(allow_remote_clipboard_write)),
            facts: PaneFacts::new(None, None, None, Some(remote)),
            ssh_phase: None,
            modes: TerminalModes::default(),
            alive: true,
            exit_code: None,
            last_redraw_nudge: None,
        }));
        let shutting_down = Arc::new(AtomicBool::new(false));
        let gate = Arc::new(OutputGate::new());

        let broker = {
            let state = state.clone();
            crate::daemon::ssh::PromptBroker::new(Box::new(move |msg: DaemonMsg| {
                let mut st = state.lock().unwrap();
                if let DaemonMsg::SshStatus { phase } = &msg {
                    st.ssh_phase = Some(phase.clone());
                }
                st.subscriptions.tell_controller(msg)
            }))
        };

        let pane = Arc::new(Self {
            id,
            owner: None,
            backend: PaneBackend::native_ssh(bridge.handle, connection.clone()),
            writer: writer.clone(),
            shutting_down: shutting_down.clone(),
            gate: gate.clone(),
            state: state.clone(),
            reader: Mutex::new(None),
            broker: Some(broker.clone()),
        });

        let death = Arc::new(DeathReporter::new(on_dead));

        let reader = Self::spawn_reader(
            state,
            shutting_down,
            gate,
            reader_handle,
            writer.clone(),
            || false,
            ForegroundProbes {
                group: Box::new(|| None),
                remote: Box::new(|| None),
                agent: Box::new(|| None),
                cwd: Box::new(|| None),
                local_group: crate::daemon::procinfo::foreground_group_is_local,
            },
            death,
        );
        *pane.reader.lock().unwrap() = Some(reader);

        crate::daemon::ssh::SshManager::global().spawn_native_session(
            id,
            spec,
            remote_start_dir,
            size,
            broker,
            bridge.data_tx,
            bridge.cmd_rx,
            connection,
        );

        Ok(pane)
    }

    pub fn deliver_auth_response(&self, request_id: u64, response: AuthResponse) {
        if let Some(broker) = &self.broker {
            broker.deliver(request_id, response);
        }
    }

    #[allow(dead_code)]
    pub fn ssh_connection(&self) -> Option<Arc<crate::daemon::ssh::SshConnection>> {
        self.backend.ssh_connection()
    }

    fn spawn_reader(
        state: Arc<Mutex<PaneState>>,
        shutting_down: Arc<AtomicBool>,
        gate: Arc<OutputGate>,
        mut reader: Box<dyn Read + Send>,
        // The pane's input side, shared so the reader can write kitty-graphics
        // query replies (`\x1b_G…;OK\x1b\\`) straight back to the PTY — see the
        // graphics sniff in the loop below.
        writer: Arc<Mutex<Box<dyn Write + Send>>>,
        foreground_running: impl Fn() -> bool + Send + 'static,
        probes: ForegroundProbes,
        death: Arc<DeathReporter>,
    ) -> JoinHandle<()> {
        let (eviction_tx, eviction_rx) = std::sync::mpsc::sync_channel(1);
        let owner = {
            let mut state = state.lock().unwrap();
            state.graphics.as_mut().map(|graphics| {
                graphics.set_wake(eviction_tx);
                graphics.work_owner()
            })
        };
        let weak_state = Arc::downgrade(&state);
        let eviction_gate = gate.clone();
        std::thread::Builder::new()
            .name("lyrel-graphics-evictions".into())
            .spawn(move || {
                while eviction_rx.recv().is_ok() {
                    let Some(state) = weak_state.upgrade() else {
                        break;
                    };
                    let mut state = state.lock().unwrap();
                    let effects = state
                        .graphics
                        .as_mut()
                        .map(GraphicsState::drain_evictions)
                        .unwrap_or_default();
                    publish_graphics_effects(&mut state, effects, &eviction_gate);
                }
            })
            .expect("spawn graphics eviction publisher");
        std::thread::Builder::new()
            .name("lyrel-daemon-pane-reader".to_string())
            .spawn(move || {
                crate::core::threads::promote_to_user_interactive();
                let mut facts_reader = FactReader::new(probes);
                let mut clipboard = ClipboardSniffer::default();
                // Kitty graphics interception (issue #213): lifts image
                // sequences out of the stream *before* the ring/subscriber see
                // them, so the base64 pixels never enter replay and the client's
                // VT parser never chews through them. Zero-copy on the common
                // no-graphics chunk — see [`GraphicsSniffer::sniff`].
                //
                // File/shm transfer (`t=s`/`t=f`/`t=t`) is honored only while the
                // pane is *local*: the object/path names resolve on this host, and
                // reading them can't leak across an SSH tunnel. A pane that starts
                // local can `ssh` out mid-session, so the flag is refreshed from
                // the same remote-context poll below. Seed it from the pane's
                // current context so the first probe answers correctly.
                let starts_local = state.lock().unwrap().facts.remote().is_none();
                let mut graphics = owner.map_or_else(|| GraphicsSniffer::new_local(starts_local), |owner| GraphicsSniffer::new_budgeted(starts_local, owner));
                let mut buf = [0u8; 65536];

                let trace = std::env::var("LYREL_TRACE").is_ok_and(|v| !v.is_empty() && v != "0");
                let mut tr_last = std::time::Instant::now();
                let mut tr_bytes: u64 = 0;
                let mut tr_reads: u32 = 0;
                let mut tr_read_t = std::time::Duration::ZERO;
                let mut tr_disp_t = std::time::Duration::ZERO;

                loop {
                    if trace && tr_last.elapsed() >= std::time::Duration::from_secs(1) {
                        // Not `eprintln!`: a daemon can be running without any
                        // standard error at all (see `daemon::server`), and a
                        // failed write there would panic the reader thread.
                        let _ = writeln!(
                            std::io::stderr(),
                            "[trace daemon] {:.1} MB/s | {} reads ({} B/read) | pty wait {:?} dispatch {:?}",
                            tr_bytes as f64 / tr_last.elapsed().as_secs_f64() / 1e6,
                            tr_reads,
                            if tr_reads > 0 { tr_bytes / tr_reads as u64 } else { 0 },
                            tr_read_t,
                            tr_disp_t,
                        );
                        tr_last = std::time::Instant::now();
                        tr_bytes = 0;
                        tr_reads = 0;
                        tr_read_t = std::time::Duration::ZERO;
                        tr_disp_t = std::time::Duration::ZERO;
                    }
                    gate.wait_below_high_water();
                    let tr0 = trace.then(std::time::Instant::now);
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            if let Some(tr0) = tr0 {
                                tr_read_t += tr0.elapsed();
                                tr_reads += 1;
                                tr_bytes += n as u64;
                            }
                            let raw = &buf[..n];
                            // Resolve locality before parsing: the first APC can name host media.
                            let locality = state.lock().unwrap().facts.locality();
                            let before = facts_reader.before_read(locality);
                            graphics.set_local(before.media_local());
                            let controller = {
                                let st = state.lock().unwrap();
                                let controller = st.subscriptions.controller_epoch();
                                clipboard.set_controller(controller, st.subscriptions.clipboard_allowed());
                                controller
                            };
                            let mut media_locality = || facts_reader.indirect_media_locality(&before);
                            let passthrough = graphics_reader::consume(raw, &mut clipboard, &mut graphics, &state, &gate, &writer, controller, &mut media_locality);
                            let bytes: &[u8] = &passthrough;
                            let observation = facts_reader.observe(bytes, &foreground_running, before);
                            let mut st = state.lock().unwrap();
                            let da1_owed = detached_device_attribute_queries(&mut st);
                            drop(st);


                            let tr1 = trace.then(std::time::Instant::now);
                            // Fact snapshots are conditional; ordinary output does not allocate.
                            let mut st = state.lock().unwrap();
                            let change = st.with_facts(|facts, emit| facts.apply_output(observation, emit));
                            graphics.set_local(st.facts.remote().is_none());
                            if let Some(tr1) = tr1 {
                                tr_disp_t += tr1.elapsed();
                            }
                            let pane = st.id;
                            let alive = st.alive;
                            drop(st);
                            answer_device_attributes(&writer, da1_owed);
                            change.publish(pane, alive, shutting_down.load(Ordering::SeqCst));
                        }
                        Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }

                death.report(&state, &shutting_down);
            })
            .expect("spawn daemon pane reader thread")
    }

    pub fn attach(&self, subscriber: Sender<DaemonMsg>) -> u64 {
        self.attach_with_permissions(subscriber, false)
    }

    pub fn attach_with_permissions(
        &self,
        subscriber: Sender<DaemonMsg>,
        allow_remote_clipboard_write: bool,
    ) -> u64 {
        // Asked before the state lock, not under it: the probe takes the pty
        // master's lock, and every other caller that holds both takes them in
        // this order.
        let foreground_command = self.backend.foreground_command_running();
        let mut st = self.state.lock().unwrap();
        let epoch = attach_subscriber_with_permissions(
            &mut st,
            subscriber,
            &self.gate,
            allow_remote_clipboard_write,
            foreground_command,
        );
        let nudge = redraw_nudge(
            &mut st,
            self.runs_foreground_program(foreground_command),
            std::time::Instant::now(),
        );
        drop(st);
        if let Some(transient) = nudge {
            self.nudge_redraw(transient);
        }
        epoch
    }

    /// Whether a program other than the shell is in the foreground, for
    /// [`redraw_nudge`]. A native ssh pane has no local pty to ask, and its
    /// alternate screen can only be the far program's.
    fn runs_foreground_program(&self, foreground_command: bool) -> bool {
        !self.backend.is_local_pty() || foreground_command
    }

    /// Puts the pty at `transient` now and back at the pane's size a moment
    /// later, which the program in the foreground hears as two `SIGWINCH`es
    /// and answers with a full repaint. See [`redraw_nudge`].
    ///
    /// Only the pty moves: the ring keeps its segment and no client is told a
    /// new size, so nothing a client holds changes. The size put
    /// back is read when it is put back, so a real resize that lands in
    /// between is what the pane ends up at.
    fn nudge_redraw(&self, transient: WinSize) {
        let resize = self.backend.redraw_resizer();
        let state = Arc::clone(&self.state);
        let _ = run_redraw_nudge(transient, REDRAW_NUDGE_SETTLE, resize, move || {
            let st = state.lock().unwrap();
            st.alive.then(|| st.ring.size())
        });
    }


    pub fn detach(&self, epoch: u64) -> bool {
        let mut st = self.state.lock().unwrap();
        let PaneState { subscriptions, graphics, alive, .. } = &mut *st;
        subscriptions.detach(epoch, &self.gate, graphics);
        !*alive && subscriptions.controller_epoch().is_none()
    }

    pub fn observe(&self, observer: Sender<DaemonMsg>, gate: Arc<OutputGate>) -> u64 {
        let foreground_command = self.backend.foreground_command_running();
        let mut st = self.state.lock().unwrap();
        let id = observe_subscriber(&mut st, observer, gate, foreground_command);
        let nudge = redraw_nudge(
            &mut st,
            self.runs_foreground_program(foreground_command),
            std::time::Instant::now(),
        );
        drop(st);
        if let Some(transient) = nudge {
            self.nudge_redraw(transient);
        }
        id
    }

    /// A message for one observer alone, such as a refusal of what it sent.
    /// False once the pane has let it go.
    pub fn tell_observer(&self, observer_id: u64, msg: DaemonMsg) -> bool {
        self.state.lock().unwrap().subscriptions.tell_observer(observer_id, msg)
    }

    pub fn unobserve(&self, observer_id: u64) {
        let mut state = self.state.lock().unwrap();
        let PaneState { subscriptions, graphics, .. } = &mut *state;
        subscriptions.unobserve(observer_id, graphics);
    }

    pub fn controls(&self, epoch: u64) -> bool {
        self.state.lock().unwrap().subscriptions.controls(epoch)
    }

    pub fn agent_state(&self) -> Option<crate::daemon::control::PaneAgentState> {
        self.state.lock().unwrap().facts.agent_state(self.id)
    }

    /// Poll only authoritative durable events; silence never finishes a turn.
    pub fn poll_agent_events(&self) {
        let observer = {
            let mut state = self.state.lock().unwrap();
            if !state.alive { return; }
            state.facts.take_durable_observer()
        };
        let Some(mut observer) = observer else {
            return;
        };
        let events = observer.poll();
        let mut state = self.state.lock().unwrap();
        if !state.alive { return; }
        let change = state.with_facts(|facts, emit| facts.accept_durable_observation(observer, events, emit));
        drop(state);
        change.publish(self.id, true, self.shutting_down.load(Ordering::SeqCst));
    }

    /// Apply an agent hook's report that came over the socket rather than
    /// through the pane's output (`ClientMsg::AgentEvent`), exactly as if the
    /// reader had found it there — once `pid`, the hook, is shown to run
    /// under this pane's shell. The pane id the hook named came from its
    /// environment, and a detached `tmux` server or the like carries that
    /// into processes this pane does not own.
    pub fn report_agent_event(&self, pid: u32, body: &str) -> Result<(), String> {
        let shell = self.backend.local_shell_pid()
            .ok_or_else(|| format!("pane {} runs no local process", self.id))?;
        let event = crate::core::cli_agent::parse_agent_event_body(body.as_bytes())
            .ok_or_else(|| "not an agent event".to_string())?;
        if !crate::daemon::procinfo::descends_from(pid, shell) {
            return Err(format!("process {pid} does not run in pane {}", self.id));
        }
        self.apply_reported_agent_event(event)
    }

    /// Apply an authenticated hook report to this pane.
    pub fn apply_reported_agent_event(
        &self,
        event: crate::core::cli_agent::AgentEvent,
    ) -> Result<(), String> {
        let mut st = self.state.lock().unwrap();
        if !st.alive {
            return Err(format!("pane {} is not running", self.id));
        }
        let change = st.with_facts(|facts, emit| facts.apply_events(vec![event], None, emit));
        drop(st);
        change.publish(self.id, true, self.shutting_down.load(Ordering::SeqCst));
        Ok(())
    }

    pub fn gate(&self) -> Arc<OutputGate> {
        self.gate.clone()
    }

    pub fn write_input(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        if let Ok(mut writer) = self.writer.lock() {
            let _ = writer.write_all(bytes);
            let _ = writer.flush();
        }
    }

    pub fn procs(&self) -> crate::daemon::protocol::PaneProcs {
        let mut out = self.backend.procs();
        out.context = Some(self.context());
        out
    }

    /// What the pane knows about itself beyond its process list.
    ///
    /// Always filled, including on the branches above that have no tree to
    /// walk — a native-SSH pane's empty `procs` is exactly the answer this has
    /// to qualify, and returning `Default::default()` there would have said
    /// "nothing is running" in the same shape as "we could not look".
    fn context(&self) -> crate::daemon::protocol::PaneContext {
        let st = self.state.lock().unwrap();
        st.facts.context(
            self.backend.is_local_pty(),
            st.modes.is_on(crate::core::term_modes::BRACKETED_PASTE),
        )
    }


    /// Resize the shared pane state before applying the new PTY geometry.
    pub fn resize(&self, size: WinSize) {
        resize_state(&mut self.state.lock().unwrap(), size, &self.gate);
        self.backend.resize(size);
    }

    /// A controller command, not PTY input. All consumers see the same clear
    /// and graphics removals under the output lock; the shell redraw follows.
    pub fn clear_presentation(&self, history: bool) {
        let mut st = self.state.lock().unwrap();
        st.subscriptions.admission().cancel();
        let Some(graphics) = st.graphics.as_mut() else {
            return;
        };
        let effects = graphics.clear_presentation(history);
        let mut frames = Vec::new();
        let mut pass = Vec::new();
        append_graphics_effects(effects, &mut pass, &mut frames, &self.writer);
        record_output(&mut st, &pass);
        fan_out_output(&mut st, &pass, frames, &self.gate);
    }


    pub fn alive(&self) -> bool {
        self.state.lock().unwrap().alive
    }

    pub fn info(&self) -> PaneInfo {
        let (cwd, osc_title, alive) = {
            let st = self.state.lock().unwrap();
            (st.facts.cwd().map(PathBuf::from), st.facts.osc_title().map(str::to_owned), st.alive)
        };
        PaneInfo {
            pane_id: self.id,
            cwd: cwd.or_else(|| self.backend.foreground_cwd()),
            title: self.backend.foreground_title(),
            osc_title,
            alive,
            owner: self.owner.clone(),
        }
    }

    pub(crate) fn remote_context(&self) -> Option<RemoteContext> {
        let cached = self.state.lock().unwrap().facts.remote().cloned();
        cached.or_else(|| self.backend.foreground_remote_context())
    }

    /// The pane's screen, capped for storage, with the mark that says how much
    /// output it had produced when the copy was taken.
    ///
    /// Both come from one acquisition of the state lock. Reading them apart
    /// would let output land in between, and the writer would then record a
    /// mark that claims to cover bytes its snapshot does not have — a pane
    /// that stopped producing at that moment would keep the stale copy for
    /// good, because its mark would never move again.
    /// The mark alone, for deciding whether a snapshot is worth taking:
    /// [`scrollback_snapshot`](Self::scrollback_snapshot) clones the whole
    /// ring under the state lock, which is a lot to pay every tick for the
    /// answer "nothing changed". A pane that moves between this read and the
    /// snapshot is fine — the snapshot returns its own mark, and that pair is
    /// what gets recorded.
    pub fn scrollback_mark(&self) -> u64 {
        self.state.lock().unwrap().ring.appended_mark()
    }

    pub fn scrollback_snapshot(
        &self,
    ) -> (Vec<crate::daemon::scrollback::Segment>, Option<String>, u64) {
        let st = self.state.lock().unwrap();
        let mut segments = st.ring.snapshot();
        crate::daemon::scrollback::trim_to(&mut segments, crate::daemon::scrollback::SNAPSHOT_CAP);
        (segments, st.facts.osc_title().map(str::to_owned), st.ring.appended_mark())
    }

    pub fn kill(&self) {
        self.hangup();
    }

    fn hangup(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        {
            let mut state = self.state.lock().unwrap();
            let PaneState { subscriptions, graphics, .. } = &mut *state;
            subscriptions.shutdown_graphics(graphics);
        }
        self.backend.hangup();
    }



}

impl Drop for DaemonPane {
    fn drop(&mut self) {
        if !self.backend.is_local_pty() {
            crate::daemon::ssh::SshManager::global().teardown_pane_forwards(self.id);
        }
        self.hangup();
        self.backend.reap(self.id);
        if let Some(handle) = self.reader.lock().unwrap().take() {
            join_bounded(handle, Duration::from_secs(2));
        }
        self.backend.remove_integration();
        // The pane is over — by a kill, by the shell exiting, or by the daemon
        // shutting down — so whatever it typed goes back to the history file it
        // was seeded from. A handoff does not come through here: nothing is
        // dropped across an `exec`, which is exactly right, because the pane on
        // the other side is the same pane and still owns its file.
        crate::daemon::history::retire(self.id);
    }
}


/// Everything the pane remembers about a chunk of output: the bytes
/// themselves, and the modes they switched on. One function so the two can
/// never drift apart — the modes are only worth anything if they were folded
/// from exactly the bytes the ring was given.
fn record_output(st: &mut PaneState, bytes: &[u8]) {
    st.ring.append(bytes);
    st.modes.feed(bytes);
}

/// The primary device attributes a client's emulator answers with.
const DA1_REPLY: &[u8] = b"\x1b[?6c";

/// A pane with no client attached has nobody to answer its queries, and a
/// replay drops replies, so one asked then is never answered. Programs that
/// end a batch of queries with DA1 wait on that answer: Claude Code's OSC 11
/// theme check hangs on it and then skips every later check, so the theme
/// flips lyrel reports never reach it. Answering DA1 here lets the batch end;
/// the colour report a client sends on attach then asks again, and is answered.
///
/// Returns how many DA1 queries the daemon owes an answer to. With a client
/// attached the client answers, so none are owed. Read under the state lock;
/// the answers are written by [`answer_device_attributes`] after it is
/// released, because a pty whose program is not reading its input blocks the
/// write, and nothing else that needs this pane's state should wait on that.
fn detached_device_attribute_queries(st: &mut PaneState) -> usize {
    let asked = st.modes.take_device_attribute_queries();
    if st.subscriptions.controller_epoch().is_some() { 0 } else { asked }
}

fn answer_device_attributes(writer: &Mutex<Box<dyn Write + Send>>, owed: usize) {
    if owed == 0 {
        return;
    }
    if let Ok(mut writer) = writer.lock() {
        for _ in 0..owed {
            let _ = writer.write_all(DA1_REPLY);
        }
        let _ = writer.flush();
    }
}



/// How long the pty stays at the transient size of a [`redraw_nudge`]. Two
/// size changes in one breath reach a program as one `SIGWINCH` (the kernel
/// does not queue them), and a program that reads the size only once it gets
/// around to the signal finds it unchanged and skips the repaint. Long enough
/// for it to see the first size, short enough that nobody looks at it.
const REDRAW_NUDGE_SETTLE: Duration = Duration::from_millis(50);

/// A pane reattached again this soon after a nudge is not nudged again: the
/// repaint the last one asked for is already in the ring, or on its way.
const REDRAW_NUDGE_COOLDOWN: Duration = Duration::from_secs(2);

/// Whether a client that was just handed the replay needs the program on the
/// pane to repaint, and if so the size to put the pty at for a moment to make
/// it (#1122). Records the nudge, so a burst of reattaches makes one.
///
/// A full-screen program paints its whole screen once, after `?1049h`, and
/// from then on only what changed. The ring keeps the last 8 MiB, so a program
/// that has run long enough has had that first paint, and its `?1049h`, pushed
/// out of the front. The replay prelude still puts the client into the
/// alternate screen, but what follows is a stream of differences from a screen
/// the client never saw, painted onto an empty one. The client has nothing to
/// rebuild it from, and neither has the daemon, which keeps no screen; only
/// the program can draw it again, and the one request every full-screen
/// program answers with a full repaint is a change of size. Asking for the
/// size it already has does nothing — the kernel sends no `SIGWINCH` for it —
/// so the pty steps one row away and back.
///
/// Only when all of it holds: the pane is in the alternate screen and the ring
/// no longer carries the switch into it (which is to say the ring has dropped
/// its front, and with it the first paint), a program owns the pane, and the
/// pane is alive. Anything else — a shell, a program whose start is still in
/// the ring — replays whole, and is left alone.
fn redraw_nudge(
    st: &mut PaneState,
    foreground_program: bool,
    now: std::time::Instant,
) -> Option<WinSize> {
    use crate::core::term_modes::ALT_SCREEN;
    if !st.alive || !foreground_program || !st.modes.is_on(ALT_SCREEN) {
        return None;
    }
    if st
        .last_redraw_nudge
        .is_some_and(|at| now.saturating_duration_since(at) < REDRAW_NUDGE_COOLDOWN)
    {
        return None;
    }
    // Last, for it folds the whole ring.
    if st.ring.modes().is_on(ALT_SCREEN) {
        return None;
    }
    st.last_redraw_nudge = Some(now);
    let size = st.ring.size();
    let rows = if size.rows > 1 {
        size.rows - 1
    } else {
        size.rows + 1
    };
    Some(WinSize { rows, ..size })
}

/// The two steps of a nudge: `transient` now, and after `settle` whatever
/// `settled` says the pane's size is by then (`None`: leave it, the pane is
/// gone). The second step runs on its own thread so the attach that asked for
/// it is not held up.
fn run_redraw_nudge(
    transient: WinSize,
    settle: Duration,
    resize: Box<dyn Fn(WinSize) + Send>,
    settled: impl FnOnce() -> Option<WinSize> + Send + 'static,
) -> std::io::Result<JoinHandle<()>> {
    resize(transient);
    std::thread::Builder::new()
        .name("lyrel-redraw-nudge".into())
        .spawn(move || {
            std::thread::sleep(settle);
            if let Some(size) = settled() {
                resize(size);
            }
        })
}

#[cfg(test)]
fn attach_subscriber(st: &mut PaneState, subscriber: Sender<DaemonMsg>) -> u64 {
    attach_subscriber_with_permissions(st, subscriber, &OutputGate::new(), false, false)
}

/// Composition only: replay inputs are borrowed under the existing pane lock.
impl PaneState {
    fn subscription_parts(&mut self) -> (&mut Subscriptions, &mut Option<GraphicsState>, ReplayState<'_>) {
        let replay = ReplayState {
            ring: &self.ring,
            modes: &self.modes,
            facts: &self.facts,
            ssh_phase: &self.ssh_phase,
            alive: self.alive,
            exit_code: self.exit_code,
        };
        (&mut self.subscriptions, &mut self.graphics, replay)
    }
}

fn attach_subscriber_with_permissions(
    st: &mut PaneState,
    subscriber: impl Into<Arc<Sender<DaemonMsg>>>,
    gate: &OutputGate,
    allow_remote_clipboard_write: bool,
    foreground_command: bool,
) -> u64 {
    let (subscriptions, graphics, replay) = st.subscription_parts();
    subscriptions.attach(replay, graphics, subscriber, gate, allow_remote_clipboard_write, foreground_command)
}

fn observe_subscriber(
    st: &mut PaneState,
    observer: impl Into<Arc<Sender<DaemonMsg>>>,
    gate: Arc<OutputGate>,
    foreground_command: bool,
) -> u64 {
    let (subscriptions, graphics, replay) = st.subscription_parts();
    subscriptions.observe(replay, graphics, observer, gate, foreground_command)
}


#[cfg(test)]
mod tests {

    use super::*;
    use super::test_support::{drain, null_writer, test_state, ws};







    /// Issue #857: the two sniffers that rewrite the stream before the ring
    /// and the client see it must hand every byte that is not theirs on
    /// unchanged, wherever a read splits it — a Pi frame (synchronized update,
    /// line marks, CJK text, cursor-up rewrite) at every split point, and a
    /// byte at a time. Both what the ring records and what the client is sent.
    #[test]
    fn out_of_band_stripping_passes_a_streamed_frame_through_every_split() {
        let frame = "\x1b[?2026h\x1b[2A\r\x1b[2K## 资源\r\n\x1b[2K\
                     \x1b]133;B\x07\x1b]133;C\x07下一步。\x1b[?2026l"
            .as_bytes();
        let writer: Arc<Mutex<Box<dyn Write + Send>>> =
            Arc::new(Mutex::new(Box::new(std::io::sink())));
        let run = |chunks: &[&[u8]]| {
            let mut clipboard = ClipboardSniffer::default();
            clipboard.set_controller(Some(1), true);
            let mut graphics = GraphicsSniffer::new_local(true);
            let mut pane = test_state(true);
            let (sender, receiver) = mpsc::channel();
            attach_subscriber(&mut pane, sender);
            drain(&receiver);
            let state = Arc::new(Mutex::new(pane));
            let gate = OutputGate::new();
            let mut passthrough = Vec::new();
            for chunk in chunks {
                passthrough.extend_from_slice(&graphics_reader::consume(
                    chunk,
                    &mut clipboard,
                    &mut graphics,
                    &state,
                    &gate,
                    &writer,
                    Some(1),
                    &mut || panic!("plain output must not probe host-media locality"),
                ));
            }
            let recorded = state.lock().unwrap().ring.flatten();
            assert_eq!(
                passthrough, recorded,
                "the signals parser sees the recorded stream"
            );
            let mut sent = Vec::new();
            for message in receiver.try_iter() {
                match message {
                    DaemonMsg::Output(bytes) => sent.extend(bytes),
                    _ => panic!("no out-of-band frame in a plain stream"),
                }
            }
            (recorded, sent)
        };
        for cut in 0..=frame.len() {
            let (recorded, sent) = run(&[&frame[..cut], &frame[cut..]]);
            assert_eq!(recorded, frame, "ring, split at byte {cut}");
            assert_eq!(sent, frame, "client, split at byte {cut}");
        }
        let bytewise: Vec<&[u8]> = frame.chunks(1).collect();
        assert_eq!(run(&bytewise), (frame.to_vec(), frame.to_vec()));
    }


    #[test]
    fn the_restore_preamble_hands_the_new_shell_a_terminal_it_can_use() {
        let bytes = restore_preamble(Some("this shell is new"));
        let text = String::from_utf8(bytes).expect("the preamble is text");
        // A snapshot is cut at the front, so it can begin inside anything: an
        // unterminated SGR run, a hidden cursor, an alternate screen whose exit
        // never came because the daemon died while a full-screen app was up.
        assert!(text.contains("\x1b[?1049l"), "leave any alternate screen");
        assert!(text.contains("\x1b[?25h"), "give the cursor back");
        assert!(text.contains("\x1b[?7h"), "put autowrap back");
        assert!(text.contains("\x1b[0m"), "drop any colour left mid-run");
        assert!(text.contains("this shell is new"));

        // #850: the same argument, for the modes that make the terminal talk
        // back. The snapshot ends inside a program that enabled mouse
        // reporting and was killed before it could disable it, so replaying it
        // leaves the emulator reporting every pointer move into a shell that
        // never asked — the line fills with SGR reports as long as the pointer
        // is over the pane. Focus reporting writes unprompted for the same
        // reason; bracketed paste and DECCKM mangle what the user types.
        for (seq, what) in [
            ("\x1b[?9l", "X10 mouse reporting"),
            ("\x1b[?1000l", "click reporting"),
            ("\x1b[?1002l", "cell-motion reporting"),
            ("\x1b[?1003l", "all-motion reporting"),
            ("\x1b[?1005l", "the UTF-8 mouse encoding"),
            ("\x1b[?1006l", "the SGR mouse encoding"),
            ("\x1b[?1015l", "the urxvt mouse encoding"),
            ("\x1b[?1016l", "the SGR-pixel mouse encoding"),
            ("\x1b[?1004l", "focus reporting"),
            ("\x1b[?2004l", "bracketed paste"),
            ("\x1b[?1l", "application cursor keys"),
            ("\x1b[=0;1u", "the kitty keyboard flags"),
        ] {
            assert!(
                text.contains(seq),
                "the preamble has to turn {what} off: the process that turned it \
                 on was killed by a hangup with no grace period and never sent \
                 its own reset, so the replay's last word on it is `on`"
            );
        }

        // Not reset: alternate scroll is on in a default terminal, so clearing
        // it would leave the pane further from the default than it started.
        assert!(
            !text.contains("\x1b[?1007l"),
            "?1007 defaults to on; resetting it walks away from the default"
        );
    }

    /// Whether the stream's last word on a private mode is `h`, `l`, or
    /// nothing — which is all a client's emulator ends up remembering of it.
    fn last_mode_switch(stream: &[u8], mode: &str) -> Option<u8> {
        let last = |needle: Vec<u8>| {
            stream
                .windows(needle.len())
                .rposition(|w| w == needle.as_slice())
        };
        let on = last(format!("\x1b[?{mode}h").into_bytes());
        let off = last(format!("\x1b[?{mode}l").into_bytes());
        match (on, off) {
            (Some(h), Some(l)) => Some(if h > l { b'h' } else { b'l' }),
            (Some(_), None) => Some(b'h'),
            (None, Some(_)) => Some(b'l'),
            (None, None) => None,
        }
    }

    /// Issue #850, along the whole chain rather than at the preamble alone: a
    /// snapshot is a raw byte stream, it seeds the restored pane's ring
    /// verbatim, and the client feeds it straight to its parser. So a snapshot
    /// whose last word on mouse reporting is `h` puts the *client* into mouse
    /// reporting, against a shell that never asked, and every pointer move
    /// over the pane is typed into its line as an SGR report.
    #[test]
    fn a_ring_restored_from_a_full_screen_snapshot_ends_with_reporting_off() {
        let size = ws(80, 24);
        // What the photograph of a pane running `claude`, `vim` or `btop`
        // actually holds. There is no epilogue: `DaemonPane::kill` is a hangup
        // with no grace period, so the program never sent its own `?1002l`,
        // and the snapshot was taken before it could have anyway.
        let snapshot = crate::daemon::scrollback::Segment {
            size,
            bytes: b"\x1b[?1049h\x1b[?1002h\x1b[?1003h\x1b[?1006h\x1b[?1004h\
\x1b[?2004hframe one\x1b[Hframe two"
                .to_vec(),
        };
        let mut ring = ReplayRing::seeded(vec![snapshot], size);
        ring.append(&restore_preamble(Some("the shell below is new")));
        let replayed = ring.flatten();

        for mode in [
            "1000", "1002", "1003", "1005", "1006", "1004", "2004", "1049",
        ] {
            assert_eq!(
                last_mode_switch(&replayed, mode),
                Some(b'l'),
                "a client replaying this ring ends with ?{mode} still on, which is \
                 what types SGR reports into the restored shell (#850)"
            );
        }
    }

    /// The ConPTY constraint, from the daemon's side. A pane whose shell runs
    /// on a ConPTY must open with an empty viewport and the cursor at the
    /// top-left, because that is the state conhost's own screen buffer starts
    /// in and every row it names afterwards is counted from there. Restored
    /// output left on screen shifts all of them, and the shell's first repaint
    /// of the line being typed lands on the old text — see
    /// [`SCROLL_RESTORED_AWAY`].
    #[test]
    fn the_preamble_clears_the_way_for_conpty_and_only_for_conpty() {
        let text = String::from_utf8(restore_preamble(Some("this shell is new"))).unwrap();
        if cfg!(windows) {
            assert!(
                text.ends_with("\x1b[2J\x1b[H"),
                "the restored screen has to be scrolled into history and the cursor \
                 homed *last*, after the banner: anything printed afterwards would \
                 take back the row conhost counts from. The preamble ends {:?}",
                &text[text.len().saturating_sub(16)..]
            );
        } else {
            assert!(
                !text.contains("\x1b[2J"),
                "on a real pty the shell positions itself relatively, so the screen \
                 the user asked to have back stays where they can see it"
            );
        }
    }

    #[test]
    fn a_restored_pane_reannounces_its_title() {
        let bytes = retitle("✳ fixing the switcher");
        assert_eq!(
            bytes, b"\x1b]0;\xe2\x9c\xb3 fixing the switcher\x07",
            "the OSC that set the title was trimmed out of the snapshot long ago; \
             the restore has to say it again or the tab comes back as the default name"
        );
    }

    #[test]
    fn a_stored_title_cannot_smuggle_control_bytes_into_the_stream() {
        let bytes = retitle("evil\x07\x1b]0;title\x1b\\rest");
        assert_eq!(
            bytes, b"\x1b]0;evil]0;title\\rest\x07",
            "a BEL or ESC inside the title would terminate the OSC early and leak \
             the rest as input to the terminal"
        );
    }

    #[test]
    fn a_banner_cannot_smuggle_extra_lines_into_the_pane() {
        let text = String::from_utf8(restore_preamble(Some("first\r\nsecond"))).unwrap();
        assert!(
            text.contains("first  second"),
            "the rule is drawn as one line, so the words placed in it stay on one line"
        );
        assert_eq!(
            text.matches("\r\n").count(),
            2,
            "one break before the rule and one after it, and no others"
        );
    }

    #[test]
    fn a_pane_with_nothing_to_say_still_gets_the_resets() {
        for banner in [None, Some(""), Some("   ")] {
            let text = String::from_utf8(restore_preamble(banner)).unwrap();
            assert!(
                text.starts_with("\x1b[?1049l"),
                "the terminal still has to be handed back in a usable state"
            );
            assert!(
                !text.contains('\n'),
                "with no words there is no rule to draw, so no line is spent on one"
            );
        }
    }




    #[test]
    fn a_window_reattaching_to_an_ssh_pane_learns_it_is_connected() {
        use crate::daemon::protocol::SshPhase;
        let mut st = test_state(true);
        st.ssh_phase = Some(SshPhase::Connected);
        let (tx, rx) = std::sync::mpsc::channel();
        attach_subscriber(&mut st, tx);
        st.subscriptions.detach(1, &OutputGate::new(), &mut st.graphics);
        assert!(
            rx.iter().any(|m| matches!(
                m,
                DaemonMsg::SshStatus {
                    phase: SshPhase::Connected
                }
            )),
            "the replay must carry the connection phase"
        );
    }


































    /// Claude Code ends its OSC 11 theme query with DA1 and waits for both.
    /// Started in a pane no window shows (an agent woken at launch), it got
    /// neither and never checked the theme again.
    #[test]
    fn a_detached_pane_answers_device_attributes_and_an_attached_one_leaves_it_to_the_client() {
        #[derive(Clone, Default)]
        struct Sink(Arc<Mutex<Vec<u8>>>);
        impl Write for Sink {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let sink = Sink::default();
        let writer: Mutex<Box<dyn Write + Send>> = Mutex::new(Box::new(sink.clone()));
        let mut st = test_state(true);

        record_output(&mut st, b"\x1b]11;?\x07\x1b[c");
        answer_device_attributes(&writer, detached_device_attribute_queries(&mut st));
        assert_eq!(sink.0.lock().unwrap().as_slice(), DA1_REPLY);

        let (tx, _rx) = mpsc::channel();
        attach_subscriber(&mut st, tx);
        record_output(&mut st, b"\x1b[c");
        answer_device_attributes(&writer, detached_device_attribute_queries(&mut st));
        assert_eq!(
            sink.0.lock().unwrap().as_slice(),
            DA1_REPLY,
            "the client answers"
        );
    }















    /// A window that reopens onto a native ssh pane it outlived attaches by
    /// pane id: it never sees the profile that dialled the host, so it sends
    /// `allow_remote_clipboard_write: false` because that is all it has. The
    /// spec's answer has to survive that, or the permission the user granted
    /// is revoked on the first re-attach and every later copy comes back
    /// `EPERM` with the switch still showing "on".
    #[test]
    fn a_spec_granted_clipboard_permission_survives_a_re_attach() {
        let mut st = test_state(true);
        st.subscriptions = Subscriptions::new(true, Some(true));
        let (tx, _rx) = mpsc::channel();
        attach_subscriber_with_permissions(&mut st, tx, &OutputGate::new(), false, false);
        assert!(st.subscriptions.clipboard_allowed());

        // And a profile that says no is not something an attaching client can
        // talk its way past either.
        let mut st = test_state(true);
        st.subscriptions = Subscriptions::new(false, Some(false));
        let (tx, _rx) = mpsc::channel();
        attach_subscriber_with_permissions(&mut st, tx, &OutputGate::new(), true, false);
        assert!(!st.subscriptions.clipboard_allowed());

        // A pane with no spec of its own — everything on a remote
        // `lyrel-server` — is exactly as permitted as its controller says.
        let mut st = test_state(true);
        let (tx, _rx) = mpsc::channel();
        attach_subscriber_with_permissions(&mut st, tx, &OutputGate::new(), true, false);
        assert!(st.subscriptions.clipboard_allowed());
        let (tx, _rx) = mpsc::channel();
        attach_subscriber_with_permissions(&mut st, tx, &OutputGate::new(), false, false);
        assert!(!st.subscriptions.clipboard_allowed());
    }

    #[test]
    fn reader_strips_allowed_clipboard_images_and_only_notifies_the_controller() {
        use base64::Engine as _;

        let state = Arc::new(Mutex::new(test_state(true)));
        let (controller_tx, controller_rx) = mpsc::channel();
        let (observer_tx, observer_rx) = mpsc::channel();
        {
            let mut st = state.lock().unwrap();
            attach_subscriber_with_permissions(
                &mut st,
                controller_tx,
                &OutputGate::new(),
                true,
                false,
            );
            observe_subscriber(&mut st, observer_tx, Arc::new(OutputGate::new()), false);
        }
        drain(&controller_rx);
        drain(&observer_rx);

        let mime = base64::engine::general_purpose::STANDARD.encode("image/png");
        let data = base64::engine::general_purpose::STANDARD.encode(b"png-data");
        let stream = format!(
            "before\x1b]5522;type=write:id=req-1\x1b\\\
             \x1b]5522;type=wdata:mime={mime};{data}\x1b\\\
             \x1b]5522;type=wdata\x1b\\after"
        );
        DaemonPane::spawn_reader(
            state.clone(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(OutputGate::new()),
            Box::new(std::io::Cursor::new(stream.into_bytes())),
            null_writer(),
            || false,
            ForegroundProbes {
                group: Box::new(|| None),
                remote: Box::new(|| None),
                agent: Box::new(|| None),
                cwd: Box::new(|| None),
                local_group: |_| Some(true),
            },
            Arc::new(DeathReporter::new(|| {})),
        )
        .join()
        .unwrap();

        assert_eq!(state.lock().unwrap().ring.flatten(), b"beforeafter");
        assert!(matches!(
            controller_rx.try_recv(),
            Ok(DaemonMsg::Output(bytes)) if bytes == b"before"
        ));
        let frame = match controller_rx.try_recv().unwrap() {
            DaemonMsg::ClipboardWrite(frame) => frame,
            other => panic!("expected ClipboardWrite, got {other:?}"),
        };
        let write = crate::core::clipboard::ClipboardWrite::decode_frame(frame).unwrap();
        assert_eq!(write.mime, "image/png");
        assert_eq!(write.data, b"png-data");
        assert_eq!(write.id.as_deref(), Some("req-1"));
        assert!(matches!(
            controller_rx.try_recv(),
            Ok(DaemonMsg::Output(bytes)) if bytes == b"after"
        ));

        assert!(matches!(
            observer_rx.try_recv(),
            Ok(DaemonMsg::Output(bytes)) if bytes == b"before"
        ));
        assert!(matches!(
            observer_rx.try_recv(),
            Ok(DaemonMsg::Output(bytes)) if bytes == b"after"
        ));
        assert!(
            !observer_rx
                .try_iter()
                .any(|msg| matches!(msg, DaemonMsg::ClipboardWrite(_))),
            "an observer must never receive a clipboard side effect"
        );
    }

    #[test]
    fn reader_rejects_clipboard_images_when_permission_is_off() {
        use base64::Engine as _;

        let state = Arc::new(Mutex::new(test_state(true)));
        let (controller_tx, controller_rx) = mpsc::channel();
        attach_subscriber(&mut state.lock().unwrap(), controller_tx);
        drain(&controller_rx);

        #[derive(Clone)]
        struct SharedBuf(Arc<Mutex<Vec<u8>>>);
        impl Write for SharedBuf {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let sink = Arc::new(Mutex::new(Vec::new()));
        let writer: Arc<Mutex<Box<dyn Write + Send>>> =
            Arc::new(Mutex::new(Box::new(SharedBuf(sink.clone()))));
        let mime = base64::engine::general_purpose::STANDARD.encode("image/png");
        let data = base64::engine::general_purpose::STANDARD.encode(b"png-data");
        let stream = format!(
            "\x1b]5522;type=write:id=nope\x1b\\\
             \x1b]5522;type=wdata:mime={mime};{data}\x1b\\\
             \x1b]5522;type=wdata\x1b\\"
        );
        DaemonPane::spawn_reader(
            state.clone(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(OutputGate::new()),
            Box::new(std::io::Cursor::new(stream.into_bytes())),
            writer,
            || false,
            ForegroundProbes {
                group: Box::new(|| None),
                remote: Box::new(|| None),
                agent: Box::new(|| None),
                cwd: Box::new(|| None),
                local_group: |_| Some(true),
            },
            Arc::new(DeathReporter::new(|| {})),
        )
        .join()
        .unwrap();

        assert!(state.lock().unwrap().ring.flatten().is_empty());
        assert!(
            !controller_rx
                .try_iter()
                .any(|msg| matches!(msg, DaemonMsg::ClipboardWrite(_)))
        );
        assert_eq!(
            *sink.lock().unwrap(),
            crate::core::clipboard::response(Some("nope"), "EPERM")
        );
    }




    #[cfg(unix)]
    #[test]
    fn the_two_endpoints_are_siblings_derived_from_one_config_dir() {
        // The bug this pins: the control socket used to ignore the config dir
        // and sit in ~/.local/share/lyrel as `daemon.sock`, the same basename the
        // pane socket uses. A `--config-dir` server therefore published someone
        // else's control endpoint, and anything deriving one path from the other
        // by filename got the wrong socket. Both must come from the config dir,
        // and they must not collide.
        let dir = std::path::Path::new("/tmp/lyrel-endpoint-test");
        let pane = crate::daemon::transport::socket_path_for(dir);
        let control = crate::host::server::socket_path_in(dir, &[]).expect("short enough path");

        assert_eq!(pane.parent(), control.parent(), "one directory, two files");
        assert_ne!(
            pane.file_name(),
            control.file_name(),
            "same basename is what made them indistinguishable"
        );
        assert_eq!(pane, dir.join("daemon.sock"));
        assert_eq!(control, dir.join("control.sock"));
    }


    /// A quick launch types the agent's command the moment the shell is up, so
    /// the agent's whole first screen can land inside the poll interval of the
    /// prompt it was typed at — and then it waits for a key, printing nothing
    /// more. Probing only on the interval, the pane never found out an agent
    /// was running. A change of foreground process group now probes at once.
    #[test]
    fn a_new_foreground_group_is_probed_inside_the_poll_interval() {
        struct Chunks(std::collections::VecDeque<Vec<u8>>);

        impl Read for Chunks {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                match self.0.pop_front() {
                    Some(chunk) => {
                        buf[..chunk.len()].copy_from_slice(&chunk);
                        Ok(chunk.len())
                    }
                    None => Ok(0),
                }
            }
        }

        let probes_for = |groups: Vec<i32>| {
            let state = Arc::new(Mutex::new(test_state(true)));
            let probes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let counted = probes.clone();
            let groups = Mutex::new(groups.into_iter());
            let handle = DaemonPane::spawn_reader(
                state,
                Arc::new(AtomicBool::new(false)),
                Arc::new(OutputGate::new()),
                // Plain output, no prompt marks: only the interval or the
                // group can trigger a probe.
                Box::new(Chunks(
                    [b"$ ".to_vec(), b"agent screen".to_vec()]
                        .into_iter()
                        .collect(),
                )),
                null_writer(),
                || false,
                ForegroundProbes {
                    group: Box::new(move || groups.lock().unwrap().next()),
                    remote: Box::new(|| None),
                    agent: Box::new(move || {
                        counted.fetch_add(1, Ordering::SeqCst);
                        None
                    }),
                    cwd: Box::new(|| None),
                    local_group: |_| Some(true),
                },
                Arc::new(DeathReporter::new(|| {})),
            );
            handle.join().unwrap();
            probes.load(Ordering::SeqCst)
        };

        assert_eq!(
            probes_for(vec![10, 10]),
            1,
            "same program: the interval rules"
        );
        assert_eq!(
            probes_for(vec![10, 11]),
            2,
            "a new program is probed at once"
        );
    }

    /// A full-screen program that has run long enough to push its `?1049h`
    /// and its first full paint out of the ring: what is left is differences.
    fn state_running_a_long_lived_program() -> PaneState {
        let mut st = test_state(true);
        record_output(&mut st, b"\x1b[?1049h\x1b[2Jthe first full paint");
        record_output(&mut st, &vec![b'x'; RING_CAP]);
        st
    }

    /// The resizes a nudge makes, in order.
    fn run_recorded_nudge(
        transient: WinSize,
        settled: impl FnOnce() -> Option<WinSize> + Send + 'static,
    ) -> Vec<WinSize> {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&calls);
        run_redraw_nudge(
            transient,
            Duration::from_millis(1),
            Box::new(move |size| record.lock().unwrap().push(size)),
            settled,
        )
        .expect("spawn the nudge thread")
        .join()
        .unwrap();
        calls.lock().unwrap().clone()
    }

    /// #1122: a client reattaching to such a program gets the alternate screen
    /// and a stream of differences from a screen it never saw. The pty steps
    /// a row away and back, so the program hears a size change and repaints.
    #[test]
    fn a_reattach_after_the_first_paint_left_the_ring_makes_the_program_repaint() {
        let mut st = state_running_a_long_lived_program();
        let (tx, _rx) = mpsc::channel();
        attach_subscriber_with_permissions(&mut st, tx, &OutputGate::new(), false, true);

        let transient = redraw_nudge(&mut st, true, std::time::Instant::now())
            .expect("the screen the program painted is gone from the ring");
        assert_eq!(transient, ws(80, 23), "one row off the pane's own size");

        let st = Arc::new(Mutex::new(st));
        let settled = Arc::clone(&st);
        let calls = run_recorded_nudge(transient, move || {
            let st = settled.lock().unwrap();
            st.alive.then(|| st.ring.size())
        });
        assert_eq!(
            calls,
            vec![ws(80, 23), ws(80, 24)],
            "away and back to where the pane was"
        );
        // Only the pty moved: the ring is still at the pane's size, so no
        // client saw a change.
        assert_eq!(st.lock().unwrap().ring.size(), ws(80, 24));
    }

    #[test]
    fn a_reattach_that_replays_whole_leaves_the_program_alone() {
        let now = std::time::Instant::now();

        // The program's start is still in the ring: the replay rebuilds it.
        let mut st = test_state(true);
        record_output(&mut st, b"\x1b[?1049h\x1b[2Jthe first full paint");
        record_output(&mut st, &vec![b'x'; 4096]);
        assert_eq!(redraw_nudge(&mut st, true, now), None, "nothing was lost");

        // The ring dropped its front, but nothing is in the alternate screen:
        // a shell's scrollback replays as what it is.
        let mut st = test_state(true);
        record_output(&mut st, b"$ cat big.log\r\n");
        record_output(&mut st, &vec![b'x'; RING_CAP]);
        assert_eq!(redraw_nudge(&mut st, true, now), None, "primary screen");

        // Left the alternate screen, and the ring cut through both switches.
        let mut st = state_running_a_long_lived_program();
        record_output(&mut st, b"\x1b[?1049l$ ");
        record_output(&mut st, &vec![b'x'; RING_CAP]);
        assert_eq!(redraw_nudge(&mut st, true, now), None, "program left");

        let mut st = state_running_a_long_lived_program();
        assert_eq!(
            redraw_nudge(&mut st, false, now),
            None,
            "no program owns the pane to repaint"
        );

        let mut st = state_running_a_long_lived_program();
        st.alive = false;
        assert_eq!(redraw_nudge(&mut st, true, now), None, "pane is dead");
    }

    #[test]
    fn a_burst_of_reattaches_makes_one_nudge() {
        let mut st = state_running_a_long_lived_program();
        let now = std::time::Instant::now();
        assert!(redraw_nudge(&mut st, true, now).is_some());
        assert_eq!(
            redraw_nudge(&mut st, true, now + Duration::from_millis(500)),
            None,
            "the repaint the first one asked for is on its way"
        );
        assert!(
            redraw_nudge(&mut st, true, now + REDRAW_NUDGE_COOLDOWN).is_some(),
            "a reattach well after it is nudged again"
        );
    }

    /// The size put back is the pane's size when it is put back: a resize that
    /// landed in between is what the pty ends at.
    #[test]
    fn a_nudge_puts_back_the_size_the_pane_has_by_then() {
        let calls = run_recorded_nudge(ws(80, 23), || Some(ws(100, 30)));
        assert_eq!(calls, vec![ws(80, 23), ws(100, 30)]);

        let calls = run_recorded_nudge(ws(80, 23), || None);
        assert_eq!(calls, vec![ws(80, 23)], "a pane that died is left alone");
    }

    #[test]
    fn a_one_row_pane_is_nudged_a_row_taller() {
        let mut st = state_running_a_long_lived_program();
        st.ring.resize(ws(80, 1));
        assert_eq!(
            redraw_nudge(&mut st, true, std::time::Instant::now()),
            Some(ws(80, 2))
        );
    }

}
