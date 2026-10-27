use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

use portable_pty::{Child, MasterPty, PtySize, native_pty_system};

use crate::core::clipboard::ClipboardSniffer;
use crate::core::kitty_graphics::{GraphicsEffect, GraphicsSniffer, GraphicsState};
use crate::core::osc::OscTokenizer;
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
mod replay;
pub mod subscriptions;
#[cfg(test)]
mod test_support;
use subscriptions::{GraphicsReplayAdmission, OutputGate, ReplayState, Subscriptions};
pub(crate) use launch::LYREL_PANE_ENV;
use launch::build_spawn_config;
use replay::ReplayRing;
#[cfg(test)]
use replay::RING_CAP;


const REMOTE_CONTEXT_POLL_INTERVAL: Duration = Duration::from_millis(500);


/// How long the death path will chase a child's exit status before giving up
/// and reporting `Exited { code: None }`. The pty hits EOF when the last slave
/// fd closes, which can land a few milliseconds ahead of the child becoming
/// reapable — that race is all this window exists to cover. Everything past it
/// is a pane that looks frozen to every attached client, so it stays short.
const EXIT_CODE_PROBE_WINDOW: Duration = Duration::from_millis(500);

const EXIT_CODE_PROBE_INTERVAL: Duration = Duration::from_millis(10);


struct PaneState {
    id: u64,
    ring: ReplayRing,
    /// Authoritative resources live with the pane, not its current controller.
    /// Taken on exit even if a client keeps the dead pane's text attached.
    graphics: Option<GraphicsState>,
    subscriptions: Subscriptions,
    cwd: Option<PathBuf>,
    /// The last title the pane reported over OSC 0/2, for the machine tree to
    /// record. See [`crate::core::machine::PaneRecord::osc_title`].
    osc_title: Option<String>,
    shell: ShellState,
    /// Whether a prompt mark has arrived since `remote` was last set.
    ///
    /// The near shell cannot be at a prompt while a connection owns its pty,
    /// so a mark that says "at a prompt" on a remote pane can only be the far
    /// shell's — and that is the proof that the far side runs lyrel's shell
    /// integration. Until it lands, the newest mark on the pane is the near
    /// shell's own "I started `ssh`", which says nothing about the far side
    /// and will never be superseded. Cleared whenever `remote` changes, so a
    /// second hop is proved on its own terms.
    remote_prompt_seen: bool,
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
    /// What this pane is running, for the machine tree to record. Distinct from
    /// `shell` above, which is the shell-integration state.
    shell_spec: Option<ShellSpec>,
    remote: Option<RemoteContext>,
    agent: Option<crate::core::cli_agent::CLIAgent>,
    agent_argv: Option<Vec<String>>,
    agent_session: Option<crate::core::cli_agent::AgentSessionState>,
    kimi_observer: Option<super::kimi::Observer>,
    alive: bool,
    exit_code: Option<i32>,
    /// When a reattach last made the program on this pane repaint; see
    /// [`redraw_nudge`].
    last_redraw_nudge: Option<std::time::Instant>,
}

fn notify(st: &mut PaneState, msg: DaemonMsg) {
    st.subscriptions.notify(msg, &mut st.graphics);
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

enum PaneBackend {
    Pty(PtyBackend),
    NativeSsh(NativeSshBackend),
}

struct ForegroundProbes {
    /// The pty's foreground process group: one ioctl, cheap enough to ask on
    /// every read, and a change in it is what says the probes below would now
    /// answer differently.
    group: Box<dyn Fn() -> Option<i32> + Send>,
    remote: Box<dyn Fn() -> Option<RemoteContext> + Send>,
    agent: Box<dyn Fn() -> Option<Option<(crate::core::cli_agent::CLIAgent, Vec<String>)>> + Send>,
    cwd: Box<dyn Fn() -> Option<PathBuf> + Send>,
    /// Same-group child permission proof, separate from the UI context parser.
    local_group: fn(i32) -> Option<bool>,
}

struct PtyBackend {
    /// Owns the PTY master until the pane is closed.
    ///
    /// Windows takes this value when the shell process exits. Dropping the
    /// ConPTY master calls `ClosePseudoConsole`, which closes the output side
    /// only after its pending bytes can be consumed by the dedicated reader.
    /// Keeping the slot optional lets that monitor end the stream without
    /// racing the reader for ownership of the exit notification.
    master: Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    #[cfg_attr(windows, allow(dead_code))]
    shell_pid: Option<u32>,
    integration_dir: Option<PathBuf>,
}

/// The pty half of a pane, before it is wired up: opened and spawned into by
/// [`DaemonPane::spawn`], inherited across an `exec` by [`DaemonPane::adopt`].
struct PtyParts {
    master: Box<dyn MasterPty + Send>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    shell_pid: Option<u32>,
    integration_dir: Option<PathBuf>,
    reader_handle: Box<dyn Read + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
}

struct NativeSshBackend {
    handle: Arc<crate::daemon::ssh::SshSessionHandle>,
    connection: crate::daemon::ssh::SharedConnection,
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

struct DeathReporter {
    reported: AtomicBool,
    on_dead: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    exit_code: Mutex<Option<Box<dyn FnMut() -> Option<i32> + Send>>>,
}

impl DeathReporter {
    fn new(on_dead: impl FnOnce() + Send + 'static) -> Self {
        Self {
            reported: AtomicBool::new(false),
            on_dead: Mutex::new(Some(Box::new(on_dead))),
            exit_code: Mutex::new(None),
        }
    }

    fn probe_exit_code(&self, probe: impl FnMut() -> Option<i32> + Send + 'static) {
        *self.exit_code.lock().unwrap() = Some(Box::new(probe));
    }

    #[cfg(windows)]
    fn has_reported(&self) -> bool {
        self.reported.load(Ordering::SeqCst)
    }

    fn report(&self, state: &Mutex<PaneState>, shutting_down: &AtomicBool) {
        if self.reported.swap(true, Ordering::SeqCst) {
            return;
        }
        let code = self
            .exit_code
            .lock()
            .unwrap()
            .as_mut()
            .and_then(|probe| probe());
        let mut st = state.lock().unwrap();
        st.alive = false;
        st.exit_code = code;
        let PaneState { subscriptions, graphics, .. } = &mut *st;
        subscriptions.release_graphics(graphics);
        let pane = st.id;
        log::info!(
            "pane {pane} exited; exit_code={code:?} shutdown_requested={}",
            shutting_down.load(Ordering::SeqCst)
        );
        if shutting_down.load(Ordering::SeqCst) {
            drop(st);
            crate::core::machine::observe_pane(pane, |p| p.live = false);
            return;
        }
        let subscribed = st.subscriptions.controller_epoch().is_some();
        notify(&mut st, DaemonMsg::Exited { code });
        drop(st);
        crate::core::machine::observe_pane(pane, |p| p.live = false);
        if subscribed {
            return;
        }
        if let Some(on_dead) = self.on_dead.lock().unwrap().take() {
            on_dead();
        }
    }
}

/// Releases the PTY master without holding its slot mutex during destruction.
///
/// `ClosePseudoConsole` may wait while the output pipe is drained, and the same
/// slot is what `resize` locks. Dropping the master only after releasing the
/// mutex keeps a concurrent resize from parking behind a close that is itself
/// waiting on the reader.
#[cfg(windows)]
fn close_pty_master(master: &Mutex<Option<Box<dyn MasterPty + Send>>>) {
    let owned = master.lock().ok().and_then(|mut slot| slot.take());
    drop(owned);
}

/// How long the Windows exit monitor lets the reader announce the death on its
/// own before doing it itself.
///
/// The reader is the better reporter: it publishes `Exited` only after it has
/// forwarded every byte that preceded EOF, which is what keeps a short-lived
/// command's final frame ahead of its exit. But EOF is not guaranteed. A
/// grandchild that inherited the ConPTY output pipe holds it open after the
/// shell is gone — `cmd /c start …` is enough — and `ClosePseudoConsole` then
/// never completes. Without this window such a pane would read as alive
/// forever to every attached client.
#[cfg(windows)]
const EXIT_DRAIN_WINDOW: Duration = Duration::from_secs(2);

#[cfg(windows)]
const EXIT_DRAIN_POLL: Duration = Duration::from_millis(10);

/// Releases the pseudoconsole, then reports the pane's death — preferring the
/// reader's EOF-ordered report and falling back to its own after `window`.
///
/// Must run on a background thread: both halves block.
#[cfg(windows)]
fn drain_then_report(
    master: Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>,
    state: Arc<Mutex<PaneState>>,
    shutting_down: Arc<AtomicBool>,
    death: Arc<DeathReporter>,
    window: Duration,
) {
    // `ClosePseudoConsole` can block until the reader drains the pipe, so it
    // cannot run on the thread that owns the deadline below.
    std::thread::Builder::new()
        .name("lyrel-daemon-pane-pty-close".to_string())
        .spawn(move || close_pty_master(&master))
        .expect("spawn daemon pane pty close thread");

    let deadline = std::time::Instant::now() + window;
    while !death.has_reported() && std::time::Instant::now() < deadline {
        std::thread::sleep(EXIT_DRAIN_POLL);
    }
    // `DeathReporter` is idempotent, so this is a no-op whenever the reader
    // already got there — which is the ordinary case.
    death.report(&state, &shutting_down);
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

/// A pty master this process inherited from its own previous image.
///
/// `portable-pty` can only hand out a master it opened, and after an `exec`
/// there is nothing left of the one it opened except the descriptor number. The
/// operations a pane actually performs on a master are few enough — resize, ask
/// the size, clone a reader, take a writer, ask which process group is in the
/// foreground — and all of them are ioctls on that descriptor.
#[cfg(unix)]
struct AdoptedMaster {
    fd: std::os::fd::OwnedFd,
}

#[cfg(unix)]
impl AdoptedMaster {
    fn from_fd(fd: std::os::fd::RawFd) -> anyhow::Result<Self> {
        use std::os::fd::FromRawFd as _;

        // A descriptor that no longer refers to a terminal is one the previous
        // image did not really hold — a number reused after a close, or a blob
        // that named the wrong one. Adopting it would produce a pane whose
        // every ioctl fails in a different way; refusing produces a pane that
        // is simply gone, which is a thing the client already handles.
        if unsafe { libc::isatty(fd) } != 1 {
            anyhow::bail!("descriptor {fd} is not a terminal");
        }
        // Crossing the exec required stripping close-on-exec; being across is
        // when it goes back on. Children this daemon spawns from here — shells,
        // `lsof`, ssh transports — must not inherit another pane's master: a
        // child holding one keeps the pty open after this pane closes it, and
        // the hangup the shell should see never comes.
        if let Err(e) = crate::daemon::handoff::close_on_exec_again(fd) {
            log::warn!("could not restore close-on-exec on adopted pty {fd}: {e}");
        }
        Ok(Self {
            fd: unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) },
        })
    }

    fn dup(&self) -> std::io::Result<std::fs::File> {
        Ok(std::fs::File::from(self.fd.try_clone()?))
    }
}

#[cfg(unix)]
impl MasterPty for AdoptedMaster {
    fn resize(&self, size: PtySize) -> anyhow::Result<()> {
        use std::os::fd::AsRawFd as _;

        let ws = libc::winsize {
            ws_row: size.rows,
            ws_col: size.cols,
            ws_xpixel: size.pixel_width,
            ws_ypixel: size.pixel_height,
        };
        if unsafe { libc::ioctl(self.fd.as_raw_fd(), libc::TIOCSWINSZ, &ws as *const _) } != 0 {
            anyhow::bail!("TIOCSWINSZ failed: {}", std::io::Error::last_os_error());
        }
        Ok(())
    }

    fn get_size(&self) -> anyhow::Result<PtySize> {
        use std::os::fd::AsRawFd as _;

        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        if unsafe { libc::ioctl(self.fd.as_raw_fd(), libc::TIOCGWINSZ, &mut ws as *mut _) } != 0 {
            anyhow::bail!("TIOCGWINSZ failed: {}", std::io::Error::last_os_error());
        }
        Ok(PtySize {
            rows: ws.ws_row,
            cols: ws.ws_col,
            pixel_width: ws.ws_xpixel,
            pixel_height: ws.ws_ypixel,
        })
    }

    fn try_clone_reader(&self) -> anyhow::Result<Box<dyn Read + Send>> {
        Ok(Box::new(self.dup()?))
    }

    /// Deliberately a plain `File`, unlike the writer `portable-pty` hands out:
    /// that one sends EOF to the shell when it is dropped. Correct for a pty
    /// whose pane is being closed, wrong for one whose pane is only changing
    /// which program serves it.
    fn take_writer(&self) -> anyhow::Result<Box<dyn Write + Send>> {
        Ok(Box::new(self.dup()?))
    }

    fn as_raw_fd(&self) -> Option<std::os::fd::RawFd> {
        use std::os::fd::AsRawFd as _;
        Some(self.fd.as_raw_fd())
    }

    fn tty_name(&self) -> Option<PathBuf> {
        use std::ffi::{CStr, OsStr};
        use std::os::fd::AsRawFd as _;
        use std::os::unix::ffi::OsStrExt as _;

        // Query the slave name from the inherited master without opening the
        // slave again: doing so would change this pane's hangup lifetime.
        // macOS fixes this ioctl's output at 128 bytes; Linux's /dev/pts/N
        // names fit the same stack buffer.
        let mut name = [0u8; 128];
        #[cfg(target_os = "macos")]
        let result = unsafe {
            libc::ioctl(
                self.fd.as_raw_fd(),
                libc::TIOCPTYGNAME as libc::c_ulong,
                name.as_mut_ptr(),
            )
        };
        #[cfg(not(target_os = "macos"))]
        let result =
            unsafe { libc::ptsname_r(self.fd.as_raw_fd(), name.as_mut_ptr().cast(), name.len()) };
        if result != 0 {
            return None;
        }
        let name = CStr::from_bytes_until_nul(&name).ok()?;
        Some(PathBuf::from(OsStr::from_bytes(name.to_bytes())))
    }

    fn process_group_leader(&self) -> Option<libc::pid_t> {
        use std::os::fd::AsRawFd as _;
        match unsafe { libc::tcgetpgrp(self.fd.as_raw_fd()) } {
            pid if pid > 0 => Some(pid),
            _ => None,
        }
    }
}

/// A child this process inherited from its own previous image.
///
/// Still genuinely our child — `exec` keeps the process, so the kernel's
/// parent-child bookkeeping is untouched and `waitpid` works. What was lost is
/// the `Child` value that knew how to ask.
#[cfg(unix)]
#[derive(Debug)]
struct AdoptedChild {
    pid: libc::pid_t,
    exited: Option<portable_pty::ExitStatus>,
}

#[cfg(unix)]
impl AdoptedChild {
    fn new(pid: u32) -> Self {
        Self {
            pid: pid as libc::pid_t,
            exited: None,
        }
    }

    fn reap(&mut self, flags: libc::c_int) -> std::io::Result<Option<portable_pty::ExitStatus>> {
        if let Some(status) = &self.exited {
            return Ok(Some(status.clone()));
        }
        let mut raw: libc::c_int = 0;
        let seen = unsafe { libc::waitpid(self.pid, &mut raw, flags) };
        if seen == 0 {
            return Ok(None);
        }
        if seen < 0 {
            let e = std::io::Error::last_os_error();
            // ECHILD means someone already reaped it, or it was never ours.
            // Either way there is no status left to collect and no point in
            // asking again — reporting an exit of zero is what an unknown but
            // finished child amounts to.
            if e.raw_os_error() == Some(libc::ECHILD) {
                let status = portable_pty::ExitStatus::with_exit_code(0);
                self.exited = Some(status.clone());
                return Ok(Some(status));
            }
            return Err(e);
        }
        let code = if libc::WIFEXITED(raw) {
            libc::WEXITSTATUS(raw) as u32
        } else if libc::WIFSIGNALED(raw) {
            128 + libc::WTERMSIG(raw) as u32
        } else {
            0
        };
        let status = portable_pty::ExitStatus::with_exit_code(code);
        self.exited = Some(status.clone());
        Ok(Some(status))
    }
}

#[cfg(unix)]
impl portable_pty::Child for AdoptedChild {
    fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
        self.reap(libc::WNOHANG)
    }

    fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
        loop {
            if let Some(status) = self.reap(0)? {
                return Ok(status);
            }
        }
    }

    fn process_id(&self) -> Option<u32> {
        Some(self.pid as u32)
    }
}

#[cfg(unix)]
#[derive(Debug)]
struct AdoptedKiller {
    pid: libc::pid_t,
}

#[cfg(unix)]
impl portable_pty::ChildKiller for AdoptedChild {
    fn kill(&mut self) -> std::io::Result<()> {
        AdoptedKiller { pid: self.pid }.kill()
    }

    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        Box::new(AdoptedKiller { pid: self.pid })
    }
}

#[cfg(unix)]
impl portable_pty::ChildKiller for AdoptedKiller {
    fn kill(&mut self) -> std::io::Result<()> {
        if unsafe { libc::kill(self.pid, libc::SIGKILL) } != 0 {
            let e = std::io::Error::last_os_error();
            // Already gone is the outcome kill was asked for.
            if e.raw_os_error() != Some(libc::ESRCH) {
                return Err(e);
            }
        }
        Ok(())
    }

    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        Box::new(AdoptedKiller { pid: self.pid })
    }
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
        let foreground = self.has_foreground_command();
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
        let foreground = self.has_foreground_command();
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
        #[cfg(windows)]
        crate::daemon::conpty::ensure_prepared()?;

        let pty_size = pty_size(size);

        // Before the shell exists, not after: a ConPTY child inherits the
        // "ignore Ctrl+C" state of whoever created it, and the daemon can be
        // carrying it from its own launch. Leaving it on costs the pane every
        // Ctrl+C it will ever be sent (#451, #314).
        #[cfg(windows)]
        crate::daemon::winproc::allow_ctrl_c_in_children();

        let pair = native_pty_system().openpty(pty_size)?;
        let spawn = build_spawn_config(id, cwd, shell, workspace.as_deref())?;

        let child = pair.slave.spawn_command(spawn.cmd)?;
        let shell_pid = child.process_id();
        log::info!("pane {id} child started; child_pid={shell_pid:?}");
        let child = Arc::new(Mutex::new(child));

        drop(pair.slave);

        let reader_handle = pair.master.try_clone_reader()?;
        let writer = Arc::new(Mutex::new(pair.master.take_writer()?));

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
            PtyParts {
                master: pair.master,
                child,
                shell_pid,
                integration_dir: spawn.integration_dir,
                reader_handle,
                writer,
            },
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
                cwd: spawn.initial_cwd,
                osc_title: restored_title,
                shell: ShellState::default(),
                remote_prompt_seen: false,
                ssh_phase: None,
                modes: TerminalModes::default(),
                shell_spec: spawn.shell.clone(),
                remote: spawn.remote.clone(),
                agent: None,
                agent_session: None,
                kimi_observer: None,
                agent_argv: None,
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
        let PtyParts {
            master,
            child,
            shell_pid,
            integration_dir,
            reader_handle,
            writer,
        } = parts;

        let id = state.id;
        let state = Arc::new(Mutex::new(state));
        let shutting_down = Arc::new(AtomicBool::new(false));
        let gate = Arc::new(OutputGate::new());
        let master = Arc::new(Mutex::new(Some(master)));

        let pane = Arc::new(Self {
            id,
            owner,
            backend: PaneBackend::Pty(PtyBackend {
                master: master.clone(),
                child: child.clone(),
                shell_pid,
                integration_dir,
            }),
            writer: writer.clone(),
            shutting_down: shutting_down.clone(),
            gate: gate.clone(),
            state: state.clone(),
            reader: Mutex::new(None),
            broker: None,
        });

        let death = Arc::new(DeathReporter::new(on_dead));
        death.probe_exit_code({
            let child = child.clone();
            move || {
                let deadline = std::time::Instant::now() + EXIT_CODE_PROBE_WINDOW;
                loop {
                    // try_lock, never lock: DaemonPane::drop holds this very
                    // mutex across a blocking child.wait(), and this probe runs
                    // on the reader thread on its way to announcing the death.
                    // Blocking for the lock would park the Exited notification
                    // behind a wait() with no bound of its own; the deadline
                    // below is the only thing clients should ever wait on.
                    if let Ok(mut child) = child.try_lock() {
                        match child.try_wait() {
                            Ok(Some(status)) => return Some(status.exit_code() as i32),
                            Ok(None) => {}
                            Err(_) => return None,
                        }
                    }
                    if std::time::Instant::now() >= deadline {
                        return None;
                    }
                    std::thread::sleep(EXIT_CODE_PROBE_INTERVAL);
                }
            }
        });

        #[cfg(windows)]
        Self::spawn_exit_monitor(
            shell_pid,
            master.clone(),
            state.clone(),
            pane.shutting_down.clone(),
            death.clone(),
        );

        let fg_master = master.clone();
        let group_master = master.clone();
        let remote_master = master.clone();
        let agent_master = master.clone();
        let cwd_master = master.clone();
        let reader = Self::spawn_reader(
            state,
            shutting_down,
            gate,
            reader_handle,
            writer.clone(),
            move || foreground_command_running(&fg_master, shell_pid),
            ForegroundProbes {
                group: Box::new(move || pty_foreground_pgid(&group_master)),
                remote: Box::new(move || foreground_remote_context(&remote_master)),
                agent: Box::new(move || foreground_agent(&agent_master)),
                cwd: Box::new(move || foreground_cwd(&cwd_master, shell_pid)),
                local_group: crate::daemon::procinfo::foreground_group_is_local,
            },
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
        let PaneBackend::Pty(pty) = &self.backend else {
            return None;
        };
        let master_fd = pty
            .master
            .lock()
            .ok()?
            .as_ref()
            .and_then(|master| master.as_raw_fd())?;
        let st = self.state.lock().unwrap();
        Some(Carried {
            id: self.id,
            owner: self.owner.clone(),
            master_fd,
            child_pid: pty.shell_pid?,
            integration_dir: pty.integration_dir.clone(),
            size: st.ring.tail_size(),
            ring: st.ring.snapshot(),
            cwd: st.cwd.clone(),
            osc_title: st.osc_title.clone(),
            shell_spec: st.shell_spec.clone(),
            shell_active: st.shell.active,
            at_prompt: st.shell.at_prompt,
            last_exit: st.shell.last_exit_code,
            remote: st.remote.clone(),
            agent: st.agent,
            agent_argv: st.agent_argv.clone(),
            agent_session: st.agent_session.clone(),
            head_modes: st.ring.head_restore_bytes(),
        })
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
        carried: crate::daemon::pane::Carried,
        on_dead: impl FnOnce() + Send + 'static,
    ) -> anyhow::Result<Arc<Self>> {
        let master = AdoptedMaster::from_fd(carried.master_fd)?;
        let reader_handle = master.try_clone_reader()?;
        let writer = Arc::new(Mutex::new(master.take_writer()?));
        let shell_pid = carried.child_pid;
        // The size the pty is actually at is the kernel's to answer, and it is
        // the truth: nobody resized it while we were not running. The carried
        // size only says where the ring's tail was cut.
        let size = master
            .get_size()
            .map(|s| WinSize {
                cols: s.cols,
                rows: s.rows,
                cell_w: carried.size.cell_w,
                cell_h: carried.size.cell_h,
            })
            .unwrap_or(carried.size);

        let (mut ring, modes) =
            ReplayRing::adopted(carried.ring, size, carried.head_modes.as_deref());
        // Not a restore banner: nothing was lost, so there is nothing to
        // announce and nothing to reset. The shell below these bytes is the
        // same shell that wrote them.
        ring.resize(size);

        Ok(Self::over_pty(
            PtyParts {
                master: Box::new(master),
                child: Arc::new(Mutex::new(Box::new(AdoptedChild::new(shell_pid)))),
                shell_pid: Some(shell_pid),
                integration_dir: carried.integration_dir,
                reader_handle,
                writer,
            },
            PaneState {
                id: carried.id,
                ring,
                graphics: Some(GraphicsState::new(size)),
                subscriptions: Subscriptions::new(false, None),
                cwd: carried.cwd,
                osc_title: carried.osc_title,
                shell_spec: carried.shell_spec,
                shell: ShellState {
                    active: carried.shell_active,
                    at_prompt: carried.at_prompt,
                    last_exit_code: carried.last_exit,
                    command: None,
                    // Not carried: the handoff record is a wire format shared
                    // with older images, and a pane mid-`ssh` that comes back
                    // claiming a prompt it cannot vouch for would be worse
                    // than one that says it does not know. It re-latches on
                    // the far side's next prompt.
                    mark_at_prompt: false,
                },
                remote_prompt_seen: false,
                ssh_phase: None,
                modes,
                remote: carried.remote,
                agent: carried.agent,
                kimi_observer: if carried.agent == Some(crate::core::cli_agent::CLIAgent::Kimi) {
                    carried.agent_session.as_ref().and_then(kimi_observer)
                } else {
                    None
                },
                agent_session: carried.agent_session,
                agent_argv: carried.agent_argv,
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
            // A native ssh pane is not running a shell of this machine's; what
            // it is, `ssh_spec` already says.
            shell_spec: None,
            cwd: None,
            osc_title: None,
            shell: ShellState::default(),
            remote_prompt_seen: false,
            ssh_phase: None,
            modes: TerminalModes::default(),
            remote: Some(remote),
            agent: None,
            agent_session: None,
            kimi_observer: None,
            agent_argv: None,
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
            backend: PaneBackend::NativeSsh(NativeSshBackend {
                handle: bridge.handle,
                connection: connection.clone(),
            }),
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
        match &self.backend {
            PaneBackend::NativeSsh(b) => b.connection.lock().unwrap().upgrade(),
            PaneBackend::Pty(_) => None,
        }
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
        let ForegroundProbes {
            group: foreground_group,
            remote: foreground_remote,
            agent: foreground_agent_fn,
            cwd: foreground_cwd_fn,
            local_group: foreground_group_locality,
        } = probes;
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
                let mut sniffer = OscSniffer::new();
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
                let starts_local = state.lock().unwrap().remote.is_none();
                let mut graphics = owner.map_or_else(|| GraphicsSniffer::new_local(starts_local), |owner| GraphicsSniffer::new_budgeted(starts_local, owner));
                let mut buf = [0u8; 65536];

                let trace = std::env::var("LYREL_TRACE").is_ok_and(|v| !v.is_empty() && v != "0");
                let mut tr_last = std::time::Instant::now();
                let mut tr_bytes: u64 = 0;
                let mut tr_reads: u32 = 0;
                let mut tr_read_t = std::time::Duration::ZERO;
                let mut tr_disp_t = std::time::Duration::ZERO;
                let mut next_remote_check = std::time::Instant::now();
                let mut last_group = None;

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
                            // The foreground-group ioctl is cheap; full process
                            // probing still runs only on a hop or the interval.
                            // Resolve locality before parsing this batch: its
                            // first APC can already name a remote file or shm.
                            let group = foreground_group();
                            let group_changed = group != last_group;
                            last_group = group;
                            let poll_before = group_changed
                                || std::time::Instant::now() >= next_remote_check;
                            let (managed, cached_local) = {
                                let st = state.lock().unwrap();
                                (st.remote.as_ref().is_some_and(|remote| remote.kind != RemoteKind::Ssh), st.remote.is_none())
                            };
                            let mut remote = (poll_before && !managed).then(&foreground_remote);
                            graphics.set_local(group.is_some() && remote.as_ref().map_or(cached_local, Option::is_none));
                            let controller = {
                                let st = state.lock().unwrap();
                                let controller = st.subscriptions.controller_epoch();
                                clipboard.set_controller(controller, st.subscriptions.clipboard_allowed());
                                controller
                            };
                            // Cached process groups are facts, not permission:
                            // exec ssh can keep the same pgid. Probe afresh only
                            // when the parser is accepting an indirect medium.
                            let mut media_locality = || {
                                if managed { Some(false) } else {
                                    let pgid = foreground_group()?;
                                    if foreground_remote().is_some() { Some(false) } else { foreground_group_locality(pgid) }
                                }
                            };
                            let passthrough = graphics_reader::consume(raw, &mut clipboard, &mut graphics, &state, &gate, &writer, controller, &mut media_locality);
                            let bytes: &[u8] = &passthrough;
                            let mut signals = sniffer.feed(bytes);
                            let mut st = state.lock().unwrap();
                            let da1_owed = detached_device_attribute_queries(&mut st);
                            drop(st);

                            // `any` first: `foreground_running` is a syscall,
                            // and most reads carry no prompt mark at all.
                            if signals.shell.iter().any(|s| s.at_prompt) && foreground_running() {
                                suppress_relayed_prompt_marks(&mut signals.shell);
                            }

                            // A prompt mark that survived the suppression above
                            // is the shell saying the command it ran is over, so
                            // the foreground has just gone back to being the
                            // shell itself. Probe right then rather than waiting
                            // out the interval: the probe is what clears the
                            // remote context an `ssh` left behind, and the
                            // interval alone can miss it forever. Polling only
                            // runs on output, and the prompt the shell just drew
                            // is the last output a pane produces until the user
                            // types again — so a pane whose `ssh` exited inside
                            // the interval kept reporting itself as remote, and
                            // everything keyed off that (the history scope ↑
                            // reads, most visibly) stayed on the far end until
                            // some unrelated output arrived (#817).
                            let back_at_prompt = signals.shell.iter().any(|s| s.at_prompt);
                            let poll_now = back_at_prompt || poll_before;
                            if poll_now {
                                next_remote_check =
                                    std::time::Instant::now() + REMOTE_CONTEXT_POLL_INTERVAL;
                            }
                            if back_at_prompt && !poll_before && !managed {
                                remote = Some(foreground_remote());
                            }
                            let agent = poll_now.then(&foreground_agent_fn).flatten();
                            let probed_cwd = poll_now.then(&foreground_cwd_fn).flatten();

                            let tr1 = trace.then(std::time::Instant::now);
                            let may_change_facts = signals.cwd.is_some()
                                || signals.title.is_some()
                                || !signals.agent_events.is_empty()
                                || signals.notification.is_some()
                                || !signals.shell.is_empty()
                                || remote.is_some()
                                || agent.is_some()
                                || probed_cwd.is_some();
                            // Read off `signals` before `apply_signals` consumes
                            // it; spent after the hop below — see the function.
                            let saw_prompt_mark =
                                signals.shell.iter().any(|s| s.mark_at_prompt);
                            let mut st = state.lock().unwrap();
                            let facts_before = may_change_facts.then(|| observed_facts(&st));
                            apply_signals(&mut st, signals);
                            if let Some(remote) = remote {
                                apply_remote_context(&mut st, remote);
                            }
                            latch_remote_prompt(&mut st, saw_prompt_mark);
                            // Keep kitty file/shm transfer gated on the pane's
                            // *current* locality: an `ssh` that just took the PTY
                            // must stop us honoring host-local object names. Cheap
                            // and only meaningful when a probe follows.
                            graphics.set_local(st.remote.is_none());
                            if let Some(agent) = agent {
                                apply_agent(&mut st, agent);
                            }
                            apply_probed_cwd(&mut st, probed_cwd);
                            if let Some(tr1) = tr1 {
                                tr_disp_t += tr1.elapsed();
                            }
                            let pane = st.id;
                            let alive = st.alive;
                            let facts_after = may_change_facts.then(|| observed_facts(&st));
                            drop(st);
                            answer_device_attributes(&writer, da1_owed);
                            if !shutting_down.load(Ordering::SeqCst)
                                && let (Some(before), Some(after)) = (facts_before, facts_after)
                                && facts_changed(&before, &after)
                            {
                                publish_facts(pane, alive, after);
                            }
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
        let foreground_command = self.has_foreground_command();
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
        match &self.backend {
            PaneBackend::Pty(_) => foreground_command,
            PaneBackend::NativeSsh(_) => true,
        }
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
        let resize: Box<dyn Fn(WinSize) + Send> = match &self.backend {
            PaneBackend::Pty(p) => {
                let master = Arc::clone(&p.master);
                Box::new(move |size| {
                    if let Ok(master) = master.lock()
                        && let Some(master) = master.as_ref()
                    {
                        let _ = master.resize(pty_size(size));
                    }
                })
            }
            PaneBackend::NativeSsh(b) => {
                let handle = Arc::clone(&b.handle);
                Box::new(move |size| handle.resize(size))
            }
        };
        let state = Arc::clone(&self.state);
        let _ = run_redraw_nudge(transient, REDRAW_NUDGE_SETTLE, resize, move || {
            let st = state.lock().unwrap();
            st.alive.then(|| st.ring.size())
        });
    }

    /// Whether something other than the pane's own shell owns the terminal.
    ///
    /// Answered by the kernel (`tcgetpgrp`), not by the pane's stored shell
    /// state, which is why it can contradict `st.shell.at_prompt` — see
    /// subscription replay. A pane with no pty to ask (native ssh, and
    /// every pane on Windows, where the pty has no foreground process group)
    /// answers "no", which is what the live suppression already assumes.
    fn has_foreground_command(&self) -> bool {
        match &self.backend {
            PaneBackend::Pty(pty) => foreground_command_running(&pty.master, pty.shell_pid),
            PaneBackend::NativeSsh(_) => false,
        }
    }

    pub fn detach(&self, epoch: u64) -> bool {
        let mut st = self.state.lock().unwrap();
        let PaneState { subscriptions, graphics, alive, .. } = &mut *st;
        subscriptions.detach(epoch, &self.gate, graphics);
        !*alive && subscriptions.controller_epoch().is_none()
    }

    pub fn observe(&self, observer: Sender<DaemonMsg>, gate: Arc<OutputGate>) -> u64 {
        let foreground_command = self.has_foreground_command();
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
        agent_state_snapshot(&self.state.lock().unwrap())
    }

    /// Poll only authoritative durable events; silence never finishes a turn.
    pub fn poll_agent_events(&self) {
        let observer = {
            let mut state = self.state.lock().unwrap();
            if !state.alive
                || state.remote.is_some()
                || state.agent != Some(crate::core::cli_agent::CLIAgent::Kimi)
            {
                return;
            }
            state.kimi_observer.take()
        };
        let Some(mut observer) = observer else {
            return;
        };
        let events = observer.poll();
        let mut state = self.state.lock().unwrap();
        let matches = state.alive
            && state.agent == Some(crate::core::cli_agent::CLIAgent::Kimi)
            && state
                .agent_session
                .as_ref()
                .and_then(|session| session.session_id.as_deref())
                == Some(observer.session_id.as_str())
            && state
                .agent_session
                .as_ref()
                .and_then(|session| session.readout.session_scope.as_ref())
                .and_then(|scope| scope.session_file.as_ref())
                .and_then(|file| file.parent())
                .map(|dir| dir.join("agents/main/wire.jsonl"))
                == Some(observer.file.clone());
        if !matches || state.kimi_observer.is_some() {
            return;
        }
        let before = observed_facts(&state);
        state.kimi_observer = Some(observer);
        match events {
            Ok(events) => apply_agent_signals(&mut state, events, None),
            Err(error) => {
                let message =
                    format!("Kimi durable state unavailable: {error}; completion is unknown");
                if let Some(session) = &mut state.agent_session
                    && session.message.as_deref() != Some(message.as_str())
                {
                    session.message = Some(message);
                    let snapshot = state.agent_session.clone();
                    notify(&mut state, DaemonMsg::AgentStatus(snapshot));
                }
            }
        }
        let after = observed_facts(&state);
        drop(state);
        if !self.shutting_down.load(Ordering::SeqCst) && facts_changed(&before, &after) {
            publish_facts(self.id, true, after);
        }
    }

    /// Apply an agent hook's report that came over the socket rather than
    /// through the pane's output (`ClientMsg::AgentEvent`), exactly as if the
    /// reader had found it there — once `pid`, the hook, is shown to run
    /// under this pane's shell. The pane id the hook named came from its
    /// environment, and a detached `tmux` server or the like carries that
    /// into processes this pane does not own.
    pub fn report_agent_event(&self, pid: u32, body: &str) -> Result<(), String> {
        let shell = self
            .pty()
            .and_then(|pty| pty.shell_pid)
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
        let before = observed_facts(&st);
        apply_agent_signals(&mut st, vec![event], None);
        let after = observed_facts(&st);
        drop(st);
        if !self.shutting_down.load(Ordering::SeqCst) && facts_changed(&before, &after) {
            publish_facts(self.id, true, after);
        }
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
        let mut out = match self.pty().and_then(|pty| {
            pty.shell_pid
                .map(|pid| crate::daemon::procinfo::snapshot(pid, pty_foreground_pgid(&pty.master)))
        }) {
            Some(procs) => procs,
            None => Default::default(),
        };
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
        crate::daemon::protocol::PaneContext {
            // The cached value only. `remote_context()` falls back to probing
            // the pty, and this is answered on a poll: paying a `/proc` walk
            // per tick for a fact the reader already refreshes on every prompt
            // would put the cost on the wrong side.
            remote: st.remote.clone(),
            local_pty: matches!(self.backend, PaneBackend::Pty(_)),
            at_prompt: st.shell.active.then_some(st.shell.mark_at_prompt),
            remote_prompt_seen: st.remote_prompt_seen,
            bracketed_paste: Some(st.modes.is_on(crate::core::term_modes::BRACKETED_PASTE)),
        }
    }

    fn pty(&self) -> Option<&PtyBackend> {
        match &self.backend {
            PaneBackend::Pty(p) => Some(p),
            PaneBackend::NativeSsh(_) => None,
        }
    }

    /// Resize the shared pane state before applying the new PTY geometry.
    pub fn resize(&self, size: WinSize) {
        resize_state(&mut self.state.lock().unwrap(), size, &self.gate);
        self.resize_pty(size);
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

    fn resize_pty(&self, size: WinSize) {
        match &self.backend {
            PaneBackend::Pty(p) => {
                if let Ok(master) = p.master.lock() {
                    if let Some(master) = master.as_ref() {
                        let _ = master.resize(pty_size(size));
                    }
                }
            }
            PaneBackend::NativeSsh(b) => b.handle.resize(size),
        }
    }

    pub fn alive(&self) -> bool {
        self.state.lock().unwrap().alive
    }

    pub fn info(&self) -> PaneInfo {
        let (cwd, osc_title, alive) = {
            let st = self.state.lock().unwrap();
            (st.cwd.clone(), st.osc_title.clone(), st.alive)
        };
        PaneInfo {
            pane_id: self.id,
            cwd: cwd.or_else(|| self.foreground_cwd()),
            title: self.foreground_title(),
            osc_title,
            alive,
            owner: self.owner.clone(),
        }
    }

    pub(crate) fn remote_context(&self) -> Option<RemoteContext> {
        let cached = self.state.lock().unwrap().remote.clone();
        cached.or_else(|| self.foreground_remote_context())
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
        (segments, st.osc_title.clone(), st.ring.appended_mark())
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
        match &self.backend {
            PaneBackend::Pty(p) => {
                #[cfg(unix)]
                Self::signal_group(p, libc::SIGHUP);
                // The console owns its attached clients; ancestry also includes
                // detached shared services that must outlive this pane.
                #[cfg(windows)]
                {
                    let master = p.master.clone();
                    std::thread::Builder::new()
                        .name("lyrel-daemon-pane-hangup".into())
                        .spawn(move || close_pty_master(&master))
                        .expect("spawn daemon pane hangup thread");
                }
                if let Ok(mut child) = p.child.lock() {
                    let _ = child.kill();
                }
                #[cfg(unix)]
                Self::signal_group(p, libc::SIGKILL);
            }
            PaneBackend::NativeSsh(b) => b.handle.close(),
        }
    }

    #[cfg(windows)]
    fn spawn_exit_monitor(
        shell_pid: Option<u32>,
        master: Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>,
        state: Arc<Mutex<PaneState>>,
        shutting_down: Arc<AtomicBool>,
        death: Arc<DeathReporter>,
    ) {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            INFINITE, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
        };

        let Some(pid) = shell_pid else { return };
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            // The process may have exited before OpenProcess ran, so go
            // straight to the close-and-report path — off this thread, since
            // pane construction is what calls us.
            std::thread::Builder::new()
                .name("lyrel-daemon-pane-exit-fallback".to_string())
                .spawn(move || {
                    drain_then_report(master, state, shutting_down, death, EXIT_DRAIN_WINDOW);
                })
                .expect("spawn daemon pane exit fallback thread");
            return;
        }
        let handle = handle as isize;
        std::thread::Builder::new()
            .name("lyrel-daemon-pane-exit-monitor".to_string())
            .spawn(move || {
                let handle = handle as windows_sys::Win32::Foundation::HANDLE;
                unsafe {
                    WaitForSingleObject(handle, INFINITE);
                    CloseHandle(handle);
                }
                drain_then_report(master, state, shutting_down, death, EXIT_DRAIN_WINDOW);
            })
            .expect("spawn daemon pane exit monitor thread");
    }

    #[cfg(unix)]
    fn signal_group(pty: &PtyBackend, sig: libc::c_int) {
        if let Some(pid) = pty.shell_pid {
            unsafe {
                libc::killpg(pid as libc::pid_t, sig);
            }
        }
        let fg = pty
            .master
            .lock()
            .ok()
            .and_then(|m| m.as_ref().and_then(|m| m.process_group_leader()));
        if let Some(fg) = fg {
            if Some(fg as u32) != pty.shell_pid {
                unsafe {
                    libc::killpg(fg, sig);
                }
            }
        }
    }

    fn foreground_cwd(&self) -> Option<PathBuf> {
        let pty = self.pty()?;
        foreground_cwd(&pty.master, pty.shell_pid)
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn foreground_title(&self) -> String {
        let Some(pty) = self.pty() else {
            return String::new();
        };
        pty.master
            .lock()
            .ok()
            .and_then(|m| m.as_ref().and_then(|m| m.process_group_leader()))
            .and_then(super::procinfo::proc_name)
            .unwrap_or_default()
    }

    #[cfg(windows)]
    fn foreground_title(&self) -> String {
        let Some(pty) = self.pty() else {
            return String::new();
        };
        let Some(pid) = pty.shell_pid else {
            return String::new();
        };
        let procs = crate::daemon::winproc::snapshot();
        crate::daemon::winproc::foreground_name(&procs, pid).unwrap_or_default()
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    fn foreground_title(&self) -> String {
        String::new()
    }

    fn foreground_remote_context(&self) -> Option<RemoteContext> {
        match &self.backend {
            PaneBackend::Pty(p) => foreground_remote_context(&p.master),
            PaneBackend::NativeSsh(_) => None,
        }
    }
}

impl Drop for DaemonPane {
    fn drop(&mut self) {
        if matches!(self.backend, PaneBackend::NativeSsh(_)) {
            crate::daemon::ssh::SshManager::global().teardown_pane_forwards(self.id);
        }
        self.hangup();
        if let PaneBackend::Pty(p) = &self.backend {
            if let Ok(mut child) = p.child.lock() {
                match child.wait() {
                    Ok(status) => log::info!(
                        "pane {} child exited; child_pid={:?} status={status}",
                        self.id,
                        p.shell_pid
                    ),
                    Err(e) => log::warn!("pane {} child wait failed: {e}", self.id),
                }
            }
        }
        if let Some(handle) = self.reader.lock().unwrap().take() {
            join_bounded(handle, Duration::from_secs(2));
        }
        if let PaneBackend::Pty(p) = &mut self.backend {
            if let Some(dir) = p.integration_dir.take() {
                let _ = std::fs::remove_dir_all(&dir);
            }
        }
        // The pane is over — by a kill, by the shell exiting, or by the daemon
        // shutting down — so whatever it typed goes back to the history file it
        // was seeded from. A handoff does not come through here: nothing is
        // dropped across an `exec`, which is exactly right, because the pane on
        // the other side is the same pane and still owns its file.
        crate::daemon::history::retire(self.id);
    }
}

fn join_bounded(handle: JoinHandle<()>, timeout: Duration) -> bool {
    let (tx, rx) = mpsc::channel();
    if std::thread::Builder::new()
        .name("lyrel-daemon-pane-join".to_string())
        .spawn(move || {
            let _ = handle.join();
            let _ = tx.send(());
        })
        .is_err()
    {
        return false;
    }
    rx.recv_timeout(timeout).is_ok()
}

fn pty_size(size: WinSize) -> PtySize {
    PtySize {
        rows: size.rows.max(1),
        cols: size.cols.max(1),
        pixel_width: size.cols.saturating_mul(size.cell_w),
        pixel_height: size.rows.saturating_mul(size.cell_h),
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
            shell: &self.shell,
            remote: &self.remote,
            cwd: &self.cwd,
            ssh_phase: &self.ssh_phase,
            agent: self.agent,
            agent_session: &self.agent_session,
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

fn agent_state_snapshot(st: &PaneState) -> Option<crate::daemon::control::PaneAgentState> {
    st.agent_session
        .clone()
        .map(|state| crate::daemon::control::PaneAgentState {
            pane_id: st.id,
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

fn observed_facts(st: &PaneState) -> ObservedFacts {
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

fn apply_signals(st: &mut PaneState, signals: SniffSignals) {
    if let Some(cwd) = signals.cwd {
        if st.cwd.as_ref() != Some(&cwd) {
            notify(st, DaemonMsg::Cwd(cwd.clone()));
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
            );
        }
        st.shell = shell.clone();
        notify(
            st,
            DaemonMsg::Prompt {
                active: shell.active,
                at_prompt: shell.at_prompt,
                last_exit: shell.last_exit_code,
            },
        );
    }
    apply_agent_signals(st, signals.agent_events, signals.notification);
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
) -> Option<super::kimi::Observer> {
    let id = session.session_id.clone()?;
    let file = session
        .readout
        .session_scope
        .as_ref()?
        .session_file
        .as_ref()?
        .parent()?
        .join("agents/main/wire.jsonl");
    Some(super::kimi::Observer::new(id, file, session.mid_turn()))
}

fn apply_agent_signals(
    st: &mut PaneState,
    events: Vec<crate::core::cli_agent::AgentEvent>,
    notification: Option<String>,
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
                notify(st, DaemonMsg::Agent(None));
            }
            ended_session = true;
            continue;
        }
        if st.agent.is_none() && event.agent.is_some() {
            st.agent = event.agent;
            notify(st, DaemonMsg::Agent(st.agent));
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
        notify(st, DaemonMsg::AgentStatus(st.agent_session.clone()));
    }
}

fn apply_probed_cwd(st: &mut PaneState, probed: Option<PathBuf>) {
    let Some(probed) = probed else {
        return;
    };
    if st.remote.is_some() {
        return;
    }
    if st.cwd.as_deref().is_some_and(|cur| same_dir(cur, &probed)) {
        return;
    }
    notify(st, DaemonMsg::Cwd(probed.clone()));
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

fn apply_remote_context(st: &mut PaneState, remote: Option<RemoteContext>) {
    if st.remote == remote {
        return;
    }
    st.cwd = None;
    // A new far side has to prove its own shell integration. The mark that is
    // standing right now belongs to whatever the pane was before this hop —
    // the near shell's "I started `ssh`", or the previous host's prompt.
    st.remote_prompt_seen = false;
    notify(st, DaemonMsg::RemoteContext(remote.clone()));
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
fn latch_remote_prompt(st: &mut PaneState, saw_prompt_mark: bool) {
    st.remote_prompt_seen |= saw_prompt_mark && st.remote.is_some();
}

fn apply_agent(
    st: &mut PaneState,
    detected: Option<(crate::core::cli_agent::CLIAgent, Vec<String>)>,
) {
    let (agent, argv) = match detected {
        Some((agent, argv)) => (Some(agent), Some(argv)),
        None => (None, None),
    };
    if st.agent == agent {
        stamp_launch_argv(st, argv);
        return;
    }
    // The session belongs to whoever was in the foreground. Switching from
    // Claude to omp (or back to the shell) must not keep the previous id.
    if st.agent_session.is_some() {
        st.agent_session = None;
        notify(st, DaemonMsg::AgentStatus(None));
    }
    if agent.is_none() {
        st.agent_argv = None;
    }
    notify(st, DaemonMsg::Agent(agent));
    st.agent = agent;
    stamp_launch_argv(st, argv);
}

fn stamp_launch_argv(st: &mut PaneState, argv: Option<Vec<String>>) {
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
        notify(st, DaemonMsg::AgentStatus(st.agent_session.clone()));
    }
}

fn foreground_command_running(
    master: &Mutex<Option<Box<dyn MasterPty + Send>>>,
    shell_pid: Option<u32>,
) -> bool {
    is_foreground_command(pty_foreground_pgid(master), shell_pid)
}

#[cfg(unix)]
fn pty_foreground_pgid(master: &Mutex<Option<Box<dyn MasterPty + Send>>>) -> Option<i32> {
    master
        .lock()
        .ok()
        .and_then(|m| m.as_ref().and_then(|m| m.process_group_leader()))
}

#[cfg(not(unix))]
fn pty_foreground_pgid(_master: &Mutex<Option<Box<dyn MasterPty + Send>>>) -> Option<i32> {
    None
}

fn is_foreground_command(fg_pgid: Option<i32>, shell_pid: Option<u32>) -> bool {
    match (fg_pgid, shell_pid) {
        (Some(pg), Some(shell)) if pg > 0 => pg as u32 != shell,
        _ => false,
    }
}

#[cfg(target_os = "macos")]
fn foreground_cwd(
    master: &Mutex<Option<Box<dyn MasterPty + Send>>>,
    shell_pid: Option<u32>,
) -> Option<PathBuf> {
    use std::ffi::CStr;

    let read_cwd = |pid: i32| -> Option<PathBuf> {
        if pid <= 0 {
            return None;
        }
        let mut vinfo: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
        let ret = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDVNODEPATHINFO,
                0,
                &mut vinfo as *mut _ as *mut libc::c_void,
                size,
            )
        };
        if ret != size {
            return None;
        }
        let s = unsafe { CStr::from_ptr(vinfo.pvi_cdir.vip_path.as_ptr() as *const libc::c_char) }
            .to_str()
            .ok()?;
        if s.is_empty() {
            None
        } else {
            Some(PathBuf::from(s))
        }
    };

    pty_foreground_pgid(master)
        .and_then(read_cwd)
        .or_else(|| read_cwd(shell_pid.map(|p| p as i32).unwrap_or(0)))
}

#[cfg(target_os = "linux")]
fn foreground_cwd(
    master: &Mutex<Option<Box<dyn MasterPty + Send>>>,
    shell_pid: Option<u32>,
) -> Option<PathBuf> {
    let read_cwd = |pid: i32| -> Option<PathBuf> {
        if pid <= 0 {
            return None;
        }
        let cwd = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
        cwd.is_dir().then_some(cwd)
    };
    pty_foreground_pgid(master)
        .and_then(read_cwd)
        .or_else(|| read_cwd(shell_pid.map(|p| p as i32).unwrap_or(0)))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn foreground_cwd(
    _master: &Mutex<Option<Box<dyn MasterPty + Send>>>,
    _shell_pid: Option<u32>,
) -> Option<PathBuf> {
    None
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn foreground_remote_context(
    master: &Mutex<Option<Box<dyn MasterPty + Send>>>,
) -> Option<RemoteContext> {
    let pid = master
        .lock()
        .ok()
        .and_then(|m| m.as_ref().and_then(|m| m.process_group_leader()))?;
    let argv = crate::daemon::remote::foreground_argv(pid)?;
    crate::daemon::remote::parse_ssh_invocation(&argv).map(|inv| inv.context)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn foreground_remote_context(
    _master: &Mutex<Option<Box<dyn MasterPty + Send>>>,
) -> Option<RemoteContext> {
    None
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn foreground_agent(
    master: &Mutex<Option<Box<dyn MasterPty + Send>>>,
) -> Option<Option<(crate::core::cli_agent::CLIAgent, Vec<String>)>> {
    let detect = || {
        let pid = master
            .lock()
            .ok()
            .and_then(|m| m.as_ref().and_then(|m| m.process_group_leader()))?;
        let argv = crate::daemon::remote::foreground_argv(pid)?;
        let agent = crate::core::cli_agent::CLIAgent::detect_from_argv_with(
            &argv,
            &crate::core::config::agent_detection_aliases(),
        )?;
        Some((agent, argv))
    };
    Some(detect())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn foreground_agent(
    _master: &Mutex<Option<Box<dyn MasterPty + Send>>>,
) -> Option<Option<(crate::core::cli_agent::CLIAgent, Vec<String>)>> {
    None
}

#[derive(Default, Clone, PartialEq, Eq)]
struct ShellState {
    active: bool,
    at_prompt: bool,
    /// `at_prompt` as the marks themselves read it, kept out of the
    /// suppression the reader applies when a foreground program is running.
    ///
    /// Two different questions share one pair of marks. "Should the local line
    /// editor engage?" must say no while `ssh` owns the pty, whoever drew the
    /// prompt — that is `at_prompt`. "Is anything running in this pane?" wants
    /// the opposite: the prompt an `ssh` is relaying belongs to the shell that
    /// is actually driving the pane, and it is the only thing this side can
    /// read about the far one. Keeping both means neither answer has to be
    /// derived from the other's.
    mark_at_prompt: bool,
    last_exit_code: Option<i32>,
    command: Option<String>,
}

#[derive(Default)]
struct SniffSignals {
    cwd: Option<PathBuf>,
    /// The last title the pane set in this read, already capped. `Some("")` is
    /// a reset — an empty OSC 0/2 clears the title rather than setting a blank
    /// one, the same way the GUI's terminal treats it.
    title: Option<String>,
    shell: Vec<ShellState>,
    agent_events: Vec<crate::core::cli_agent::AgentEvent>,
    notification: Option<String>,
}

struct OscSniffer {
    tok: OscTokenizer,
    shell: ShellState,
    /// Whether the title the pane is showing still belongs to something that
    /// is running — see [`crate::core::osc::TitleLifetime`] (#889).
    title_life: crate::core::osc::TitleLifetime,
    notes: crate::core::osc::Notifications,
}

impl OscSniffer {
    fn new() -> Self {
        Self {
            tok: OscTokenizer::new(&[b"0", b"2", b"7", b"133", b"9", b"99", b"777"]),
            shell: ShellState::default(),
            title_life: crate::core::osc::TitleLifetime::default(),
            notes: crate::core::osc::Notifications::default(),
        }
    }

    fn feed(&mut self, bytes: &[u8]) -> SniffSignals {
        let mut signals = SniffSignals::default();
        let shell = &mut self.shell;
        let title_life = &mut self.title_life;
        let notes = &mut self.notes;
        self.tok.feed(bytes, |payload| {
            // In stream order, so that a shell which re-titles itself right
            // after the `D` mark gets the last word over the retirement.
            if title_life.saw(payload) == crate::core::osc::TitleEffect::Retire {
                signals.title = Some(String::new());
            }
            if let Some(path) = parse_osc7(payload) {
                signals.cwd = Some(path);
            } else if let Some(title) = parse_osc_title(payload) {
                signals.title = Some(title);
            } else if let Some(rest) = payload.strip_prefix(b"133;") {
                if handle_osc133(shell, rest) {
                    match signals.shell.last_mut() {
                        Some(last) if last.at_prompt == shell.at_prompt => {
                            *last = shell.clone();
                        }
                        _ => signals.shell.push(shell.clone()),
                    }
                }
            } else if let Some(event) = crate::core::cli_agent::parse_agent_event(payload) {
                signals.agent_events.push(event);
            } else if let Some((title, body)) = notes.parse(payload) {
                if title.as_deref() != Some(crate::core::cli_agent::AGENT_EVENT_SENTINEL) {
                    signals.notification = Some(body);
                }
            }
        });
        signals
    }
}

fn handle_osc133(shell: &mut ShellState, rest: &[u8]) -> bool {
    // Generic OSC 133 blocks may be emitted by the foreground TUI, including
    // when replaying a transcript. Only our shell integration owns its argv.
    let command_mark = rest.strip_suffix(b";aid=lyrel");
    let rest = command_mark.unwrap_or(rest);
    shell.active = true;
    match rest.first() {
        Some(b'A') | Some(b'B') => shell.at_prompt = true,
        Some(b'C') => {
            shell.at_prompt = false;
            if command_mark.is_some() {
                shell.command = rest
                    .strip_prefix(b"C;")
                    .map(|c| String::from_utf8_lossy(&percent_decode(c)).into_owned())
                    .filter(|s| !s.trim().is_empty());
            }
        }
        Some(b'D') => {
            shell.at_prompt = true;
            if command_mark.is_some() {
                shell.command = None;
            }
            shell.last_exit_code = rest
                .strip_prefix(b"D;")
                .and_then(|c| std::str::from_utf8(c).ok())
                .and_then(|s| s.trim().parse::<i32>().ok());
        }
        _ => return false,
    }
    // The unsuppressed twin, set here so every arm gets it and no future arm
    // can forget to. Suppression happens later, on the reader thread, and only
    // ever touches `at_prompt`.
    shell.mark_at_prompt = shell.at_prompt;
    true
}

#[cfg(unix)]
fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
}

#[cfg(not(unix))]
fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    let s = String::from_utf8_lossy(bytes);
    PathBuf::from(strip_uri_drive_slash(s.as_ref()))
}

#[cfg_attr(unix, allow(dead_code))]
fn strip_uri_drive_slash(path: &str) -> &str {
    let b = path.as_bytes();
    if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        &path[1..]
    } else {
        path
    }
}

pub(crate) fn parse_osc7(payload: &[u8]) -> Option<PathBuf> {
    let rest = payload.strip_prefix(b"7;")?;
    let path_bytes: &[u8] = if let Some(after) = rest.strip_prefix(b"file://") {
        let idx = after.iter().position(|&c| c == b'/')?;
        &after[idx..]
    } else if rest.first() == Some(&b'/') {
        rest
    } else {
        return None;
    };
    let decoded = percent_decode(path_bytes);
    if decoded.is_empty() {
        return None;
    }
    Some(path_from_bytes(&decoded))
}

/// Longest title the tree will keep. The window that owns the pane shows about
/// forty columns of it; the rest is only ever paid for — in `machine.json`, and
/// in a `PaneFacts` delta to every attached client — so it is cut here rather
/// than at each place that renders one.
const MAX_OSC_TITLE: usize = 256;

/// The title from an OSC 0 (icon *and* window) or OSC 2 (window) payload,
/// capped. An empty payload comes back as `Some("")`: that is a reset, which
/// the caller has to tell apart from "no title in this read".
///
/// OSC 1 is deliberately not read. It sets the icon name alone, which the GUI's
/// terminal ignores — and the point of recording a title at all is to agree
/// with what the GUI shows.
pub(crate) fn parse_osc_title(payload: &[u8]) -> Option<String> {
    let rest = payload
        .strip_prefix(b"0;")
        .or_else(|| payload.strip_prefix(b"2;"))?;
    let title = String::from_utf8_lossy(rest);
    let title = title.trim();
    Some(match title.chars().count() > MAX_OSC_TITLE {
        true => title.chars().take(MAX_OSC_TITLE).collect(),
        false => title.to_string(),
    })
}

fn percent_decode(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'%' && i + 2 < input.len() {
            if let (Some(h), Some(l)) = (hex_val(input[i + 1]), hex_val(input[i + 2])) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(input[i]);
        i += 1;
    }
    out
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use super::test_support::{drain, null_writer, test_state, ws};
    use std::path::Path;

    #[test]
    fn a_hook_report_counts_only_from_inside_the_pane() {
        let (tx, rx) = mpsc::channel();
        let pane = DaemonPane::spawn(
            1,
            Some(PathBuf::from("/")),
            ws(80, 24),
            Some(ShellSpec {
                program: "sh".into(),
                args: vec!["-c".into(), "exec cat".into()],
                args_are_lyrel_defaults: false,
            }),
            None,
            None,
            None,
            false,
            || {},
        )
        .expect("spawn pane");
        pane.attach(tx);
        let shell = pane.pty().and_then(|p| p.shell_pid).expect("shell pid");
        let body = r#"{"v":1,"agent":"omp","event":"prompt-submit","session_id":"s-1"}"#;

        let outsider = pane.report_agent_event(std::process::id(), body);
        let garbled = pane.report_agent_event(shell, "{not json");
        let applied = pane.report_agent_event(shell, body);
        let mut status = None;
        while let Ok(msg) = rx.recv_timeout(std::time::Duration::from_millis(200)) {
            if let DaemonMsg::AgentStatus(s) = msg {
                status = s;
            }
        }
        pane.kill();

        assert!(
            outsider.is_err(),
            "the test runner does not run in the pane"
        );
        assert!(garbled.is_err());
        assert_eq!(applied, Ok(()));
        let status = status.expect("the report reached the pane's subscribers");
        assert_eq!(status.status, crate::core::cli_agent::AgentStatus::Working);
        assert_eq!(status.session_id.as_deref(), Some("s-1"));
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn live_pane_reports_an_uninstrumented_shells_cwd() {
        let target = std::path::Path::new("/usr");

        let (tx, rx) = mpsc::channel();
        let pane = DaemonPane::spawn(
            1,
            Some(PathBuf::from("/")),
            ws(80, 24),
            Some(ShellSpec {
                program: "sh".into(),
                args: vec!["-c".into(), "cd /usr && exec cat".into()],
                args_are_lyrel_defaults: false,
            }),
            None,
            None,
            None,
            false,
            || {},
        )
        .expect("spawn pane");
        pane.attach(tx);

        let mut reported = None;
        for _ in 0..200 {
            pane.write_input(b"\n");
            while let Ok(msg) = rx.try_recv() {
                if let DaemonMsg::Cwd(p) = msg {
                    reported = Some(p);
                }
            }
            if reported.as_deref() == Some(target) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        pane.kill();

        assert_eq!(
            reported.as_deref(),
            Some(target),
            "a pane whose shell emits no OSC 7 must still report where it \
             actually is — this is what a new tab inherits (issue #187)"
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn live_pty_child_argv_detects_the_agent() {
        use portable_pty::{CommandBuilder, PtySize, native_pty_system};

        let pty = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let mut cmd = CommandBuilder::new("bash");
        cmd.args(["-c", "exec -a pi cat"]);
        let mut child = pty.slave.spawn_command(cmd).expect("spawn child");
        let master = Mutex::new(Some(pty.master));

        let mut detected = None;
        for _ in 0..200 {
            if let Some(agent) = foreground_agent(&master).flatten() {
                detected = Some(agent);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let _ = child.kill();
        let _ = child.wait();

        let (agent, argv) = detected.expect("agent detected from live PTY child");
        assert_eq!(
            agent,
            crate::core::cli_agent::CLIAgent::Pi,
            "a live PTY child with argv[0]=pi must be detected as Pi"
        );
        assert_eq!(
            argv.first().map(String::as_str),
            Some("pi"),
            "the observed argv rides along with the detection"
        );
    }

    #[test]
    fn join_bounded_returns_true_when_the_thread_finishes() {
        let handle = std::thread::spawn(|| {});
        assert!(join_bounded(handle, Duration::from_secs(5)));
    }

    #[test]
    fn join_bounded_times_out_on_a_stuck_thread() {
        let (unblock, blocked) = mpsc::channel::<()>();
        let handle = std::thread::spawn(move || {
            let _ = blocked.recv();
        });
        assert!(!join_bounded(handle, Duration::from_millis(50)));
        drop(unblock);
    }



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

    #[cfg(unix)]
    #[test]
    fn a_descriptor_that_is_not_a_terminal_is_refused_rather_than_adopted() {
        use std::os::fd::AsRawFd as _;

        let file = std::fs::File::open("/dev/null").expect("open /dev/null");
        let err = AdoptedMaster::from_fd(file.as_raw_fd())
            .err()
            .expect("a plain file is not a pty");
        assert!(
            err.to_string().contains("not a terminal"),
            "the refusal was {err}"
        );
        // Refused means not taken: the caller still owns what it passed, and
        // dropping `file` here is what closes it. Adopting first and failing
        // later would have closed a descriptor belonging to someone else.
        drop(file);
    }

    #[cfg(unix)]
    #[test]
    fn an_adopted_master_goes_back_to_close_on_exec() {
        let pair = native_pty_system()
            .openpty(PtySize::default())
            .expect("open a pty");
        let tty = pair
            .master
            .tty_name()
            .expect("a fresh pty has a slave name");
        let fd = pair
            .master
            .as_raw_fd()
            .expect("a fresh master has a descriptor");
        // What crossing the exec leaves behind: `dup` hands out a descriptor
        // with close-on-exec clear, exactly like one that was deliberately
        // stripped to survive.
        let inherited = unsafe { libc::dup(fd) };
        assert!(inherited >= 0, "dup the master");
        let master = AdoptedMaster::from_fd(inherited).expect("a pty is adoptable");
        assert_eq!(master.tty_name().as_deref(), Some(tty.as_path()));
        let flags = unsafe { libc::fcntl(inherited, libc::F_GETFD) };
        assert!(
            flags >= 0 && flags & libc::FD_CLOEXEC != 0,
            "an adopted master left inheritable ends up in every child this daemon spawns, \
             and a child holding it keeps the pty from ever hanging up"
        );
        drop(master);
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
    fn sniff_osc7_cwd() {
        let mut s = OscSniffer::new();
        let sig = s.feed(b"\x1b]7;file://host/Users/me/dev\x07");
        assert_eq!(sig.cwd, Some(PathBuf::from("/Users/me/dev")));
    }

    /// The tree's only source of the name a window actually shows on a tab. A
    /// tokenizer that does not list 0 and 2 sniffs nothing here, and the
    /// switcher goes back to calling every agent tab "Claude Code".
    #[test]
    fn sniff_osc_title() {
        let mut s = OscSniffer::new();
        assert_eq!(
            s.feed(b"\x1b]0;user@host:~/dev\x07").title.as_deref(),
            Some("user@host:~/dev")
        );
        assert_eq!(
            s.feed(b"\x1b]2;\xe2\x9c\xb3 fixing the switcher\x1b\\")
                .title
                .as_deref(),
            Some("✳ fixing the switcher")
        );
        // The last one in a read is the one that stuck.
        assert_eq!(
            s.feed(b"\x1b]0;first\x07\x1b]0;second\x07")
                .title
                .as_deref(),
            Some("second")
        );
        // An empty title is a reset, and has to be told apart from a read with
        // no title in it at all.
        assert_eq!(s.feed(b"\x1b]2;\x07").title.as_deref(), Some(""));
        assert_eq!(s.feed(b"plain output\r\n").title, None);
        // OSC 1 names the icon, which the window ignores; so do we.
        assert_eq!(s.feed(b"\x1b]1;icon\x07").title, None);

        let long = format!("\x1b]0;{}\x07", "t".repeat(MAX_OSC_TITLE + 10));
        assert_eq!(
            s.feed(long.as_bytes()).title.map(|t| t.chars().count()),
            Some(MAX_OSC_TITLE)
        );
    }

    /// Titles arrive interleaved with everything else in the same read, and the
    /// tokenizer has one identifier filter for the lot: sniffing a title must
    /// not cost a cwd or a prompt mark.
    #[test]
    fn a_title_does_not_swallow_the_other_marks_in_the_same_read() {
        let mut s = OscSniffer::new();
        let sig = s.feed(b"\x1b]0;user@host:~/dev\x07\x1b]7;file://host/dev\x07\x1b]133;A\x07");
        assert_eq!(sig.title.as_deref(), Some("user@host:~/dev"));
        assert_eq!(sig.cwd, Some(PathBuf::from("/dev")));
        assert!(sig.shell.last().unwrap().at_prompt);
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

    /// The client forgets its cwd on every `RemoteContext`, so a replay that
    /// sent the far shell's directory first would have it wiped straight away.
    #[test]
    fn a_window_reattaching_to_an_ssh_pane_keeps_its_remote_directory() {
        let mut st = test_state(true);
        st.remote = Some(RemoteContext {
            kind: RemoteKind::NativeSsh,
            argv: Vec::new(),
            target: "alice@box".into(),
        });
        st.cwd = Some(PathBuf::from("/home/alice/my_service"));
        let (tx, rx) = std::sync::mpsc::channel();
        attach_subscriber(&mut st, tx);
        st.subscriptions.detach(1, &OutputGate::new(), &mut st.graphics);
        let order: Vec<&str> = rx
            .iter()
            .filter_map(|m| match m {
                DaemonMsg::RemoteContext(_) => Some("remote"),
                DaemonMsg::Cwd(_) => Some("cwd"),
                _ => None,
            })
            .collect();
        assert_eq!(order, ["remote", "cwd"]);
    }

    #[test]
    fn a_title_is_kept_until_it_changes_and_a_reset_clears_it() {
        let mut st = test_state(true);
        apply_signals(
            &mut st,
            SniffSignals {
                title: Some("✳ fixing the switcher".into()),
                ..SniffSignals::default()
            },
        );
        assert_eq!(st.osc_title.as_deref(), Some("✳ fixing the switcher"));

        // A read with no title in it leaves the stored one alone.
        apply_signals(&mut st, SniffSignals::default());
        assert_eq!(st.osc_title.as_deref(), Some("✳ fixing the switcher"));

        apply_signals(
            &mut st,
            SniffSignals {
                title: Some(String::new()),
                ..SniffSignals::default()
            },
        );
        assert_eq!(st.osc_title, None, "an empty title clears, not blanks");
    }

    /// #889: the tab went on reading "✳ fixing the switcher" long after Claude
    /// Code had exited and the pane was back at its own prompt, because
    /// nothing in the pane path ever retired an OSC 0/2 — only another one
    /// replaced it. The `D` mark says the command that wrote it is over.
    #[test]
    fn a_title_a_command_set_is_retired_when_that_command_finishes() {
        let mut st = test_state(true);
        let mut s = OscSniffer::new();

        let mut read = |st: &mut PaneState, bytes: &[u8]| apply_signals(st, s.feed(bytes));

        read(
            &mut st,
            b"\x1b]7;file://h/work/lyrel\x07\x1b]133;A\x07\x1b]133;B\x07",
        );
        read(&mut st, b"\x1b]133;C;claude\x07");
        read(&mut st, b"\x1b]2;\xe2\x9c\xb3 fixing the switcher\x1b\\");
        assert_eq!(
            st.osc_title.as_deref(),
            Some("✳ fixing the switcher"),
            "a running command names the tab"
        );

        // The user's precmd chain can take hundreds of ms, which is why the
        // `D` emitter is prepended to it — so the mark routinely lands in a
        // read of its own, ahead of anything the next prompt writes.
        read(&mut st, b"\x1b]133;D;0\x07");
        assert_eq!(
            st.osc_title, None,
            "the program that wrote the title has exited"
        );
        assert_eq!(
            st.cwd.as_deref(),
            Some(std::path::Path::new("/work/lyrel")),
            "and the rung below it in the label ladder still knows the place"
        );
    }

    /// The two cases the retirement must not touch: a program that is still
    /// running, and a shell that titles its own prompt.
    #[test]
    fn a_running_program_and_a_shells_own_prompt_title_both_keep_theirs() {
        let mut st = test_state(true);
        let mut s = OscSniffer::new();

        apply_signals(&mut st, s.feed(b"\x1b]133;C;vim\x07\x1b]2;vim\x1b\\"));
        assert_eq!(
            st.osc_title.as_deref(),
            Some("vim"),
            "nothing said the command ended"
        );

        // A shell that re-titles itself does it between the `D` and the `A`
        // (lyrel's zsh helper prepends the `D`; the PowerShell one titles in
        // its prompt function), so the title is the last word in the read.
        apply_signals(
            &mut st,
            s.feed(b"\x1b]133;D;0\x07\x1b]0;me@box:~/dev\x07\x1b]133;A\x07"),
        );
        assert_eq!(
            st.osc_title.as_deref(),
            Some("me@box:~/dev"),
            "the prompt's own title outranks the retirement it follows"
        );

        // And that prompt title is nobody's command to retire.
        apply_signals(&mut st, s.feed(b"\x1b]133;C;ls\x07\x1b]133;D;0\x07"));
        assert_eq!(st.osc_title.as_deref(), Some("me@box:~/dev"));
    }

    /// A pane with no shell integration never sees a mark, so nothing changes
    /// for it: whatever it last called itself is all lyrel has.
    #[test]
    fn a_pane_without_shell_integration_keeps_its_title() {
        let mut st = test_state(true);
        let mut s = OscSniffer::new();
        apply_signals(&mut st, s.feed(b"\x1b]0;user@host:~/dev\x07"));
        apply_signals(&mut st, s.feed(b"lots of output\r\n"));
        assert_eq!(st.osc_title.as_deref(), Some("user@host:~/dev"));
    }

    #[test]
    fn sniff_osc133_prompt() {
        let mut s = OscSniffer::new();
        let b = s.feed(b"\x1b]133;B\x07");
        assert!(b.shell.last().unwrap().active);
        assert!(b.shell.last().unwrap().at_prompt);

        let c = s.feed(b"\x1b]133;C\x07");
        assert!(!c.shell.last().unwrap().at_prompt);

        let d = s.feed(b"\x1b]133;D;130\x07");
        assert!(d.shell.last().unwrap().at_prompt);
        assert_eq!(d.shell.last().unwrap().last_exit_code, Some(130));
    }

    #[test]
    fn a_full_command_cycle_in_one_chunk_still_reports_leaving_the_prompt() {
        let mut s = OscSniffer::new();
        s.feed(b"\x1b]133;A\x07\x1b]133;B\x07");

        let sig =
            s.feed(b"\x1b]133;C;echo%20hi\x07hi\r\n\x1b]133;D;0\x07\x1b]133;A\x07\x1b]133;B\x07");
        let states: Vec<bool> = sig.shell.iter().map(|s| s.at_prompt).collect();
        assert_eq!(
            states,
            vec![false, true],
            "the chunk must report leaving the prompt and coming back, not just the end state"
        );
        assert_eq!(sig.shell.last().unwrap().last_exit_code, Some(0));
        assert_eq!(sig.shell.last().unwrap().command, None);
    }

    #[test]
    fn marks_on_the_same_side_of_the_prompt_boundary_fold_into_one_state() {
        let mut s = OscSniffer::new();
        let sig = s.feed(b"\x1b]133;D;3\x07\x1b]133;A\x07\x1b]133;B\x07");
        assert_eq!(sig.shell.len(), 1, "D/A/B is one at-prompt state");
        assert!(sig.shell[0].at_prompt);
        assert_eq!(sig.shell[0].last_exit_code, Some(3));
    }

    #[cfg(windows)]
    #[test]
    fn resumed_output_marks_do_not_release_the_running_agent() {
        use crate::core::cli_agent::CLIAgent;

        let mut sniffer = OscSniffer::new();
        let mut state = test_state(true);
        apply_signals(&mut state, sniffer.feed(b"\x1b]133;C;omp;aid=lyrel\x07"));
        assert_eq!(state.agent, Some(CLIAgent::OhMyPi));

        // Minimized from a real omp /resume: replayed output contains its own
        // prompt/output block, but the enclosing shell command has not exited.
        apply_signals(
            &mut state,
            sniffer.feed(b"\x1b]133;A\x07\x1b]133;B\x07\x1b]133;C\x07\x1b]133;D;0\x07"),
        );
        assert_eq!(state.agent, Some(CLIAgent::OhMyPi));
        assert_eq!(state.shell.command.as_deref(), Some("omp"));
        apply_signals(&mut state, sniffer.feed(b"\x1b]133;D;0;aid=lyrel\x07"));
        assert!(
            state.agent.is_none(),
            "the actual shell exit releases the agent"
        );
    }

    #[test]
    fn sniff_osc133_command_capture_drives_agent_detection() {
        let custom = std::collections::HashMap::new();
        let mut s = OscSniffer::new();

        let c = s.feed(b"\x1b]133;C;omp%20--help;aid=lyrel\x07");
        let shell = c.shell.last().unwrap();
        assert!(!shell.at_prompt);
        assert_eq!(shell.command.as_deref(), Some("omp --help"));
        assert_eq!(
            agent_from_shell_mark(shell, &custom),
            Some((
                crate::core::cli_agent::CLIAgent::OhMyPi,
                vec!["omp".to_string(), "--help".to_string()],
            ))
        );

        let d = s.feed(b"\x1b]133;D;0;aid=lyrel\x07");
        let shell = d.shell.last().unwrap();
        assert_eq!(shell.command, None);
        assert_eq!(agent_from_shell_mark(shell, &custom), None);

        let c = s.feed(b"\x1b]133;C;git%20status;aid=lyrel\x07");
        let shell = c.shell.last().unwrap();
        assert_eq!(shell.command.as_deref(), Some("git status"));
        assert_eq!(agent_from_shell_mark(shell, &custom), None);

        let c = s.feed(b"\x1b]133;C;;aid=lyrel\x07");
        assert_eq!(c.shell.last().unwrap().command, None);

        let _ = s.feed(b"\x1b]133;C;pi;aid=lyrel\x07");
        let a = s.feed(b"\x1b]133;A\x1b]133;B\x07");
        assert_eq!(a.shell.last().unwrap().command.as_deref(), Some("pi"));
        let d = s.feed(b"\x1b]133;D;0;aid=lyrel\x07");
        assert_eq!(d.shell.last().unwrap().command, None);

        let c = s.feed(b"\x1b]133;C;echo%20a%0Aecho%20b;aid=lyrel\x07");
        assert_eq!(
            c.shell.last().unwrap().command.as_deref(),
            Some("echo a\necho b")
        );

        let c = s.feed(b"\x1b]133;C;%20%20;aid=lyrel\x07");
        assert_eq!(c.shell.last().unwrap().command, None);
    }

    #[test]
    fn sniff_osc133_stray_marks_do_not_reapply_mark_detection() {
        let mut s = OscSniffer::new();

        let mut prev = ShellState::default();
        let c = s
            .feed(b"\x1b]133;C;.%5Cdev.ps1;aid=lyrel\x07")
            .shell
            .pop()
            .unwrap();
        assert!(shell_mark_capture_changed(&prev, &c));
        assert_eq!(
            agent_from_shell_mark(&c, &std::collections::HashMap::new()),
            None
        );
        prev = c;

        let ab = s.feed(b"\x1b]133;A\x1b]133;B\x07").shell.pop().unwrap();
        assert!(!shell_mark_capture_changed(&prev, &ab));
        prev = ab;

        let d = s.feed(b"\x1b]133;D;0;aid=lyrel\x07").shell.pop().unwrap();
        assert!(shell_mark_capture_changed(&prev, &d));
    }

    #[test]
    fn sniff_osc133_edit_mode_does_not_emit_prompt_state() {
        let mut s = OscSniffer::new();
        let sig = s.feed(b"\x1b]133;V;1\x07");
        assert!(
            sig.shell.is_empty(),
            "edit-mode metadata must not bump prompt state or prompt sequence"
        );

        let b = s.feed(b"\x1b]133;B\x07");
        assert!(b.shell.last().unwrap().active);
        assert!(b.shell.last().unwrap().at_prompt);
    }

    #[test]
    fn foreground_command_distinguishes_the_shell_from_a_command() {
        assert!(!is_foreground_command(Some(1000), Some(1000)));
        assert!(is_foreground_command(Some(2000), Some(1000)));
        assert!(!is_foreground_command(None, Some(1000)));
        assert!(!is_foreground_command(Some(2000), None));
        assert!(!is_foreground_command(Some(0), Some(1000)));
    }

    #[test]
    fn foreground_program_prompt_marks_do_not_claim_the_prompt() {
        let mut s = OscSniffer::new();
        let mut signals = s.feed(b"\x1b]133;A\x1b]133;B\x07");
        assert!(
            signals.shell.last().unwrap().at_prompt,
            "the raw marks read as at-prompt"
        );

        let ssh_running = is_foreground_command(Some(2000), Some(1000));
        if signals.shell.iter().any(|st| st.at_prompt) && ssh_running {
            suppress_relayed_prompt_marks(&mut signals.shell);
        }
        assert!(
            !signals.shell.last().unwrap().at_prompt,
            "a foreground program's prompt marks must not engage the local editor"
        );
        assert!(
            signals.shell.last().unwrap().mark_at_prompt,
            "the mark's own reading survives: over `ssh` it is the far shell speaking, and \
             the only thing this side knows about whether it is busy (#840)"
        );

        let mut local = s.feed(b"\x1b]133;A\x1b]133;B\x07");
        let shell_idle = is_foreground_command(Some(1000), Some(1000));
        if local.shell.iter().any(|st| st.at_prompt) && shell_idle {
            suppress_relayed_prompt_marks(&mut local.shell);
        }
        assert!(local.shell.last().unwrap().at_prompt);
    }

    /// The collapse that suppression performs must not hand the pane back a
    /// prompt it has already left. A far shell's whole turn can arrive in one
    /// read — the previous command's `D`, the prompt's `A`/`B`, then the next
    /// command's `C` — and once `at_prompt` is cleared across the batch those
    /// two entries differ only in the mark's own reading. The collapse keeps
    /// the earlier entry, so that reading has to travel with it or freeness
    /// answers `free` while a remote command is running (#840).
    #[test]
    fn suppression_collapses_a_batch_onto_its_newest_marks_reading() {
        let mut s = OscSniffer::new();
        // Bare `133;C`: nushell's integration emits it with no command name, as
        // do several third-party ones, so `command` cannot tell the two entries
        // apart either.
        let mut turn = s.feed(b"\x1b]133;D;0\x07\x1b]133;A\x07\x1b]133;B\x07\x1b]133;C\x07");
        suppress_relayed_prompt_marks(&mut turn.shell);
        assert_eq!(
            turn.shell.len(),
            1,
            "still one `Prompt` message, exactly as before the mark was added"
        );
        assert!(
            !turn.shell.last().unwrap().mark_at_prompt,
            "the newest mark started a command, so the pane is not at a prompt"
        );

        // And the other order: a command that starts and finishes inside one
        // read has to end at the prompt, not at the `C` that opened it.
        let mut turn = s.feed(b"\x1b]133;C\x07\x1b]133;D;0\x07\x1b]133;A\x07");
        suppress_relayed_prompt_marks(&mut turn.shell);
        assert_eq!(turn.shell.len(), 1);
        assert!(
            turn.shell.last().unwrap().mark_at_prompt,
            "the newest mark is the prompt the far shell just drew"
        );
    }

    /// A prompt mark that arrives while the pane is pointed at a remote host
    /// can only be the far shell's — the near one cannot be at a prompt while
    /// the connection owns its pty. That is what proves the far side runs the
    /// shell integration, and it is the difference between "the remote is
    /// busy" and "we cannot tell" (#840).
    #[test]
    fn a_remote_panes_prompt_mark_latches_and_a_new_hop_clears_it() {
        let mark = |mark_at_prompt: bool, command: Option<&str>| ShellState {
            active: true,
            at_prompt: false,
            mark_at_prompt,
            last_exit_code: None,
            command: command.map(str::to_string),
        };
        let hop = |target: &str| RemoteContext {
            kind: RemoteKind::Ssh,
            argv: vec!["ssh".into(), target.into()],
            target: target.into(),
        };
        /// One pass of the reader's ordering: signals, then the hop the same
        /// read detected, then the latch.
        fn read(st: &mut PaneState, shell: Vec<ShellState>, remote: Option<Option<RemoteContext>>) {
            let saw_prompt_mark = shell.iter().any(|s| s.mark_at_prompt);
            apply_signals(
                st,
                SniffSignals {
                    shell,
                    ..SniffSignals::default()
                },
            );
            if let Some(remote) = remote {
                apply_remote_context(st, remote);
            }
            latch_remote_prompt(st, saw_prompt_mark);
        }

        // Local: the same mark proves nothing about any far side.
        let mut st = test_state(true);
        read(&mut st, vec![mark(true, None)], None);
        assert!(!st.remote_prompt_seen);

        // The near shell's own "I started `ssh`" — no prompt in it — must not
        // latch, even though the hop lands in the same read.
        read(
            &mut st,
            vec![mark(false, Some("ssh build-box"))],
            Some(Some(hop("build-box"))),
        );
        assert!(
            !st.remote_prompt_seen,
            "starting the connection is not evidence about what is behind it"
        );

        // The far shell's opening prompt does — including when it arrives in
        // the very read that first sees the pane as remote, which on a unix
        // host is the common case: the relayed prompt is suppressed, so it
        // cannot trigger the immediate re-probe that would have split the two.
        let mut fresh = test_state(true);
        read(
            &mut fresh,
            vec![mark(true, None)],
            Some(Some(hop("build-box"))),
        );
        assert!(fresh.remote_prompt_seen);
        read(&mut st, vec![mark(true, None)], None);
        assert!(st.remote_prompt_seen);

        // A second hop has to prove itself over again.
        read(&mut st, Vec::new(), Some(Some(hop("other-box"))));
        assert!(!st.remote_prompt_seen);

        // And coming back to the local shell clears it too.
        read(&mut st, vec![mark(true, None)], None);
        assert!(st.remote_prompt_seen);
        read(&mut st, Vec::new(), Some(None));
        assert!(!st.remote_prompt_seen);
    }

    #[test]
    fn sniff_resyncs_on_new_osc_after_an_unterminated_one() {
        let mut s = OscSniffer::new();
        let sig = s.feed(b"\x1b]133;A\x1b]133;B\x07");
        assert!(
            sig.shell.last().is_some_and(|sh| sh.at_prompt),
            "OSC 133;B after an unterminated 133;A was dropped (no resync on `]`)"
        );

        let mut s = OscSniffer::new();
        let sig = s.feed(b"\x1b]7;file://host/dropped\x1b]7;file://host/kept\x07");
        assert_eq!(sig.cwd, Some(PathBuf::from("/kept")));
    }

    #[test]
    fn at_prompt_covers_prompt_draw_gap_across_chunks() {
        let mut s = OscSniffer::new();

        assert!(!s.feed(b"\x1b]133;C\x07").shell.last().unwrap().at_prompt);

        let d = s.feed(b"\x1b]133;D;0\x07");
        assert!(
            d.shell.last().unwrap().at_prompt,
            "D should mark us back at the prompt before the prompt text is drawn"
        );

        let chunk = s.feed(
            b"\x1b]133;A\x07\x1b]7;file://host/repo/lyrel\x07\r\nlyrel git:(main) \xe2\x9e\x9c ",
        );
        assert!(
            chunk.shell.last().unwrap().at_prompt,
            "prompt visible but at_prompt=false — the mis-routing window is still open"
        );

        assert!(s.feed(b"\x1b]133;B\x07").shell.last().unwrap().at_prompt);
    }

    #[test]
    fn pty_size_clamps_and_computes_pixels() {
        let ps = pty_size(WinSize {
            cols: 80,
            rows: 24,
            cell_w: 8,
            cell_h: 17,
        });
        assert_eq!(ps.rows, 24);
        assert_eq!(ps.cols, 80);
        assert_eq!(ps.pixel_width, 80 * 8);
        assert_eq!(ps.pixel_height, 24 * 17);

        let z = pty_size(WinSize {
            cols: 0,
            rows: 0,
            cell_w: 0,
            cell_h: 0,
        });
        assert_eq!(z.rows, 1);
        assert_eq!(z.cols, 1);
        assert_eq!(z.pixel_width, 0);
        assert_eq!(z.pixel_height, 0);

        let big = pty_size(WinSize {
            cols: u16::MAX,
            rows: u16::MAX,
            cell_w: u16::MAX,
            cell_h: u16::MAX,
        });
        assert_eq!(big.pixel_width, u16::MAX);
        assert_eq!(big.pixel_height, u16::MAX);
    }

    #[test]
    fn parse_osc7_forms_and_rejections() {
        assert_eq!(
            parse_osc7(b"7;file://host/Users/me/dev"),
            Some(PathBuf::from("/Users/me/dev"))
        );
        assert_eq!(parse_osc7(b"7;file:///etc"), Some(PathBuf::from("/etc")));
        assert_eq!(parse_osc7(b"7;/var/log"), Some(PathBuf::from("/var/log")));
        assert_eq!(
            parse_osc7(b"7;file://host/a%20b"),
            Some(PathBuf::from("/a b"))
        );
        assert_eq!(
            parse_osc7(b"7;file://host/%E4%B8%AD%E6%96%87"),
            Some(PathBuf::from("/中文"))
        );
        assert_eq!(
            parse_osc7(b"7;file://host/tmp/a%2520b"),
            Some(PathBuf::from("/tmp/a%20b"))
        );
        assert!(parse_osc7(b"8;file://host/x").is_none());
        assert!(parse_osc7(b"7;file://host").is_none());
        assert!(parse_osc7(b"7;relative/path").is_none());
        assert!(parse_osc7(b"7;file://host").is_none());
    }

    #[test]
    fn strip_uri_drive_slash_only_unwraps_drive_paths() {
        assert_eq!(strip_uri_drive_slash("/C:/Users/foo"), "C:/Users/foo");
        assert_eq!(strip_uri_drive_slash("/d:/x"), "d:/x");
        assert_eq!(strip_uri_drive_slash("/home/me/dev"), "/home/me/dev");
        assert_eq!(strip_uri_drive_slash("//host/share"), "//host/share");
        assert_eq!(strip_uri_drive_slash("C:/already"), "C:/already");
        assert_eq!(strip_uri_drive_slash("/"), "/");
    }

    #[test]
    fn percent_decode_handles_escapes_and_garbage() {
        assert_eq!(percent_decode(b"a%20b"), b"a b");
        assert_eq!(percent_decode(b"%2F"), b"/");
        assert_eq!(percent_decode(b"%2f"), b"/");
        assert_eq!(percent_decode(b"%GG"), b"%GG");
        assert_eq!(percent_decode(b"x%2"), b"x%2");
        assert_eq!(percent_decode(b"plain"), b"plain");
    }

    #[test]
    fn hex_val_ranges() {
        assert_eq!(hex_val(b'0'), Some(0));
        assert_eq!(hex_val(b'9'), Some(9));
        assert_eq!(hex_val(b'a'), Some(10));
        assert_eq!(hex_val(b'f'), Some(15));
        assert_eq!(hex_val(b'A'), Some(10));
        assert_eq!(hex_val(b'F'), Some(15));
        assert!(hex_val(b'g').is_none());
        assert!(hex_val(b' ').is_none());
        assert!(hex_val(b'/').is_none());
    }

    #[test]
    fn osc133_exit_code_parsing() {
        let mut s = OscSniffer::new();
        let d = s.feed(b"\x1b]133;D\x07");
        assert!(d.shell.last().unwrap().at_prompt);
        assert_eq!(d.shell.last().unwrap().last_exit_code, None);

        let d = s.feed(b"\x1b]133;D;oops\x07");
        assert_eq!(d.shell.last().unwrap().last_exit_code, None);

        let d = s.feed(b"\x1b]133;D;-1\x07");
        assert_eq!(d.shell.last().unwrap().last_exit_code, Some(-1));
    }


    #[test]
    fn kimi_late_hooks_cannot_reopen_a_completed_durable_main_turn() {
        use crate::core::cli_agent::{AgentStatus, CLIAgent, parse_agent_event_body};
        let mut state = test_state(true);
        state.agent = Some(CLIAgent::Kimi);
        let start = parse_agent_event_body(br#"{"agent":"kimi","event":"session-start","session_id":"s","session_scope":{"session_file":"/isolated/s/state.json"}}"#).unwrap();
        let ended = parse_agent_event_body(br#"{"agent":"kimi","event":"stop","session_id":"s","source":"kimi-durable-wire","requests":[],"result":{"outcome":"succeeded"}}"#).unwrap();
        apply_agent_signals(&mut state, vec![start, ended], None);
        let snapshot = state.agent_session.clone().unwrap();
        let late = parse_agent_event_body(br#"{"agent":"kimi","event":"permission-request","session_id":"s","request":{"id":"late-p","kind":"permission","resolved":false}}"#).unwrap();
        apply_agent_signals(&mut state, vec![late], None);
        assert_eq!(
            state.agent_session.as_ref().unwrap().status,
            AgentStatus::Done
        );
        assert_eq!(state.agent_session.unwrap(), snapshot);
    }

    #[test]
    fn observed_facts_prefer_the_sessions_argv_and_carry_its_status() {
        use crate::core::cli_agent::{AgentSessionState, AgentStatus, CLIAgent};

        let mut st = test_state(true);
        assert_eq!(
            observed_facts(&st),
            ObservedFacts {
                cwd: None,
                osc_title: None,
                agent: None,
                shell: None,
            }
        );

        st.cwd = Some(PathBuf::from("/work/api"));
        st.agent = Some(CLIAgent::OhMyPi);
        st.agent_argv = Some(vec!["omp".into()]);
        st.agent_session = Some(AgentSessionState {
            status: AgentStatus::Working,
            session_id: Some("sess-1".into()),
            launch_argv: Some(vec!["omp".into(), "--model".into(), "opus".into()]),
            ..Default::default()
        });

        let facts = observed_facts(&st);
        assert_eq!(facts.cwd.as_deref(), Some("/work/api"));
        let agent = facts.agent.expect("an agent in the foreground is a fact");
        assert_eq!(agent.agent, CLIAgent::OhMyPi);
        assert_eq!(agent.session_id.as_deref(), Some("sess-1"));
        assert_eq!(
            agent.launch_argv.as_deref(),
            Some(&["omp".to_string(), "--model".into(), "opus".into()][..]),
            "the session's own argv outranks the identity poll's capture"
        );
        assert_eq!(agent.status, Some(AgentStatus::Working));

        st.agent_session = None;
        let facts = observed_facts(&st);
        assert_eq!(
            facts.agent.unwrap().launch_argv.as_deref(),
            Some(&["omp".to_string()][..])
        );
    }

    #[test]
    fn a_pane_killed_with_its_agent_keeps_the_facts_a_resume_needs() {
        use crate::core::cli_agent::{AgentSessionState, CLIAgent};
        use crate::core::machine::{
            AgentFacts, MACHINE_FILE, MachineStore, OBSERVE_SLOT, PaneSeed, publish_observations,
            withdraw_observations,
        };

        const PANE: u64 = 77;
        let _slot = OBSERVE_SLOT.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::TempDir::new().unwrap();
        let store = MachineStore::open(dir.path().join(MACHINE_FILE));
        let ws = store.workspace_create(None, None, None).unwrap();
        store
            .tab_create(
                ws.id,
                None,
                PaneSeed {
                    pane: PANE,
                    cwd: Some("/work/api".to_string()),
                    ssh_spec: None,
                    agent: Some(AgentFacts {
                        agent: CLIAgent::OhMyPi,
                        session_id: Some("sess-1".to_string()),
                        launch_argv: Some(vec!["omp".to_string()]),
                        session_scope: None,
                        status: None,
                    }),
                    shell: None,
                },
                None,
                None,
            )
            .unwrap();
        publish_observations(&store);

        let run = |shutting_down: bool| {
            let mut state = test_state(true);
            state.id = PANE;
            state.agent = Some(CLIAgent::OhMyPi);
            state.agent_session = Some(AgentSessionState {
                session_id: Some("sess-1".to_string()),
                ..Default::default()
            });
            DaemonPane::spawn_reader(
                Arc::new(Mutex::new(state)),
                Arc::new(AtomicBool::new(shutting_down)),
                Arc::new(OutputGate::new()),
                Box::new(std::io::Cursor::new(b"\x1b]133;D;0\x07".to_vec())),
                null_writer(),
                || false,
                ForegroundProbes {
                    group: Box::new(|| None),
                    remote: Box::new(|| None),
                    agent: Box::new(|| Some(None)),
                    cwd: Box::new(|| None),
                    local_group: |_| Some(true),
                },
                Arc::new(DeathReporter::new(|| {})),
            )
            .join()
            .unwrap();
        };

        run(true);
        let kept = store
            .pane(PANE)
            .expect("the record outlives the pane")
            .agent
            .expect("a teardown must not report the agent away");
        assert_eq!(kept.session_id.as_deref(), Some("sess-1"));

        run(false);
        assert!(
            store.pane(PANE).unwrap().agent.is_none(),
            "an agent that left a pane still in use is a fact, and clears"
        );

        withdraw_observations();
    }

    #[test]
    fn an_explicit_agent_exit_clears_the_live_and_reopened_machine_snapshot() {
        use crate::core::cli_agent::{AgentSessionState, CLIAgent};
        use crate::core::machine::{
            MACHINE_FILE, MachineStore, OBSERVE_SLOT, PaneSeed, publish_observations,
            withdraw_observations,
        };

        const PANE: u64 = 79;
        let _slot = OBSERVE_SLOT.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(MACHINE_FILE);
        let store = MachineStore::open(&path);
        let ws = store.workspace_create(None, None, None).unwrap();
        store
            .tab_create(ws.id, None, PaneSeed::bare(PANE), None, None)
            .unwrap();
        publish_observations(&store);

        let mut state = test_state(true);
        state.id = PANE;
        state.agent = Some(CLIAgent::OhMyPi);
        state.agent_argv = Some(vec!["omp".into()]);
        state.agent_session = Some(AgentSessionState {
            session_id: Some("explicitly-exited-session".into()),
            launch_argv: state.agent_argv.clone(),
            ..Default::default()
        });
        publish_facts(PANE, true, observed_facts(&state));
        store.flush();
        assert!(
            MachineStore::open(&path)
                .pane(PANE)
                .unwrap()
                .agent
                .is_some()
        );

        let state = Arc::new(Mutex::new(state));
        let stream = concat!(
            "\x1b]777;notify;lyrel://cli-agent;",
            r#"{"agent":"omp","event":"session-end","session_id":"explicitly-exited-session"}"#,
            "\x07",
        );
        DaemonPane::spawn_reader(
            state.clone(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(OutputGate::new()),
            Box::new(std::io::Cursor::new(stream.as_bytes().to_vec())),
            null_writer(),
            || false,
            ForegroundProbes {
                group: Box::new(|| None),
                remote: Box::new(|| None),
                // Unsupported or unavailable process probes must not be needed
                // to honor the agent's own explicit shutdown event.
                agent: Box::new(|| None),
                cwd: Box::new(|| None),
                local_group: |_| Some(true),
            },
            Arc::new(DeathReporter::new(|| {})),
        )
        .join()
        .unwrap();
        store.flush();
        let saved_agent = MachineStore::open(&path).pane(PANE).unwrap().agent;
        let live_agent = store.pane(PANE).unwrap().agent;
        withdraw_observations();

        assert!(
            saved_agent.is_none(),
            "an explicitly exited agent must not be restored after restarting the daemon"
        );
        assert!(live_agent.is_none(), "the live machine tree must clear too");
        let mut state = state.lock().unwrap();
        assert!(state.agent.is_none());
        assert!(state.agent_session.is_none());
        assert!(state.agent_argv.is_none());
        assert!(agent_state_snapshot(&state).is_none());
        let (tx, rx) = mpsc::channel();
        attach_subscriber(&mut state, tx);
        assert!(!rx.try_iter().any(|msg| matches!(
            msg,
            DaemonMsg::Agent(Some(_)) | DaemonMsg::AgentStatus(Some(_))
        )));
    }

    #[test]
    fn a_finished_or_waiting_turn_keeps_its_reconnect_and_resume_identity() {
        use crate::core::cli_agent::{AgentStatus, CLIAgent, parse_agent_event_body};

        for (body, expected) in [
            (
                r#"{"agent":"omp","event":"prompt-submit"}"#,
                AgentStatus::Working,
            ),
            (r#"{"agent":"omp","event":"stop"}"#, AgentStatus::Done),
            (
                r#"{"agent":"omp","event":"permission-request","request":{"id":"approval","kind":"permission"}}"#,
                AgentStatus::Waiting,
            ),
        ] {
            let mut state = test_state(true);
            apply_agent(&mut state, Some((CLIAgent::OhMyPi, vec!["omp".into()])));
            let start = parse_agent_event_body(
                br#"{"agent":"omp","event":"session-start","session_id":"still-running","session_scope":{"session_file":"/isolated/still-running.jsonl"}}"#,
            )
            .unwrap();
            apply_agent_signals(&mut state, vec![start], None);
            apply_agent_signals(
                &mut state,
                vec![parse_agent_event_body(body.as_bytes()).unwrap()],
                None,
            );
            let snapshot = agent_state_snapshot(&state).expect("reconnect keeps a live agent");
            assert_eq!(snapshot.agent, Some(CLIAgent::OhMyPi));
            assert_eq!(snapshot.state.status, expected);
            assert_eq!(snapshot.state.session_id.as_deref(), Some("still-running"));
            let facts = observed_facts(&state)
                .agent
                .expect("resume identity is kept");
            assert_eq!(facts.session_id, snapshot.state.session_id);
            assert_eq!(facts.launch_argv, Some(vec!["omp".into()]));
            assert!(facts.session_scope.is_some());
            let (tx, rx) = mpsc::channel();
            attach_subscriber_with_permissions(&mut state, tx, &OutputGate::new(), false, true);
            let replay: Vec<_> = rx.try_iter().collect();
            assert!(
                replay
                    .iter()
                    .any(|msg| matches!(msg, DaemonMsg::Agent(Some(CLIAgent::OhMyPi))))
            );
            assert!(replay.iter().any(|msg| matches!(
                msg,
                DaemonMsg::AgentStatus(Some(session)) if session == &snapshot.state
            )));
        }
    }

    #[test]
    fn a_shutdown_after_the_foreground_exit_does_not_reclaim_the_pane() {
        use crate::core::cli_agent::{CLIAgent, parse_agent_event_body};

        let mut state = test_state(true);
        apply_agent(&mut state, Some((CLIAgent::OhMyPi, vec!["omp".into()])));
        apply_agent(&mut state, None);
        let ended = parse_agent_event_body(
            br#"{"agent":"omp","event":"session-end","session_id":"already-exited"}"#,
        )
        .unwrap();
        apply_agent_signals(&mut state, vec![ended], None);
        assert!(state.agent.is_none());
        assert!(state.agent_session.is_none());
        assert!(observed_facts(&state).agent.is_none());
    }

    #[test]
    fn a_late_shutdown_from_the_previous_session_does_not_clear_the_current_agent() {
        use crate::core::cli_agent::{CLIAgent, parse_agent_event_body};

        let mut state = test_state(true);
        apply_agent(&mut state, Some((CLIAgent::OhMyPi, vec!["omp".into()])));
        let start = parse_agent_event_body(
            br#"{"agent":"omp","event":"session-start","session_id":"current-session"}"#,
        )
        .unwrap();
        apply_agent_signals(&mut state, vec![start], None);
        let before = state.agent_session.clone();
        let ended = parse_agent_event_body(
            br#"{"agent":"omp","event":"session-end","session_id":"previous-session"}"#,
        )
        .unwrap();
        apply_agent_signals(&mut state, vec![ended], None);
        assert_eq!(state.agent, Some(CLIAgent::OhMyPi));
        assert_eq!(state.agent_session, before);
    }

    /// The whole path a title takes into the tree: sniffed out of the pane's
    /// own output, kept on its state, and written to the record every other
    /// viewer reads. Nothing else can name the tabs of a workspace this process
    /// does not own — without this the switcher fell back to the agent, and
    /// every tab of a workspace running one read "Claude Code".
    #[test]
    fn a_pane_title_reaches_the_machine_tree() {
        use crate::core::machine::{
            MACHINE_FILE, MachineStore, OBSERVE_SLOT, PaneSeed, publish_observations,
            withdraw_observations,
        };

        const PANE: u64 = 78;
        let _slot = OBSERVE_SLOT.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::TempDir::new().unwrap();
        let store = MachineStore::open(dir.path().join(MACHINE_FILE));
        let ws = store.workspace_create(None, None, None).unwrap();
        store
            .tab_create(ws.id, None, PaneSeed::bare(PANE), None, None)
            .unwrap();
        publish_observations(&store);

        let run = |had: Option<&str>, output: &[u8]| {
            let mut state = test_state(true);
            state.id = PANE;
            state.osc_title = had.map(str::to_string);
            DaemonPane::spawn_reader(
                Arc::new(Mutex::new(state)),
                Arc::new(AtomicBool::new(false)),
                Arc::new(OutputGate::new()),
                Box::new(std::io::Cursor::new(output.to_vec())),
                null_writer(),
                || false,
                ForegroundProbes {
                    group: Box::new(|| None),
                    remote: Box::new(|| None),
                    agent: Box::new(|| Some(None)),
                    cwd: Box::new(|| None),
                    local_group: |_| Some(true),
                },
                Arc::new(DeathReporter::new(|| {})),
            )
            .join()
            .unwrap();
            store.pane(PANE).expect("the record was seeded").osc_title
        };

        assert_eq!(
            run(None, b"\x1b]0;\xe2\x9c\xb3 fixing the switcher\x07").as_deref(),
            Some("✳ fixing the switcher")
        );
        assert_eq!(
            run(Some("✳ fixing the switcher"), b"\x1b]2;\x07"),
            None,
            "a pane that resets its title clears the one on record"
        );

        withdraw_observations();
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

    #[test]
    fn sentinel_events_drive_agent_session_state() {
        use crate::core::cli_agent::{AgentStatus, CLIAgent};

        let mut st = test_state(true);
        let (tx, rx) = mpsc::channel();
        attach_subscriber(&mut st, tx);
        drain(&rx);

        let mut sniffer = OscSniffer::new();
        let stream = concat!(
            "\x1b]777;notify;lyrel://cli-agent;",
            r#"{"v":1,"agent":"omp","event":"session-start","session_id":"sid-9"}"#,
            "\x07",
            "\x1b]777;notify;lyrel://cli-agent;",
            r#"{"v":1,"agent":"omp","event":"prompt-submit"}"#,
            "\x07",
        );
        apply_signals(&mut st, sniffer.feed(stream.as_bytes()));

        assert_eq!(st.agent, Some(CLIAgent::OhMyPi));
        let sess = st.agent_session.clone().expect("session state exists");
        assert_eq!(sess.status, AgentStatus::Working);
        assert_eq!(sess.session_id.as_deref(), Some("sid-9"));
        assert!(sess.rich);

        assert!(matches!(
            rx.try_recv(),
            Ok(DaemonMsg::Agent(Some(CLIAgent::OhMyPi)))
        ));
        assert!(matches!(
            rx.try_recv(),
            Ok(DaemonMsg::AgentStatus(Some(s))) if s.status == AgentStatus::Working
        ));

        let waiting = concat!(
            "\x1b]777;notify;lyrel://cli-agent;",
            r#"{"agent":"omp","event":"permission-request","message":"OhMyPi needs your permission to use Bash","request":{"id":"tool-9","kind":"permission"}}"#,
            "\x07",
        );
        apply_signals(&mut st, sniffer.feed(waiting.as_bytes()));
        assert_eq!(
            st.agent_session.as_ref().unwrap().status,
            AgentStatus::Waiting
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(DaemonMsg::AgentStatus(Some(s))) if s.message.as_deref().unwrap().contains("permission")
        ));

        apply_agent(&mut st, None);
        assert!(st.agent_session.is_none());
        assert!(matches!(rx.try_recv(), Ok(DaemonMsg::AgentStatus(None))));
        assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Agent(None))));
    }

    #[test]
    fn a_foreign_agent_hook_does_not_replace_the_foreground_session() {
        use crate::core::cli_agent::CLIAgent;

        let mut st = test_state(true);
        let mut sniffer = OscSniffer::new();
        let omp = concat!(
            "\x1b]777;notify;lyrel://cli-agent;",
            r#"{"v":1,"agent":"omp","event":"session-start","session_id":"01a0d21f-2f79-72e0-bb16-d6d908aaa6e0"}"#,
            "\x07",
        );
        apply_signals(&mut st, sniffer.feed(omp.as_bytes()));
        assert_eq!(st.agent, Some(CLIAgent::OhMyPi));

        let pi = concat!(
            "\x1b]777;notify;lyrel://cli-agent;",
            r#"{"v":1,"agent":"pi","event":"session-start","session_id":"e18a20ff-4c8c-4b94-867b-dd4b79032a6c"}"#,
            "\x07",
        );
        apply_signals(&mut st, sniffer.feed(pi.as_bytes()));
        assert_eq!(
            st.agent_session.as_ref().unwrap().session_id.as_deref(),
            Some("01a0d21f-2f79-72e0-bb16-d6d908aaa6e0"),
            "a Pi hook on an omp pane must not steal the session id"
        );

        apply_agent(&mut st, Some((CLIAgent::Pi, vec!["pi".into()])));
        assert!(
            st.agent_session.is_none(),
            "switching the foreground agent drops the previous session"
        );
    }

    #[test]
    fn a_kitty_notification_marks_a_hookless_agent_waiting() {
        use crate::core::cli_agent::{AgentStatus, CLIAgent};

        let mut st = test_state(true);
        st.agent = Some(CLIAgent::OhMyPi);
        let mut sniffer = OscSniffer::new();
        apply_signals(
            &mut st,
            sniffer.feed(
                b"\x1b]99;i=7:d=0:p=title;Oh My Pi\x07\x1b]99;i=7:p=body;OhMyPi needs your permission\x07\x1b]99;i=7:d=1:a=focus;\x07",
            ),
        );
        let sess = st.agent_session.clone().unwrap();
        assert_eq!(sess.status, AgentStatus::Waiting);
        assert_eq!(
            sess.message.as_deref(),
            Some("OhMyPi needs your permission")
        );
    }

    #[test]
    fn opaque_notifications_only_fall_back_when_no_rich_state() {
        use crate::core::cli_agent::{AgentSessionState, AgentStatus, CLIAgent};

        let mut st = test_state(true);
        let mut sniffer = OscSniffer::new();
        apply_signals(&mut st, sniffer.feed(b"\x1b]9;Build finished\x07"));
        assert!(st.agent_session.is_none());

        st.agent = Some(CLIAgent::Pi);
        apply_signals(&mut st, sniffer.feed(b"\x1b]9;Pi wants to run tests\x07"));
        let sess = st.agent_session.clone().unwrap();
        assert_eq!(sess.status, AgentStatus::Waiting);
        assert!(!sess.rich);

        st.agent_session = Some(AgentSessionState {
            status: AgentStatus::Working,
            message: None,
            session_id: Some("sid".into()),
            launch_argv: None,
            rich: true,
            cwd: None,
            activity: 0,
            turns: 0,
            pending_requests: Vec::new(),
            request_resume_status: AgentStatus::Idle,
            pending_stop: false,
            readout: Default::default(),
        });
        apply_signals(&mut st, sniffer.feed(b"\x1b]9;noise\x07"));
        assert_eq!(
            st.agent_session.as_ref().unwrap().status,
            AgentStatus::Working
        );
    }

    #[test]
    fn probed_cwd_corrects_a_pane_whose_shell_never_reports_osc7() {
        let mut st = test_state(true);
        st.cwd = Some(PathBuf::from("/Users/alice"));
        let (tx, rx) = mpsc::channel();
        attach_subscriber(&mut st, tx);
        drain(&rx);

        apply_probed_cwd(&mut st, Some(PathBuf::from("/Users/alice/dev/lyrel")));

        assert_eq!(st.cwd.as_deref(), Some(Path::new("/Users/alice/dev/lyrel")));
        assert!(
            matches!(rx.try_recv(), Ok(DaemonMsg::Cwd(p)) if p == PathBuf::from("/Users/alice/dev/lyrel"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn probed_cwd_keeps_the_shells_spelling_for_a_symlinked_path() {
        let tmp = std::env::temp_dir().join(format!("lyrel-cwd-{}", std::process::id()));
        let real = tmp.join("real");
        let link = tmp.join("link");
        std::fs::create_dir_all(&real).unwrap();
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let mut st = test_state(true);
        st.cwd = Some(link.clone());
        let (tx, rx) = mpsc::channel();
        attach_subscriber(&mut st, tx);
        drain(&rx);

        apply_probed_cwd(&mut st, Some(real.canonicalize().unwrap()));

        assert_eq!(st.cwd.as_deref(), Some(link.as_path()));
        assert!(rx.try_recv().is_err(), "same directory → nothing to tell");

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn probed_cwd_matching_the_reported_one_says_nothing() {
        let mut st = test_state(true);
        st.cwd = Some(PathBuf::from("/Users/alice/dev/lyrel"));
        let (tx, rx) = mpsc::channel();
        attach_subscriber(&mut st, tx);
        drain(&rx);

        apply_probed_cwd(&mut st, Some(PathBuf::from("/Users/alice/dev/lyrel")));

        assert_eq!(st.cwd.as_deref(), Some(Path::new("/Users/alice/dev/lyrel")));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn probed_cwd_declines_to_speak_for_a_remote_pane() {
        let mut st = test_state(true);
        st.remote = Some(RemoteContext {
            kind: RemoteKind::Ssh,
            argv: vec!["ssh".into(), "build-box".into()],
            target: "build-box".into(),
        });
        st.cwd = None;
        let (tx, rx) = mpsc::channel();
        attach_subscriber(&mut st, tx);
        drain(&rx);

        apply_probed_cwd(&mut st, Some(PathBuf::from("/Users/alice/dev/lyrel")));

        assert_eq!(st.cwd, None);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn probed_cwd_absent_leaves_the_reported_cwd_alone() {
        let mut st = test_state(true);
        st.cwd = Some(PathBuf::from("/work"));
        let (tx, rx) = mpsc::channel();
        attach_subscriber(&mut st, tx);
        drain(&rx);

        apply_probed_cwd(&mut st, None);

        assert_eq!(st.cwd.as_deref(), Some(Path::new("/work")));
        assert!(rx.try_recv().is_err());
    }


    #[test]
    fn death_notifies_observers_but_only_controllers_defer_the_reap() {
        let with_observer_only = Arc::new(Mutex::new(test_state(true)));
        let (observer_tx, observer_rx) = mpsc::channel();
        observe_subscriber(
            &mut with_observer_only.lock().unwrap(),
            observer_tx,
            Arc::new(OutputGate::new()),
            false,
        );
        drain(&observer_rx);
        let (dead_tx, dead_rx) = mpsc::channel();
        DeathReporter::new(move || dead_tx.send(()).unwrap())
            .report(&with_observer_only, &AtomicBool::new(false));
        assert!(matches!(
            observer_rx.try_recv(),
            Ok(DaemonMsg::Exited { code: None })
        ));
        assert!(
            dead_rx.try_recv().is_ok(),
            "read-only observers must not keep a dead pane in the registry"
        );

        let with_both = Arc::new(Mutex::new(test_state(true)));
        let (controller_tx, controller_rx) = mpsc::channel();
        let (observer_tx, observer_rx) = mpsc::channel();
        {
            let mut st = with_both.lock().unwrap();
            attach_subscriber(&mut st, controller_tx);
            observe_subscriber(&mut st, observer_tx, Arc::new(OutputGate::new()), false);
        }
        drain(&controller_rx);
        drain(&observer_rx);
        let (dead_tx, dead_rx) = mpsc::channel();
        DeathReporter::new(move || dead_tx.send(()).unwrap())
            .report(&with_both, &AtomicBool::new(false));
        assert!(matches!(
            controller_rx.try_recv(),
            Ok(DaemonMsg::Exited { code: None })
        ));
        assert!(matches!(
            observer_rx.try_recv(),
            Ok(DaemonMsg::Exited { code: None })
        ));
        assert!(
            dead_rx.try_recv().is_err(),
            "an attached death is still the detach path's to reclaim"
        );
    }

    #[test]
    fn agent_state_snapshot_reports_only_panes_with_a_session() {
        use crate::core::cli_agent::{AgentSessionState, AgentStatus, CLIAgent};

        let mut st = test_state(true);
        st.id = 42;
        assert_eq!(agent_state_snapshot(&st), None);

        st.agent = Some(CLIAgent::OhMyPi);
        assert_eq!(
            agent_state_snapshot(&st),
            None,
            "a detected agent without session state is not yet a fact worth listing"
        );

        st.agent_session = Some(AgentSessionState {
            status: AgentStatus::Waiting,
            session_id: Some("sess-1".into()),
            ..Default::default()
        });
        let snap = agent_state_snapshot(&st).expect("a session state is the fact");
        assert_eq!(snap.pane_id, 42);
        assert_eq!(snap.agent, Some(CLIAgent::OhMyPi));
        assert_eq!(snap.state.status, AgentStatus::Waiting);
        assert_eq!(snap.state.session_id.as_deref(), Some("sess-1"));
    }

    /// The foreground probe only ever runs on output, and the prompt a shell
    /// draws after a command is the last output a pane produces until the user
    /// types again. So an `ssh` that exited inside the poll interval used to
    /// leave the pane reporting itself as remote indefinitely: nothing came
    /// along to probe on. A prompt mark now forces the probe (#817).
    #[test]
    fn a_prompt_mark_reprobes_the_foreground_inside_the_poll_interval() {
        /// Hands the reader one chunk per `read`, so two prompt marks arrive as
        /// two passes through the loop a few microseconds apart — well inside
        /// `REMOTE_CONTEXT_POLL_INTERVAL`.
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

        let state = Arc::new(Mutex::new(test_state(true)));
        let (sub_tx, sub_rx) = mpsc::channel();
        attach_subscriber(&mut state.lock().unwrap(), sub_tx);
        drain(&sub_rx);

        let probes_taken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let taken = probes_taken.clone();
        let remote = Box::new(move || {
            // First probe: `ssh` holds the pty. Every one after: it is gone.
            (taken.fetch_add(1, Ordering::SeqCst) == 0).then(|| RemoteContext {
                kind: RemoteKind::Ssh,
                argv: vec!["ssh".into(), "box".into()],
                target: "box".into(),
            })
        });

        let handle = DaemonPane::spawn_reader(
            state.clone(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(OutputGate::new()),
            Box::new(Chunks(
                [
                    b"\x1b]133;C;ssh box\x07".to_vec(),
                    b"\x1b]133;D;0\x07".to_vec(),
                ]
                .into_iter()
                .collect(),
            )),
            null_writer(),
            || false,
            ForegroundProbes {
                group: Box::new(|| None),
                remote,
                agent: Box::new(|| None),
                cwd: Box::new(|| None),
                local_group: |_| Some(true),
            },
            Arc::new(DeathReporter::new(|| {})),
        );
        handle.join().unwrap();

        assert_eq!(
            probes_taken.load(Ordering::SeqCst),
            2,
            "the prompt mark did not force a second probe"
        );
        assert!(
            state.lock().unwrap().remote.is_none(),
            "the pane still reports the ssh session it has already left"
        );
        let reported: Vec<Option<String>> = sub_rx
            .try_iter()
            .filter_map(|msg| match msg {
                DaemonMsg::RemoteContext(ctx) => Some(ctx.map(|c| c.target)),
                _ => None,
            })
            .collect();
        assert_eq!(
            reported,
            vec![Some("box".to_string()), None],
            "the client was never told the pane came home"
        );
    }

    #[test]
    fn reader_eof_with_subscriber_sends_exited_not_on_dead() {
        let state = Arc::new(Mutex::new(test_state(true)));
        let (sub_tx, sub_rx) = mpsc::channel();
        attach_subscriber(&mut state.lock().unwrap(), sub_tx);
        drain(&sub_rx);
        let dead = Arc::new(AtomicBool::new(false));
        let dead_flag = dead.clone();

        let handle = DaemonPane::spawn_reader(
            state.clone(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(OutputGate::new()),
            Box::new(std::io::Cursor::new(b"tail".to_vec())),
            null_writer(),
            || false,
            ForegroundProbes {
                group: Box::new(|| None),
                remote: Box::new(|| None),
                agent: Box::new(|| None),
                cwd: Box::new(|| None),
                local_group: |_| Some(true),
            },
            Arc::new(DeathReporter::new(move || {
                dead_flag.store(true, Ordering::SeqCst)
            })),
        );
        handle.join().unwrap();

        assert!(!state.lock().unwrap().alive);
        assert_eq!(state.lock().unwrap().ring.flatten(), b"tail");
        assert!(matches!(sub_rx.try_recv(), Ok(DaemonMsg::Output(b)) if b == b"tail"));
        assert!(matches!(
            sub_rx.try_recv(),
            Ok(DaemonMsg::Exited { code: None })
        ));
        assert!(
            !dead.load(Ordering::SeqCst),
            "an attached death is the detach path's to reclaim, not on_dead's"
        );
    }

    #[cfg(windows)]
    #[test]
    fn closing_conpty_waits_for_delayed_reader_output_before_exit() {
        /// A master whose destruction opens the simulated ConPTY output pipe.
        /// The production `ConPtyMasterPty` performs the equivalent transition
        /// by calling `ClosePseudoConsole` from its destructor.
        struct SignallingMaster {
            released: Arc<(Mutex<bool>, Condvar)>,
        }

        impl Drop for SignallingMaster {
            fn drop(&mut self) {
                let (lock, ready) = &*self.released;
                *lock.lock().unwrap() = true;
                ready.notify_all();
            }
        }

        impl MasterPty for SignallingMaster {
            fn resize(&self, _size: PtySize) -> anyhow::Result<()> {
                Ok(())
            }

            fn get_size(&self) -> anyhow::Result<PtySize> {
                Ok(PtySize::default())
            }

            fn try_clone_reader(&self) -> anyhow::Result<Box<dyn Read + Send>> {
                Ok(Box::new(std::io::empty()))
            }

            fn take_writer(&self) -> anyhow::Result<Box<dyn Write + Send>> {
                Ok(Box::new(std::io::sink()))
            }
        }

        /// Models a reader that stays delayed well past any plausible grace
        /// period, then receives a final output chunk followed by EOF.
        struct DelayedTailReader {
            released: Arc<(Mutex<bool>, Condvar)>,
            tail: std::io::Cursor<Vec<u8>>,
            delayed: bool,
        }

        impl Read for DelayedTailReader {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if !self.delayed {
                    let (lock, ready) = &*self.released;
                    let mut released = lock.lock().unwrap();
                    while !*released {
                        released = ready.wait(released).unwrap();
                    }
                    drop(released);
                    std::thread::sleep(Duration::from_millis(800));
                    self.delayed = true;
                }
                self.tail.read(buf)
            }
        }

        let released = Arc::new((Mutex::new(false), Condvar::new()));
        let master: Mutex<Option<Box<dyn MasterPty + Send>>> =
            Mutex::new(Some(Box::new(SignallingMaster {
                released: released.clone(),
            })));
        let state = Arc::new(Mutex::new(test_state(true)));
        let (sub_tx, sub_rx) = mpsc::channel();
        attach_subscriber(&mut state.lock().unwrap(), sub_tx);
        drain(&sub_rx);

        let reader = DaemonPane::spawn_reader(
            state,
            Arc::new(AtomicBool::new(false)),
            Arc::new(OutputGate::new()),
            Box::new(DelayedTailReader {
                released,
                tail: std::io::Cursor::new(b"final output".to_vec()),
                delayed: false,
            }),
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
        );

        close_pty_master(&master);
        assert!(
            sub_rx.recv_timeout(Duration::from_millis(600)).is_err(),
            "closing the master must not publish Exited while the reader is still delayed"
        );
        assert!(matches!(
            sub_rx.recv_timeout(Duration::from_secs(1)),
            Ok(DaemonMsg::Output(bytes)) if bytes == b"final output"
        ));
        assert!(matches!(
            sub_rx.recv_timeout(Duration::from_secs(1)),
            Ok(DaemonMsg::Exited { code: None })
        ));
        assert!(sub_rx.try_recv().is_err(), "no output may follow Exited");
        reader.join().unwrap();
    }

    /// An inert stand-in for the ConPTY master: releasing it is instant, which
    /// is what `ClosePseudoConsole` does once the pipe has no pending bytes.
    #[cfg(windows)]
    struct InertMaster;

    #[cfg(windows)]
    impl MasterPty for InertMaster {
        fn resize(&self, _size: PtySize) -> anyhow::Result<()> {
            Ok(())
        }

        fn get_size(&self) -> anyhow::Result<PtySize> {
            Ok(PtySize::default())
        }

        fn try_clone_reader(&self) -> anyhow::Result<Box<dyn Read + Send>> {
            Ok(Box::new(std::io::empty()))
        }

        fn take_writer(&self) -> anyhow::Result<Box<dyn Write + Send>> {
            Ok(Box::new(std::io::sink()))
        }
    }

    #[cfg(windows)]
    fn exit_drain_fixture() -> (
        Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>,
        Arc<Mutex<PaneState>>,
        Arc<AtomicBool>,
        Arc<DeathReporter>,
        mpsc::Receiver<DaemonMsg>,
    ) {
        let state = Arc::new(Mutex::new(test_state(true)));
        let (sub_tx, sub_rx) = mpsc::channel();
        attach_subscriber(&mut state.lock().unwrap(), sub_tx);
        drain(&sub_rx);
        (
            Arc::new(Mutex::new(Some(
                Box::new(InertMaster) as Box<dyn MasterPty + Send>
            ))),
            state,
            Arc::new(AtomicBool::new(false)),
            Arc::new(DeathReporter::new(|| {})),
            sub_rx,
        )
    }

    /// A grandchild that inherited the ConPTY output pipe keeps it open after
    /// the shell is gone, so the reader never sees EOF and can never publish
    /// the exit. The monitor's drain window is the only thing that stops such
    /// a pane from reading as alive forever.
    #[cfg(windows)]
    #[test]
    fn a_pty_that_never_reaches_eof_still_reports_the_exit() {
        struct NeverEofReader;

        impl Read for NeverEofReader {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                // Long enough to outlast the window below, bounded so the
                // thread cannot outlive the test binary.
                std::thread::sleep(Duration::from_secs(5));
                Ok(0)
            }
        }

        let (master, state, shutting_down, death, sub_rx) = exit_drain_fixture();
        let _reader = DaemonPane::spawn_reader(
            state.clone(),
            shutting_down.clone(),
            Arc::new(OutputGate::new()),
            Box::new(NeverEofReader),
            null_writer(),
            || false,
            ForegroundProbes {
                group: Box::new(|| None),
                remote: Box::new(|| None),
                agent: Box::new(|| None),
                cwd: Box::new(|| None),
                local_group: |_| Some(true),
            },
            death.clone(),
        );

        drain_then_report(
            master,
            state,
            shutting_down,
            death,
            Duration::from_millis(50),
        );
        assert!(
            matches!(
                sub_rx.recv_timeout(Duration::from_secs(1)),
                Ok(DaemonMsg::Exited { code: None })
            ),
            "the monitor must report the exit once its drain window elapses"
        );
    }

    /// The ordinary case: EOF arrives, the reader reports, and the monitor
    /// neither waits out its window nor publishes a second `Exited`.
    #[cfg(windows)]
    #[test]
    fn the_exit_monitor_defers_to_the_reader_that_saw_eof() {
        let (master, state, shutting_down, death, sub_rx) = exit_drain_fixture();
        let reader = DaemonPane::spawn_reader(
            state.clone(),
            shutting_down.clone(),
            Arc::new(OutputGate::new()),
            Box::new(std::io::empty()),
            null_writer(),
            || false,
            ForegroundProbes {
                group: Box::new(|| None),
                remote: Box::new(|| None),
                agent: Box::new(|| None),
                cwd: Box::new(|| None),
                local_group: |_| Some(true),
            },
            death.clone(),
        );

        let started = std::time::Instant::now();
        drain_then_report(master, state, shutting_down, death, Duration::from_secs(30));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the monitor must stop waiting as soon as the reader has reported"
        );
        assert!(matches!(
            sub_rx.recv_timeout(Duration::from_secs(1)),
            Ok(DaemonMsg::Exited { code: None })
        ));
        assert!(
            sub_rx.try_recv().is_err(),
            "the idempotent reporter must not publish a second Exited"
        );
        reader.join().unwrap();
    }

    #[test]
    fn reader_drains_late_text_without_reviving_graphics_after_exit_report() {
        struct ControlledReader {
            entered: Option<Sender<()>>,
            chunks: mpsc::Receiver<Vec<u8>>,
        }

        impl Read for ControlledReader {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if let Some(entered) = self.entered.take() {
                    entered.send(()).unwrap();
                }
                let bytes = self
                    .chunks
                    .recv_timeout(Duration::from_secs(5))
                    .map_err(|error| {
                        std::io::Error::other(format!("controlled reader: {error}"))
                    })?;
                assert!(bytes.len() <= buf.len());
                buf[..bytes.len()].copy_from_slice(&bytes);
                Ok(bytes.len())
            }
        }

        let state = Arc::new(Mutex::new(test_state(true)));
        let (sub_tx, sub_rx) = mpsc::channel();
        attach_subscriber(&mut state.lock().unwrap(), sub_tx);
        drain(&sub_rx);
        let shutting_down = Arc::new(AtomicBool::new(false));
        let death = Arc::new(DeathReporter::new(|| {}));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (chunks_tx, chunks_rx) = mpsc::channel();
        let reader = DaemonPane::spawn_reader(
            state.clone(),
            shutting_down.clone(),
            Arc::new(OutputGate::new()),
            Box::new(ControlledReader {
                entered: Some(entered_tx),
                chunks: chunks_rx,
            }),
            null_writer(),
            || false,
            ForegroundProbes {
                group: Box::new(|| None),
                remote: Box::new(|| None),
                agent: Box::new(|| None),
                cwd: Box::new(|| None),
                local_group: |_| Some(true),
            },
            death.clone(),
        );
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        // Same ordering as the Windows fallback monitor: report the death
        // while the reader is blocked, then let pending PTY bytes arrive.
        death.report(&state, &shutting_down);
        assert!(state.lock().unwrap().graphics.is_none());
        assert!(matches!(
            sub_rx.recv_timeout(Duration::from_secs(1)),
            Ok(DaemonMsg::Exited { .. })
        ));
        chunks_tx.send(b"late ".to_vec()).unwrap();
        chunks_tx
            .send(b"\x1b_Ga=T,i=9,f=32,s=1,v=1,q=2;/wAA".to_vec())
            .unwrap();
        chunks_tx.send(b"/w==\x1b\\final ".to_vec()).unwrap();
        chunks_tx.send(b"\x1b]5522;type=write:id=late\x1b\\\x1b]5522;type=wdata:mime=aW1hZ2UvcG5n;cG5nLWRhdGE=\x1b\\\x1b]5522;type=wdata\x1b\\tail".to_vec()).unwrap();
        chunks_tx.send(Vec::new()).unwrap();
        reader.join().unwrap();

        let mut output = Vec::new();
        for message in sub_rx.try_iter() {
            match message {
                DaemonMsg::Output(bytes) => output.extend(bytes),
                DaemonMsg::Graphics(_) => panic!("graphics cannot revive after the pane exits"),
                DaemonMsg::ClipboardWrite(_) => {
                    panic!("clipboard permission must still be enforced")
                }
                DaemonMsg::Exited { .. } => panic!("exit reporting remains idempotent"),
                _ => {}
            }
        }
        assert_eq!(output, b"late final tail");
        let state = state.lock().unwrap();
        assert_eq!(state.ring.flatten(), b"late final tail");
        assert!(state.graphics.is_none());
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

    #[test]
    #[cfg(unix)]
    fn reader_gates_indirect_media_before_first_ssh_bytes_and_restores_local_queries() {
        use base64::Engine as _;
        use std::sync::atomic::AtomicUsize;
        struct Chunks {
            chunks: std::collections::VecDeque<(usize, Vec<u8>)>,
            phase: Arc<AtomicUsize>,
        }
        impl Read for Chunks {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let Some((phase, chunk)) = self.chunks.pop_front() else {
                    return Ok(0);
                };
                self.phase.store(phase, Ordering::SeqCst);
                buf[..chunk.len()].copy_from_slice(&chunk);
                Ok(chunk.len())
            }
        }
        struct Replies(Arc<Mutex<Vec<u8>>>);
        impl Write for Replies {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tty-graphics-protocol-host-only.rgba");
        std::fs::write(&path, [255, 0, 0, 255]).unwrap();
        let name =
            base64::engine::general_purpose::STANDARD.encode(path.as_os_str().as_encoded_bytes());
        let query = |id| format!("\x1b_Ga=q,i={id},t=f,f=32,s=1,v=1;{name}\x1b\\").into_bytes();
        let run = |chunks: Vec<(usize, Vec<u8>)>, same_group: bool, unknown: bool| {
            let phase = Arc::new(AtomicUsize::new(0));
            let group_phase = phase.clone();
            let remote_phase = phase.clone();
            let sink = Arc::new(Mutex::new(Vec::new()));
            DaemonPane::spawn_reader(
                Arc::new(Mutex::new(test_state(true))),
                Arc::new(AtomicBool::new(false)),
                Arc::new(OutputGate::new()),
                Box::new(Chunks {
                    chunks: chunks
                        .into_iter()
                        .filter(|(_, bytes)| !bytes.is_empty())
                        .collect(),
                    phase,
                }),
                Arc::new(Mutex::new(Box::new(Replies(sink.clone())))),
                || false,
                ForegroundProbes {
                    group: Box::new(move || {
                        let remote = matches!(group_phase.load(Ordering::SeqCst), 1 | 2);
                        if unknown && remote {
                            None
                        } else {
                            Some(if remote && !same_group { 11 } else { 10 })
                        }
                    }),
                    remote: Box::new(move || {
                        (!unknown && matches!(remote_phase.load(Ordering::SeqCst), 1 | 2)).then(
                            || RemoteContext {
                                kind: RemoteKind::Ssh,
                                argv: vec!["ssh".into(), "other-host".into()],
                                target: "other-host".into(),
                            },
                        )
                    }),
                    agent: Box::new(|| None),
                    cwd: Box::new(|| None),
                    local_group: |_| Some(true),
                },
                Arc::new(DeathReporter::new(|| {})),
            )
            .join()
            .unwrap();
            let replies = sink.lock().unwrap().clone();
            replies
        };
        let remote_query = query(2);
        for same_group in [false, true] {
            for cut in 0..=remote_query.len() {
                let replies = run(
                    vec![
                        (0, query(1)),
                        (1, remote_query[..cut].to_vec()),
                        (2, remote_query[cut..].to_vec()),
                        (2, query(3)),
                        (3, query(4)),
                    ],
                    same_group,
                    false,
                );
                assert_eq!(replies, b"\x1b_Gi=1;OK\x1b\\\x1b_Gi=2;ENOTSUPPORTED:Host-local transfer unavailable\x1b\\\x1b_Gi=3;ENOTSUPPORTED:Host-local transfer unavailable\x1b\\\x1b_Gi=4;OK\x1b\\", "same_group={same_group}, split={cut}");
            }
        }
        let split = (name.len() / 8) * 4;
        for (same_group, unknown) in [(false, false), (true, false), (false, true)] {
            let replies = run(
                vec![
                    (
                        0,
                        format!(
                            "\x1b_Ga=q,i=9,t=f,f=32,s=1,v=1,m=1;{}\x1b\\",
                            &name[..split]
                        )
                        .into_bytes(),
                    ),
                    (1, b"\x1b_Gm=0;".to_vec()),
                    (3, format!("{}\x1b\\", &name[split..]).into_bytes()),
                    (3, query(4)),
                ],
                same_group,
                unknown,
            );
            assert_eq!(
                replies,
                b"\x1b_Gi=9;ENOTSUPPORTED:Host-local transfer unavailable\x1b\\\x1b_Gi=4;OK\x1b\\",
                "a pending transfer that crossed remote/unknown cannot become local again: same_group={same_group}, unknown={unknown}"
            );
        }
        assert!(path.exists(), "query does not delete a plain file");
    }

    #[test]
    fn reader_poll_applies_the_probed_cwd() {
        let state = Arc::new(Mutex::new(test_state(true)));
        let (sub_tx, sub_rx) = mpsc::channel();
        {
            let mut st = state.lock().unwrap();
            st.cwd = Some(PathBuf::from("/Users/alice"));
            attach_subscriber(&mut st, sub_tx);
            drain(&sub_rx);
        }

        let handle = DaemonPane::spawn_reader(
            state.clone(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(OutputGate::new()),
            Box::new(std::io::Cursor::new(b"alice@host ~ % ".to_vec())),
            null_writer(),
            || false,
            ForegroundProbes {
                group: Box::new(|| None),
                remote: Box::new(|| None),
                agent: Box::new(|| None),
                cwd: Box::new(|| Some(PathBuf::from("/Users/alice/dev/lyrel"))),
                local_group: |_| Some(true),
            },
            Arc::new(DeathReporter::new(|| {})),
        );
        handle.join().unwrap();

        assert_eq!(
            state.lock().unwrap().cwd.as_deref(),
            Some(Path::new("/Users/alice/dev/lyrel"))
        );
        assert!(matches!(sub_rx.try_recv(), Ok(DaemonMsg::Output(_))));
        assert!(
            matches!(sub_rx.try_recv(), Ok(DaemonMsg::Cwd(p)) if p == PathBuf::from("/Users/alice/dev/lyrel"))
        );
    }

    #[test]
    fn reader_eof_without_subscriber_fires_on_dead() {
        let state = Arc::new(Mutex::new(test_state(true)));
        let (dead_tx, dead_rx) = mpsc::channel();

        let handle = DaemonPane::spawn_reader(
            state.clone(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(OutputGate::new()),
            Box::new(std::io::Cursor::new(Vec::new())),
            null_writer(),
            || false,
            ForegroundProbes {
                group: Box::new(|| None),
                remote: Box::new(|| None),
                agent: Box::new(|| None),
                cwd: Box::new(|| None),
                local_group: |_| Some(true),
            },
            Arc::new(DeathReporter::new(move || dead_tx.send(()).unwrap())),
        );
        handle.join().unwrap();

        assert!(!state.lock().unwrap().alive);
        assert!(dead_rx.try_recv().is_ok(), "unattached death → on_dead");
    }

    #[test]
    fn reader_eof_during_shutdown_is_silent() {
        let state = Arc::new(Mutex::new(test_state(true)));
        let dead = Arc::new(AtomicBool::new(false));
        let dead_flag = dead.clone();

        let handle = DaemonPane::spawn_reader(
            state.clone(),
            Arc::new(AtomicBool::new(true)),
            Arc::new(OutputGate::new()),
            Box::new(std::io::Cursor::new(Vec::new())),
            null_writer(),
            || false,
            ForegroundProbes {
                group: Box::new(|| None),
                remote: Box::new(|| None),
                agent: Box::new(|| None),
                cwd: Box::new(|| None),
                local_group: |_| Some(true),
            },
            Arc::new(DeathReporter::new(move || {
                dead_flag.store(true, Ordering::SeqCst)
            })),
        );
        handle.join().unwrap();

        assert!(!state.lock().unwrap().alive);
        assert!(!dead.load(Ordering::SeqCst));
    }

    #[test]
    fn death_reporter_notifies_once_across_racing_callers() {
        let state = Arc::new(Mutex::new(test_state(true)));
        let (sub_tx, sub_rx) = mpsc::channel();
        attach_subscriber(&mut state.lock().unwrap(), sub_tx);
        drain(&sub_rx);
        let shutting_down = AtomicBool::new(false);
        let calls = Arc::new(AtomicBool::new(false));
        let calls_flag = calls.clone();
        let death = DeathReporter::new(move || calls_flag.store(true, Ordering::SeqCst));

        death.report(&state, &shutting_down);
        death.report(&state, &shutting_down);

        assert!(!state.lock().unwrap().alive);
        assert!(matches!(
            sub_rx.try_recv(),
            Ok(DaemonMsg::Exited { code: None })
        ));
        assert!(
            sub_rx.try_recv().is_err(),
            "a second report must not re-notify"
        );
    }

    #[test]
    fn death_reporter_fires_on_dead_at_most_once() {
        let state = Arc::new(Mutex::new(test_state(true)));
        let shutting_down = AtomicBool::new(false);
        let (dead_tx, dead_rx) = mpsc::channel();
        let death = DeathReporter::new(move || dead_tx.send(()).unwrap());

        death.report(&state, &shutting_down);
        death.report(&state, &shutting_down);

        assert!(dead_rx.try_recv().is_ok(), "unattached death → on_dead");
        assert!(dead_rx.try_recv().is_err(), "on_dead must fire only once");
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

    #[test]
    fn apply_signals_updates_state() {
        let mut st = test_state(true);

        apply_signals(
            &mut st,
            SniffSignals {
                cwd: Some(PathBuf::from("/tmp/x")),
                ..SniffSignals::default()
            },
        );
        assert_eq!(st.cwd, Some(PathBuf::from("/tmp/x")));

        apply_signals(
            &mut st,
            SniffSignals {
                shell: vec![ShellState {
                    active: true,
                    at_prompt: true,
                    mark_at_prompt: true,
                    last_exit_code: Some(0),
                    command: None,
                }],
                ..SniffSignals::default()
            },
        );
        assert!(st.shell.active && st.shell.at_prompt);
        assert_eq!(st.shell.last_exit_code, Some(0));

        apply_signals(&mut st, SniffSignals::default());
        assert_eq!(st.cwd, Some(PathBuf::from("/tmp/x")));
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

    #[cfg(windows)]
    #[test]
    fn closing_a_pane_reaps_console_clients_but_preserves_detached_services() {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
        use windows_sys::Win32::System::Threading::{
            CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS, OpenProcess, PROCESS_SYNCHRONIZE,
            PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject,
        };

        const TEST: &str = "daemon::pane::tests::closing_a_pane_reaps_console_clients_but_preserves_detached_services";
        const DIRECTORY: &str = "LYREL_TEST_CONSOLE_OWNERSHIP";
        const ROLE: &str = "LYREL_TEST_CONSOLE_ROLE";
        let executable = std::env::current_exe().unwrap();
        if let Some(directory) = std::env::var_os(DIRECTORY) {
            if std::env::var_os(ROLE).is_some() {
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
            let directory = PathBuf::from(directory);
            for role in ["attached", "detached"] {
                let child = Command::new(&executable)
                    .args(["--exact", TEST, "--nocapture"])
                    .env(ROLE, role)
                    .creation_flags(if role == "detached" {
                        DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
                    } else {
                        0
                    })
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap();
                std::fs::write(directory.join(role), child.id().to_string()).unwrap();
            }
            while !directory.join("exit").exists() {
                std::thread::sleep(Duration::from_millis(20));
            }
            return;
        }

        struct Clients(Vec<OwnedHandle>);
        impl Drop for Clients {
            fn drop(&mut self) {
                for handle in &self.0 {
                    unsafe {
                        TerminateProcess(handle.as_raw_handle(), 1);
                        WaitForSingleObject(handle.as_raw_handle(), 5_000);
                    }
                }
            }
        }
        for natural_exit in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let script = directory.path().join("start.ps1");
            std::fs::write(&script, format!(
                "$env:{DIRECTORY}='{}'\n& '{}' --exact {TEST} --nocapture\nexit $LASTEXITCODE\n",
                directory.path().display().to_string().replace('\'', "''"),
                executable.display().to_string().replace('\'', "''")
            )).unwrap();
            let pane = DaemonPane::spawn(
                u64::MAX - 10,
                Some(directory.path().into()),
                ws(80, 24),
                Some(ShellSpec {
                    program: "powershell.exe".into(),
                    args: vec![
                        "-NoProfile".into(),
                        "-NonInteractive".into(),
                        "-File".into(),
                        script.to_string_lossy().into_owned(),
                    ],
                    args_are_lyrel_defaults: false,
                }),
                None,
                None,
                None,
                false,
                || {},
            )
            .unwrap();
            let mut clients = Clients(Vec::new());
            for role in ["attached", "detached"] {
                let path = directory.path().join(role);
                let deadline = std::time::Instant::now() + Duration::from_secs(20);
                let pid = loop {
                    if let Some(pid) = std::fs::read_to_string(&path)
                        .ok()
                        .and_then(|text| text.parse().ok())
                    {
                        break pid;
                    }
                    assert!(std::time::Instant::now() < deadline, "{role} did not start");
                    std::thread::sleep(Duration::from_millis(20));
                };
                let handle =
                    unsafe { OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, 0, pid) };
                assert!(!handle.is_null(), "open the actual {role} process");
                clients
                    .0
                    .push(unsafe { OwnedHandle::from_raw_handle(handle) });
            }
            if natural_exit {
                let (tx, rx) = mpsc::channel();
                pane.attach(tx);
                std::fs::write(directory.path().join("exit"), b"exit").unwrap();
                loop {
                    if matches!(
                        rx.recv_timeout(Duration::from_secs(20)).unwrap(),
                        DaemonMsg::Exited { .. }
                    ) {
                        break;
                    }
                }
            } else {
                pane.kill();
            }
            drop(pane);
            assert_eq!(
                unsafe { WaitForSingleObject(clients.0[0].as_raw_handle(), 10_000) },
                WAIT_OBJECT_0,
                "attached console client survived pane close (natural_exit={natural_exit})"
            );
            assert_eq!(
                unsafe { WaitForSingleObject(clients.0[1].as_raw_handle(), 0) },
                WAIT_TIMEOUT,
                "detached service was killed by pane close (natural_exit={natural_exit})"
            );
        }
    }
}
