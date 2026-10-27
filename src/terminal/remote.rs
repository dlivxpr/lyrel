use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use rio_vt::ansi::CursorShape;
use rio_vt::clipboard::ClipboardType;
use rio_vt::config::colors::ColorRgb;
use rio_vt::crosswords::Crosswords;
use rio_vt::crosswords::Mode;
use rio_vt::crosswords::grid::Dimensions as _;
use rio_vt::event::{EventListener, RioEvent, WindowId};
use rio_vt::performer::handler::Handler as _;

use crate::terminal::bridge::ParseBridge;
use parking_lot::FairMutex;

use std::collections::VecDeque;

use crate::core::cli_agent::{AgentSessionState, AgentStatus, CLIAgent};
use crate::core::config::CursorStyle as ConfigCursorStyle;
use crate::core::osc::{Note, NoteBudget};
use crate::daemon::protocol::{
    AuthPromptKind, AuthResponse, ClientMsg, DaemonMsg, LoopbackForward, LoopbackForwardRequest,
    ManagedForward, NativeSshSpec, PaneProcs, RemoteContext, RestoreFrom, SftpEntry,
    SftpJobProgress, SftpOp, SftpOpResult, SftpTransferSpec, ShellSpec, SshForwardRule, SshPhase,
    WinSize, WorkspaceOp, WorkspaceRequest,
};
use crate::daemon::transport::{self, Stream};
use lyrel_core::core::term_modes::KeyboardStacks;

use super::size::TermSize;

mod reader;
#[cfg(test)]
mod test_support;

use reader::ReaderTask;

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

/// How much unsent input a pane holds before it calls the link lost. A link
/// that is draining never gets near this: the sender thread is parked waiting
/// for work, so the queue holds at most the frames of one burst. Reaching it
/// means nothing on the far side has taken a byte for as long as it takes to
/// type — or paste — four megabytes, which is a dead link, not a slow one.
const MAX_BACKLOG: usize = 4 << 20;

/// How long a teardown lets the sender finish what is already queued before it
/// cuts the socket out from under it. `Detach` is the last frame a pane sends
/// and it is worth a moment: on a draining link the sender is idle, so it goes
/// out in microseconds and this returns at once. On a link that has stopped
/// draining it will never go out at all, and closing the pane must not wait
/// around to discover that — the daemon reads the closed socket as a detach
/// anyway.
const DRAIN_GRACE: std::time::Duration = std::time::Duration::from_millis(50);

/// How long a resize the daemon promised to echo is taken to still be on its
/// way. Past this, a grid that has not reached the requested geometry is not
/// waiting on anything: the request or its echo went missing, and the size is
/// sent again. The local daemon echoes in well under a millisecond and a
/// routed one within a round trip; a resend that beats a slow echo costs one
/// `TIOCSWINSZ` of the size the pty already has, which signals nobody.
const RESIZE_ECHO_GRACE: std::time::Duration = std::time::Duration::from_secs(1);

/// Whether `term`'s grid is `size`, as the emulator would size it — it never
/// goes below `MIN_COLUMNS` × `MIN_SCREEN_LINES`, so a sliver of a pane asking
/// for one column is already as narrow as it can get.
fn grid_holds(term: &Crosswords<EventProxy>, size: TermSize) -> bool {
    use super::size::{MIN_COLUMNS, MIN_SCREEN_LINES};
    term.columns() == size.cols.max(MIN_COLUMNS)
        && term.screen_lines() == size.rows.max(MIN_SCREEN_LINES)
}

/// What the sender thread needs to report a link that stopped taking input.
/// The signals the pane holds, cloned out so the failure can be raised from the
/// thread that actually meets it.
#[derive(Clone)]
struct InputLoss {
    /// Set by the first frame *this* link refused, so the loss is said once
    /// rather than once per keystroke. One per link rather than one per pane: a
    /// relink hands the retired sender's last, doomed write and the new
    /// sender's first real one two different flags, so the dying link cannot
    /// spend the new link's one chance to speak.
    said: Arc<AtomicBool>,
    reader_quit: Arc<AtomicBool>,
    exited: Arc<AtomicBool>,
    proxy: EventProxy,
}

impl InputLoss {
    fn new(reader_quit: Arc<AtomicBool>, exited: Arc<AtomicBool>, proxy: EventProxy) -> InputLoss {
        InputLoss {
            said: Arc::new(AtomicBool::new(false)),
            reader_quit,
            exited,
            proxy,
        }
    }

    /// The link refused a frame. Every keystroke after the first would say the
    /// same thing, so this side says it once; and unless the reader has been
    /// retired for a relink or a release, the pane is marked exited the way the
    /// reader marks it on EOF — it is the same socket, noticed from the writing
    /// side first — so the window shows the pane as gone instead of taking
    /// input into it that nothing will ever read. The reader still raises its
    /// own `Exit` when it finds the same socket closed; the handler is
    /// idempotent, so a link that is genuinely gone may be reported twice.
    ///
    /// This is hardening for a *closed* link, not the cure for #673: a socket
    /// some process holds open and never reads accepts writes into its send
    /// buffer, so they succeed and vanish until the buffer fills, and nothing
    /// here fires until the backlog bound does. What stops that pane existing
    /// at all is `attach_on` refusing to call a silent `Attach` attached.
    fn note(&self, err: &std::io::Error) {
        if self.said.swap(true, Ordering::SeqCst) {
            return;
        }
        log::warn!("the daemon link stopped taking this pane's input: {err}");
        if self.reader_quit.load(Ordering::SeqCst) {
            return;
        }
        self.exited.store(true, Ordering::SeqCst);
        self.proxy.send(PaneEvent::Wakeup);
        self.proxy.send(PaneEvent::Exit);
    }
}

#[derive(Default)]
struct SendQueue {
    frames: VecDeque<Vec<u8>>,
    /// Bytes queued but not yet on the wire — including the batch the sender
    /// currently holds, which is why this is not just `frames`' total. That
    /// batch is exactly what a stalled link parks in, so leaving it out would
    /// mean the backlog bound could never be reached.
    bytes: usize,
    /// How much of `bytes` belongs to frames that were over the whole bound on
    /// their own, and so were let through on their own terms. The bound is
    /// raised by exactly this while they are outstanding, and put back the
    /// moment the queue empties — otherwise one big paste would leave every
    /// keystroke behind it looking like a dead link.
    oversize: usize,
    /// Set by teardown, or by a sender that has given up: either way the queue
    /// takes nothing more. Teardown's sender writes what is left first.
    closing: bool,
    /// Set by the sender once there is nothing left to write. Teardown waits
    /// on this for `DRAIN_GRACE`, no longer.
    drained: bool,
}

/// A pane's writing half, moved onto a thread of its own.
///
/// The socket underneath is blocking and has no write timeout, so a peer that
/// stops reading parks `write(2)` in the kernel until it starts again. Every
/// caller of `RemoteTerminal::write` is a gpui event handler — a keystroke, a
/// paste, a mouse report, a focus change — and one UI thread draws every
/// window, so a park there is every window frozen. On macOS a unix stream gives
/// up after 8K of send buffer, which is about 1400 keystrokes: a single paste.
///
/// So nothing on the UI thread touches the socket. Frames are encoded, queued,
/// and handed to a sender thread that is welcome to park for as long as the far
/// end makes it.
struct LinkWriter {
    /// A second handle on the same socket, kept for `shutdown` alone.
    /// `shutdown(2)` returns at once even while another thread is parked in
    /// `write(2)` on that socket — which is exactly the state teardown has to
    /// be able to break, and exactly what a handle behind the sender's own lock
    /// could not do.
    closer: Stream,
    queue: Arc<(Mutex<SendQueue>, std::sync::Condvar)>,
    loss: InputLoss,
    thread: Option<JoinHandle<()>>,
}

#[derive(Clone)]
struct LinkSender {
    queue: Arc<(Mutex<SendQueue>, std::sync::Condvar)>,
    loss: InputLoss,
}

impl LinkSender {
    fn send(&self, msg: ClientMsg) {
        Self::enqueue(&self.queue, &self.loss, msg);
    }

    fn enqueue(
        queue: &Arc<(Mutex<SendQueue>, std::sync::Condvar)>,
        loss: &InputLoss,
        msg: ClientMsg,
    ) {
        let mut frame = Vec::new();
        if let Err(error) = msg.encode(&mut frame) {
            log::warn!("could not encode a frame for this pane's link: {error}");
            return;
        }
        let (lock, wake) = &**queue;
        let Ok(mut q) = lock.lock() else {
            return;
        };
        if q.closing {
            return;
        }
        let oversize = frame.len() > MAX_BACKLOG && q.bytes == 0;
        if !oversize && q.bytes + frame.len() > MAX_BACKLOG + q.oversize {
            drop(q);
            loss.note(&std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                format!("nothing has drained this pane's link for {MAX_BACKLOG} bytes of input"),
            ));
            return;
        }
        if oversize {
            q.oversize = frame.len();
        }
        q.bytes += frame.len();
        q.frames.push_back(frame);
        q.drained = false;
        drop(q);
        wake.notify_one();
    }
}

impl LinkWriter {
    fn new(stream: Stream, loss: InputLoss) -> std::io::Result<LinkWriter> {
        let closer = stream.try_clone()?;
        let queue = Arc::new((Mutex::new(SendQueue::default()), std::sync::Condvar::new()));
        let sending = Arc::clone(&queue);
        let sender_loss = loss.clone();
        let thread = std::thread::Builder::new()
            .name("lyrel-pane-writer".into())
            .spawn(move || send_loop(stream, sending, sender_loss))?;
        Ok(LinkWriter {
            closer,
            queue,
            loss,
            thread: Some(thread),
        })
    }

    /// Queues a frame. Never blocks: the socket belongs to the sender thread,
    /// and the only thing that happens here is a push onto a `VecDeque`.
    fn send(&self, msg: ClientMsg) {
        LinkSender::enqueue(&self.queue, &self.loss, msg);
    }

    fn sender(&self) -> LinkSender {
        LinkSender {
            queue: self.queue.clone(),
            loss: self.loss.clone(),
        }
    }

    /// Retires the sender and cuts the link. Gives what is queued `DRAIN_GRACE`
    /// to go out — see the constant — and then shuts the socket down whether it
    /// went or not. Never joins: a sender parked on a dead link would take the
    /// UI thread down with it, the same trap `stop_reader` documents.
    ///
    /// Called twice on the way out — `stop_reader` closes the link, then the
    /// field drop closes it again — so the second call has to be free rather
    /// than another `DRAIN_GRACE` spent waiting for a sender that is already
    /// gone. A retired handle is one whose thread has been let go.
    fn close(&mut self) {
        if self.thread.is_none() {
            return;
        }
        let (lock, wake) = &*self.queue;
        if let Ok(mut q) = lock.lock() {
            q.closing = true;
            wake.notify_one();
            if let Ok((waited, _)) = wake.wait_timeout_while(q, DRAIN_GRACE, |q| !q.drained) {
                drop(waited);
            }
        }
        let _ = self.closer.shutdown(std::net::Shutdown::Both);
        drop(self.thread.take());
    }
}

impl Drop for LinkWriter {
    fn drop(&mut self) {
        self.close();
    }
}

fn send_loop(
    mut stream: Stream,
    queue: Arc<(Mutex<SendQueue>, std::sync::Condvar)>,
    loss: InputLoss,
) {
    use std::io::Write as _;
    let (lock, wake) = &*queue;

    // Abandons whatever is still queued and reports the link settled. Wakes a
    // teardown that is inside `DRAIN_GRACE` waiting to hear it: what is left
    // here is never going out, and there is nothing to be gained by making the
    // window sit out the rest of the grace period to find that out. Closes the
    // queue on the way, too — with no thread left to drain it, anything queued
    // after this is just a keystroke held onto until the pane drops.
    let give_up = || {
        if let Ok(mut q) = lock.lock() {
            q.frames.clear();
            q.bytes = 0;
            q.oversize = 0;
            q.closing = true;
            q.drained = true;
            wake.notify_all();
        }
    };

    loop {
        let batch = {
            let Ok(mut q) = lock.lock() else { return };
            loop {
                if !q.frames.is_empty() {
                    break;
                }
                // Nothing left to write, so the link is as flushed as this
                // thread can make it. Said before the `closing` check, so a
                // teardown racing a sender that has already finished hears it
                // rather than waiting out `DRAIN_GRACE` for nothing.
                q.drained = true;
                wake.notify_all();
                if q.closing {
                    return;
                }
                let Ok(next) = wake.wait(q) else { return };
                q = next;
            }
            std::mem::take(&mut q.frames)
        };
        // Counted against the backlog until it is actually out. Discounting it
        // at the moment it left the `VecDeque` would let a sender parked on the
        // first frame of a huge batch hold the whole thing off the books, and
        // the bound the batch is meant to enforce would never be reached.
        let taken: usize = batch.iter().map(Vec::len).sum();
        // Written with the lock released: parking here is the whole point, and
        // a sender holding the queue lock while it parked would put every
        // `send` — every keystroke — behind the same wait it exists to absorb.
        for frame in batch {
            if let Err(e) = stream.write_all(&frame) {
                loss.note(&e);
                give_up();
                return;
            }
        }
        if let Err(e) = stream.flush() {
            loss.note(&e);
            give_up();
            return;
        }
        if let Ok(mut q) = lock.lock() {
            q.bytes = q.bytes.saturating_sub(taken);
            if q.bytes == 0 {
                q.oversize = 0;
            }
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
    size: TermSize,
    synced_size: bool,
    /// This link reattached to a pane that was already running rather than
    /// spawning a fresh one, so whatever it reports first — an agent in the
    /// foreground included — predates this client.
    reattached: bool,
    /// The `(cell_w, cell_h)` last sent to the daemon, in device pixels. Tracked
    /// alongside `size` so a display-scale change still reaches the child even
    /// when the grid dimensions are unchanged.
    synced_cell: (u16, u16),
    /// When the last `ClientMsg::Resize` went down the link. On an echoing
    /// route it is what tells "the echo is still in flight" apart from "the
    /// echo is never coming" — see [`RESIZE_ECHO_GRACE`].
    resize_sent_at: Option<std::time::Instant>,
    link: LinkWriter,
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
    viewport_reporter: GraphicsViewportReporter,
    clipboard_writes: Arc<Mutex<VecDeque<lyrel_core::core::clipboard::ClipboardWrite>>>,
    osc_notes: OscNotes,
    /// On the pane rather than its view so it survives a relink; only the
    /// view's poll touches it.
    note_budget: Mutex<NoteBudget>,
    clipboard_write_busy: Arc<AtomicBool>,
    route: PaneRoute,
    proxy: EventProxy,
    spawn_claim: Option<(u64, WinSize)>,
    reader: ReaderTask,
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

/// The workspace id a spawn carries, so the pane's shell gets `$LYREL_WS` and a
/// CLI running in it can use workspace-scoped verbs with no argument.
///
/// This is the same id as `owner`, but decided separately: `owner` is also
/// gated on FEATURE_PANE_OWNER, while the workspace field rides the owned spawn
/// kind and needs no feature probe. Local routes only — a remote server keeps
/// its own machine tree, and this id names a workspace in ours.
fn spawn_workspace(owner: Option<&str>, route: &PaneRoute) -> Option<String> {
    owner.filter(|_| route.is_local()).map(|id| id.to_string())
}

impl RemoteTerminal {
    pub fn spawn_on(
        route: &PaneRoute,
        size: TermSize,
        cell_w: u16,
        cell_h: u16,
        cwd: Option<PathBuf>,
        shell: Option<ShellSpec>,
        owner: Option<String>,
        restore: Option<RestoreFrom>,
        cancellation: Option<&SpawnCancellation>,
    ) -> anyhow::Result<(Self, u64)> {
        let retry_cwd = cwd.clone();
        let retry_shell = shell.clone();
        let retry_owner = owner.clone();
        let retry_restore = restore.clone();
        match Self::spawn_once(
            route,
            size,
            cell_w,
            cell_h,
            cwd,
            shell,
            owner,
            restore,
            cancellation,
        ) {
            Ok(term) => Ok(term),
            Err(first_err) if route.is_local() && daemon_not_listening(&first_err) => {
                log::warn!(
                    "local daemon unavailable during spawn; attempting recovery: {first_err:#}"
                );
                if let Err(start_err) = crate::daemon::spawn::ensure_running() {
                    return Err(anyhow::anyhow!(
                        "daemon not running ({first_err}); starting one failed: {start_err}"
                    ));
                }
                Self::spawn_once(route, size, cell_w, cell_h, retry_cwd, retry_shell, retry_owner, retry_restore, cancellation)
                    .map_err(|second_err| {
                        anyhow::anyhow!(
                            "daemon not running ({first_err}); started one but Spawn still failed: {second_err}"
                        )
                    })
            }
            Err(first_err)
                if route.is_local() && daemon_disconnected_before_spawn_reply(&first_err) =>
            {
                log::warn!(
                    "spawn connection ended before reply; probing daemon without forcing a restart: {first_err:#}"
                );
                if let Err(restart_err) = crate::daemon::spawn::ensure_running() {
                    return Err(anyhow::anyhow!(
                        "daemon disconnected before Spawn reply ({first_err}); recovery failed: {restart_err}"
                    ));
                }
                Self::spawn_once(route, size, cell_w, cell_h, retry_cwd, retry_shell, retry_owner, retry_restore, cancellation).map_err(|second_err| {
                    anyhow::anyhow!(
                        "daemon disconnected before Spawn reply ({first_err}); recovery attempted but Spawn still failed: {second_err}"
                    )
                })
            }
            Err(err) => Err(err),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_once(
        route: &PaneRoute,
        size: TermSize,
        cell_w: u16,
        cell_h: u16,
        cwd: Option<PathBuf>,
        shell: Option<ShellSpec>,
        owner: Option<String>,
        restore: Option<RestoreFrom>,
        cancellation: Option<&SpawnCancellation>,
    ) -> anyhow::Result<(Self, u64)> {
        let mut stream = connect_routed(route)?;
        Self::negotiate_pane_protocol(&mut stream, attach_reply_wait(route))?;
        let images = crate::terminal::images::ImageStore::new();
        images.set_cell_metrics(cell_w, cell_h);
        Self::send_graphics_capacity(&mut stream, &images)?;
        let await_claim = route.is_local() && cancellation.is_some();
        if await_claim
            && !crate::daemon::spawn::local_daemon_supports(
                crate::daemon::protocol::FEATURE_SPAWN_CLAIM,
            )
        {
            anyhow::bail!(
                "the local daemon does not support cancellable pane creation; restart it to update"
            );
        }
        let win = win_size(size, cell_w, cell_h);

        let workspace = spawn_workspace(owner.as_deref(), route);
        let spawned_in = cwd.clone();

        let owner = owner.filter(|_| {
            route.is_local()
                && crate::daemon::spawn::local_daemon_supports(
                    crate::daemon::protocol::FEATURE_PANE_OWNER,
                )
        });

        // Only the local daemon's feature list is known here; a routed spawn
        // reaches a server whose build we have not asked about, and one that
        // predates the field would silently drop it. Sending it anyway would
        // cost nothing but would make the log claim a restore that never
        // happened, so the local case is the only one that asks.
        let restore = restore.and_then(|want| {
            let local = route.is_local();
            let supported = crate::daemon::spawn::local_daemon_supports(
                crate::daemon::protocol::FEATURE_RESTORE_SCROLLBACK,
            );
            if local && supported {
                return Some(want);
            }
            // Said out loud, because the symptom of dropping it here is a pane
            // that opens blank — which is also what a pane with nothing stored
            // looks like, and what a daemon that refused would produce. Three
            // causes, one appearance; without this line the only way to tell
            // them apart is to read the source.
            log::info!(
                "not asking to restore pane {}'s screen: local={local} \
                 daemon-supports-restore={supported}",
                want.pane_id
            );
            None
        });

        ClientMsg::Spawn {
            cwd,
            size: win,
            shell,
            owner,
            workspace,
            restore,
            allow_remote_clipboard_write: route.allow_remote_clipboard_write(),
            await_claim,
        }
        .encode(&mut stream)?;
        let reply = if let Some(cancellation) = cancellation.filter(|_| await_claim) {
            cancellable_spawn_reply_with_images(&mut stream, cancellation, &images)?
        } else {
            spawn_reply(&mut stream, attach_reply_wait(route), "Spawn", &images)?
        };
        let pane_id = match reply {
            DaemonMsg::Spawned { pane_id } => pane_id,
            // Passed through, not wrapped: the caller already logs which
            // spawn this was, and the window shows only this text.
            DaemonMsg::Error(msg) => return Err(anyhow::anyhow!(msg)),
            other => {
                return Err(anyhow::anyhow!(
                    "unexpected daemon reply to Spawn: {other:?}"
                ));
            }
        };

        let mut term = Self::from_stream_parts(
            stream,
            size,
            Vec::new(),
            PtySource::for_route(route),
            false,
            images,
        )?;
        term.route = route.clone();
        term.spawn_claim = await_claim.then_some((pane_id, win));
        term.seed_cwd(spawned_in);
        Ok((term, pane_id))
    }

    fn negotiate_pane_protocol(
        stream: &mut Stream,
        wait: std::time::Duration,
    ) -> anyhow::Result<()> {
        let _ = stream.set_read_timeout(Some(wait));
        let result = crate::daemon::protocol::negotiate_pane(stream);
        let _ = stream.set_read_timeout(None);
        result?;
        Ok(())
    }
    fn send_graphics_capacity(
        stream: &mut Stream,
        images: &crate::terminal::images::ImageStore,
    ) -> anyhow::Result<()> {
        let (encoded_bytes, decoded_bytes) = images.graphics_capacity();
        ClientMsg::GraphicsCapacity {
            encoded_bytes,
            decoded_bytes,
            offers: true,
        }
        .encode(stream)?;
        Ok(())
    }

    pub(crate) fn claim_spawn(&mut self) {
        if let Some((pane_id, size)) = self.spawn_claim.take() {
            self.link.send(ClientMsg::Attach {
                pane_id,
                size,
                allow_remote_clipboard_write: self.route.allow_remote_clipboard_write(),
            });
        }
    }

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

    pub fn attach_on(
        route: &PaneRoute,
        size: TermSize,
        cell_w: u16,
        cell_h: u16,
        pane_id: u64,
    ) -> anyhow::Result<Self> {
        let mut stream = connect_routed(route)?;
        Self::negotiate_pane_protocol(&mut stream, attach_reply_wait(route))?;
        let images = crate::terminal::images::ImageStore::new();
        images.set_cell_metrics(cell_w, cell_h);
        Self::send_graphics_capacity(&mut stream, &images)?;
        let win = win_size(size, cell_w, cell_h);

        ClientMsg::Attach {
            pane_id,
            size: win,
            allow_remote_clipboard_write: route.allow_remote_clipboard_write(),
        }
        .encode(&mut stream)?;
        let buffered = attach_reply_prefix_with_images(
            &mut stream,
            pane_id,
            attach_reply_wait(route),
            &images,
        )?;
        let mut term = Self::from_stream_parts(
            stream,
            size,
            buffered,
            PtySource::for_route(route),
            true,
            images,
        )?;
        term.route = route.clone();
        Ok(term)
    }

    /// Opens the daemon connection a relink will adopt, and waits for the
    /// daemon's verdict on the `Attach`. Bytes read past the verdict are
    /// returned so the adopted reader loses none of the replay. Waiting here
    /// is what makes a relink's failure classifiable: fire-and-forget handed
    /// a "no such pane" refusal to the reader thread, which could only die
    /// with it — indistinguishable from the link dropping again.
    pub fn open_relink(
        route: &PaneRoute,
        pane_id: u64,
        size: TermSize,
        cell_w: u16,
        cell_h: u16,
        images: crate::terminal::images::ImageStore,
    ) -> anyhow::Result<(Stream, Vec<u8>)> {
        let mut stream = connect_routed(route)?;
        Self::negotiate_pane_protocol(&mut stream, attach_reply_wait(route))?;
        Self::send_graphics_capacity(&mut stream, &images)?;
        ClientMsg::Attach {
            pane_id,
            size: win_size(size, cell_w, cell_h),
            allow_remote_clipboard_write: route.allow_remote_clipboard_write(),
        }
        .encode(&mut stream)?;
        let buffered = attach_reply_prefix_with_images(
            &mut stream,
            pane_id,
            attach_reply_wait(route),
            &images,
        )?;
        Ok((stream, buffered))
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
        self.stop_reader();
        while self.events.try_recv().is_ok() {}
        if let Ok(mut pending) = self.clipboard_writes.lock() {
            pending.clear();
        }
        self.clipboard_write_busy.store(false, Ordering::Release);

        let read_half = stream.try_clone()?;

        // A relink keeps the view, and the view keeps the status it last saw
        // before the link dropped. That is already a baseline, and the better
        // one: if the agent finished its turn while the link was down, the
        // daemon's replayed `Done` against the view's `Working` is exactly the
        // edge the reader must be told about. So the relink's replay is read
        // as a live report, and a cold-attach mark nobody took yet is dropped
        // rather than left to swallow that edge.
        if let Ok(mut guard) = self.agent_session.lock() {
            guard.replayed = false;
        }

        self.exited_flag.store(false, Ordering::SeqCst);
        self.exited = false;
        {
            let mut term = self.term.lock();
            ParseBridge::new(
                &mut term,
                self.proxy.clone(),
                &mut KeyboardStacks::default(),
                Some(self.proxy.parse_state.history_limit.load(Ordering::Relaxed)),
            )
            .reset_state();
        }
        // The grid was just reset, so every image anchor now points nowhere.
        // Drop them; the daemon does not replay out-of-band image frames, so a
        // browser redraws on its next transmit (see issue #213's reattach note).
        self.images.clear_for_relink();

        let quit = Arc::new(AtomicBool::new(false));
        let link = LinkWriter::new(
            stream,
            InputLoss::new(quit.clone(), self.exited_flag.clone(), self.proxy.clone()),
        )?;
        let reader = ReaderTask::spawn(
            self.term.clone(),
            self.proxy.clone(),
            read_half,
            buffered,
            quit.clone(),
            ReaderSignals {
                cwd: self.cwd.clone(),
                shell: self.shell_state.clone(),
                remote: self.remote_context.clone(),
                agent: self.agent.clone(),
                agent_session: self.agent_session.clone(),
                // The daemon does replay the stored status down this link, but
                // the view already has a baseline from before the drop — see
                // above.
                awaiting_replay: false,
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
                // Deliberately the pane's existing answer rather than one
                // rebuilt from `route`: the pty on the far side is the same pty
                // it was before the link dropped, and only this value still
                // remembers what a `RemoteContext` taught the old reader.
                local_conpty: self.local_conpty.clone(),
                color_scheme_updates: self.color_scheme_updates.clone(),
            },
            link.sender(),
            self.viewport_reporter.clone(),
        );
        self.reader = reader;
        // Installed after the reader, so the sender reports a refusal
        // against the reader this link actually has. Assigning retires the old
        // `LinkWriter` through its `Drop`, which is what closes the old socket.
        self.link = link;
        *self.viewport_reporter.link.lock().unwrap() = (self.link.sender(), None);
        // The new reader can establish its Screen before the sender swap.
        // Re-publish on this link even when the pane stays hidden afterward.
        self.viewport_reporter.report();
        self.route = route.clone();
        self.synced_size = false;
        self.resize(size, cell_w, cell_h);
        Ok(())
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
        let read_half = stream.try_clone()?;
        let write_half = stream;

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

        let reader_quit = Arc::new(AtomicBool::new(false));
        let local_conpty = Arc::new(AtomicBool::new(pty == PtySource::LocalConpty));
        let color_scheme_updates = Arc::new(AtomicBool::new(false));
        let link = LinkWriter::new(
            write_half,
            InputLoss::new(reader_quit.clone(), exited_flag.clone(), proxy.clone()),
        )?;
        let viewport_reporter =
            GraphicsViewportReporter::new(&term, images.clone(), link.sender())?;
        let reader = ReaderTask::spawn(
            term.clone(),
            proxy.clone(),
            read_half,
            buffered,
            reader_quit.clone(),
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
            link.sender(),
            viewport_reporter.clone(),
        );

        Ok(Self {
            viewport_reporter,
            term,
            events: rx,
            palette: super::palette::build(),
            exited: false,
            size,
            synced_size: false,
            reattached: awaiting_replay,
            synced_cell: (0, 0),
            resize_sent_at: None,
            spawn_claim: None,
            link,
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
            route: PaneRoute::Local,
            proxy,
            reader,
            local_conpty,
            color_scheme_updates,
        })
    }

    pub fn detach_link(&mut self) {
        self.link.send(ClientMsg::Detach);
        self.stop_reader();
        self.poll_exited();
    }

    /// Retires ordered consumption before the writer closes its socket.
    fn stop_reader(&mut self) {
        self.reader.retire(&self.images);
        // The sender owns the socket now, and closing it is its job: `close`
        // gives whatever is queued a brief moment to go out and then shuts the
        // socket down regardless, which is also what wakes the reader.
        self.link.close();
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
        self.link.send(ClientMsg::Input(bytes.into_owned()));
    }

    /// Whether the daemon behind this pane echoes a `DaemonMsg::Size` into the
    /// output stream when it applies our `ClientMsg::Resize`. When it does, the
    /// local grid reflow is deferred to that echo on the reader thread: any
    /// backlog still queued between daemon and client was produced at the old
    /// geometry, and reflowing before it drains parses old-width bytes into a
    /// new-width grid (maximize during a burst of output garbled the pane).
    ///
    /// The local daemon is probed directly. A routed pane consults the answer
    /// its route carries, read off the host's control hello when the route was
    /// built — the daemon serves one `ClientMsg` per connection, so asking
    /// `Version` per pane would cost a whole routed connection each time. The
    /// remaining cases never defer: an older server does not advertise the
    /// echo, an unroutable route reaches no daemon at all, and deferral must
    /// not hang the grid on an echo nobody promised.
    fn resize_echoed(&self) -> bool {
        match &self.route {
            PaneRoute::Local => crate::daemon::spawn::local_daemon_supports(
                crate::daemon::protocol::FEATURE_RESIZE_ECHO,
            ),
            PaneRoute::Remote { resize_echo, .. } => *resize_echo,
            PaneRoute::Unroutable(_) => false,
        }
    }

    /// Live panes clear only through the daemon's ordered output stream. Once
    /// the reader has observed exit/EOF, the retained presentation belongs to
    /// this client and must remain clearable without a functioning transport.
    /// Returns whether the caller should ask the live child to redraw.
    pub fn clear_presentation(&mut self, history: bool) -> bool {
        self.poll_exited();
        if !self.exited && !self.child_exited() {
            self.link.send(ClientMsg::ClearPresentation { history });
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
        self.images.set_cell_metrics(cell_w, cell_h);
        let echoed = self.resize_echoed();
        // The cell size has to be part of the early-out, not just cols/rows: it
        // is reported in *device* pixels, so moving the window between a 2x and
        // a 1x display changes `ws_xpixel`/`ws_ypixel` while the grid stays
        // exactly the same. Comparing only `size` there would skip the resize
        // and leave a pixel-aware child rendering for the old framebuffer.
        let cell = (cell_w, cell_h);
        if self.synced_size && size == self.size && cell == self.synced_cell {
            // Already asked for, so the only question is whether the grid got
            // there. It is not enough to trust the request (#893): on an
            // echoing route the grid moves only when the daemon's `Size` comes
            // back, and a request or an echo that went missing left a pane
            // painting a grid shorter than its bounds — the bottom rows blank,
            // the child still drawing for the old size — with every later
            // frame asking for the same size and being skipped here, until a
            // divider drag asked for a different one.
            if echoed {
                // Disagreement while an echo can still be on its way is just
                // the echo (or a replay segment mid-apply) not having landed
                // yet. Asked first because it needs no lock.
                if self
                    .resize_sent_at
                    .is_some_and(|at| at.elapsed() < RESIZE_ECHO_GRACE)
                {
                    return;
                }
                // Tried rather than waited for, as the painter does: this runs
                // every frame, and a held lock is the reader mid-batch — the
                // next frame asks again.
                match self.term.try_lock() {
                    Some(term) if !grid_holds(&term, size) => {}
                    _ => return,
                }
            } else if grid_holds(&self.term.lock(), size) {
                return;
            }
        }
        self.synced_size = true;
        self.size = size;
        self.synced_cell = cell;
        *self.proxy.parse_state.paint_cursor.lock().unwrap() = None;
        if !echoed {
            let shell_redraws = !self.is_local_conpty() && self.at_prompt();
            super::prompt_reflow::resize(
                &mut self.term.lock(),
                size,
                shell_redraws,
                self.is_local_conpty(),
            );
        }

        let win = win_size(size, cell_w, cell_h);
        self.link.send(ClientMsg::Resize(win));
        self.resize_sent_at = Some(std::time::Instant::now());
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
        self.viewport_reporter.clone()
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
        self.size
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

    /// `remote_start_dir` is a directory on the far host, carried as
    /// [`ClientMsg::SpawnNativeSsh`]'s `cwd`.
    pub fn spawn_native_ssh(
        size: TermSize,
        cell_w: u16,
        cell_h: u16,
        remote_start_dir: Option<PathBuf>,
        spec: Box<NativeSshSpec>,
    ) -> anyhow::Result<(Self, u64)> {
        match Self::spawn_native_ssh_once(
            size,
            cell_w,
            cell_h,
            remote_start_dir.clone(),
            spec.clone(),
        ) {
            Err(first_err) if daemon_disconnected_before_spawn_reply(&first_err) => {
                log::warn!(
                    "spawn connection ended before reply; probing daemon without forcing a restart: {first_err:#}"
                );
                if let Err(restart_err) = crate::daemon::spawn::ensure_running() {
                    return Err(anyhow::anyhow!(
                        "daemon disconnected before SpawnNativeSsh reply ({first_err}); recovery failed: {restart_err}"
                    ));
                }
                Self::spawn_native_ssh_once(size, cell_w, cell_h, remote_start_dir, spec).map_err(|second_err| {
                    anyhow::anyhow!(
                        "daemon disconnected before SpawnNativeSsh reply ({first_err}); recovery attempted but it still failed: {second_err}"
                    )
                })
            }
            other => other,
        }
    }

    fn spawn_native_ssh_once(
        size: TermSize,
        cell_w: u16,
        cell_h: u16,
        remote_start_dir: Option<PathBuf>,
        spec: Box<NativeSshSpec>,
    ) -> anyhow::Result<(Self, u64)> {
        let mut stream = connect()?;
        Self::negotiate_pane_protocol(&mut stream, attach_reply_wait(&PaneRoute::Local))?;
        let images = crate::terminal::images::ImageStore::new();
        images.set_cell_metrics(cell_w, cell_h);
        Self::send_graphics_capacity(&mut stream, &images)?;
        let win = win_size(size, cell_w, cell_h);
        let endpoint = (spec.host.clone(), spec.port);
        let user = spec.user.clone();
        let auto_supplied_password = spec.password.is_some();

        ClientMsg::SpawnNativeSsh {
            cwd: remote_start_dir,
            size: win,
            spec,
        }
        .encode(&mut stream)?;
        // Native SSH is always dialled through the local daemon, whatever the
        // far end turns out to be, so this waits on the local budget.
        let pane_id = match spawn_reply(
            &mut stream,
            attach_reply_wait(&PaneRoute::Local),
            "SpawnNativeSsh",
            &images,
        )? {
            DaemonMsg::Spawned { pane_id } => pane_id,
            DaemonMsg::Error(msg) => {
                return Err(anyhow::anyhow!("daemon refused SpawnNativeSsh: {msg}"));
            }
            other => {
                return Err(anyhow::anyhow!(
                    "unexpected daemon reply to SpawnNativeSsh: {other:?}"
                ));
            }
        };

        // The local daemon dialled this one, but it opened no pty for it: the
        // pane is an ssh channel bridged straight through, so the bytes are the
        // far host's raw pty and no conhost ever sees them.
        let mut term =
            Self::from_stream_parts(stream, size, Vec::new(), PtySource::Raw, false, images)?;
        term.ssh_endpoint = Some(endpoint);
        term.ssh_user = Some(user);
        term.auto_supplied_password = auto_supplied_password;
        Ok((term, pane_id))
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
        self.link.send(ClientMsg::AuthResponse {
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

fn daemon_not_listening(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause.downcast_ref::<std::io::Error>().is_some_and(|io| {
            matches!(
                io.kind(),
                std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
            )
        })
    })
}

/// How long an `Attach` may stay silent before it is called unanswered.
///
/// Only a silent connection ever pays this: a dead pane answers `Error` at
/// once and a live one answers `Size` at once, so the budget is not a cost in
/// the common case. What bounds it is the caller — a local restore attaches
/// synchronously on the UI thread, one pane after another, so N silent panes
/// hold the window still for N times this. What argues for more is the other
/// side of the same silence: a daemon merely slow to serve — an execve handoff
/// keeps the listener and its backlog across the exec, and a fresh daemon is
/// adopting panes and seeding ids before it can take an `Attach` — would have
/// served this connection a moment later, and calling it unanswered spawns a
/// fresh pane over a live one, carries the live pane's history onto it and
/// starts an agent resume against a session the old process still holds. The
/// [`AttachUnanswered`] verdict is confirmed against that case in `attach_on`
/// rather than by waiting longer here.
fn attach_reply_wait(route: &PaneRoute) -> std::time::Duration {
    match route.is_local() {
        true => std::time::Duration::from_secs(2),
        false => std::time::Duration::from_secs(15),
    }
}

fn graphics_admission_reply(
    message: DaemonMsg,
    stream: &mut Stream,
    images: &crate::terminal::images::ImageStore,
) -> std::io::Result<Option<DaemonMsg>> {
    match message {
        DaemonMsg::GraphicsOffer {
            token,
            key,
            width,
            height,
            encoded_bytes,
            metadata_bytes,
        } => {
            let accepted = images.offer(token, key, width, height, encoded_bytes, metadata_bytes);
            if let Err(error) = (ClientMsg::GraphicsGrant { token, accepted }).encode(stream) {
                images.cancel_offer(token);
                return Err(error);
            }
            Ok(None)
        }
        DaemonMsg::GraphicsCancel { token } => {
            images.cancel_offer(token);
            Ok(None)
        }
        message => Ok(Some(message)),
    }
}

#[cfg(test)]
fn cancellable_spawn_reply(
    stream: &mut Stream,
    cancellation: &SpawnCancellation,
) -> anyhow::Result<DaemonMsg> {
    cancellable_spawn_reply_with_images(
        stream,
        cancellation,
        &crate::terminal::images::ImageStore::new(),
    )
}

/// Polling only bounds cancellation latency, not pane creation. Read's caller
/// retains partial frames, so a delayed or split Spawned frame is never lost.
fn cancellable_spawn_reply_with_images(
    stream: &mut Stream,
    cancellation: &SpawnCancellation,
    images: &crate::terminal::images::ImageStore,
) -> anyhow::Result<DaemonMsg> {
    struct Reader<'a> {
        stream: &'a mut Stream,
        cancellation: &'a SpawnCancellation,
    }
    impl std::io::Read for Reader<'_> {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            loop {
                if self.cancellation.cancelled() {
                    return Err(std::io::Error::other("pane spawn cancelled"));
                }
                match self.stream.read(bytes) {
                    Err(error) if would_block(&error) => continue,
                    result => return result,
                }
            }
        }
    }
    stream.set_read_timeout(Some(std::time::Duration::from_millis(100)))?;
    let reply = loop {
        let message = DaemonMsg::read(&mut Reader {
            stream: &mut *stream,
            cancellation,
        });
        match message.and_then(|message| graphics_admission_reply(message, stream, images)) {
            Ok(Some(message)) => break Ok(message),
            Ok(None) => continue,
            Err(error) => break Err(error),
        }
    };
    let reset = stream.set_read_timeout(None);
    if cancellation.cancelled() {
        let _ = stream.shutdown(std::net::Shutdown::Both);
        images.cancel_unclaimed_offers();
        anyhow::bail!("pane spawn cancelled");
    }
    reset?;
    if reply.is_err() {
        images.cancel_unclaimed_offers();
    }
    reply.map_err(|error| anyhow::Error::new(error).context("reading the daemon's answer to Spawn"))
}

/// Legacy synchronous/native-SSH callers retain their bounded exchange. GUI
/// shell creation instead uses `cancellable_spawn_reply`: expiration is not a
/// failed Spawn, because the daemon may still be creating its real pane.
fn spawn_reply(
    stream: &mut Stream,
    wait: std::time::Duration,
    what: &str,
    images: &crate::terminal::images::ImageStore,
) -> anyhow::Result<DaemonMsg> {
    let _ = stream.set_read_timeout(Some(wait));
    let reply = loop {
        match DaemonMsg::read(stream)
            .and_then(|message| graphics_admission_reply(message, stream, images))
        {
            Ok(Some(message)) => break Ok(message),
            Ok(None) => continue,
            Err(error) => break Err(error),
        }
    };
    let _ = stream.set_read_timeout(None);
    if reply.is_err() {
        images.cancel_unclaimed_offers();
    }
    match reply {
        Ok(msg) => Ok(msg),
        Err(e) if would_block(&e) => {
            log::warn!("daemon request timed out; request={what} timeout={wait:?}");
            Err(anyhow::anyhow!("no answer to {what} within {wait:?}"))
        }
        Err(e) => {
            Err(anyhow::Error::new(e).context(format!("reading the daemon's answer to {what}")))
        }
    }
}

/// An `Attach` that produced no bytes within its wait — nobody served the
/// connection. Distinct from a refusal (`Error` frame) and from a hangup so
/// the caller can say the true thing: the pane may well still exist.
#[derive(Debug)]
struct AttachUnanswered {
    pane_id: u64,
    wait: std::time::Duration,
}

impl std::fmt::Display for AttachUnanswered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the daemon did not answer Attach for pane {} within {:?} (an attached pane replays \
             its screen at once, so nobody is serving this connection)",
            self.pane_id, self.wait
        )
    }
}

impl std::error::Error for AttachUnanswered {}

/// Whether `err` is an `Attach` that went unanswered, as opposed to refused or
/// hung up on. The pane behind an unanswered attach may still be alive.
#[cfg(all(test, unix))]
fn attach_unanswered(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|cause| cause.downcast_ref::<AttachUnanswered>().is_some())
}

/// The daemon explicitly reported that the requested pane does not exist.
/// Other Error frames (including graphics admission failures) are retryable
/// attach failures, not evidence that the original session is gone.
#[derive(Debug)]
struct AttachRefused {
    message: String,
}

impl std::fmt::Display for AttachRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "daemon refused Attach: {}", self.message)
    }
}

impl std::error::Error for AttachRefused {}

/// Whether the daemon explicitly reported the requested pane missing. Only
/// this verdict permits abandoning a relink or spawning a replacement shell.
pub fn attach_refused(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|cause| cause.downcast_ref::<AttachRefused>().is_some())
}

/// Reads far enough into the daemon's answer to an `Attach` to classify it, and
/// hands back every byte read so the reader thread loses none of the replay.
///
/// Silence is a verdict, not a quiet pane. A daemon that attached replays the
/// pane's ring before it reads a byte of our input, and the ring always holds
/// a segment, so the first frame on a good attach is a `Size` — a quiet pane
/// still sends that. An `Attach` that produced nothing within `wait` is one
/// nobody is serving: a daemon still mid-restart, a socket held open by a
/// process that will never read it. Taken for success it gave the window a
/// pane drawn as restored whose every keystroke went into that socket — no
/// fresh shell, no restored-screen banner, no agent resume, and Ctrl-C did
/// nothing (#673). Treat it as an attach failure and retain the pane identity;
/// only an explicit missing-pane reply allows a replacement shell.
#[cfg(test)]
fn attach_reply_prefix(
    stream: &mut Stream,
    pane_id: u64,
    wait: std::time::Duration,
) -> anyhow::Result<Vec<u8>> {
    attach_reply_prefix_with_images(
        stream,
        pane_id,
        wait,
        &crate::terminal::images::ImageStore::new(),
    )
}

fn attach_reply_prefix_with_images(
    stream: &mut Stream,
    pane_id: u64,
    wait: std::time::Duration,
    images: &crate::terminal::images::ImageStore,
) -> anyhow::Result<Vec<u8>> {
    let result = attach_reply_prefix_inner(stream, pane_id, wait, images);
    if result.is_err() {
        images.cancel_unclaimed_offers();
        let _ = stream.set_read_timeout(None);
    }
    result
}

fn attach_reply_prefix_inner(
    stream: &mut Stream,
    pane_id: u64,
    wait: std::time::Duration,
    images: &crate::terminal::images::ImageStore,
) -> anyhow::Result<Vec<u8>> {
    use std::io::Read as _;

    let _ = stream.set_read_timeout(Some(wait));
    let mut buffered: Vec<u8> = Vec::new();
    let mut scratch = [0u8; 4096];
    let mut kind = None;
    while kind.is_none() {
        match stream.read(&mut scratch) {
            Ok(0) => {
                let _ = stream.set_read_timeout(None);
                return Err(anyhow::anyhow!(
                    "the daemon closed the connection without answering Attach for pane {pane_id}"
                ));
            }
            Ok(n) => buffered.extend_from_slice(&scratch[..n]),
            Err(e) if would_block(&e) => break,
            Err(e) => {
                let _ = stream.set_read_timeout(None);
                return Err(anyhow::Error::new(e).context(format!(
                    "reading the daemon's answer to Attach for pane {pane_id}"
                )));
            }
        }
        kind = crate::daemon::protocol::peek_frame_kind(&buffered);
        while kind.is_some_and(crate::daemon::protocol::is_graphics_admission_kind) {
            let Some((frame_kind, payload)) = crate::daemon::protocol::take_frame(&mut buffered)?
            else {
                kind = None;
                break;
            };
            let message = DaemonMsg::from_frame(frame_kind, payload)?;
            graphics_admission_reply(message, stream, images)?;
            kind = crate::daemon::protocol::peek_frame_kind(&buffered);
        }
    }
    let _ = stream.set_read_timeout(None);
    if buffered.is_empty() {
        return Err(anyhow::Error::new(AttachUnanswered { pane_id, wait }));
    }
    if !kind.is_some_and(crate::daemon::protocol::is_error_kind) {
        return Ok(buffered);
    }
    let message = read_error_frame(stream, &mut buffered, wait).ok_or_else(|| {
        anyhow::anyhow!("incomplete daemon error answering Attach for pane {pane_id}")
    })?;
    if message == format!("no such pane {pane_id}") {
        Err(anyhow::Error::new(AttachRefused { message }))
    } else {
        Err(anyhow::anyhow!("daemon refused Attach: {message}"))
    }
}

fn read_error_frame(
    stream: &mut Stream,
    buffered: &mut Vec<u8>,
    wait: std::time::Duration,
) -> Option<String> {
    use std::io::Read as _;

    let _ = stream.set_read_timeout(Some(wait));
    let mut scratch = [0u8; 1024];
    let message = loop {
        match crate::daemon::protocol::take_frame(buffered) {
            Ok(Some(frame)) => match DaemonMsg::from_frame(frame.0, frame.1) {
                Ok(DaemonMsg::Error(message)) => break Some(message),
                _ => break None,
            },
            Ok(None) => match stream.read(&mut scratch) {
                Ok(0) => break None,
                Ok(n) => buffered.extend_from_slice(&scratch[..n]),
                Err(_) => break None,
            },
            Err(_) => break None,
        }
    };
    let _ = stream.set_read_timeout(None);
    message
}

fn would_block(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

fn daemon_disconnected_before_spawn_reply(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause.downcast_ref::<std::io::Error>().is_some_and(|io| {
            matches!(
                io.kind(),
                std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::BrokenPipe
            )
        })
    })
}

impl Drop for RemoteTerminal {
    fn drop(&mut self) {
        self.link.send(ClientMsg::Detach);
        self.stop_reader();
    }
}



type OscNotes = Arc<Mutex<VecDeque<Note>>>;

fn connect() -> anyhow::Result<Stream> {
    transport::connect().map_err(|e| {
        anyhow::Error::new(e).context(format!(
            "connect to daemon at {}",
            transport::endpoint_display()
        ))
    })
}

fn connect_routed(route: &PaneRoute) -> anyhow::Result<Stream> {
    if let PaneRoute::Unroutable(reason) = route {
        return Err(anyhow::anyhow!("{reason}"));
    }
    let Some(header) = route.header() else {
        return connect();
    };

    lyrel_core::host::guard_off_ui();

    // No `ensure_wsl_server` here on purpose. The daemon runs exactly the same
    // probe inside `router::open_link` before it opens the link, so asking from
    // this side too bought nothing and cost a second full round of `wsl.exe`
    // invocations — five of them, serially, on every single pane. On a machine
    // where a `wsl.exe` round trip is slow (issue #454 measured 3.3s) that
    // duplicate was half of the wait before a new tab could take a key.
    //
    // Nothing is lost by dropping it: the returned path was discarded, the
    // failure is reported just as well through the route ack below, and the
    // first-install consent question still reaches this process — the daemon
    // runs its probe under `RouteSetup::blocking`, which installs the relay
    // that turns the question into a frame on this very connection.
    let mut stream = connect()?;
    let ack = crate::daemon::router::negotiate(&mut stream, header)
        .map_err(|e| anyhow::anyhow!("route this pane to {}: {e}", header.describe()))?;
    log::debug!(
        "pane routed to {} over {}",
        header.describe(),
        ack.link.as_deref().unwrap_or("?")
    );
    Ok(stream)
}

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

    fn ssh_workspace() -> PaneWorkspace {
        PaneWorkspace {
            workspace: crate::core::session::WorkspaceId::new(),
            target: crate::core::session::RemoteTarget::Direct {
                user: "me".into(),
                host: "build-box".into(),
                port: 22,
            },
            spec: Some(Box::new(
                serde_json::from_str(
                    r#"{"host":"build-box","port":22,"user":"me","auth_mode":"auto"}"#,
                )
                .unwrap(),
            )),
            label: None,
            resize_echo: false,
        }
    }

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
    fn a_local_spawn_carries_its_workspace_so_the_shell_gets_lyrel_ws() {
        let ws = "0d4e1a54-0000-4000-8000-000000000001";

        assert_eq!(
            spawn_workspace(Some(ws), &PaneRoute::Local).as_deref(),
            Some(ws),
            "without this the pane's shell has no $LYREL_WS and every \
             workspace-scoped CLI verb in it needs an explicit address"
        );

        assert_eq!(
            spawn_workspace(None, &PaneRoute::Local),
            None,
            "a pane outside any workspace must not claim one"
        );

        assert_eq!(
            spawn_workspace(Some(ws), &PaneRoute::for_workspace(Some(&ssh_workspace()))),
            None,
            "a remote server has its own machine tree; this id names a workspace in ours"
        );
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
    fn spawn_retry_only_for_daemon_disconnects() {
        let eof: anyhow::Error =
            std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "closed").into();
        assert!(daemon_disconnected_before_spawn_reply(&eof));

        let refused = anyhow::anyhow!("daemon refused Spawn: configured shell missing");
        assert!(!daemon_disconnected_before_spawn_reply(&refused));
    }

    #[test]
    fn only_a_dead_daemon_is_worth_starting_one_for() {
        let connect_failed = |kind| -> anyhow::Error {
            anyhow::Error::new(std::io::Error::new(kind, "no listener"))
                .context("connect to daemon at /tmp/lyrel.sock")
        };
        assert!(daemon_not_listening(&connect_failed(
            std::io::ErrorKind::ConnectionRefused
        )));
        assert!(daemon_not_listening(&connect_failed(
            std::io::ErrorKind::NotFound
        )));

        let refused = anyhow::anyhow!("daemon refused Spawn: configured shell missing");
        assert!(!daemon_not_listening(&refused));
        let eof: anyhow::Error =
            std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "closed").into();
        assert!(!daemon_not_listening(&eof));
    }

    #[test]
    fn an_attach_to_a_missing_pane_is_an_error_not_a_disconnect() {
        let (mut client_side, mut daemon_side) = UnixStream::pair().unwrap();
        DaemonMsg::Error("no such pane 7".to_string())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();

        let err = attach_reply_prefix(&mut client_side, 7, attach_reply_wait(&PaneRoute::Local))
            .expect_err("a missing pane must fail");
        assert!(
            format!("{err:#}").contains("no such pane 7"),
            "the daemon's own wording is what names which pane went: {err:#}"
        );
    }

    #[test]
    fn an_attach_the_daemon_hangs_up_on_is_an_error() {
        let (mut client_side, daemon_side) = UnixStream::pair().unwrap();
        drop(daemon_side);
        assert!(
            attach_reply_prefix(&mut client_side, 7, attach_reply_wait(&PaneRoute::Local)).is_err()
        );
    }

    /// #673: a daemon that attached replays the pane's ring at once, and the
    /// ring always holds a segment, so even a pane that has printed nothing
    /// answers with a `Size`. A connection that stays silent for the whole wait
    /// is therefore one nobody is serving — read as success, it became a pane
    /// the window drew as restored while every keystroke, Ctrl-C included,
    /// went into a socket nothing drained.
    #[test]
    fn an_attach_nobody_answers_is_not_an_attach() {
        let (mut client_side, _daemon_side) = UnixStream::pair().unwrap();
        let wait = std::time::Duration::from_millis(200);
        let err = attach_reply_prefix(&mut client_side, 7, wait)
            .expect_err("silence for the whole wait must not pass for an attached pane");
        assert!(
            format!("{err:#}").contains("did not answer"),
            "the failure has to say the daemon never answered, not that the pane is gone: {err:#}"
        );
        assert!(
            attach_unanswered(&err),
            "silence is its own verdict, told apart from a refusal or a hangup"
        );
        // A refusal is not it: the pane really is gone then, and the caller
        // may say so.
        let (mut client_side, mut daemon_side) = UnixStream::pair().unwrap();
        DaemonMsg::Error("no such pane 7".into())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        let refused = attach_reply_prefix(&mut client_side, 7, wait).expect_err("refused");
        assert!(!attach_unanswered(&refused));
    }

    /// The relink retry loop keys off this split: a refusal means the pane is
    /// gone on its machine and asking again can only repeat the answer, while
    /// silence and a hangup are transport trouble worth another try.
    #[test]
    fn a_refused_attach_is_final_and_every_other_failure_is_not() {
        let wait = std::time::Duration::from_millis(200);

        let (mut client_side, mut daemon_side) = UnixStream::pair().unwrap();
        DaemonMsg::Error("no such pane 7".into())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        let refused = attach_reply_prefix(&mut client_side, 7, wait).expect_err("refused");
        assert!(attach_refused(&refused));
        assert!(
            format!("{refused:#}").contains("no such pane 7"),
            "the daemon's own words survive: {refused:#}"
        );

        let (mut client_side, _held_open) = UnixStream::pair().unwrap();
        let silent = attach_reply_prefix(&mut client_side, 7, wait).expect_err("silent");
        assert!(
            !attach_refused(&silent),
            "silence is not a verdict on the pane"
        );

        let (mut client_side, daemon_side) = UnixStream::pair().unwrap();
        drop(daemon_side);
        let hungup = attach_reply_prefix(&mut client_side, 7, wait).expect_err("hung up");
        assert!(
            !attach_refused(&hungup),
            "a hangup is not a verdict on the pane"
        );
    }

    #[test]
    fn graphics_budget_refusal_is_not_evidence_that_the_pane_is_gone() {
        for message in [
            "Desktop graphics budget cannot admit retained session content",
            "Graphics replay connection closed during admission",
            "Graphics replay failed",
        ] {
            let (mut client_side, mut daemon_side) = UnixStream::pair().unwrap();
            DaemonMsg::Error(message.into())
                .encode(&mut daemon_side)
                .unwrap();
            daemon_side.flush().unwrap();
            let error =
                attach_reply_prefix(&mut client_side, 7, std::time::Duration::from_millis(200))
                    .expect_err("admission refused");
            assert!(
                !attach_refused(&error),
                "only an explicit missing-pane verdict can abandon the original session: {error:#}"
            );
            assert!(format!("{error:#}").contains(message));
        }
    }

    /// The other side of the rule above: the frame a quiet pane does send is
    /// enough. Nothing else is required of the daemon for the attach to stand.
    #[test]
    fn a_size_frame_alone_is_a_live_attach() {
        let (mut client_side, mut daemon_side) = UnixStream::pair().unwrap();
        DaemonMsg::Size(WinSize {
            cols: 80,
            rows: 24,
            cell_w: 8,
            cell_h: 17,
        })
        .encode(&mut daemon_side)
        .unwrap();
        daemon_side.flush().unwrap();

        let buffered =
            attach_reply_prefix(&mut client_side, 7, std::time::Duration::from_millis(200))
                .expect("the daemon's first frame is the attach ack");
        assert!(
            !buffered.is_empty(),
            "the Size frame was read to classify the reply; it must reach the reader"
        );
    }

    #[test]
    fn a_local_attach_does_not_wait_as_long_as_a_remote_one() {
        let local = attach_reply_wait(&PaneRoute::Local);
        let remote = attach_reply_wait(&PaneRoute::for_workspace(Some(&ssh_workspace())));
        assert!(local < remote, "{local:?} must be the shorter wait");
        assert!(
            local <= std::time::Duration::from_secs(2),
            "the UI thread is holding still for this"
        );
    }

    #[test]
    fn a_live_attach_hands_its_replay_bytes_to_the_reader() {
        crate::core::config::pin_test_config_dir();
        let (mut client_side, mut daemon_side) = UnixStream::pair().unwrap();
        DaemonMsg::Snapshot(b"hello".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();

        let buffered =
            attach_reply_prefix(&mut client_side, 7, attach_reply_wait(&PaneRoute::Local))
                .expect("a live pane attaches");
        assert!(
            !buffered.is_empty(),
            "the classification read the Snapshot frame; it must come back"
        );
        let term = RemoteTerminal::from_stream_with(
            client_side,
            TermSize::new(80, 24),
            buffered,
            PtySource::Raw,
        )
        .unwrap();

        let mut got = String::new();
        for _ in 0..200 {
            {
                let t = term.term.lock();
                let grid = &t.grid;
                got.clear();
                for col in 0..5usize {
                    got.push(
                        grid[rio_vt::crosswords::pos::Line(0)]
                            [rio_vt::crosswords::pos::Column(col)]
                        .c(),
                    );
                }
            }
            if got == "hello" {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            got, "hello",
            "the pre-read replay must still reach the grid"
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
    fn write_sends_input_frames() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        term.write(Vec::<u8>::new());
        term.write(b"echo hi\r".to_vec());

        match ClientMsg::read(&mut daemon_side).unwrap() {
            ClientMsg::Input(bytes) => assert_eq!(bytes, b"echo hi\r"),
            other => panic!("expected Input, got {other:?}"),
        }
    }

    /// A peer that holds the link open and stops reading is what the router
    /// looks like from here when the far end is congested: `copy_bidirectional`
    /// stops draining our half and the send buffer fills. The socket is
    /// blocking and has no write timeout, so `write(2)` parks — and it used to
    /// park on the UI thread, which draws every window. macOS gives a unix
    /// stream 8K, so it took about 1400 keystrokes, or one paste.
    ///
    /// Typing into such a pane must now cost the window nothing at all.
    #[test]
    fn a_link_that_stopped_reading_does_not_park_the_writing_thread() {
        crate::core::config::pin_test_config_dir();
        let (client_side, daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        // Far past the 8K that used to be fatal, one keystroke at a time, on
        // this very thread: a park anywhere in here is a frozen window.
        let started = std::time::Instant::now();
        for _ in 0..64 * 1024 {
            term.write(vec![b'x']);
        }
        let spent = started.elapsed();

        drop(daemon_side);
        assert!(
            spent < std::time::Duration::from_secs(1),
            "64K keystrokes into a link nobody is draining took {spent:?} — \
             the caller is a gpui event handler, so this is the UI thread"
        );
    }

    /// The backlog is bounded, and reaching the bound is not a slow link but a
    /// dead one: nothing has taken a byte for four megabytes of typing. Saying
    /// so is what stops the pane quietly swallowing input forever, and it is
    /// said in the same words — and once — as an outright refused write.
    #[test]
    fn a_backlog_nothing_drains_is_reported_as_a_lost_link() {
        crate::core::config::pin_test_config_dir();
        let (client_side, daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        // 8K a shot rather than a byte, so this is a few hundred frames and not
        // a few million: the bound is on bytes queued, not on frames.
        //
        // Twice the bound rather than a frame or two past it. The sender does
        // get some of this onto the wire before it parks — as much as the send
        // buffer holds, which is 8K on macOS but a couple hundred K on Linux —
        // and that much is discounted from the backlog. A margin narrower than
        // the widest of those buffers is a test that passes on one platform and
        // not the other.
        let chunk = vec![b'x'; 8 << 10];
        for _ in 0..2 * MAX_BACKLOG / chunk.len() {
            term.write(chunk.clone());
        }

        assert!(
            term.exited_flag.load(Ordering::SeqCst),
            "a link that has taken nothing for {MAX_BACKLOG} bytes is gone, and the pane must say so"
        );
        let mut exits = 0;
        while let Ok(ev) = term.events.try_recv() {
            exits += usize::from(matches!(ev, PaneEvent::Exit));
        }
        assert_eq!(exits, 1, "said once, not once per keystroke");
        drop(daemon_side);
    }

    /// One frame can be larger than the whole backlog bound: a paste is
    /// whatever the clipboard holds. That is not a link nothing is draining,
    /// and a pane must not die of being pasted into — nor may the keystrokes
    /// that follow read as a backlog merely because the paste is still going.
    #[test]
    fn a_paste_larger_than_the_backlog_bound_is_not_a_dead_link() {
        crate::core::config::pin_test_config_dir();
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        // A peer that keeps reading: the link is healthy, just carrying a lot.
        let draining = std::thread::spawn(move || {
            let _ = std::io::copy(&mut daemon_side, &mut std::io::sink());
        });

        term.write(vec![b'x'; MAX_BACKLOG + (1 << 20)]);
        for _ in 0..1024 {
            term.write(vec![b'y']);
        }

        assert!(
            !term.exited_flag.load(Ordering::SeqCst),
            "a five megabyte paste is a paste, not a link that has stopped taking input"
        );
        drop(term);
        draining.join().unwrap();
    }

    /// Input the link refuses used to vanish: `write` threw the error away, so a
    /// pane whose daemon had stopped reading kept taking keystrokes into
    /// nothing. The refusal now marks the pane exited by the reader's own signal
    /// — only our sending half is shut here, so the reader is still parked on
    /// an open receiving half and the writing side is the one that finds out.
    /// (Shutting the peer's receiving half instead is not portable: Linux
    /// answers the next write with EPIPE, macOS buffers it.)
    ///
    /// The refusal is met on the sender thread now, so the pane learns of it a
    /// moment after the keystroke rather than during it. That is the trade the
    /// queue buys: the window never waits on the socket to find out.
    #[test]
    fn a_write_the_link_refuses_marks_the_pane_gone_once() {
        crate::core::config::pin_test_config_dir();
        let (client_side, _daemon_side) = UnixStream::pair().unwrap();
        let term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        term.link
            .closer
            .shutdown(std::net::Shutdown::Write)
            .unwrap();

        term.write(b"echo hi\r".to_vec());
        let noticed = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !term.exited_flag.load(Ordering::SeqCst) && std::time::Instant::now() < noticed {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            term.exited_flag.load(Ordering::SeqCst),
            "a refused Input is the link gone, and the pane has to say so"
        );
        let mut exits = 0;
        while let Ok(ev) = term.events.try_recv() {
            exits += usize::from(matches!(ev, PaneEvent::Exit));
        }
        assert_eq!(
            exits, 1,
            "reported through the same event the reader raises on EOF"
        );

        term.write(b"echo again\r".to_vec());
        while let Ok(ev) = term.events.try_recv() {
            exits += usize::from(matches!(ev, PaneEvent::Exit));
        }
        assert_eq!(exits, 1, "said once, not once per keystroke");
    }

    /// A link retired for a relink takes no more input — `stop_reader` closes
    /// the queue and shuts the socket down — and that must not read as the pane
    /// dying under the swap, for the same reason the retired reader exits
    /// silently. The frame is now turned away at the queue rather than by the
    /// socket, but the pane has to stay alive either way.
    #[test]
    fn a_write_on_a_retired_link_does_not_mark_the_pane_gone() {
        crate::core::config::pin_test_config_dir();
        let (client_side, _daemon_side) = UnixStream::pair().unwrap();
        let mut term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        term.stop_reader();

        term.write(b"echo hi\r".to_vec());
        assert!(
            !term.exited_flag.load(Ordering::SeqCst),
            "the pane is being relinked or released, not dying"
        );
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
    fn first_resize_always_syncs_then_dedups() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let mut term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        term.resize(TermSize::new(80, 24), 8, 17);
        match ClientMsg::read(&mut daemon_side).unwrap() {
            ClientMsg::Resize(ws) => assert_eq!((ws.cols, ws.rows), (80, 24)),
            other => panic!("expected the first Resize to be sent, got {other:?}"),
        }

        term.resize(TermSize::new(80, 24), 8, 17);
        term.write(b"marker".to_vec());
        match ClientMsg::read(&mut daemon_side).unwrap() {
            ClientMsg::Input(bytes) => assert_eq!(bytes, b"marker"),
            other => panic!("expected Input (dup resize sends nothing), got {other:?}"),
        }
    }

    #[test]
    fn layout_resize_reasserts_geometry_after_a_late_size_frame() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let mut term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        term.resize(TermSize::new(100, 40), 8, 17);
        assert!(matches!(
            ClientMsg::read(&mut daemon_side).unwrap(),
            ClientMsg::Resize(_)
        ));

        DaemonMsg::Size(WinSize {
            cols: 120,
            rows: 30,
            cell_w: 8,
            cell_h: 17,
        })
        .encode(&mut daemon_side)
        .unwrap();
        DaemonMsg::Snapshot(b"old screen".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();
        for _ in 0..200 {
            if term.term.lock().columns() == 120 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(term.term.lock().columns(), 120, "replay geometry applied");

        term.resize(TermSize::new(100, 40), 8, 17);
        assert_eq!(term.term.lock().columns(), 100);
        assert_eq!(term.term.lock().screen_lines(), 40);
        assert!(matches!(
            ClientMsg::read(&mut daemon_side).unwrap(),
            ClientMsg::Resize(ws) if ws.cols == 100 && ws.rows == 40
        ));
    }

    #[test]
    fn resize_updates_size_and_notifies_daemon() {
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let mut term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();

        term.resize(TermSize::new(100, 40), 9, 18);
        assert_eq!(term.size(), TermSize::new(100, 40));
        match ClientMsg::read(&mut daemon_side).unwrap() {
            ClientMsg::Resize(ws) => {
                assert_eq!((ws.cols, ws.rows, ws.cell_w, ws.cell_h), (100, 40, 9, 18));
            }
            other => panic!("expected Resize, got {other:?}"),
        }
    }

    #[test]
    fn echoed_resize_defers_the_grid_reflow_to_the_daemons_size_frame() {
        use rio_vt::crosswords::pos::{Column, Line};

        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let mut term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        // A remote route whose host advertised the echo. The local daemon's
        // answer rides the same gate through `local_daemon_supports`, so this
        // covers the deferral for both.
        term.route = PaneRoute::Remote {
            header: Box::new(crate::daemon::router::RouteHeader::wsl("Ubuntu-22.04")),
            resize_echo: true,
        };

        term.resize(TermSize::new(120, 30), 8, 17);
        assert!(matches!(
            ClientMsg::read(&mut daemon_side).unwrap(),
            ClientMsg::Resize(ws) if ws.cols == 120 && ws.rows == 30
        ));
        assert_eq!(
            term.term.lock().columns(),
            80,
            "the grid keeps the old geometry until the daemon's echo arrives"
        );

        // Old-width bytes still in flight ahead of the echo: a CUP to column
        // 100 must clamp on the 80-col grid. The same addressing after the
        // echo reaches the real column on the 120-col grid.
        DaemonMsg::Output(b"\x1b[1;100HA".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        DaemonMsg::Size(WinSize {
            cols: 120,
            rows: 30,
            cell_w: 8,
            cell_h: 17,
        })
        .encode(&mut daemon_side)
        .unwrap();
        DaemonMsg::Output(b"\x1b[2;100HB".to_vec())
            .encode(&mut daemon_side)
            .unwrap();
        daemon_side.flush().unwrap();

        for _ in 0..400 {
            {
                let t = term.term.lock();
                if t.columns() == 120 && t.grid[Line(1)][Column(99)].c() == 'B' {
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let t = term.term.lock();
        let grid = &t.grid;
        assert_eq!(
            grid[Line(0)][Column(79)].c(),
            'A',
            "bytes queued before the echo parse at the old width"
        );
        assert_eq!(
            grid[Line(1)][Column(99)].c(),
            'B',
            "bytes after the echo parse at the new width"
        );
    }

    /// A route whose daemon promised to echo every resize, over a socket pair
    /// the test plays the daemon on. The switch tests build the same route
    /// against a real pane.
    fn echoing_pair() -> (RemoteTerminal, UnixStream) {
        let (client_side, daemon_side) = UnixStream::pair().unwrap();
        let mut term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        term.route = PaneRoute::Remote {
            header: Box::new(crate::daemon::router::RouteHeader::wsl("Ubuntu-22.04")),
            resize_echo: true,
        };
        daemon_side
            .set_read_timeout(Some(std::time::Duration::from_millis(150)))
            .unwrap();
        (term, daemon_side)
    }

    /// The geometry of the next frame the pane sent, if it sent one at all.
    fn next_resize(daemon_side: &mut UnixStream) -> Option<(u16, u16)> {
        match ClientMsg::read(daemon_side) {
            Ok(ClientMsg::Resize(ws)) => Some((ws.cols, ws.rows)),
            Ok(other) => panic!("expected a Resize, got {other:?}"),
            Err(_) => None,
        }
    }

    /// Makes the last resize old enough that an echo for it is no longer
    /// expected, without the test sitting out the grace for real.
    fn outlive_the_echo_grace(term: &mut RemoteTerminal) {
        term.resize_sent_at = std::time::Instant::now()
            .checked_sub(RESIZE_ECHO_GRACE + std::time::Duration::from_millis(1));
    }

    fn wait_for_columns(term: &RemoteTerminal, cols: usize) {
        for _ in 0..400 {
            if term.term.lock().columns() == cols {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the grid never reached {cols} columns");
    }

    /// #893: a pane came back from sleep or a workspace switch painting a grid
    /// shorter than its bounds, and stayed that way until a divider drag. On an
    /// echoing route the grid only moves when the daemon's `Size` comes back,
    /// and the early-out trusted the request: once asked for, the same size
    /// was never sent again, so a request or echo that went missing stranded
    /// the grid — and the child — at the old geometry for good.
    #[test]
    fn an_echoed_resize_whose_echo_never_came_is_asked_for_again() {
        let (mut term, mut daemon_side) = echoing_pair();
        term.resize(TermSize::new(120, 30), 8, 17);
        assert_eq!(next_resize(&mut daemon_side), Some((120, 30)));

        // Every frame asks again. While the echo can still be on its way that
        // is not a reason to resend.
        term.resize(TermSize::new(120, 30), 8, 17);
        assert_eq!(
            next_resize(&mut daemon_side),
            None,
            "an echo still in flight must not be chased with duplicates"
        );

        // The echo never comes. Past the grace the grid is still 80x24, and
        // the frame that notices must ask for the size again.
        outlive_the_echo_grace(&mut term);
        term.resize(TermSize::new(120, 30), 8, 17);
        assert_eq!(
            next_resize(&mut daemon_side),
            Some((120, 30)),
            "a resize whose echo never arrived was never retried, so the grid \
             stays at the old size until the layout asks for a different one"
        );
        assert_eq!(
            term.term.lock().columns(),
            80,
            "the retry still waits for the echo rather than reflowing early"
        );

        // Once the echo lands the size is settled, however old the request.
        DaemonMsg::Size(WinSize {
            cols: 120,
            rows: 30,
            cell_w: 8,
            cell_h: 17,
        })
        .encode(&mut daemon_side)
        .unwrap();
        daemon_side.flush().unwrap();
        wait_for_columns(&term, 120);
        assert_eq!(term.term.lock().screen_lines(), 30);
        outlive_the_echo_grace(&mut term);
        term.resize(TermSize::new(120, 30), 8, 17);
        assert_eq!(
            next_resize(&mut daemon_side),
            None,
            "a grid that reached the requested size must not be resized again"
        );
    }

    /// The other way the grid and the request can part on an echoing route: a
    /// `Size` frame that lands after the echo moves the grid off the size the
    /// layout asked for. The non-echoing path already re-asserts there
    /// (`layout_resize_reasserts_geometry_after_a_late_size_frame`); the
    /// echoing one skipped it, since the request itself had not changed.
    #[test]
    fn an_echoed_resize_reasserts_geometry_after_a_late_size_frame() {
        let (mut term, mut daemon_side) = echoing_pair();
        term.resize(TermSize::new(120, 30), 8, 17);
        assert_eq!(next_resize(&mut daemon_side), Some((120, 30)));
        for (cols, rows) in [(120, 30), (100, 12)] {
            DaemonMsg::Size(WinSize {
                cols,
                rows,
                cell_w: 8,
                cell_h: 17,
            })
            .encode(&mut daemon_side)
            .unwrap();
        }
        daemon_side.flush().unwrap();
        wait_for_columns(&term, 100);

        outlive_the_echo_grace(&mut term);
        term.resize(TermSize::new(120, 30), 8, 17);
        assert_eq!(
            next_resize(&mut daemon_side),
            Some((120, 30)),
            "a grid knocked off the requested size was left there"
        );
    }

    /// A relink re-sends the geometry the pane has now — including a resize
    /// made after the old link had already died, which reached no daemon.
    #[test]
    fn a_relink_delivers_the_resize_made_while_the_link_was_down() {
        crate::core::config::pin_test_config_dir();
        let (old_client, mut old_daemon) = UnixStream::pair().unwrap();
        let mut term = RemoteTerminal::from_stream(old_client, TermSize::new(80, 24)).unwrap();
        term.resize(TermSize::new(100, 40), 8, 17);
        assert!(matches!(
            ClientMsg::read(&mut old_daemon).unwrap(),
            ClientMsg::Resize(ws) if ws.cols == 100 && ws.rows == 40
        ));

        // The link drops; the layout keeps moving.
        drop(old_daemon);
        term.resize(TermSize::new(120, 30), 8, 17);

        let (new_client, mut new_daemon) = UnixStream::pair().unwrap();
        new_daemon
            .set_read_timeout(Some(std::time::Duration::from_secs(3)))
            .unwrap();
        // What `relink_plan` hands the relink: the size the view has now.
        let size = term.size();
        term.adopt_relink(new_client, Vec::new(), &PaneRoute::Local, size, 8, 17)
            .unwrap();
        assert!(
            matches!(
                ClientMsg::read(&mut new_daemon).unwrap(),
                ClientMsg::Resize(ws) if ws.cols == 120 && ws.rows == 30
            ),
            "the relinked daemon must be told the size the pane has now"
        );
    }
    #[test]
    fn a_remote_route_without_the_advertised_echo_reflows_at_request_time() {
        crate::core::config::pin_test_config_dir();
        let (client_side, mut daemon_side) = UnixStream::pair().unwrap();
        let mut term = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24)).unwrap();
        // What `for_workspace` builds when the host's hello named no echo —
        // an older server, or a link that was down when the route was made.
        term.route = PaneRoute::Remote {
            header: Box::new(crate::daemon::router::RouteHeader::wsl("Ubuntu-22.04")),
            resize_echo: false,
        };

        term.resize(TermSize::new(120, 30), 8, 17);
        assert_eq!(
            term.term.lock().columns(),
            120,
            "with no promised echo the grid must reflow at request time — \
             deferring would leave it at the old width forever"
        );
        assert!(matches!(
            ClientMsg::read(&mut daemon_side).unwrap(),
            ClientMsg::Resize(ws) if ws.cols == 120 && ws.rows == 30
        ));

        // The other route with no daemon to promise anything.
        term.route = PaneRoute::Unroutable("no ssh details".into());
        assert!(!term.resize_echoed(), "no route, no echo to wait for");
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

#[cfg(test)]
mod spawn_reply_consumer_tests {
    use super::test_support::socket_pair;
    use super::*;
    use std::io::Write as _;

    #[test]
    fn a_slow_split_spawn_reply_keeps_its_real_pane_id() {
        let (mut client, mut daemon) = socket_pair();
        let cancellation = SpawnCancellation::default();
        let worker =
            std::thread::spawn(move || cancellable_spawn_reply(&mut client, &cancellation));
        let mut bytes = Vec::new();
        DaemonMsg::Spawned { pane_id: 42 }
            .encode(&mut bytes)
            .unwrap();
        // Longer than the old local request deadline. A partial header also
        // catches restarting frame decoding after a cancellation poll.
        daemon.write_all(&bytes[..2]).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2200));
        daemon.write_all(&bytes[2..]).unwrap();
        assert_eq!(
            worker.join().unwrap().unwrap(),
            DaemonMsg::Spawned { pane_id: 42 }
        );
    }

    #[test]
    fn closing_a_pending_slot_cancels_a_silent_spawn_reader() {
        let (mut client, mut daemon) = socket_pair();
        daemon
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let cancellation = SpawnCancellation::default();
        let worker_cancellation = cancellation.clone();
        let (started, ready) = std::sync::mpsc::channel();
        let (finished, done) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            started.send(()).unwrap();
            let result = cancellable_spawn_reply(&mut client, &worker_cancellation);
            finished.send(result.map(|_| ())).unwrap();
        });
        ready.recv().unwrap();
        cancellation.cancel();
        let error = done
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("cancellation must release the worker, not leave a silent read forever")
            .expect_err("cancelled creation is not a successful pane");
        assert!(
            !daemon_disconnected_before_spawn_reply(&error),
            "cancellation must not retry Spawn"
        );
        let mut byte = [0];
        assert_eq!(
            daemon.read(&mut byte).unwrap(),
            0,
            "cancellation closes only its Spawn connection"
        );
        worker.join().unwrap();
    }

    #[test]
    fn an_explicit_spawn_failure_is_delivered_without_recovery() {
        let (mut client, mut daemon) = socket_pair();
        DaemonMsg::Error("configured shell missing".into())
            .encode(&mut daemon)
            .unwrap();
        let reply = cancellable_spawn_reply(&mut client, &SpawnCancellation::default()).unwrap();
        assert_eq!(reply, DaemonMsg::Error("configured shell missing".into()));
    }

    #[test]
    fn only_landing_a_provisional_terminal_sends_its_matching_attach() {
        crate::core::config::pin_test_config_dir();
        let (client, mut daemon) = socket_pair();
        let mut terminal = RemoteTerminal::from_stream(client, TermSize::new(80, 24)).unwrap();
        terminal.spawn_claim = Some((43, win_size(TermSize::new(80, 24), 8, 17)));
        terminal.claim_spawn();
        daemon
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        assert!(matches!(
            ClientMsg::read(&mut daemon).unwrap(),
            ClientMsg::Attach { pane_id: 43, .. }
        ));
        terminal.claim_spawn();
        daemon
            .set_read_timeout(Some(std::time::Duration::from_millis(100)))
            .unwrap();
        assert!(
            ClientMsg::read(&mut daemon).is_err(),
            "a second landing cannot claim twice"
        );
    }
}
