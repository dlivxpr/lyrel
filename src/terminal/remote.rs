use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use rio_vt::ansi::CursorShape;
use rio_vt::clipboard::ClipboardType;
use rio_vt::config::colors::ColorRgb;
use rio_vt::crosswords::Crosswords;
use rio_vt::crosswords::Mode;
use rio_vt::crosswords::grid::Dimensions as _;
use rio_vt::event::{EventListener, RioEvent, WindowId};
use rio_vt::performer::handler::Handler as _;

use parking_lot::FairMutex;

use std::collections::VecDeque;

use crate::core::cli_agent::{AgentSessionState, AgentStatus, CLIAgent};
use crate::core::config::CursorStyle as ConfigCursorStyle;
use crate::core::osc::{Note, NoteBudget};
use crate::daemon::protocol::{
    AuthPromptKind, AuthResponse, ClientMsg, DaemonMsg, LoopbackForward, LoopbackForwardRequest,
    ManagedForward, NativeSshSpec, PaneProcs, RemoteContext, RemoteKind, RestoreFrom, SftpEntry,
    SftpJobProgress, SftpOp, SftpOpResult, SftpTransferSpec, ShellSpec, SshForwardRule, SshPhase,
    WinSize, WorkspaceOp, WorkspaceRequest,
};
use crate::daemon::transport::Stream;
use gpui_kit::EntityId;

use super::size::TermSize;

pub(crate) mod handshake;
mod connection;
mod reader;
mod writer;
#[cfg(test)]
mod test_support;

use connection::PaneConnection;
use handshake::{connect, connect_routed};
use writer::LinkSender;

/// What a pane can ask its window to do, whether the request came from the
/// emulator mid-parse (translated from `RioEvent` by [`EventProxy`]) or from
/// the pane's own plumbing (`Wakeup` after each applied frame, `Exit` on a
/// dead link, title bookkeeping the reader decides).
///
/// rio-vt events that have no meaning here are dropped in the translation:
/// desktop notifications (owned by the daemon's OSC sniffer, showing them
/// from both sides would double every toast), progress reports, cwd pokes,
/// palette-change and graphics bookkeeping, mouse-cursor/blink dirty flags,
/// and Rio's own window-management verbs.
#[derive(Clone)]
pub enum PaneEvent {
    /// New terminal content available.
    Wakeup,
    /// Window title change.
    Title(String),
    /// Reset to the default window title.
    ResetTitle,
    /// Write some text to the PTY.
    PtyWrite(String),
    /// Shutdown request.
    Exit,
    /// Request to store a text string in the clipboard.
    ClipboardStore(ClipboardType, String),
    /// Request to write the contents of the clipboard to the PTY; the
    /// formatter transforms the clipboard text into the reply's escape form.
    ClipboardLoad(
        ClipboardType,
        Arc<dyn Fn(&str) -> String + Sync + Send + 'static>,
    ),
    /// Request to write an RGB color's value to the PTY, formatter attached.
    ColorRequest(
        usize,
        Arc<dyn Fn(ColorRgb) -> String + Sync + Send + 'static>,
    ),
    /// Terminal bell ring.
    Bell,
    /// Request to write the text area size, formatter attached.
    TextAreaSizeRequest(Arc<dyn Fn(rio_vt::event::WindowSize) -> String + Sync + Send + 'static>),
}

impl std::fmt::Debug for PaneEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PaneEvent::Wakeup => write!(f, "Wakeup"),
            PaneEvent::Title(title) => write!(f, "Title({title})"),
            PaneEvent::ResetTitle => write!(f, "ResetTitle"),
            PaneEvent::PtyWrite(text) => write!(f, "PtyWrite({text})"),
            PaneEvent::Exit => write!(f, "Exit"),
            PaneEvent::ClipboardStore(ty, text) => write!(f, "ClipboardStore({ty:?}, {text})"),
            PaneEvent::ClipboardLoad(ty, _) => write!(f, "ClipboardLoad({ty:?})"),
            PaneEvent::ColorRequest(index, _) => write!(f, "ColorRequest({index})"),
            PaneEvent::Bell => write!(f, "Bell"),
            PaneEvent::TextAreaSizeRequest(_) => write!(f, "TextAreaSizeRequest"),
        }
    }
}

#[derive(Clone)]
pub struct EventProxy {
    tx: smol::channel::Sender<PaneEvent>,
    parse_state: Arc<ParseState>,
}

struct ParseState {
    replaying: AtomicBool,
    history_limit: AtomicUsize,
    /// ConPTY's transient visible caret, never a VT parsing position.
    paint_cursor: Mutex<Option<rio_vt::crosswords::pos::Pos>>,
}

impl EventProxy {
    /// A pane-local event (not one of the emulator's).
    fn send(&self, event: PaneEvent) {
        let _ = self.tx.try_send(event);
    }
}

impl EventListener for EventProxy {
    fn send_event(&self, event: RioEvent, _window_id: WindowId) {
        let event = match event {
            RioEvent::PtyWrite(_, text) => PaneEvent::PtyWrite(text),
            // Rio has no payload-less title reset: clearing (RIS, or popping
            // an empty title into place) arrives as an empty title, which is
            // what "show the default" has always meant here.
            RioEvent::Title(_, title) if title.is_empty() => PaneEvent::ResetTitle,
            RioEvent::Title(_, title) => PaneEvent::Title(title),
            RioEvent::ResetTitle => PaneEvent::ResetTitle,
            RioEvent::Bell(_) => PaneEvent::Bell,
            RioEvent::ClipboardStore(ty, text) => PaneEvent::ClipboardStore(ty, text),
            RioEvent::ClipboardLoad(_, ty, fmt) => PaneEvent::ClipboardLoad(ty, fmt),
            RioEvent::ColorRequest(_, index, fmt) => PaneEvent::ColorRequest(index, fmt),
            RioEvent::TextAreaSizeRequest(_, fmt) => PaneEvent::TextAreaSizeRequest(fmt),
            // `Crosswords::exit()` reports this way; it is the pane ending.
            RioEvent::CloseTerminal(_) => PaneEvent::Exit,
            // Dropped on purpose, each for its own reason:
            //
            // `DesktopNotification` (OSC 9/777) — desktop notifications are
            // owned by the daemon-side OSC sniffer; forwarding Rio's copy
            // would show every notification twice.
            //
            // `ProgressReport` (OSC 9;4) — not a capability this product
            // advertises; the parse completes and the bar goes nowhere.
            //
            // `RenderRoute`/`TerminalDamaged`/`Render` — per-damage paint
            // hints. The reader emits one `Wakeup` per parsed batch, which
            // is the pane's only repaint signal; per-cell damage traffic
            // would flood the channel at output rate.
            //
            // `MouseCursorDirty` — hover and link cursor state are the
            // view's own; repaints arrive with the batch `Wakeup`.
            //
            // `ColorChange` — an echo of an OSC 4 set landing in
            // `term.colors`. The paint path reads this product's own
            // palette by design (queries are answered from it too), so
            // there is no consumer.
            //
            // `CurrentDirectoryChanged` — the daemon's sniffer is the
            // authoritative cwd source; this echoes the same OSC 7.
            //
            // `CursorBlinkingChange` — blink is a config-owned paint
            // property; the previous emulator's equivalent event had no
            // consumer here either.
            //
            // `UpdateGraphics`, `GlyphProtocolInstalled`,
            // `GlyphProtocolQuery` — image and glyph-protocol traffic is
            // owned out-of-band (the daemon lifts Kitty transmissions; the
            // grid stays text-only), so parsed placements have no renderer.
            //
            // Everything else is rioterm-the-application's own traffic
            // (window/tab/font management), which a headless `Crosswords`
            // never emits.
            RioEvent::DesktopNotification { .. }
            | RioEvent::ProgressReport(_)
            | RioEvent::RenderRoute(_)
            | RioEvent::TerminalDamaged(_)
            | RioEvent::Render
            | RioEvent::MouseCursorDirty
            | RioEvent::ColorChange(..)
            | RioEvent::CurrentDirectoryChanged(_)
            | RioEvent::CursorBlinkingChange
            | RioEvent::CursorBlinkingChangeOnRoute(_)
            | RioEvent::UpdateGraphics { .. }
            | RioEvent::GlyphProtocolQuery { .. } => return,
            _ => return,
        };
        if self.parse_state.replaying.load(Ordering::Relaxed)
            && matches!(
                event,
                PaneEvent::PtyWrite(_)
                    | PaneEvent::ColorRequest(..)
                    | PaneEvent::TextAreaSizeRequest(_)
                    | PaneEvent::ClipboardStore(..)
                    | PaneEvent::ClipboardLoad(..)
                    | PaneEvent::Bell
            )
        {
            return;
        }
        let _ = self.tx.try_send(event);
    }
}

#[derive(Default, Clone, Copy)]
struct ShellState {
    active: bool,
    at_prompt: bool,
    last_exit: Option<i32>,
    seq: u64,
    cycle: u64,
}

/// The pane's agent status as the client last heard it, plus how it heard it.
///
/// A reattach — the app restarting onto panes the daemon kept alive, or a
/// dropped link coming back — has the daemon replay the pane's *stored* status
/// as an ordinary `AgentStatus` frame ([`crate::daemon`]'s `replay_state`).
/// Nothing on the wire distinguishes it from a live transition, and a client
/// that reads it as one concludes that every restored agent finished its turn
/// in the instant the window opened. `replayed` is that distinction, kept in
/// the same lock as the value it describes so a reader can never observe the
/// status without also learning where it came from.
#[derive(Default)]
struct AgentSlot {
    state: Option<AgentSessionState>,
    /// `state` arrived as an attach replay and no one has adopted it yet.
    /// Cleared by the first taker — the view adopts it as a baseline rather
    /// than as an edge.
    replayed: bool,
}

struct ReaderSignals {
    cwd: Arc<Mutex<Option<PathBuf>>>,
    shell: Arc<Mutex<ShellState>>,
    remote: Arc<Mutex<Option<RemoteContext>>>,
    agent: Arc<Mutex<Option<CLIAgent>>>,
    agent_session: Arc<Mutex<AgentSlot>>,
    /// Whether this link still owes us the attach replay. The reader keeps it
    /// as a plain local: a link's replay is a property of that link's stream
    /// position, and nothing outside the reader thread ever needs to read it.
    awaiting_replay: bool,
    exited: Arc<AtomicBool>,
    child_exited: Arc<AtomicBool>,
    zle_reading: Arc<AtomicBool>,
    shell_vi_mode: Arc<AtomicBool>,
    running_command: Arc<Mutex<String>>,
    auth: Arc<Mutex<VecDeque<(u64, AuthPromptKind)>>>,
    phase: Arc<Mutex<Option<SshPhase>>>,
    /// Kitty-graphics images the daemon lifted out of the stream (issue #213),
    /// anchored to the grid for the paint path to blit. The reader applies
    /// daemon-owned resource/placement updates in stream order.
    images: crate::terminal::images::ImageStore,
    clipboard_writes: Arc<Mutex<VecDeque<lyrel_core::core::clipboard::ClipboardWrite>>>,
    osc_notes: OscNotes,
    clipboard_write_busy: Arc<AtomicBool>,
    /// Whether this pane's pty is one a conhost renders into, and so whether
    /// the reader puts back the cursor a repaint parked. Decided per pane from
    /// its [`PtySource`], and shared rather than copied because the reader can
    /// learn better mid-stream — see the `RemoteContext` arm.
    local_conpty: Arc<AtomicBool>,
    /// Whether the program in the pane switched on DEC mode 2031.
    color_scheme_updates: Arc<AtomicBool>,
}

/// What kind of pty is at the far end of a pane's link, which is what decides
/// whether a conhost stands between the application and us.
///
/// The distinction is not the platform this client was built for. A Windows
/// client's panes are a mix: a local shell — or `wsl.exe`, or an `ssh` client,
/// which are ordinary programs inside the same ConPTY — is rendered by conhost,
/// while a pane on a remote `lyrel-server` (Linux or macOS only, see
/// [`lyrel_core::daemon::install::asset::asset_for_uname`]) or on a native-SSH
/// channel is a raw unix pty whose bytes reach us untouched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtySource {
    /// A pty this machine's Windows daemon opened: a ConPTY, with conhost
    /// painting frames into it.
    LocalConpty,
    /// A pty nothing repaints on our behalf — every pty on unix, and every pty
    /// reached over a link.
    Raw,
}

impl PtySource {
    /// The source a pane spawned or attached on `route` reads from. The
    /// platform enters here and nowhere else: a local route means a pty this
    /// machine opened, and only on Windows is that a ConPTY.
    ///
    /// `Unroutable` is a route that could not be resolved, so it never gets a
    /// pty at all; answering as if it were local costs nothing and keeps the
    /// match total.
    fn for_route(route: &PaneRoute) -> PtySource {
        match route {
            PaneRoute::Local | PaneRoute::Unroutable(_) if cfg!(windows) => PtySource::LocalConpty,
            _ => PtySource::Raw,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PaneWorkspace {
    pub workspace: crate::core::session::WorkspaceId,
    pub target: crate::core::session::RemoteTarget,
    pub spec: Option<Box<NativeSshSpec>>,
    /// The name the workspace answers to — its own label, or its machine's
    /// when it has none. A pane keeps answering to this name when its link
    /// dies (#438 gave SSH panes their host here; without this, a workspace
    /// pane fell back to the bare app name and the tab read "lyrel —
    /// disconnected").
    pub label: Option<String>,
    /// Whether the daemon serving this workspace's panes echoes a `Size` frame
    /// when it applies a resize — read off the host's control hello by whoever
    /// built this value, because this module may not ask the network itself.
    /// `false` whenever the answer is unknown (link down, server too old to
    /// advertise), which keeps the safe reflow-at-request behavior.
    pub resize_echo: bool,
}

impl PaneWorkspace {
    pub fn shares_localhost(&self) -> bool {
        matches!(self.target, crate::core::session::RemoteTarget::Wsl { .. })
    }

    pub fn route_header(&self) -> anyhow::Result<crate::daemon::router::RouteHeader> {
        use crate::core::session::RemoteTarget;
        use crate::daemon::router::RouteHeader;
        let header = match (&self.target, &self.spec) {
            (RemoteTarget::Wsl { distro }, _) => RouteHeader::wsl(distro.clone()),
            (RemoteTarget::LocalStdio { program, args }, _) => {
                let mut argv: Vec<&str> = args.iter().map(String::as_str).collect();
                if !argv.contains(&"--pane") {
                    argv.push("--pane");
                }
                RouteHeader::local_stdio(program.clone(), &argv)
            }
            (_, Some(spec)) => RouteHeader::ssh((**spec).clone()),
            (_, None) => {
                // Deliberately not the target: a `Profile` spells itself as
                // its config UUID in `Display` and in `Debug` alike, and a
                // deleted profile is exactly what empties `spec` here. This
                // sentence is not only logged — `land_pane` hands it to the
                // pending pane, which prints the reason verbatim under
                // "could not reach {machine}", so the UUID reached the screen
                // (#485). The workspace's own name is what every other
                // surface calls this thing.
                return Err(match self.label.as_deref() {
                    Some(label) => anyhow::anyhow!(
                        "{label} has no SSH connection details, so its panes cannot be routed"
                    ),
                    None => anyhow::anyhow!(
                        "this workspace has no SSH connection details, so its panes \
                         cannot be routed"
                    ),
                });
            }
        };
        Ok(header.for_pane())
    }
}

#[derive(Clone, Debug, Default)]
pub enum PaneRoute {
    #[default]
    Local,
    Remote {
        header: Box<crate::daemon::router::RouteHeader>,
        /// Whether the daemon at the far end of this route echoes a `Size`
        /// frame when it applies a resize, per its host's control hello (see
        /// [`PaneWorkspace::resize_echo`]). Riding on the route puts the
        /// answer everywhere a route is assigned — spawn, attach, relink —
        /// and a relink builds its route fresh, so a reconnect re-answers it.
        resize_echo: bool,
    },
    Unroutable(String),
}

impl PaneRoute {
    pub fn for_workspace(workspace: Option<&PaneWorkspace>) -> PaneRoute {
        match workspace {
            None => PaneRoute::Local,
            Some(ws) => match ws.route_header() {
                Ok(header) => PaneRoute::Remote {
                    header: Box::new(header),
                    resize_echo: ws.resize_echo,
                },
                Err(e) => PaneRoute::Unroutable(e.to_string()),
            },
        }
    }

    pub fn header(&self) -> Option<&crate::daemon::router::RouteHeader> {
        match self {
            PaneRoute::Remote { header, .. } => Some(header),
            PaneRoute::Local | PaneRoute::Unroutable(_) => None,
        }
    }

    pub fn is_local(&self) -> bool {
        matches!(self, PaneRoute::Local)
    }

    fn allow_remote_clipboard_write(&self) -> bool {
        match self {
            PaneRoute::Remote { header, .. } => match &header.target {
                crate::daemon::router::RouteTarget::Ssh(spec) => spec.remote_clipboard_write,
                _ => false,
            },
            PaneRoute::Local | PaneRoute::Unroutable(_) => false,
        }
    }
}


#[derive(Clone)]
pub(super) struct GraphicsViewportReporter {
    term: std::sync::Weak<FairMutex<Crosswords<EventProxy>>>,
    images: crate::terminal::images::ImageStore,
    displayed: Arc<AtomicBool>,
    link: Arc<
        Mutex<(
            LinkSender,
            Option<crate::daemon::protocol::GraphicsViewport>,
        )>,
    >,
    retry: std::sync::mpsc::SyncSender<()>,
}

impl GraphicsViewportReporter {
    fn new(
        term: &Arc<FairMutex<Crosswords<EventProxy>>>,
        images: crate::terminal::images::ImageStore,
        sender: LinkSender,
    ) -> std::io::Result<Self> {
        let (retry, pending) = std::sync::mpsc::sync_channel(1);
        let reporter = Self {
            term: Arc::downgrade(term),
            images,
            displayed: Arc::new(AtomicBool::new(true)),
            link: Arc::new(Mutex::new((sender, None))),
            retry,
        };
        let term = reporter.term.clone();
        let images = reporter.images.clone();
        let displayed = reporter.displayed.clone();
        let link = reporter.link.clone();
        // One bounded wake, not queued viewport snapshots. The worker may wait
        // for a PTY reader or UI grid guard, then samples the latest state under
        // grid -> store -> link ordering. It owns no retry sender, so dropping
        // the last reporter closes the channel and retires the worker.
        std::thread::Builder::new()
            .name("lyrel-graphics-viewport".into())
            .spawn(move || {
                while pending.recv().is_ok() {
                    let Some(term) = term.upgrade() else { return };
                    let term = term.lock();
                    Self::publish(&term, &images, &displayed, &link);
                }
            })?;
        Ok(reporter)
    }

    pub(super) fn displayed(&self) -> Arc<AtomicBool> {
        self.displayed.clone()
    }

    pub(super) fn set_displayed(&self, on: bool) {
        {
            let mut link = self.link.lock().unwrap();
            if self.displayed.swap(on, Ordering::Relaxed) == on {
                return;
            }
            if !on && let Some(mut viewport) = link.1 {
                viewport.rows = 0;
                link.0.send(ClientMsg::GraphicsViewport(viewport));
                link.1 = Some(viewport);
            }
        }
        self.report();
    }

    pub(super) fn report(&self) {
        let Some(term) = self.term.upgrade() else {
            return;
        };
        let Some(term) = term.try_lock() else {
            // A scroll already changed the grid. Never discard its final
            // desired viewport just because the reader currently owns it.
            let _ = self.retry.try_send(());
            return;
        };
        Self::publish(&term, &self.images, &self.displayed, &self.link);
    }

    fn relink(&self, sender: LinkSender) {
        *self.link.lock().unwrap() = (sender, None);
        // The successor may already have established its Screen. Publish on
        // the new writer even when the pane remains hidden after the swap.
        self.report();
    }

    fn publish(
        term: &Crosswords<EventProxy>,
        images: &crate::terminal::images::ImageStore,
        displayed: &AtomicBool,
        link: &Mutex<(
            LinkSender,
            Option<crate::daemon::protocol::GraphicsViewport>,
        )>,
    ) {
        let origin = term.grid.lines_evicted() as i64 + term.grid.history_size() as i64;
        let Some(mut viewport) = images.viewport(
            origin,
            term.display_offset(),
            term.screen_lines(),
            term.columns(),
            true,
        ) else {
            return;
        };
        // Sampling and enqueueing stay inside the grid guard. A queued retry
        // cannot publish an old snapshot after a newer synchronous report.
        let mut link = link.lock().unwrap();
        // Visibility changes also hold this lock. A report sampled before a
        // hide must not restore visible rows after its withdrawal was sent.
        if !displayed.load(Ordering::Relaxed) {
            viewport.rows = 0;
        }
        if link.1 != Some(viewport) {
            link.0.send(ClientMsg::GraphicsViewport(viewport));
            link.1 = Some(viewport);
        }
    }
}

pub struct RemoteTerminal {
    pub term: Arc<FairMutex<Crosswords<EventProxy>>>,
    pub events: smol::channel::Receiver<PaneEvent>,
    pub palette: [ColorRgb; 256],
    pub exited: bool,
    /// This link reattached to a pane that was already running rather than
    /// spawning a fresh one, so whatever it reports first — an agent in the
    /// foreground included — predates this client.
    reattached: bool,
    cwd: Arc<Mutex<Option<PathBuf>>>,
    shell_state: Arc<Mutex<ShellState>>,
    remote_context: Arc<Mutex<Option<RemoteContext>>>,
    exited_flag: Arc<AtomicBool>,
    child_exited: Arc<AtomicBool>,
    zle_reading: Arc<AtomicBool>,
    shell_vi_mode: Arc<AtomicBool>,
    /// The line the shell said it is running, from the OSC 133;C mark, still
    /// percent-escaped as the integration sent it. Empty while at a prompt.
    ///
    /// This is the only place the *text* of a running command exists on the
    /// client: the frame the daemon sends says a command runs, not which one.
    /// The close confirmation has to name what it is about to end.
    running_command: Arc<Mutex<String>>,
    auth_prompts: Arc<Mutex<VecDeque<(u64, AuthPromptKind)>>>,
    ssh_phase: Arc<Mutex<Option<SshPhase>>>,
    ssh_endpoint: Option<(String, u16)>,
    /// The account the SSH connection authenticates as. `ssh_endpoint` is what
    /// the disconnect strip and the forward sheet need; the keychain files a
    /// password under the user as well, so the auth sheet needs this too.
    ssh_user: Option<String>,
    auto_supplied_password: bool,
    agent: Arc<Mutex<Option<CLIAgent>>>,
    agent_session: Arc<Mutex<AgentSlot>>,
    /// Derived decode/render cache for the daemon's authoritative graphics state.
    /// The reader applies ordered out-of-band `Graphics` updates and captures
    /// client-grid placement anchors; protocol identities and resource lifetimes
    /// remain owned by the living daemon pane.
    images: crate::terminal::images::ImageStore,
    clipboard_writes: Arc<Mutex<VecDeque<lyrel_core::core::clipboard::ClipboardWrite>>>,
    osc_notes: OscNotes,
    /// On the pane rather than its view so it survives a relink; only the
    /// view's poll touches it.
    note_budget: Mutex<NoteBudget>,
    clipboard_write_busy: Arc<AtomicBool>,
    proxy: EventProxy,
    connection: PaneConnection,
    /// Whether this pane's pty is a ConPTY, for input encoding and repairing
    /// the cursor a repaint parks. Held here so a relink hands the same answer
    /// to the reader it starts: a pane's pty does not change kind when the link
    /// to it is rebuilt, and the route a relink carries cannot tell a
    /// native-SSH pane from a local shell.
    local_conpty: Arc<AtomicBool>,
    /// Whether the program in the pane switched on DEC mode 2031.
    color_scheme_updates: Arc<AtomicBool>,
}

/// Owned by one pending GUI slot, never shared with another spawn attempt.
#[derive(Clone, Default)]
pub(crate) struct SpawnCancellation(Arc<AtomicBool>);

impl SpawnCancellation {
    pub(crate) fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    fn cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}


impl RemoteTerminal {

    /// Remember the directory the daemon was asked to spawn in, so everything
    /// that keys off a pane's cwd — the sidebar's repo grouping above all —
    /// has an answer before the shell gets far enough to report its own via
    /// OSC 7. The reader thread is already running, so a report that beat us
    /// here wins: it describes where the shell actually landed, which is not
    /// always where we asked (a missing directory sends the daemon home, an
    /// rc file may `cd` on its own).
    pub(crate) fn seed_cwd(&self, cwd: Option<PathBuf>) {
        let Some(cwd) = cwd else { return };
        if let Ok(mut guard) = self.cwd.lock() {
            guard.get_or_insert(cwd);
        }
    }


    pub fn adopt_relink(
        &mut self,
        stream: Stream,
        buffered: Vec<u8>,
        route: &PaneRoute,
        size: TermSize,
        cell_w: u16,
        cell_h: u16,
    ) -> anyhow::Result<()> {
        let signals = self.reader_signals(false);
        self.connection.adopt_relink(
            stream, &self.term, &self.proxy, &self.events, &mut self.exited, signals,
            buffered, route,
        )?;
        self.resize(size, cell_w, cell_h);
        Ok(())
    }

    fn reader_signals(&self, awaiting_replay: bool) -> ReaderSignals {
        ReaderSignals {
            cwd: self.cwd.clone(),
            shell: self.shell_state.clone(),
            remote: self.remote_context.clone(),
            agent: self.agent.clone(),
            agent_session: self.agent_session.clone(),
            awaiting_replay,
            exited: self.exited_flag.clone(),
            child_exited: self.child_exited.clone(),
            zle_reading: self.zle_reading.clone(),
            shell_vi_mode: self.shell_vi_mode.clone(),
            running_command: self.running_command.clone(),
            auth: self.auth_prompts.clone(),
            phase: self.ssh_phase.clone(),
            images: self.images.clone(),
            clipboard_writes: self.clipboard_writes.clone(),
            osc_notes: self.osc_notes.clone(),
            clipboard_write_busy: self.clipboard_write_busy.clone(),
            local_conpty: self.local_conpty.clone(),
            color_scheme_updates: self.color_scheme_updates.clone(),
        }
    }

    /// A pane the client *reattached* to rather than spawned — the shape the
    /// app restores last session's tabs in, where the head of the stream is the
    /// daemon replaying state the pane already had.
    #[cfg(test)]
    pub(super) fn from_stream_reattached(stream: Stream, size: TermSize) -> anyhow::Result<Self> {
        Self::from_stream_parts(
            stream,
            size,
            Vec::new(),
            PtySource::for_route(&PaneRoute::Local),
            true,
            crate::terminal::images::ImageStore::new(),
        )
    }

    /// A test pane backed by a pty of this machine's own.
    #[cfg(test)]
    pub(super) fn from_stream(stream: Stream, size: TermSize) -> anyhow::Result<Self> {
        Self::from_stream_with(
            stream,
            size,
            Vec::new(),
            PtySource::for_route(&PaneRoute::Local),
        )
    }

    #[cfg(test)]
    pub(super) fn from_stream_with(
        stream: Stream,
        size: TermSize,
        buffered: Vec<u8>,
        pty: PtySource,
    ) -> anyhow::Result<Self> {
        Self::from_stream_parts(
            stream,
            size,
            buffered,
            pty,
            false,
            crate::terminal::images::ImageStore::new(),
        )
    }

    /// `awaiting_replay` says this link is an attach rather than a spawn, and
    /// so that the frames at the head of its stream describe a pane that was
    /// already running — see [`AgentSlot`]. It has to be decided here rather
    /// than set on the returned terminal: the reader starts inside this
    /// function, and against a daemon that answers promptly the replay can be
    /// parsed before the caller gets its value back.
    fn from_stream_parts(
        stream: Stream,
        size: TermSize,
        buffered: Vec<u8>,
        pty: PtySource,
        awaiting_replay: bool,
        images: crate::terminal::images::ImageStore,
    ) -> anyhow::Result<Self> {

        let user_config = crate::core::config::Config::load();
        let (tx, rx) = smol::channel::unbounded();
        let proxy = EventProxy {
            tx,
            parse_state: Arc::new(ParseState {
                replaying: AtomicBool::new(false),
                history_limit: AtomicUsize::new(user_config.scrollback_limit),
                paint_cursor: Mutex::new(None),
            }),
        };

        let term = build_term(size, proxy.clone(), &user_config);
        let term = Arc::new(FairMutex::new(term));

        let cwd: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
        let shell_state: Arc<Mutex<ShellState>> = Arc::new(Mutex::new(ShellState::default()));
        let remote_context: Arc<Mutex<Option<RemoteContext>>> = Arc::new(Mutex::new(None));
        let agent: Arc<Mutex<Option<CLIAgent>>> = Arc::new(Mutex::new(None));
        let agent_session: Arc<Mutex<AgentSlot>> = Arc::new(Mutex::new(AgentSlot::default()));
        let exited_flag = Arc::new(AtomicBool::new(false));
        let child_exited = Arc::new(AtomicBool::new(false));
        let zle_reading = Arc::new(AtomicBool::new(false));
        let shell_vi_mode = Arc::new(AtomicBool::new(false));
        let running_command: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        let auth_prompts: Arc<Mutex<VecDeque<(u64, AuthPromptKind)>>> =
            Arc::new(Mutex::new(VecDeque::new()));
        let ssh_phase: Arc<Mutex<Option<SshPhase>>> = Arc::new(Mutex::new(None));
        let clipboard_writes = Arc::new(Mutex::new(VecDeque::new()));
        let osc_notes = OscNotes::default();
        let clipboard_write_busy = Arc::new(AtomicBool::new(false));

        let local_conpty = Arc::new(AtomicBool::new(pty == PtySource::LocalConpty));
        let color_scheme_updates = Arc::new(AtomicBool::new(false));
        let connection = PaneConnection::start(
            stream,
            buffered,
            size,
            &term,
            &proxy,
            ReaderSignals {
                cwd: cwd.clone(),
                shell: shell_state.clone(),
                remote: remote_context.clone(),
                agent: agent.clone(),
                agent_session: agent_session.clone(),
                awaiting_replay,
                exited: exited_flag.clone(),
                child_exited: child_exited.clone(),
                zle_reading: zle_reading.clone(),
                shell_vi_mode: shell_vi_mode.clone(),
                running_command: running_command.clone(),
                auth: auth_prompts.clone(),
                phase: ssh_phase.clone(),
                images: images.clone(),
                clipboard_writes: clipboard_writes.clone(),
                osc_notes: osc_notes.clone(),
                clipboard_write_busy: clipboard_write_busy.clone(),
                local_conpty: local_conpty.clone(),
                color_scheme_updates: color_scheme_updates.clone(),
            },
        )?;

        Ok(Self {
            term,
            events: rx,
            palette: super::palette::build(),
            exited: false,
            reattached: awaiting_replay,
            cwd,
            shell_state,
            remote_context,
            exited_flag,
            child_exited,
            zle_reading,
            shell_vi_mode,
            running_command,
            auth_prompts,
            ssh_phase,
            ssh_endpoint: None,
            ssh_user: None,
            auto_supplied_password: false,
            agent,
            agent_session,
            images,
            clipboard_writes,
            osc_notes,
            note_budget: Mutex::default(),
            clipboard_write_busy,
            proxy,
            connection,
            local_conpty,
            color_scheme_updates,
        })
    }

    pub(crate) fn claim_spawn(&mut self) {
        self.connection.claim_spawn();
    }

    pub fn detach_link(&mut self) {
        self.connection.detach(&self.images);
        self.poll_exited();
    }

    fn stop_reader(&mut self) {
        self.connection.retire(&self.images);
    }

    pub fn apply_user_config(&self, user_config: &crate::core::config::Config) {
        let mut term = self.term.lock();
        self.proxy
            .parse_state
            .history_limit
            .store(user_config.scrollback_limit, Ordering::Relaxed);
        apply_config(&mut term, user_config);
    }

    pub fn poll_exited(&mut self) {
        if self.exited_flag.load(Ordering::SeqCst) {
            self.exited = true;
        }
    }

    pub fn child_exited(&self) -> bool {
        self.child_exited.load(Ordering::SeqCst)
    }

    pub(super) fn is_local_conpty(&self) -> bool {
        self.local_conpty.load(Ordering::Relaxed)
    }

    pub(super) fn paint_cursor_position(&self) -> Option<rio_vt::crosswords::pos::Pos> {
        if self.is_local_conpty() {
            *self.proxy.parse_state.paint_cursor.lock().unwrap()
        } else {
            None
        }
    }

    pub(super) fn color_scheme_updates(&self) -> bool {
        self.color_scheme_updates.load(Ordering::Relaxed)
    }

    /// Queues a keystroke — or a paste, or a mouse report — for the link.
    ///
    /// Callers are gpui event handlers on the UI thread, so this returns
    /// without touching the socket. A link that has stopped draining is a
    /// problem for the sender thread, not for the window.
    pub fn write<B: Into<Cow<'static, [u8]>>>(&self, bytes: B) {
        let bytes = bytes.into();
        if bytes.is_empty() {
            return;
        }
        self.connection.send(ClientMsg::Input(bytes.into_owned()));
    }


    /// Live panes clear only through the daemon's ordered output stream. Once
    /// the reader has observed exit/EOF, the retained presentation belongs to
    /// this client and must remain clearable without a functioning transport.
    /// Returns whether the caller should ask the live child to redraw.
    pub fn clear_presentation(&mut self, history: bool) -> bool {
        self.poll_exited();
        if !self.exited && !self.child_exited() {
            self.connection.send(ClientMsg::ClearPresentation { history });
            return true;
        }

        // A writer may notice failure before the reader. Retire it before
        // touching either cache, so buffered frames and late decode completions
        // cannot restore a presentation the user has just cleared.
        self.stop_reader();
        let mut term = self.term.lock();
        let rows = term.screen_lines();
        term.clear_screen(rio_vt::ansi::ClearMode::All);
        if history {
            term.clear_screen(rio_vt::ansi::ClearMode::Saved);
        }
        term.selection = None;
        term.scroll_display(rio_vt::crosswords::grid::Scroll::Bottom);
        let history_origin = term.grid.lines_evicted() as i64 + term.grid.history_size() as i64;
        self.images
            .clear_presentation(history, rows, history_origin);
        *self.proxy.parse_state.paint_cursor.lock().unwrap() = None;
        false
    }

    pub fn resize(&mut self, size: TermSize, cell_w: u16, cell_h: u16) {
        self.connection.resize(
            &self.term, &self.proxy, size, cell_w, cell_h,
            &self.local_conpty, &self.shell_state, &self.images,
        );
    }

    pub fn foreground_cwd(&self) -> Option<PathBuf> {
        self.cwd.lock().ok().and_then(|g| g.clone())
    }

    pub fn remote_context(&self) -> Option<RemoteContext> {
        self.remote_context.lock().ok().and_then(|g| g.clone())
    }

    /// See the field: an attach to a running pane, not a fresh spawn.
    pub fn reattached(&self) -> bool {
        self.reattached
    }

    pub fn at_prompt(&self) -> bool {
        self.shell_state
            .lock()
            .map(|s| s.active && s.at_prompt)
            .unwrap_or(false)
    }

    pub fn prompt_seq(&self) -> u64 {
        self.shell_state.lock().map(|s| s.seq).unwrap_or(0)
    }

    pub fn prompt_cycle(&self) -> u64 {
        self.shell_state.lock().map(|s| s.cycle).unwrap_or(0)
    }

    pub fn last_exit_code(&self) -> Option<i32> {
        self.shell_state.lock().ok().and_then(|s| s.last_exit)
    }

    pub fn shell_active(&self) -> bool {
        self.shell_state.lock().map(|s| s.active).unwrap_or(false)
    }

    pub fn foreground_agent(&self) -> Option<CLIAgent> {
        self.agent.lock().ok().and_then(|g| *g)
    }

    pub(super) fn viewport_reporter(&self) -> GraphicsViewportReporter {
        self.connection.viewport_reporter()
    }

    /// The kitty-graphics image store for this pane. Cheap handle clone — the
    /// store is an `Arc<Mutex<..>>` shared with the reader thread, which places
    /// and deletes images as out-of-band frames arrive from the daemon.
    pub fn images(&self) -> crate::terminal::images::ImageStore {
        self.images.clone()
    }

    /// Desktop notifications the program wrote (OSC 9, 99, 777), oldest
    /// first; the view decides whether each is shown.
    pub fn take_osc_notes(&self) -> Vec<Note> {
        self.osc_notes
            .lock()
            .map(|mut notes| notes.drain(..).collect())
            .unwrap_or_default()
    }

    /// Of `notes` about to be shown, the ones this pane's [`NoteBudget`] lets
    /// through, and whether notes it dropped earlier are due to be said.
    pub fn pace_osc_notes(
        &self,
        mut notes: Vec<Note>,
        now: std::time::Instant,
    ) -> (Vec<Note>, bool) {
        let Ok(mut budget) = self.note_budget.lock() else {
            return (notes, false);
        };
        let (show, dropped) = budget.admit(now, notes.len());
        notes.truncate(show);
        (notes, dropped)
    }

    pub fn pop_clipboard_write(&self) -> Option<lyrel_core::core::clipboard::ClipboardWrite> {
        self.clipboard_writes
            .lock()
            .ok()
            .and_then(|mut pending| pending.pop_front())
    }

    pub fn finish_clipboard_write(&self) {
        self.clipboard_write_busy.store(false, Ordering::Release);
    }

    pub fn agent_session(&self) -> Option<AgentSessionState> {
        self.agent_session.lock().ok().and_then(|g| g.state.clone())
    }

    /// The status the daemon replayed when this link attached, handed out once.
    ///
    /// `Some(status)` means what [`Self::agent_session`] reports right now is
    /// stored state from before this client existed — the caller should take it
    /// as its starting point, not as something that just happened. Answering
    /// only once is what keeps the very next live transition an edge again.
    pub fn take_replayed_agent_status(&self) -> Option<Option<AgentStatus>> {
        let mut guard = self.agent_session.lock().ok()?;
        if !guard.replayed {
            return None;
        }
        guard.replayed = false;
        Some(guard.state.as_ref().map(|s| s.status))
    }

    pub fn zle_reading(&self) -> bool {
        self.zle_reading.load(Ordering::Relaxed)
    }

    pub fn shell_vi_mode(&self) -> bool {
        self.shell_vi_mode.load(Ordering::Relaxed)
    }

    /// The line the shell reported running, still percent-escaped. Empty at a
    /// prompt, and empty for a shell whose integration marks the start of a
    /// command without naming it.
    pub fn running_command(&self) -> String {
        self.running_command
            .lock()
            .map(|c| c.clone())
            .unwrap_or_default()
    }

    pub fn size(&self) -> TermSize {
        self.connection.size()
    }

    pub fn list_panes_on(route: &PaneRoute) -> Vec<crate::daemon::protocol::PaneInfo> {
        Self::try_list_panes_on(route).unwrap_or_default()
    }

    pub fn try_list_panes_on(
        route: &PaneRoute,
    ) -> anyhow::Result<Vec<crate::daemon::protocol::PaneInfo>> {
        let mut stream = connect_routed(route)?;
        ClientMsg::List.encode(&mut stream)?;
        match DaemonMsg::read(&mut stream)? {
            DaemonMsg::PaneList(list) => Ok(list),
            other => Err(anyhow::anyhow!("unexpected reply to List: {other:?}")),
        }
    }

    pub fn kill_pane_on(route: &PaneRoute, pane_id: u64) {
        if let Ok(mut stream) = connect_routed(route) {
            let _ = ClientMsg::Kill { pane_id }.encode(&mut stream);
            let _ = stream.shutdown(std::net::Shutdown::Write);
        }
    }

    pub fn ensure_loopback_forward(
        pane_id: u64,
        remote_host: &str,
        remote_port: u16,
    ) -> anyhow::Result<LoopbackForward> {
        let mut stream = connect()?;
        ClientMsg::EnsureLoopbackForward(LoopbackForwardRequest {
            pane_id,
            remote_host: remote_host.to_string(),
            remote_port,
        })
        .encode(&mut stream)?;
        match DaemonMsg::read(&mut stream)? {
            DaemonMsg::LoopbackForward(forward) => Ok(forward),
            DaemonMsg::Error(msg) => Err(anyhow::anyhow!(msg)),
            other => Err(anyhow::anyhow!(
                "unexpected reply to EnsureLoopbackForward: {other:?}"
            )),
        }
    }


    pub fn take_auth_prompt(&self) -> Option<(u64, AuthPromptKind)> {
        self.auth_prompts
            .lock()
            .ok()
            .and_then(|mut q| q.pop_front())
    }

    pub fn take_auth_banner(&self) -> Option<String> {
        let mut q = self.auth_prompts.lock().ok()?;
        if matches!(q.front(), Some((_, AuthPromptKind::Banner { .. }))) {
            if let Some((_, AuthPromptKind::Banner { text })) = q.pop_front() {
                return Some(text);
            }
        }
        None
    }

    pub fn has_pending_auth(&self) -> bool {
        self.auth_prompts
            .lock()
            .map(|q| !q.is_empty())
            .unwrap_or(false)
    }

    pub fn ssh_phase(&self) -> Option<SshPhase> {
        self.ssh_phase.lock().ok().and_then(|g| g.clone())
    }

    pub fn ssh_endpoint(&self) -> Option<(String, u16)> {
        self.ssh_endpoint.clone()
    }

    pub fn ssh_user(&self) -> Option<String> {
        self.ssh_user.clone()
    }

    pub fn auto_supplied_password(&self) -> bool {
        self.auto_supplied_password
    }

    pub fn respond_auth(&self, request_id: u64, response: AuthResponse) {
        self.connection.send(ClientMsg::AuthResponse {
            request_id,
            response,
        });
    }

    pub fn sftp_list(pane_id: u64, path: &str) -> Result<Vec<SftpEntry>, String> {
        fn query(pane_id: u64, path: String) -> anyhow::Result<Result<Vec<SftpEntry>, String>> {
            let mut stream = connect()?;
            ClientMsg::SftpList { pane_id, path }.encode(&mut stream)?;
            Ok(match DaemonMsg::read(&mut stream)? {
                DaemonMsg::SftpEntries(entries) => Ok(entries),
                DaemonMsg::Error(msg) => Err(msg),
                other => Err(format!("unexpected reply to SftpList: {other:?}")),
            })
        }
        query(pane_id, path.to_string()).unwrap_or_else(|e| Err(e.to_string()))
    }

    pub fn sftp_op(pane_id: u64, op: SftpOp) -> SftpOpResult {
        fn query(pane_id: u64, op: SftpOp) -> anyhow::Result<SftpOpResult> {
            let mut stream = connect()?;
            ClientMsg::SftpOp { pane_id, op }.encode(&mut stream)?;
            Ok(match DaemonMsg::read(&mut stream)? {
                DaemonMsg::SftpOpResult(result) => result,
                DaemonMsg::Error(msg) => SftpOpResult::Error(msg),
                other => SftpOpResult::Error(format!("unexpected reply to SftpOp: {other:?}")),
            })
        }
        query(pane_id, op).unwrap_or_else(|e| SftpOpResult::Error(e.to_string()))
    }

    pub fn sftp_transfer_start(spec: SftpTransferSpec) -> Result<u64, String> {
        fn query(spec: SftpTransferSpec) -> anyhow::Result<Result<u64, String>> {
            let mut stream = connect()?;
            ClientMsg::SftpTransferStart(spec).encode(&mut stream)?;
            Ok(match DaemonMsg::read(&mut stream)? {
                DaemonMsg::SftpTransferStarted { job_id } => Ok(job_id),
                DaemonMsg::Error(msg) => Err(msg),
                other => Err(format!("unexpected reply to SftpTransferStart: {other:?}")),
            })
        }
        query(spec).unwrap_or_else(|e| Err(e.to_string()))
    }

    pub fn sftp_transfer_cancel(job_id: u64) -> Vec<SftpJobProgress> {
        fn query(job_id: u64) -> anyhow::Result<Vec<SftpJobProgress>> {
            let mut stream = connect()?;
            ClientMsg::SftpTransferCancel { job_id }.encode(&mut stream)?;
            match DaemonMsg::read(&mut stream)? {
                DaemonMsg::SftpTransferProgress(jobs) => Ok(jobs),
                other => Err(anyhow::anyhow!(
                    "unexpected reply to SftpTransferCancel: {other:?}"
                )),
            }
        }
        query(job_id).unwrap_or_default()
    }

    /// A failed poll is not an empty transfer list: the caller has to be able
    /// to keep the jobs it already knows about, so this reports the failure
    /// the way `sftp_list` does rather than answering with an empty `Vec`.
    pub fn sftp_transfer_list(pane_id: u64) -> Result<Vec<SftpJobProgress>, String> {
        fn query(pane_id: u64) -> anyhow::Result<Result<Vec<SftpJobProgress>, String>> {
            let mut stream = connect()?;
            ClientMsg::SftpTransferList { pane_id }.encode(&mut stream)?;
            Ok(match DaemonMsg::read(&mut stream)? {
                DaemonMsg::SftpTransferProgress(jobs) => Ok(jobs),
                DaemonMsg::Error(msg) => Err(msg),
                other => Err(format!("unexpected reply to SftpTransferList: {other:?}")),
            })
        }
        query(pane_id).unwrap_or_else(|e| Err(e.to_string()))
    }

    /// `None` when the request never got a list back — which is not the same
    /// as getting an empty one, because only the caller of a *failed* request
    /// still has to keep showing what it had.
    pub fn add_forward(pane_id: u64, rule: SshForwardRule) -> Option<Vec<ManagedForward>> {
        fn query(pane_id: u64, rule: SshForwardRule) -> anyhow::Result<Vec<ManagedForward>> {
            let mut stream = connect()?;
            ClientMsg::AddForward { pane_id, rule }.encode(&mut stream)?;
            match DaemonMsg::read(&mut stream)? {
                DaemonMsg::ForwardList(list) => Ok(list),
                DaemonMsg::Error(msg) => Err(anyhow::anyhow!(msg)),
                other => Err(anyhow::anyhow!("unexpected reply to AddForward: {other:?}")),
            }
        }
        query(pane_id, rule)
            .inspect_err(|e| log::warn!("AddForward failed: {e}"))
            .ok()
    }

    /// `None` when the request never got a list back — see `add_forward`.
    pub fn remove_forward(pane_id: u64, forward_id: u64) -> Option<Vec<ManagedForward>> {
        fn query(pane_id: u64, forward_id: u64) -> anyhow::Result<Vec<ManagedForward>> {
            let mut stream = connect()?;
            ClientMsg::RemoveForward {
                pane_id,
                forward_id,
            }
            .encode(&mut stream)?;
            match DaemonMsg::read(&mut stream)? {
                DaemonMsg::ForwardList(list) => Ok(list),
                other => Err(anyhow::anyhow!(
                    "unexpected reply to RemoveForward: {other:?}"
                )),
            }
        }
        query(pane_id, forward_id)
            .inspect_err(|e| log::warn!("RemoveForward failed: {e}"))
            .ok()
    }

    /// The list after switching one forward on or off, or why it was not
    /// switched — the daemon's own refusal, which names what is in the way.
    pub fn set_forward_enabled(
        pane_id: u64,
        forward_id: u64,
        enabled: bool,
    ) -> anyhow::Result<Vec<ManagedForward>> {
        let mut stream = connect()?;
        ClientMsg::SetForwardEnabled {
            pane_id,
            forward_id,
            enabled,
        }
        .encode(&mut stream)?;
        match DaemonMsg::read(&mut stream)? {
            DaemonMsg::ForwardList(list) => Ok(list),
            DaemonMsg::Error(msg) => Err(anyhow::anyhow!(msg)),
            other => Err(anyhow::anyhow!(
                "unexpected reply to SetForwardEnabled: {other:?}"
            )),
        }
    }

    pub fn list_forwards(pane_id: u64) -> Vec<ManagedForward> {
        fn query(pane_id: u64) -> anyhow::Result<Vec<ManagedForward>> {
            let mut stream = connect()?;
            ClientMsg::ListForwards { pane_id }.encode(&mut stream)?;
            match DaemonMsg::read(&mut stream)? {
                DaemonMsg::ForwardList(list) => Ok(list),
                other => Err(anyhow::anyhow!(
                    "unexpected reply to ListForwards: {other:?}"
                )),
            }
        }
        query(pane_id).unwrap_or_default()
    }

    const WORKSPACE_OP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

    pub fn on_workspace(req: WorkspaceRequest) -> anyhow::Result<DaemonMsg> {
        let mut stream = connect()?;
        let _ = stream.set_read_timeout(Some(Self::WORKSPACE_OP_TIMEOUT));
        ClientMsg::OnWorkspace(Box::new(req)).encode(&mut stream)?;
        match DaemonMsg::read(&mut stream)? {
            DaemonMsg::Error(msg) => Err(anyhow::anyhow!(msg)),
            reply => Ok(reply),
        }
    }

    pub fn workspace_request(
        ws: &PaneWorkspace,
        view_pane: u64,
        op: WorkspaceOp,
    ) -> Option<WorkspaceRequest> {
        Some(WorkspaceRequest {
            workspace: ws.workspace,
            spec: ws.spec.clone()?,
            view_pane,
            op,
        })
    }

    pub fn query_procs(pane_id: u64) -> PaneProcs {
        fn query(pane_id: u64) -> anyhow::Result<PaneProcs> {
            let mut stream = connect()?;
            ClientMsg::QueryProcs { pane_id }.encode(&mut stream)?;
            match DaemonMsg::read(&mut stream)? {
                DaemonMsg::Procs(procs) => Ok(procs),
                other => Err(anyhow::anyhow!("unexpected reply to QueryProcs: {other:?}")),
            }
        }
        query(pane_id).unwrap_or_default()
    }
}


impl Drop for RemoteTerminal {
    fn drop(&mut self) {
        self.connection.detach(&self.images);
    }
}

pub(crate) fn notify_desktop(title: Option<&str>, body: &str) {
    notify_desktop_for_pane(title, body, None);
}

/// Show a desktop notification.
///
/// Naming a `pane` makes the notification clickable where the platform supports
/// it, so activating it reveals that pane. It is an `EntityId` on purpose: the
/// tray dispatch identifies leaves by their gpui entity id, and taking a bare
/// `u64` here is what lets a caller hand over a `pane_id` — a different number,
/// assigned by the daemon — and get a notification that reveals nothing.
///
/// macOS always goes through `macos_notify`, clickable or not. Elsewhere, every
/// other case (no pane, unsupported platform, Windows toast queue full) falls
/// back to the plain `notify-rust` path below, which is why
/// `try_clickable_notification` reports whether it took the job.
pub(crate) fn notify_desktop_for_pane(title: Option<&str>, body: &str, pane: Option<EntityId>) {
    let summary = sanitize_notification_text(title.unwrap_or("lyrel"), NOTIFY_TITLE_MAX);
    let body = sanitize_notification_text(body, NOTIFY_BODY_MAX);

    #[cfg(target_os = "macos")]
    {
        macos_notify::deliver(summary, body, pane.map(|p| p.as_u64()));
    }
    #[cfg(not(target_os = "macos"))]
    {
        if let Some(pane) = pane
            && try_clickable_notification(&summary, &body, pane.as_u64())
        {
            return;
        }

        std::thread::spawn(move || {
            let mut notif = notify_rust::Notification::new();
            notif.summary(&summary).body(&body);
            // Without our own AUMID, the Windows backend falls back to
            // PowerShell's — icon and name included. Only set ours once the
            // shell has indexed a shortcut carrying it: for an AUMID it does
            // not know, `show()` reports success and drops the toast, so the
            // ugly fallback beats the branded one every time we are not sure.
            #[cfg(target_os = "windows")]
            if let Some(app_id) = crate::core::aumid::toast_app_id() {
                notif.app_id(app_id);
            }
            let _ = notif.show();
        });
    }
}

/// Longest title / body we hand to a notification backend.
///
/// The text comes straight off the terminal (OSC 9/777 payloads, window
/// titles, an agent's own status line), so it is attacker-shaped: arbitrarily
/// long, and free to contain control bytes. Every backend truncates in its own
/// ugly way, and on Windows a stray `ESC` makes the toast XML fail to parse and
/// the whole notification disappear — so clamp both here, once, for all paths.
const NOTIFY_TITLE_MAX: usize = 96;
const NOTIFY_BODY_MAX: usize = 512;

fn sanitize_notification_text(s: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut taken = 0usize;
    for ch in s.chars() {
        // XML 1.0 allows exactly tab / LF / CR out of the control range.
        if ch.is_control() && !matches!(ch, '\t' | '\n' | '\r') {
            continue;
        }
        if taken == max_chars {
            out.push('…');
            break;
        }
        out.push(ch);
        taken += 1;
    }
    out
}

/// What a click on a macOS notification has to carry back: the pane to reveal.
///
/// It rides in the notification's `identifier`, which is the one field the
/// center hands back verbatim on activation without a dictionary round trip.
/// Shape: `lyrel-pane-<pid>-<leaf>-<seq>`.
///
/// The pid is what makes a stale identifier fail closed. Notifications outlive
/// the process that sent them, and the center hands a click on one of those to
/// whatever process now owns the bundle id — a relaunched lyrel, or a second
/// instance running alongside. A leaf id is a gpui entity id, which a fresh
/// process hands out again in the same order, so without the pid that click
/// would reveal an unrelated pane. The sequence number keeps two notifications
/// for the same pane apart — the center treats a repeated identifier as
/// "replace the earlier one".
#[cfg(any(target_os = "macos", test))]
const NOTIFICATION_ID_PREFIX: &str = "lyrel-pane-";

#[cfg(any(target_os = "macos", test))]
fn notification_identifier(leaf_id: u64) -> String {
    use std::sync::atomic::AtomicU64;
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    format!("{NOTIFICATION_ID_PREFIX}{pid}-{leaf_id}-{seq}")
}

#[cfg(any(target_os = "macos", test))]
fn leaf_id_in_identifier(identifier: &str) -> Option<u64> {
    let rest = identifier.strip_prefix(NOTIFICATION_ID_PREFIX)?;
    let (pid, rest) = rest.split_once('-')?;
    if pid.parse::<u32>().ok()? != std::process::id() {
        return None;
    }
    let (leaf_id, _seq) = rest.split_once('-')?;
    leaf_id.parse().ok()
}

/// macOS notifications, straight to `NSUserNotificationCenter`.
///
/// This used to go through `mac-notification-sys` with `wait_for_click`, so a
/// click could reveal the pane. The way that crate notices a click is to park
/// the sending thread and, for every notification outstanding, add a repeating
/// 0.5 s timer to the *main* run loop that calls `deliveredNotifications` — a
/// synchronous XPC round trip — to see whether the banner is still there. A
/// banner nobody clicks stays in Notification Center, so its timer never goes
/// away. Sampled with nine outstanding: a fifth of the UI thread inside that
/// XPC, every window juddering, and each agent turn adding one more timer.
///
/// Here the click arrives through the center's delegate, which is what the
/// API is for: no parked thread, no timer, nothing on the main thread until
/// the user actually clicks. The pane rides in the notification's identifier.
///
/// Nothing else may touch the center on this platform. `mac-notification-sys`
/// installs a delegate of its own the first time it sends, the last
/// `setDelegate:` wins, and `notify-rust` is that crate on macOS — so its
/// `show` is never called here, only its `set_application`, which does not
/// install one (that is what names a bare `cargo run` binary to the center).
#[cfg(target_os = "macos")]
#[allow(
    deprecated,
    reason = "UNUserNotificationCenter needs a signed, entitled bundle; NSUserNotification is what a bare binary can use"
)]
mod macos_notify {
    use objc2::rc::{Retained, autoreleasepool};
    use objc2::runtime::ProtocolObject;
    use objc2::{AnyThread, define_class, msg_send};
    use objc2_foundation::{
        NSObject, NSObjectProtocol, NSString, NSUserNotification, NSUserNotificationCenter,
        NSUserNotificationCenterDelegate,
    };

    define_class!(
        // SAFETY: NSObject has no subclassing requirements; `Delegate` has no
        // ivars and no `Drop`.
        #[unsafe(super = NSObject)]
        #[name = "LyrelNotificationDelegate"]
        struct Delegate;

        // SAFETY: `NSObjectProtocol` has no safety requirements.
        unsafe impl NSObjectProtocol for Delegate {}

        // SAFETY: `NSUserNotificationCenterDelegate` has no safety requirements.
        unsafe impl NSUserNotificationCenterDelegate for Delegate {
            /// Runs on the main thread, when the user clicks the banner or the
            /// entry in Notification Center. A channel push, then the clicked
            /// entry is dropped from the center — a one-way message, unlike the
            /// `deliveredNotifications` round trip this module exists to avoid.
            #[unsafe(method(userNotificationCenter:didActivateNotification:))]
            fn did_activate(
                &self,
                center: &NSUserNotificationCenter,
                notification: &NSUserNotification,
            ) {
                if let Some(leaf_id) = notification
                    .identifier()
                    .and_then(|id| super::leaf_id_in_identifier(&id.to_string()))
                {
                    super::reveal_pane(leaf_id);
                }
                center.removeDeliveredNotification(notification);
            }
        }
    );

    fn install_delegate() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            // Name the delivering application to the center before it is first
            // touched: the center drops requests from a process with no bundle
            // identity, which is what a bare `cargo run` binary is. This
            // swizzles `-[NSBundle bundleIdentifier]` for the main bundle to
            // `io.github.dlivxpr.lyrel` when LaunchServices knows that id (a bundled
            // lyrel.app, or a machine that has one installed); when it does not,
            // the swizzle's own default, `com.apple.Terminal`, is what the
            // center sees. It can only be called once per process, so there is
            // no second chance to pass a different name.
            let _ = notify_rust::set_application("io.github.dlivxpr.lyrel");
            // SAFETY: `NSObject`'s `init` takes nothing and returns the object.
            let delegate: Retained<Delegate> = unsafe { msg_send![Delegate::alloc(), init] };
            let center = NSUserNotificationCenter::defaultUserNotificationCenter();
            // SAFETY: `delegate` is a `Delegate`, which conforms to the protocol.
            unsafe { center.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
            // The center holds its delegate unretained. Ours lives as long as
            // the process, so it is simply never released.
            std::mem::forget(delegate);
        });
    }

    /// Hands the notification to the center and returns. `deliverNotification:`
    /// is an XPC message, so it goes out on a short-lived thread rather than
    /// from wherever the OSC sequence was parsed — never the UI thread.
    pub(super) fn deliver(title: String, body: String, leaf_id: Option<u64>) {
        std::thread::spawn(move || {
            autoreleasepool(|_| {
                install_delegate();
                let notification = NSUserNotification::new();
                notification.setTitle(Some(&NSString::from_str(&title)));
                notification.setInformativeText(Some(&NSString::from_str(&body)));
                if let Some(leaf_id) = leaf_id {
                    let id = super::notification_identifier(leaf_id);
                    notification.setIdentifier(Some(&NSString::from_str(&id)));
                }
                NSUserNotificationCenter::defaultUserNotificationCenter()
                    .deliverNotification(&notification);
            });
        });
    }
}

#[cfg(all(target_os = "windows", not(test)))]
fn try_clickable_notification(title: &str, body: &str, leaf_id: u64) -> bool {
    // An AUMID the shell has not indexed makes `Show` report success and drops
    // the toast on the floor, so without one the notify-rust path — which
    // deliberately keeps PowerShell's identity rather than lose the toast — is
    // the only one that shows anything at all.
    if crate::core::aumid::toast_app_id().is_none() {
        return false;
    }
    let Some(tx) = toast_thread() else {
        return false;
    };
    tx.try_send((title.to_string(), body.to_string(), leaf_id))
        .is_ok()
}

#[cfg(all(target_os = "windows", not(test)))]
type ToastRequest = (String, String, u64);

/// Toasts are built, shown and *kept* on one dedicated thread.
///
/// A `ToastNotification` has to stay alive for its `Activated` event to reach
/// us, and Windows parks toasts in Action Center long after they leave the
/// screen — a thread per toast sleeping out a fixed window would both leak
/// threads and stop working the moment the timer expired. Keeping them on a
/// single thread also means the WinRT objects never cross a thread boundary.
#[cfg(all(target_os = "windows", not(test)))]
fn toast_thread() -> Option<&'static std::sync::mpsc::SyncSender<ToastRequest>> {
    use std::sync::OnceLock;
    use std::sync::mpsc::sync_channel;

    static TX: OnceLock<Option<std::sync::mpsc::SyncSender<ToastRequest>>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = sync_channel::<ToastRequest>(32);
        std::thread::Builder::new()
            .name("lyrel-toasts".into())
            .spawn(move || toast_loop(rx))
            .map_err(|e| log::warn!("failed to start the toast thread: {e}"))
            .ok()
            .map(|_| tx)
    })
    .as_ref()
}

#[cfg(all(target_os = "windows", not(test)))]
fn toast_loop(rx: std::sync::mpsc::Receiver<ToastRequest>) {
    use std::collections::VecDeque;
    use windows::UI::Notifications::ToastNotification;
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

    // MTA lets the Activated callback run on a thread-pool thread without a
    // message loop of our own.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    // How many past toasts stay clickable. Dropping the oldest is what bounds
    // the memory here; there is no useful deadline to expire them on.
    const MAX_LIVE_TOASTS: usize = 32;
    let mut live: VecDeque<ToastNotification> = VecDeque::new();

    while let Ok((title, body, leaf_id)) = rx.recv() {
        if let Some(toast) = show_windows_toast(&title, &body, leaf_id) {
            if live.len() == MAX_LIVE_TOASTS {
                live.pop_front();
            }
            live.push_back(toast);
        }
    }
}

#[cfg(all(target_os = "windows", not(test)))]
fn show_windows_toast(
    title: &str,
    body: &str,
    leaf_id: u64,
) -> Option<windows::UI::Notifications::ToastNotification> {
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::Foundation::TypedEventHandler;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
    use windows::core::{IInspectable, Ref};

    let app_id = crate::core::aumid::toast_app_id()?;

    let xml = format!(
        r#"<toast>
  <visual>
    <binding template="ToastText02">
      <text id="1">{}</text>
      <text id="2">{}</text>
    </binding>
  </visual>
</toast>"#,
        xml_escape(title),
        xml_escape(body)
    );

    let doc = match XmlDocument::new() {
        Ok(d) => {
            if let Err(e) = d.LoadXml(&xml.into()) {
                log::warn!("failed to load toast xml: {e}");
                return None;
            }
            d
        }
        Err(e) => {
            log::warn!("failed to create toast xml document: {e}");
            return None;
        }
    };

    let toast = match ToastNotification::CreateToastNotification(&doc) {
        Ok(t) => t,
        Err(e) => {
            log::warn!("failed to create toast notification: {e}");
            return None;
        }
    };

    if let Err(e) = toast.Activated(&TypedEventHandler::new(
        move |_sender: Ref<ToastNotification>, _args: Ref<IInspectable>| {
            reveal_pane(leaf_id);
            Ok(())
        },
    )) {
        log::warn!("failed to register toast activated handler: {e}");
    }

    let notifier = match ToastNotificationManager::CreateToastNotifierWithId(&app_id.into()) {
        Ok(n) => n,
        Err(e) => {
            log::warn!("failed to create toast notifier: {e}");
            return None;
        }
    };

    if let Err(e) = notifier.Show(&toast) {
        log::warn!("failed to show toast: {e}");
        return None;
    }

    Some(toast)
}

/// Ask the tray dispatch loop to bring `leaf_id` to the front. Runs on whatever
/// thread the platform hands the activation to, so it only touches the channel.
#[cfg(any(target_os = "macos", all(target_os = "windows", not(test))))]
fn reveal_pane(leaf_id: u64) {
    if let Some(tx) = crate::ui::tray::sender() {
        let _ = tx.try_send(crate::ui::tray::TrayAction::RevealPane { leaf_id });
    }
}

#[cfg(not(any(target_os = "macos", all(target_os = "windows", not(test)))))]
fn try_clickable_notification(_title: &str, _body: &str, _leaf_id: u64) -> bool {
    // Linux notifications go through notify-rust; click-to-reveal would need a
    // D-Bus action listener of its own.
    false
}

#[cfg(target_os = "windows")]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod notification_tests {
    use super::*;

    #[test]
    fn a_notification_identifier_carries_its_pane_back_out() {
        let id = notification_identifier(42);
        assert_eq!(leaf_id_in_identifier(&id), Some(42));
    }

    #[test]
    fn two_notifications_for_one_pane_do_not_replace_each_other() {
        // The center replaces a delivered notification whose identifier repeats.
        assert_ne!(notification_identifier(7), notification_identifier(7));
    }

    #[test]
    fn a_foreign_identifier_reveals_nothing() {
        let pid = std::process::id();
        for id in [
            String::new(),
            "lyrel-pane-".into(),
            "lyrel-pane-x-1".into(),
            "lyrel-pane-42".into(),
            "other-42-1".into(),
            format!("lyrel-pane-{pid}"),
            format!("lyrel-pane-{pid}-42"),
            format!("lyrel-pane-{pid}-x-1"),
        ] {
            assert_eq!(leaf_id_in_identifier(&id), None, "{id:?}");
        }
    }

    #[test]
    fn a_notification_from_another_process_reveals_nothing() {
        // Notifications outlive the process that sent them, and gpui hands out
        // the same entity ids again in a fresh process. A stale click must not
        // land on whatever pane holds that id now.
        let other_pid = std::process::id().wrapping_add(1);
        let stale = format!("{NOTIFICATION_ID_PREFIX}{other_pid}-42-0");
        assert_eq!(leaf_id_in_identifier(&stale), None);
    }

    #[test]
    fn sanitizing_drops_control_bytes_but_keeps_line_breaks() {
        let raw = "build \x1b[31mfailed\x07\nsee log\ttail";
        assert_eq!(
            sanitize_notification_text(raw, NOTIFY_BODY_MAX),
            "build [31mfailed\nsee log\ttail"
        );
    }

    #[test]
    fn sanitizing_clamps_by_chars_not_bytes() {
        let out = sanitize_notification_text(&"账".repeat(200), 8);
        assert_eq!(out, format!("{}…", "账".repeat(8)));
        assert_eq!(sanitize_notification_text("short", 8), "short");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn xml_escaping_covers_every_entity_once() {
        assert_eq!(
            xml_escape(r#"a & b < c > "d" 'e'"#),
            "a &amp; b &lt; c &gt; &quot;d&quot; &apos;e&apos;"
        );
    }
}

type OscNotes = Arc<Mutex<VecDeque<Note>>>;


/// Builds the pane's emulator from the user's config.
///
/// rio-vt has no runtime-replaceable `Config` object; what a pane carries is
/// the constructor's scrollback limit and default cursor shape, plus the word
/// separators, which `Crosswords` does not accept at all — selection reads
/// those from the lyrel config directly (`terminal::smart_select`).
fn build_term(
    size: TermSize,
    proxy: EventProxy,
    user_config: &crate::core::config::Config,
) -> Crosswords<EventProxy> {
    Crosswords::new(
        size,
        cursor_shape_for(user_config.cursor_style),
        proxy,
        // A pane is not a window and is never routed by Rio: both ids are
        // constant, and replies the pane cares about name the pty through the
        // pane's own writer, not these.
        WindowId::from(0),
        0,
        user_config.scrollback_limit,
    )
}

/// Applies a live config reload. The primary grid takes its new history limit
/// immediately; while the alternate screen is active, the parse bridge applies
/// the shared desired limit at the exact transition back to the primary grid.
/// Cursor defaults still preserve any shape explicitly selected by a program.
fn apply_config(term: &mut Crosswords<EventProxy>, user_config: &crate::core::config::Config) {
    if !term.mode().contains(Mode::ALT_SCREEN) {
        term.grid.update_history(user_config.scrollback_limit);
    }
    let shape = cursor_shape_for(user_config.cursor_style);
    let followed_default = term.cursor_shape == term.default_cursor_shape;
    term.default_cursor_shape = shape;
    if followed_default {
        term.cursor_shape = shape;
        term.blinking_cursor = false;
    }
}

/// Test emulator with what the pane config always carried under the fork:
/// 10_000 lines of scrollback and a block default cursor, no listener.
#[cfg(test)]
pub(super) fn test_term(size: TermSize) -> Crosswords<rio_vt::event::VoidListener> {
    Crosswords::new(
        size,
        CursorShape::Block,
        rio_vt::event::VoidListener,
        WindowId::from(0),
        0,
        10_000,
    )
}

fn cursor_shape_for(style: ConfigCursorStyle) -> CursorShape {
    match style {
        ConfigCursorStyle::Block => CursorShape::Block,
        ConfigCursorStyle::Bar => CursorShape::Beam,
        ConfigCursorStyle::Underline => CursorShape::Underline,
    }
}

fn win_size(size: TermSize, cell_w: u16, cell_h: u16) -> WinSize {
    WinSize {
        cols: size.cols as u16,
        rows: size.rows as u16,
        cell_w,
        cell_h,
    }
}

/// Ungated on purpose: what a workspace can build a route out of is the same
/// on every platform, and so is the name the refusal carries.
#[cfg(test)]
mod route_header_tests {
    use super::*;
    use crate::core::session::{RemoteTarget, WorkspaceId};

    fn unroutable(target: RemoteTarget, label: Option<&str>) -> PaneWorkspace {
        PaneWorkspace {
            workspace: WorkspaceId::new(),
            target,
            spec: None,
            label: label.map(str::to_string),
            resize_echo: false,
        }
    }

    /// A deleted profile is what empties `spec`, and the refusal built here is
    /// what the pending pane prints verbatim under "could not reach
    /// {machine}" — so this is one of the screens #485 is about. It used to
    /// carry `{target:?}`, which for a `Profile` is its config UUID and
    /// nothing else.
    #[test]
    fn an_unroutable_workspace_is_not_named_by_its_profile_uuid() {
        let id = uuid::Uuid::new_v4();
        let gone = RemoteTarget::Profile { id };

        let named = unroutable(gone.clone(), Some("lager"));
        let e = named
            .route_header()
            .expect_err("no spec, no route")
            .to_string();
        assert!(
            !e.contains(&id.to_string()),
            "a bare profile UUID reached the UI: {e}"
        );
        assert!(
            e.contains("lager"),
            "the entry's own name is what it is called: {e}"
        );
        assert!(e.contains("cannot be routed"), "{e}");

        // Nothing to call it by is still no reason to print the UUID.
        let bare = unroutable(gone, None);
        let e = bare
            .route_header()
            .expect_err("no spec, no route")
            .to_string();
        assert!(
            !e.contains(&id.to_string()),
            "a bare profile UUID reached the UI: {e}"
        );
        assert!(e.contains("cannot be routed"), "{e}");
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use super::test_support::ssh_workspace;


    #[test]
    fn a_local_pane_prefixes_nothing() {
        assert!(PaneRoute::Local.header().is_none());
        assert!(PaneRoute::for_workspace(None).header().is_none());
        assert!(matches!(PaneRoute::for_workspace(None), PaneRoute::Local));
        assert!(matches!(PaneRoute::default(), PaneRoute::Local));
    }

    #[test]
    fn a_remote_pane_routes_to_its_machine_on_the_pane_channel() {
        let route = PaneRoute::for_workspace(Some(&ssh_workspace()));
        let header = route.header().expect("a remote pane is routed");
        assert_eq!(
            header.channel,
            crate::daemon::router::RouteChannel::Pane,
            "a pane must not be sent to the control socket"
        );
        assert_eq!(header.describe(), "ssh me@build-box:22");

        let mut ws = ssh_workspace();
        ws.resize_echo = true;
        assert!(
            matches!(
                PaneRoute::for_workspace(Some(&ws)),
                PaneRoute::Remote {
                    resize_echo: true,
                    ..
                }
            ),
            "what the host's hello said about the resize echo rides the route"
        );
    }

    #[test]
    fn a_wsl_workspace_routes_by_distro() {
        let ws = PaneWorkspace {
            workspace: crate::core::session::WorkspaceId::new(),
            target: crate::core::session::RemoteTarget::Wsl {
                distro: "Ubuntu-22.04".into(),
            },
            spec: None,
            label: None,
            resize_echo: false,
        };
        let route = PaneRoute::for_workspace(Some(&ws));
        let header = route.header().expect("WSL is routed");
        assert_eq!(header.describe(), "wsl Ubuntu-22.04");
        assert_eq!(header.channel, crate::daemon::router::RouteChannel::Pane);
    }

    #[test]
    fn a_local_stdio_workspace_routes_to_a_child_process_on_the_pane_dialect() {
        let ws = PaneWorkspace {
            workspace: crate::core::session::WorkspaceId::new(),
            target: crate::core::session::RemoteTarget::LocalStdio {
                program: "/tmp/lyrel-server".into(),
                args: vec!["--stdio".into()],
            },
            spec: None,
            label: None,
            resize_echo: false,
        };
        let route = PaneRoute::for_workspace(Some(&ws));
        let header = route.header().expect("a local child is routable");
        assert_eq!(header.channel, crate::daemon::router::RouteChannel::Pane);
        match &header.target {
            crate::daemon::router::RouteTarget::LocalStdio { program, args } => {
                assert_eq!(program, "/tmp/lyrel-server");
                assert_eq!(args, &vec!["--stdio".to_string(), "--pane".to_string()]);
            }
            other => panic!("wrong target: {other:?}"),
        }
    }

    #[test]
    fn an_unroutable_workspace_is_not_treated_as_local() {
        let ws = PaneWorkspace {
            workspace: crate::core::session::WorkspaceId::new(),
            target: crate::core::session::RemoteTarget::Alias {
                alias: "build-box".into(),
            },
            spec: None,
            label: None,
            resize_echo: false,
        };
        let route = PaneRoute::for_workspace(Some(&ws));
        assert!(matches!(route, PaneRoute::Unroutable(_)));
        assert!(route.header().is_none(), "nothing to route to");
        let err = connect_routed(&route).expect_err("must not reach the local daemon");
        assert!(err.to_string().contains("cannot be routed"), "{err}");
    }


    #[test]
    fn only_a_local_pane_may_restart_the_local_daemon() {
        assert!(PaneRoute::Local.is_local());
        assert!(PaneRoute::for_workspace(None).is_local());

        assert!(
            !PaneRoute::for_workspace(Some(&ssh_workspace())).is_local(),
            "a routed pane's disconnect is the remote's failure, not the local daemon's"
        );
        assert!(
            !PaneRoute::Unroutable("no ssh details".into()).is_local(),
            "nothing was ever asked of the local daemon"
        );
    }

    #[test]
    fn kitty_keyboard_negotiation_reports_the_requested_mode() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        DaemonMsg::Output(b"\x1b[>7u\x1b[?u".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();

        let mut reply = None;
        for _ in 0..200 {
            while let Ok(event) = term.events.try_recv() {
                if let PaneEvent::PtyWrite(text) = event {
                    reply = Some(text);
                }
            }
            if reply.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        assert_eq!(reply.as_deref(), Some("\x1b[?7u"));
        assert!(
            term.term
                .lock()
                .mode()
                .contains(Mode::DISAMBIGUATE_ESC_CODES)
        );
    }

    #[test]
    fn deep_keyboard_mode_pushes_leave_the_reader_alive() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        let mut payload = b"\x1b[>1u".repeat(4097);
        payload.extend_from_slice(b"\x1b[?u");
        DaemonMsg::Output(payload).encode(&mut daemon_side).unwrap();
        daemon_side.flush().unwrap();

        let mut reply = None;
        for _ in 0..200 {
            while let Ok(event) = term.events.try_recv() {
                if let PaneEvent::PtyWrite(text) = event {
                    reply = Some(text);
                }
            }
            if reply.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        assert_eq!(
            reply.as_deref(),
            Some("\x1b[?1u"),
            "the reader thread must survive a deep mode-push run and still answer queries"
        );
    }

    #[test]
    fn emoji_presentation_sequences_reserve_two_columns() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        DaemonMsg::Output("\u{2764}\u{FE0F}x".as_bytes().to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();

        let mut row = String::new();
        for _ in 0..200 {
            {
                let t = term.term.lock();
                let grid = &t.grid;
                row.clear();
                for col in 0..3usize {
                    row.push(
                        grid[rio_vt::crosswords::pos::Line(0)]
                            [rio_vt::crosswords::pos::Column(col)]
                        .c(),
                    );
                }
            }
            if row.contains('x') {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        assert_eq!(
            row, "\u{2764} x",
            "❤️ must hold two columns (glyph + spacer) before the next glyph"
        );
    }


    #[test]
    fn reader_feeds_local_grid() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();

        let size = TermSize::new(80, 24);
        let term = RemoteTerminal::from_stream(client_side, size).unwrap();

        DaemonMsg::Output(b"hello".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        DaemonMsg::Cwd(PathBuf::from("/tmp/work"))
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();

        let mut got = String::new();
        for _ in 0..200 {
            {
                let t = term.term.lock();
                let grid = &t.grid;
                got.clear();
                for col in 0..5usize {
                    let cell = &grid[rio_vt::crosswords::pos::Line(0)]
                        [rio_vt::crosswords::pos::Column(col)];
                    got.push(cell.c());
                }
            }
            if got == "hello" {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(got, "hello", "reader thread should have fed the grid");

        let mut cwd = None;
        for _ in 0..200 {
            cwd = term.foreground_cwd();
            if cwd.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(cwd, Some(PathBuf::from("/tmp/work")));

        drop(daemon_side);
        for _ in 0..200 {
            if term.exited_flag.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(term.exited_flag.load(Ordering::SeqCst));
    }

    #[test]
    fn cursor_style_sequence_overrides_and_resets_to_user_default() {
        crate::core::config::pin_test_config_dir();
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        let mut user_config = crate::core::config::Config::default();
        user_config.cursor_style = ConfigCursorStyle::Underline;
        term.apply_user_config(&user_config);

        let mut shape = term.term.lock().cursor_shape;
        assert_eq!(shape, CursorShape::Underline);

        DaemonMsg::Output(b"\x1b[6 q".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        for _ in 0..200 {
            shape = term.term.lock().cursor_shape;
            if shape == CursorShape::Beam {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(shape, CursorShape::Beam);

        DaemonMsg::Output(b"\x1b[0 q".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        for _ in 0..200 {
            shape = term.term.lock().cursor_shape;
            if shape == CursorShape::Underline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(shape, CursorShape::Underline);
    }

    /// Feeds `output` to a pane configured with `configured`, one daemon frame
    /// per chunk, and reports the cursor shape once all of it has been parsed.
    fn cursor_shape_after(configured: ConfigCursorStyle, output: &[&[u8]]) -> CursorShape {
        use rio_vt::crosswords::pos::{Column, Line};

        // The pane reads config.json when it is built; keep that off the
        // user's real one.
        crate::core::config::pin_test_config_dir();
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        let mut user_config = crate::core::config::Config::default();
        user_config.cursor_style = configured;
        term.apply_user_config(&user_config);

        for chunk in output {
            DaemonMsg::Output(chunk.to_vec())
                .encode(&mut daemon_side)
                .unwrap();
        }
        // A sentinel painted after everything else: once it is on the grid,
        // every chunk before it has been parsed, so the shape read below is
        // the final one and not a transient match.
        DaemonMsg::Output(b"\x1b[24;1H#".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        for _ in 0..400 {
            if term.term.lock().grid[Line(23)][Column(0)].c() == '#' {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let term = term.term.lock();
        assert_eq!(
            term.grid[Line(23)][Column(0)].c(),
            '#',
            "output never parsed"
        );
        term.cursor_shape
    }

    /// #837: both spellings of the DECSCUSR reset land on `cursor_style`.
    #[test]
    fn cursor_style_reset_returns_to_a_configured_bar_or_underline() {
        for reset in [&b"\x1b[0 q"[..], &b"\x1b[ q"[..]] {
            for (configured, want) in [
                (ConfigCursorStyle::Bar, CursorShape::Beam),
                (ConfigCursorStyle::Underline, CursorShape::Underline),
            ] {
                assert_eq!(
                    cursor_shape_after(configured, &[b"\x1b[6 q", b"\x1b[2 q", reset]),
                    want,
                    "{configured:?} after {reset:?}"
                );
            }
        }
    }

    /// #837: what nvim actually sends on exit under `xterm-256color` is `Se`,
    /// `\e[2 q` — a literal steady block. The command's `D` hands the prompt
    /// its configured cursor back, whether the block arrives in the same
    /// frame as the marks or in one of its own.
    #[test]
    fn a_block_an_exiting_command_left_is_undone_at_its_finish_mark() {
        for (configured, want) in [
            (ConfigCursorStyle::Bar, CursorShape::Beam),
            (ConfigCursorStyle::Underline, CursorShape::Underline),
        ] {
            assert_eq!(
                cursor_shape_after(
                    configured,
                    &[b"\x1b]133;C;nvim x\x07\x1b[2 q\x1b]133;D;0\x07$ "],
                ),
                want,
                "{configured:?}, one frame"
            );
            assert_eq!(
                cursor_shape_after(
                    configured,
                    &[
                        b"\x1b]133;C;nvim x\x07",
                        b"\x1b[6 q",
                        b"\x1b[2 q\x1b[?1049l",
                        b"\x1b]133;D;0\x07\x1b]133;A\x07$ ",
                    ],
                ),
                want,
                "{configured:?}, split frames"
            );
        }
    }

    #[test]
    fn reader_surfaces_auth_prompt_and_status() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        DaemonMsg::SshStatus {
            phase: SshPhase::Authenticating,
        }
        .encode(&mut daemon_side)
        .unwrap();
        DaemonMsg::AuthPrompt {
            request_id: 7,
            prompt: AuthPromptKind::Password {
                user: "deploy".into(),
                host: "10.0.0.5".into(),
            },
        }
        .encode(&mut daemon_side)
        .unwrap();
        daemon_side.flush().unwrap();

        let mut prompt = None;
        for _ in 0..200 {
            if let Some(p) = term.take_auth_prompt() {
                prompt = Some(p);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let (id, kind) = prompt.expect("auth prompt should have surfaced");
        assert_eq!(id, 7);
        assert!(matches!(kind, AuthPromptKind::Password { .. }));
        assert_eq!(term.ssh_phase(), Some(SshPhase::Authenticating));
        assert!(!term.has_pending_auth());
    }

    #[test]
    fn child_exit_is_distinguished_from_daemon_disconnect() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        DaemonMsg::Exited { code: Some(0) }
            .encode(&mut daemon_side)
            .unwrap();
        for _ in 0..200 {
            if term.exited_flag.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(term.exited_flag.load(Ordering::SeqCst));
        assert!(
            term.child_exited(),
            "an Exited frame is a genuine child exit"
        );

        let (client_side, daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        drop(daemon_side);
        for _ in 0..200 {
            if term.exited_flag.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(term.exited_flag.load(Ordering::SeqCst));
        assert!(
            !term.child_exited(),
            "a disconnect is not a child exit — auto-close must not fire"
        );
    }



    #[test]
    fn win_size_carries_grid_and_cell_dims() {
        let ws = win_size(TermSize::new(80, 24), 8, 17);
        assert_eq!(ws.cols, 80);
        assert_eq!(ws.rows, 24);
        assert_eq!(ws.cell_w, 8);
        assert_eq!(ws.cell_h, 17);
    }


    #[test]
    fn attach_replay_runs_at_the_daemon_reported_size() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        DaemonMsg::Size(WinSize {
            cols: 120,
            rows: 30,
            cell_w: 8,
            cell_h: 17,
        })
        .encode(&mut daemon_side)
        .unwrap();
        DaemonMsg::Snapshot(vec![b'x'; 100])
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();

        let (mut tail, mut wrapped_has_ink) = (' ', true);
        for _ in 0..200 {
            {
                use rio_vt::crosswords::grid::Dimensions as _;
                let t = term.term.lock();
                let grid = &t.grid;
                if grid.columns() >= 120 {
                    tail = grid[rio_vt::crosswords::pos::Line(0)]
                        [rio_vt::crosswords::pos::Column(99)]
                    .c();
                    wrapped_has_ink = !grid[rio_vt::crosswords::pos::Line(1)].is_clear();
                }
            }
            if tail == 'x' {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(tail, 'x', "replay should run at the recorded 120-col width");
        assert!(
            !wrapped_has_ink,
            "a 100-char line must not wrap on a 120-col grid"
        );
    }


    #[test]
    fn at_prompt_stays_false_while_shell_integration_is_inactive() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        assert!(!term.shell_active(), "no report yet → integration inactive");

        DaemonMsg::Prompt {
            active: false,
            at_prompt: true,
            last_exit: None,
        }
        .encode(&mut daemon_side)
        .unwrap();
        DaemonMsg::Output(b"m".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();

        let mut synced = false;
        for _ in 0..200 {
            let c = term.term.lock().grid[rio_vt::crosswords::pos::Line(0)]
                [rio_vt::crosswords::pos::Column(0)]
            .c();
            if c == 'm' {
                synced = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(synced, "reader should have applied both frames");
        assert!(!term.shell_active());
        assert!(!term.at_prompt(), "inactive shell must gate at_prompt off");
    }

    #[test]
    fn at_prompt_follows_daemon_prompt_reports() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        assert!(!term.at_prompt());

        DaemonMsg::Prompt {
            active: true,
            at_prompt: true,
            last_exit: Some(0),
        }
        .encode(&mut daemon_side)
        .unwrap();
        daemon_side.flush().unwrap();

        let mut at = false;
        for _ in 0..200 {
            if term.at_prompt() {
                at = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(at, "at_prompt should become true after the Prompt report");
    }

    /// #654, end to end on the reader: a shell at its prompt (the daemon's
    /// `Prompt` report) whose clock sits one column short of the edge, the
    /// pane narrowed by several columns at a time and widened back, each step
    /// followed by zsh's SIGWINCH redraw. Every live echo has to hand the
    /// prompt back before reflowing, or each narrowing strands the head of the
    /// old prompt and the widening joins them into a line of fragments.
    #[test]
    fn a_live_size_echo_at_a_prompt_leaves_one_prompt() {
        use rio_vt::crosswords::grid::Dimensions as _;
        use rio_vt::crosswords::pos::{Column, Line};

        crate::core::config::pin_test_config_dir();
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(98, 12)).unwrap();
        // zsh 5.9's redraw for `PROMPT='[~] '` and an `RPROMPT` clock.
        let redraw = |cols: u16, clock: &str| {
            format!(
                "\r\r\x1b[0m\x1b[J[~] \x1b[K\x1b[{}C{clock}\x1b[{}D",
                cols - 13,
                cols - 5
            )
            .into_bytes()
        };
        DaemonMsg::Prompt {
            active: true,
            at_prompt: true,
            last_exit: Some(0),
        }
        .encode(&mut daemon_side)
        .unwrap();
        DaemonMsg::Output(redraw(98, "16:46:20"))
            .encode(&mut daemon_side)
            .unwrap();
        for cols in [92, 86, 92, 98] {
            DaemonMsg::Size(WinSize {
                cols,
                rows: 12,
                cell_w: 8,
                cell_h: 17,
            })
            .encode(&mut daemon_side)
            .unwrap();
            let clock = if cols == 98 { "16:46:33" } else { "16:46:2x" };
            DaemonMsg::Output(redraw(cols, clock))
                .encode(&mut daemon_side)
                .unwrap();
        }
        daemon_side.flush().unwrap();

        let text = || {
            let t = term.term.lock();
            let grid = &t.grid;
            let top = -(grid.history_size() as i32);
            (top..grid.screen_lines() as i32)
                .map(|line| {
                    (0..grid.columns())
                        .map(|col| grid[Line(line)][Column(col)].c())
                        .collect::<String>()
                        .trim_end()
                        .to_string()
                })
                .filter(|row| !row.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        };
        for _ in 0..400 {
            if term.term.lock().columns() == 98 && text().contains("16:46:33") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let text = text();
        assert!(
            text.contains("16:46:33"),
            "the last redraw never landed:\n{text}"
        );
        assert_eq!(
            text.matches("[~]").count(),
            1,
            "a narrowing stranded part of an old prompt:\n{text}"
        );
    }

    #[test]
    fn foreground_agent_follows_daemon_agent_reports() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        assert_eq!(term.foreground_agent(), None, "none before any report");

        let poll = |want: Option<CLIAgent>| {
            for _ in 0..200 {
                if term.foreground_agent() == want {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            false
        };

        DaemonMsg::Agent(Some(CLIAgent::OhMyPi))
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        assert!(poll(Some(CLIAgent::OhMyPi)), "agent report should surface");

        DaemonMsg::Agent(None).encode(&mut daemon_side).unwrap();
        daemon_side.flush().unwrap();
        assert!(poll(None), "agent exit should clear it");
    }

    /// The daemon replays a reattached pane's stored agent status as an
    /// ordinary report. Nothing on the wire says so, so the link has to
    /// remember that the first report it hears is that replay — and that
    /// everything after it is live.
    #[test]
    fn a_reattached_link_marks_only_its_first_status_report_as_replayed() {
        use crate::core::cli_agent::{AgentSessionState, AgentStatus};

        crate::core::config::pin_test_config_dir();
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term =
            RemoteTerminal::from_stream_reattached(client_side, TermSize::new(80, 24)).unwrap();
        assert_eq!(
            term.take_replayed_agent_status(),
            None,
            "nothing replayed until the frame actually arrives"
        );

        let report = |status, daemon: &mut UnixStream| {
            DaemonMsg::AgentStatus(Some(AgentSessionState {
                status,
                message: None,
                session_id: Some("sid-1".into()),
                launch_argv: None,
                rich: true,
                cwd: None,
                activity: 0,
                turns: 0,
                pending_requests: Vec::new(),
                request_resume_status: Default::default(),
                pending_stop: false,
                readout: Default::default(),
            }))
            .encode(daemon)
            .unwrap();
            daemon.flush().unwrap();
        };
        let poll = |want: AgentStatus| {
            for _ in 0..200 {
                if term.agent_session().map(|s| s.status) == Some(want) {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            false
        };

        report(AgentStatus::Done, &mut daemon_side);
        assert!(poll(AgentStatus::Done), "the replayed status should land");
        assert_eq!(
            term.take_replayed_agent_status(),
            Some(Some(AgentStatus::Done)),
            "the first report on a reattached link is stored state"
        );
        assert_eq!(
            term.take_replayed_agent_status(),
            None,
            "only one taker gets it"
        );

        report(AgentStatus::Working, &mut daemon_side);
        assert!(poll(AgentStatus::Working), "the live status should land");
        assert_eq!(
            term.take_replayed_agent_status(),
            None,
            "everything after the replay is something the client watched happen"
        );
    }

    /// A pane with no agent session to replay sends no status frame at all, so
    /// the replay window has to close on its own — otherwise the first report
    /// from an agent launched *later* would be discounted as stored state.
    #[test]
    fn live_output_closes_the_replay_window() {
        use crate::core::cli_agent::{AgentSessionState, AgentStatus};

        crate::core::config::pin_test_config_dir();
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term =
            RemoteTerminal::from_stream_reattached(client_side, TermSize::new(80, 24)).unwrap();

        DaemonMsg::Output(b"$ omp\r\n".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        DaemonMsg::AgentStatus(Some(AgentSessionState {
            status: AgentStatus::Done,
            message: None,
            session_id: Some("sid-1".into()),
            launch_argv: None,
            rich: true,
            cwd: None,
            activity: 0,
            turns: 0,
            pending_requests: Vec::new(),
            request_resume_status: Default::default(),
            pending_stop: false,
            readout: Default::default(),
        }))
        .encode(&mut daemon_side)
        .unwrap();
        daemon_side.flush().unwrap();

        for _ in 0..200 {
            if term.agent_session().is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            term.agent_session().map(|s| s.status),
            Some(AgentStatus::Done),
            "the status still lands"
        );
        assert_eq!(
            term.take_replayed_agent_status(),
            None,
            "a report that follows live output is live"
        );
    }

    #[test]
    fn agent_session_follows_daemon_status_reports() {
        use crate::core::cli_agent::{AgentSessionState, AgentStatus};

        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        assert_eq!(term.agent_session(), None, "none before any report");

        let poll = |want: &dyn Fn(Option<AgentSessionState>) -> bool| {
            for _ in 0..200 {
                if want(term.agent_session()) {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            false
        };

        DaemonMsg::AgentStatus(Some(AgentSessionState {
            status: AgentStatus::Waiting,
            message: Some("Claude needs your permission".into()),
            session_id: Some("sid-1".into()),
            launch_argv: None,
            rich: true,
            cwd: None,
            activity: 0,
            turns: 0,
            readout: Default::default(),
            pending_requests: Vec::new(),
            request_resume_status: Default::default(),
            pending_stop: false,
        }))
        .encode(&mut daemon_side)
        .unwrap();
        daemon_side.flush().unwrap();
        assert!(
            poll(&|s| s.is_some_and(|s| s.status == AgentStatus::Waiting
                && s.session_id.as_deref() == Some("sid-1")
                && s.rich)),
            "status report should surface with message + session id"
        );

        DaemonMsg::AgentStatus(None)
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        assert!(poll(&|s| s.is_none()), "a None report clears the session");
    }

    #[test]
    fn the_spawn_directory_answers_until_the_shell_reports_its_own() {
        crate::core::config::pin_test_config_dir();
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        let poll = |want: Option<&str>| {
            let want = want.map(PathBuf::from);
            for _ in 0..200 {
                if term.foreground_cwd() == want {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            false
        };

        assert_eq!(term.foreground_cwd(), None, "nothing known before the seed");
        term.seed_cwd(Some(PathBuf::from("/repo/lyrel")));
        assert_eq!(
            term.foreground_cwd(),
            Some(PathBuf::from("/repo/lyrel")),
            "the sidebar can group this pane without waiting for the shell"
        );

        DaemonMsg::Cwd(PathBuf::from("/repo/lyrel/crates"))
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        assert!(
            poll(Some("/repo/lyrel/crates")),
            "the shell's own report replaces the seed"
        );
    }

    #[test]
    fn a_shell_report_that_beat_the_seed_wins() {
        crate::core::config::pin_test_config_dir();
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        DaemonMsg::Cwd(PathBuf::from("/somewhere/else"))
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        for _ in 0..200 {
            if term.foreground_cwd().is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        term.seed_cwd(Some(PathBuf::from("/repo/lyrel")));
        assert_eq!(
            term.foreground_cwd(),
            Some(PathBuf::from("/somewhere/else")),
            "where the shell actually landed beats where we asked it to"
        );
    }

    #[test]
    fn zle_reading_follows_live_prompt_end_marks() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        let poll = |want: bool| {
            for _ in 0..200 {
                if term.zle_reading() == want {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            false
        };
        assert!(!term.zle_reading(), "conservative false before any mark");

        DaemonMsg::Snapshot(b"\x1b]133;B\x07".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        DaemonMsg::Output(b"\x1b]133;D;0\x07m".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        let mut synced = false;
        for _ in 0..200 {
            let c = term.term.lock().grid[rio_vt::crosswords::pos::Line(0)]
                [rio_vt::crosswords::pos::Column(0)]
            .c();
            if c == 'm' {
                synced = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(synced, "reader should have applied both frames");
        assert!(
            !term.zle_reading(),
            "replayed B / live D must not arm the flag"
        );

        DaemonMsg::Output(b"\x1b]133;B\x07".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        assert!(poll(true), "live B should arm zle_reading");

        DaemonMsg::Output(b"\x1b]133;C\x07".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        assert!(poll(false), "C (command start) should disarm zle_reading");
    }

    #[test]
    fn the_running_command_is_the_line_the_shell_marked_and_ends_with_it() {
        crate::core::config::pin_test_config_dir();
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        let poll = |want: &str| {
            for _ in 0..200 {
                if term.running_command() == want {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            false
        };
        assert_eq!(term.running_command(), "", "nothing runs before a mark");

        // The escaping is the integration's; this side stores it verbatim and
        // the reader of the field undoes it.
        DaemonMsg::Output(b"\x1b]133;C;printf '100%25'\x07".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        assert!(
            poll("printf '100%25'"),
            "C should record the line it carried, got {:?}",
            term.running_command()
        );

        // Back at a prompt the command is over — holding the name would make
        // the next close question ask about something that already finished.
        DaemonMsg::Output(b"\x1b]133;B\x07".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        assert!(poll(""), "a new prompt clears the running command");

        // A bare `C` starts a command without naming it (the PowerShell path).
        // It must not leave the previous name standing.
        DaemonMsg::Output(b"\x1b]133;C;cargo build\x07".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        assert!(poll("cargo build"));
        DaemonMsg::Output(b"\x1b]133;C\x07".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        assert!(poll(""), "an unnamed command start does not inherit a name");
    }

    #[test]
    fn shell_vi_mode_follows_live_prompt_mode_marks_without_disarming_zle() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        let poll = |vi: bool, zle: bool| {
            for _ in 0..200 {
                if term.shell_vi_mode() == vi && term.zle_reading() == zle {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            false
        };

        assert!(!term.shell_vi_mode(), "conservative false before any mark");
        assert!(!term.zle_reading(), "zle also starts false");

        DaemonMsg::Output(b"\x1b]133;B\x07".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        assert!(poll(false, true), "B should arm zle only");

        DaemonMsg::Output(b"\x1b]133;V;1\x07".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        assert!(
            poll(true, true),
            "V;1 should set shell vi-mode without disarming zle"
        );

        DaemonMsg::Output(b"\x1b]133;V;0\x07".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        assert!(
            poll(false, true),
            "V;0 should clear shell vi-mode without disarming zle"
        );

        DaemonMsg::Output(b"\x1b]133;C\x07".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        assert!(poll(false, false), "C still disarms zle");
    }

    #[test]
    fn shell_vi_mode_is_restored_from_snapshot_replay() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        let poll = |vi: bool| {
            for _ in 0..200 {
                if term.shell_vi_mode() == vi {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            false
        };

        DaemonMsg::Snapshot(b"\x1b]133;V;1\x07".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        assert!(
            poll(true),
            "attached clients should inherit the prompt's vi-mode state"
        );
        assert!(
            !term.zle_reading(),
            "historical replay must not imply zle is currently reading"
        );

        DaemonMsg::Snapshot(b"\x1b]133;V;0\x07".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        assert!(poll(false), "a replayed V;0 should clear vi-mode state");
    }

    fn full_dump(term: &RemoteTerminal) -> String {
        use rio_vt::crosswords::grid::Dimensions as _;
        let t = term.term.lock();
        let grid = &t.grid;
        let mut out = String::new();
        for l in -(grid.history_size() as i32)..grid.screen_lines() as i32 {
            for c in 0..grid.columns() {
                out.push(
                    grid[rio_vt::crosswords::pos::Line(l)][rio_vt::crosswords::pos::Column(c)].c(),
                );
            }
            out.push('\n');
        }
        out
    }

    fn tui_frame(lines: &[String], prev_rows: usize) -> Vec<u8> {
        let mut b = Vec::new();
        if prev_rows > 1 {
            b.extend_from_slice(format!("\r\x1b[{}A\x1b[J", prev_rows - 1).as_bytes());
        }
        b.extend_from_slice(lines.join("\r\n").as_bytes());
        b
    }

    #[test]
    fn segmented_ring_replay_reproduces_live_rendering() {
        const MARK: &str = "DUPMARK";
        let frame_lines = |f: usize| -> Vec<String> {
            (0..10)
                .map(|i| format!("{MARK} f{f:02} l{i:02} {:.<74}", ""))
                .collect()
        };
        let mut history = Vec::new();
        for f in 0..8 {
            history.extend(tui_frame(&frame_lines(f), if f == 0 { 0 } else { 10 }));
        }

        let wait_for = |term: &RemoteTerminal, needle: &str| -> String {
            let mut dump = String::new();
            for _ in 0..400 {
                dump = full_dump(term);
                if dump.contains(needle) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            dump
        };
        let ws = |cols: u16| WinSize {
            cols,
            rows: 24,
            cell_w: 8,
            cell_h: 17,
        };

        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let mut live = RemoteTerminal::from_stream(client_side, TermSize::new(100, 24)).unwrap();
        DaemonMsg::Output(history.clone())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        let dump = wait_for(&live, "f07 l09");
        assert!(dump.contains("f07 l09"), "live output should have landed");
        live.resize(TermSize::new(80, 24), 8, 17);
        let live_count = full_dump(&live).matches(MARK).count();
        assert_eq!(
            live_count, 10,
            "live rendering is clean: each redraw erases the previous frame, \
             so exactly one 10-line copy survives the resize"
        );

        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let replay = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        DaemonMsg::Size(ws(100)).encode(&mut daemon_side).unwrap();
        DaemonMsg::Snapshot(history.clone())
            .encode(&mut daemon_side)
            .unwrap();
        DaemonMsg::Size(ws(80)).encode(&mut daemon_side).unwrap();
        DaemonMsg::Snapshot(Vec::new())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        let dump = wait_for(&replay, "f07 l09");
        {
            let mut cols = 0;
            for _ in 0..400 {
                cols = replay.term.lock().columns();
                if cols == 80 {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert_eq!(cols, 80, "the trailing pair must end the grid at 80 cols");
        }
        let replay_count = dump.matches(MARK).count();
        assert_eq!(
            replay_count, live_count,
            "the segmented replay must reproduce the live rendering exactly"
        );

        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let flat = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        DaemonMsg::Size(ws(80)).encode(&mut daemon_side).unwrap();
        DaemonMsg::Snapshot(history)
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        let dump = wait_for(&flat, "f07 l09");
        let flat_count = dump.matches(MARK).count();
        assert!(
            flat_count > live_count,
            "flat replay at the final width should duplicate (got {flat_count}); \
             if it stopped, the segmented path may no longer be exercising anything"
        );
    }
}

