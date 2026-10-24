//! Streaming kitty-graphics-protocol extractor (APC `ESC _ G … ST`).
//!
//! The daemon-side counterpart to [`crate::core::osc`]: a tiny, resumable state
//! machine that pulls kitty graphics commands out of the raw PTY byte stream
//! *without* running a full VT parser. It exists for the same reason the OSC
//! sniffer does — the client's emulator (`rio-vt`) only registers graphics it
//! parses as bookkeeping; the product surface that turns a transmission into
//! an image the pane paints lives here. We tap the bytes instead.
//!
//! This lives daemon-side so the pixel payload can be lifted out of the stream
//! *before* it enters the replay ring — a full-window RGBA frame is hundreds of
//! KB, and letting it accumulate in the ring would make reattach replay
//! catastrophic. The daemon decodes here and forwards a compact out-of-band
//! frame to the client; the base64 text never rides the socket, and the VT
//! parser never has to chew through it.
//!
//! Upload, independent placement, transmit-and-display, query and static delete
//! commands feed the living pane's GraphicsState. Media acceptance depends on
//! where the sender lives:
//!
//! - Direct (`t=d`, base64 inline) is always honored — it is the only medium
//!   that survives lyrel's socket + SSH topology.
//! - File (`t=f`/`t=t`) and shared memory (`t=s`) are honored only on a *local*
//!   unix pane, where the name resolves on this host and the daemon can read it
//!   without anything crossing a tunnel. Everywhere else — a remote pane, or a
//!   non-unix host where [`MediumTransfer::resolve`] can't do the work — the
//!   probe is answered *unsupported* so a sender like `terminal-browser` falls
//!   back to `t=d` on its own rather than transmitting into a black hole.
//!
//! Protocol reference: <https://sw.kovidgoyal.net/kitty/graphics-protocol/>

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};

pub use super::graphics_state::{
    GraphicsEffect, GraphicsState, GraphicsUpdate, PlaceholderSlice, Placement,
    placeholder_prototype, placeholder_row,
};

/// Cap on a single APC payload (one chunk) we'll buffer before abandoning it.
/// `terminal-browser` chunks direct transmissions into 4 KiB base64 pieces, but
/// the spec only *recommends* chunking — a sender is free to put a whole frame
/// in one command, and an uncompressed 4K RGBA frame is ~44 MB of base64. Size
/// this to admit a one-shot frame that still fits [`MAX_TRANSMISSION_BASE64`];
/// [`ApcTokenizer::push_graphics`] logs whatever it has to drop.
const MAX_APC_PAYLOAD: usize = MAX_TRANSMISSION_BASE64;

/// Cap on the total base64 bytes received for one chunked transmission.
///
/// The decoded transfer must leave room for GraphicsUpdate's metadata within
/// the pane frame. A 48 MiB base64 limit admits an uncompressed 4K RGBA image
/// while remaining below the 64 MiB pane-frame ceiling.
const MAX_TRANSMISSION_BASE64: usize = 48 << 20; // 48 MiB

/// Cap on the *resolved* pixel bytes of one image, whatever medium carried it.
/// Bounds the file/shm fast path (where the sender names an object whose size we
/// don't control) and backstops the direct path, keeping every frame we hand the
/// daemon inside [`crate::daemon::protocol::MAX_FRAME`].
pub const MAX_IMAGE_BYTES: usize = 32 << 20;

/// A streaming tokenizer that splits raw output into a *passthrough* byte stream
/// and the kitty graphics commands lifted out of it.
///
/// Feed it raw output bytes. It invokes `on_passthrough` with every byte that is
/// **not** part of a `_G` graphics sequence — that is the exact input with each
/// `ESC _ G … ST` removed, and it is what the daemon appends to the replay ring
/// and forwards to the client. It invokes `on_command` with the complete payload
/// of each `_G` command (the bytes after `ESC _`, terminator excluded — e.g.
/// `Ga=T,f=32,…;<base64>`). Non-graphics APC sequences (any payload not starting
/// with `G`) are passed through verbatim, since only kitty graphics is ours to
/// intercept; everything else must reach the client's VT parser unchanged. State
/// persists across `feed` calls, so a sequence split over several reads is still
/// handled.
pub struct ApcTokenizer {
    /// Bytes of a `_G` command accumulated after `ESC _ G` while it can still
    /// terminate. Cleared whenever a command finishes or is abandoned.
    buf: Vec<u8>,
    state: State,
    work: super::graphics_state::retention::Work,
}

#[derive(Default, Clone, Copy)]
enum State {
    /// Not inside an escape sequence; bytes are passthrough.
    #[default]
    Ground,
    /// Held one `ESC` in ground state; a following `_` opens an APC.
    Esc,
    /// Held `ESC _`; the next byte decides graphics (`G`) vs. passthrough APC.
    ApcStart,
    /// Buffering a `_G` graphics command (stripped from passthrough).
    ApcGraphics,
    /// Saw `ESC` inside a graphics command — a following `\` is the terminator.
    ApcGraphicsEsc,
    /// Forwarding a non-graphics APC as passthrough (bytes kept verbatim).
    PassApc,
    /// Saw `ESC` inside a passthrough APC — a following `\` is the terminator.
    PassApcEsc,
    /// Dropping an abandoned/oversized graphics APC (emitted nowhere).
    ApcDrop,
    /// Saw `ESC` inside a dropped APC — a following `\` is the terminator.
    ApcDropEsc,
}

impl Default for ApcTokenizer {
    fn default() -> Self {
        Self::new()
    }
}

impl ApcTokenizer {
    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            state: State::Ground,
            work: {
                let retention = super::graphics_state::retention::Retention::shared();
                retention.work(retention.owner())
            },
        }
    }

    /// Feed one chunk of output. `on_passthrough` receives runs of non-graphics
    /// bytes (the input minus every `ESC _ G … ST`); `on_command` receives the
    /// `G…` payload of each graphics command that terminates within the chunk.
    ///
    /// Like the OSC tokenizer, the states that dominate a real stream — `Ground`
    /// between sequences, and the scan-to-terminator states inside an APC — skip
    /// ahead with SIMD `memchr` rather than stepping per byte; only the few
    /// escape-decision states step one byte at a time. APC has no `BEL`
    /// terminator (kitty always closes with `ST`), so only `ESC` can end a
    /// payload. A held `ESC`/`ESC _` that later proves to be passthrough is
    /// re-emitted as a constant, so nothing needs to survive across `feed` calls
    /// but the small state tag and the graphics buffer.
    pub fn feed(
        &mut self,
        bytes: &[u8],
        on_passthrough: impl FnMut(&[u8]),
        mut on_command: impl FnMut(&[u8]),
    ) {
        self.tokenize(bytes, on_passthrough, |command, aborted, _| {
            if !aborted {
                on_command(command);
            }
        });
    }

    // Aborts must reach the reassembler in the same ordered stream as complete
    // commands, including oversize/interruption across different PTY reads.
    fn tokenize(
        &mut self,
        bytes: &[u8],
        mut on_passthrough: impl FnMut(&[u8]),
        mut on_command: impl FnMut(&[u8], bool, bool),
    ) {
        let mut i = 0;
        while i < bytes.len() {
            match self.state {
                State::Ground => match memchr::memchr(0x1b, &bytes[i..]) {
                    Some(off) => {
                        if off > 0 {
                            on_passthrough(&bytes[i..i + off]);
                        }
                        self.state = State::Esc;
                        i += off + 1;
                    }
                    None => {
                        on_passthrough(&bytes[i..]);
                        return;
                    }
                },
                State::PassApc => match memchr::memchr(0x1b, &bytes[i..]) {
                    Some(off) => {
                        if off > 0 {
                            on_passthrough(&bytes[i..i + off]);
                        }
                        self.state = State::PassApcEsc;
                        i += off + 1;
                    }
                    None => {
                        on_passthrough(&bytes[i..]);
                        return;
                    }
                },
                State::ApcDrop => match memchr::memchr3(0x1b, 0x18, 0x1a, &bytes[i..]) {
                    Some(off) => {
                        self.state = if bytes[i + off] == 0x1b {
                            State::ApcDropEsc
                        } else {
                            State::Ground
                        };
                        i += off + 1;
                    }
                    None => return,
                },
                State::ApcGraphics => match memchr::memchr3(0x1b, 0x18, 0x1a, &bytes[i..]) {
                    Some(off) => {
                        if self.push_graphics(&bytes[i..i + off]) {
                            if bytes[i + off] == 0x1b {
                                self.state = State::ApcGraphicsEsc;
                            } else {
                                on_command(&self.buf, true, false);
                                self.clear_buffer();
                                self.state = State::Ground;
                            }
                            i += off + 1;
                        } else {
                            on_command(&self.buf, true, true);
                            self.clear_buffer();
                            // Oversized: abandon and let the drop state consume
                            // the terminator we just found (leave `i` on the ESC).
                            self.state = State::ApcDrop;
                            i += off;
                        }
                    }
                    None => {
                        if !self.push_graphics(&bytes[i..]) {
                            on_command(&self.buf, true, true);
                            self.clear_buffer();
                            self.state = State::ApcDrop;
                        }
                        return;
                    }
                },
                State::Esc => match bytes[i] {
                    b'_' => {
                        self.clear_buffer();
                        self.state = State::ApcStart;
                        i += 1;
                    }
                    // A run of ESCs: the earlier one was a lone ESC (passthrough);
                    // keep the newest one held.
                    0x1b => {
                        on_passthrough(b"\x1b");
                        i += 1;
                    }
                    // The ESC began some other escape: emit it and re-examine this
                    // byte from ground (it is not an ESC, so it joins the run).
                    _ => {
                        on_passthrough(b"\x1b");
                        self.state = State::Ground;
                    }
                },
                State::ApcStart => {
                    if bytes[i] == b'G' {
                        self.clear_buffer();
                        if !self.push_graphics(b"G") {
                            self.state = State::ApcDrop;
                            i += 1;
                            continue;
                        }
                        self.state = State::ApcGraphics;
                        i += 1;
                    } else {
                        // Not ours: emit the held `ESC _` and forward the rest of
                        // the APC verbatim (re-examine this byte in `PassApc`).
                        on_passthrough(b"\x1b_");
                        self.state = State::PassApc;
                    }
                }
                State::ApcGraphicsEsc => match bytes[i] {
                    b'\\' => {
                        on_command(&self.buf, false, false);
                        self.clear_buffer();
                        self.state = State::Ground;
                        i += 1;
                    }
                    0x1b => i += 1, // a run of ESCs; stay poised for `\`
                    // A bare `ESC _` re-opens: the next byte decides graphics again.
                    b'_' => {
                        on_command(&self.buf, true, false);
                        self.clear_buffer();
                        self.state = State::ApcStart;
                        i += 1;
                    }
                    // ESC began some other escape: abandon this graphics command.
                    // Its bytes were meant as graphics, so they stay stripped —
                    // but the escape itself belongs to the terminal, so forward
                    // it and re-examine this byte from Ground rather than eating
                    // both. Swallowing them would turn a `\x1b[31m` that follows
                    // an unterminated graphics command into literal `31m`.
                    _ => {
                        on_command(&self.buf, true, false);
                        self.clear_buffer();
                        on_passthrough(b"\x1b");
                        self.state = State::Ground;
                    }
                },
                State::PassApcEsc => match bytes[i] {
                    b'\\' => {
                        on_passthrough(b"\x1b\\"); // ST is part of the forwarded APC
                        self.state = State::Ground;
                        i += 1;
                    }
                    0x1b => {
                        on_passthrough(b"\x1b"); // a lone ESC in APC data
                        i += 1;
                    }
                    // A bare `ESC _` re-opens, same as the two graphics states
                    // do. Without this, a foreign APC that never sends its ST
                    // swallows every `ESC _G …` after it — the graphics get
                    // forwarded as APC text that the client's vte then discards,
                    // so the image is simply lost.
                    b'_' => {
                        self.buf.clear();
                        self.state = State::ApcStart;
                        i += 1;
                    }
                    _ => {
                        on_passthrough(b"\x1b");
                        self.state = State::PassApc; // re-examine this byte
                    }
                },
                State::ApcDropEsc => match bytes[i] {
                    b'\\' => {
                        self.state = State::Ground;
                        i += 1;
                    }
                    0x1b => i += 1,
                    b'_' => {
                        self.clear_buffer();
                        self.state = State::ApcStart;
                        i += 1;
                    }
                    // As in `ApcGraphicsEsc`: the dropped command's bytes stay
                    // stripped, but the escape that interrupted it is the
                    // terminal's and has to reach the client intact.
                    _ => {
                        on_passthrough(b"\x1b");
                        self.state = State::Ground;
                    }
                },
            }
        }
    }

    fn clear_buffer(&mut self) {
        self.buf = Vec::new();
        self.work.resize(0);
    }

    /// Append within the cap; keep a bounded header prefix on failure so the
    /// ordered abort can correlate an error before the caller clears it.
    fn push_graphics(&mut self, run: &[u8]) -> bool {
        if run.len() > MAX_APC_PAYLOAD.saturating_sub(self.buf.len()) {
            log::debug!(
                "kitty graphics: dropping a command whose payload passed {MAX_APC_PAYLOAD} bytes"
            );
            if self.buf.len() < 4096 {
                let prefix = run.len().min(4096 - self.buf.len());
                let required = self.buf.len() + prefix;
                if self.work.resize(required + self.buf.capacity()) {
                    self.buf
                        .reserve_exact(required.saturating_sub(self.buf.len()));
                    self.work.resize(self.buf.capacity());
                    self.buf.extend_from_slice(&run[..prefix]);
                }
            }
            return false;
        }
        let required = self.buf.len() + run.len();
        if required > self.buf.capacity() {
            let capacity = required
                .max(self.buf.capacity().saturating_mul(2))
                .min(MAX_APC_PAYLOAD);
            if !self.work.resize(capacity + self.buf.capacity()) {
                return false;
            }
            self.buf.reserve_exact(capacity - self.buf.len());
            self.work.resize(self.buf.capacity());
        }
        self.buf.extend_from_slice(run);
        true
    }
}

/// Daemon-side splitter: the [`ApcTokenizer`] wired to a [`GraphicsParser`].
///
/// This is the one type the PTY reader loop touches. Feed it raw output and a
/// passthrough sink; it strips every kitty graphics sequence, funnels the
/// passthrough bytes to the sink (for the replay ring and the client), and
/// returns complete graphics requests for the pane-owned GraphicsState, or
/// immediate malformed-transfer replies to write back to the PTY.
#[derive(Default)]
pub struct GraphicsSniffer {
    tokenizer: ApcTokenizer,
    parser: GraphicsParser,
}

impl GraphicsSniffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// A sniffer whose parser may honor file/shm transfer (local pane only).
    /// See [`GraphicsParser::new_local`].
    pub fn new_local(local: bool) -> Self {
        Self {
            tokenizer: ApcTokenizer::new(),
            parser: GraphicsParser::new_local(local),
        }
    }

    pub(crate) fn new_budgeted(local: bool, owner: u64) -> Self {
        let mut sniffer = Self::new_local(local);
        let retention = super::graphics_state::retention::Retention::shared();
        sniffer.tokenizer.work = retention.work(owner);
        sniffer.parser.work = Some(retention.work(owner));
        sniffer
    }

    pub(crate) fn begin_read(&mut self) {
        self.parser.completed_bytes = 0;
        if let Some(work) = &mut self.parser.work {
            work.resize(
                self.parser
                    .pending
                    .as_ref()
                    .map_or(0, |p| p.data.capacity()),
            );
        }
    }

    /// Update whether the sender currently shares this host's filesystem. A pane
    /// is local when lyrel spawned a local shell and no foreground `ssh` owns the
    /// PTY; that can flip mid-session, and it gates whether the next `a=q` probe
    /// is answered `OK` for file/shm transfer. Cheap enough to call per chunk.
    pub fn set_local(&mut self, local: bool) {
        self.parser.local = local;
    }

    /// Feed one chunk of raw PTY output. `on_passthrough` receives the byte
    /// stream with all graphics sequences removed; the returned events are the
    /// complete requests and malformed-transfer replies, in order.
    ///
    /// This drops the *relative order* of passthrough vs. events; for the daemon
    /// loop, prefer [`sniff`](Self::sniff), which preserves it. Retained as the
    /// low-level primitive the unit tests drive.
    pub fn feed(&mut self, bytes: &[u8], on_passthrough: impl FnMut(&[u8])) -> Vec<Event> {
        self.begin_read();
        let Self { tokenizer, parser } = self;
        let mut events = Vec::new();
        tokenizer.tokenize(bytes, on_passthrough, |cmd, aborted, exhausted| {
            if let Some(ev) = if aborted {
                parser.abort(cmd, exhausted)
            } else {
                parser.feed(cmd)
            } {
                events.push(ev);
            }
        });
        events
    }

    /// Feed a chunk and get back its content **in stream order**, taking a
    /// zero-copy fast path on the overwhelmingly common chunk that carries no
    /// graphics at all.
    ///
    /// Order matters: a kitty image is placed at the cursor cell as it stood
    /// *when its command appeared in the stream*, so the client must apply the
    /// text before an image before resolving that image's anchor. Returning
    /// ordered [`Segment`]s lets the daemon forward `Output`/`Image` frames
    /// interleaved exactly as they arrived, so the client's mirror cursor is
    /// correct at each placement.
    ///
    /// The fast path fires when the tokenizer is between sequences *and* the
    /// chunk holds no APC opener (`ESC _`) — then the input **is** one big
    /// output segment, returned borrowed with no allocation. A build's worth of
    /// colored stdout (full of CSI escapes but no APC) stays on this path. The
    /// trailing-`ESC` guard keeps an `ESC _` straddling a chunk boundary on the
    /// slow path, where the held-`ESC` state carries it across correctly.
    pub fn sniff<'a>(&mut self, bytes: &'a [u8]) -> Sniffed<'a> {
        self.begin_read();
        self.sniff_part(bytes)
    }

    pub(crate) fn sniff_part<'a>(&mut self, bytes: &'a [u8]) -> Sniffed<'a> {
        if matches!(self.tokenizer.state, State::Ground)
            && bytes.last() != Some(&0x1b)
            && memchr::memmem::find(bytes, b"\x1b_").is_none()
        {
            return Sniffed::Plain(bytes);
        }
        let Self { tokenizer, parser } = self;
        // Both tokenizer callbacks push onto the same segment list; `RefCell`
        // lets the passthrough and command sinks share it without either taking
        // a lasting mutable borrow the borrow checker would reject.
        let segments: std::cell::RefCell<Vec<Segment>> = std::cell::RefCell::new(Vec::new());
        tokenizer.tokenize(
            bytes,
            |run| {
                // Coalesce adjacent passthrough runs into one output segment so
                // the client applies them in a single `advance`.
                let mut segs = segments.borrow_mut();
                if let Some(Segment::Output(buf)) = segs.last_mut() {
                    buf.extend_from_slice(run);
                } else {
                    segs.push(Segment::Output(run.to_vec()));
                }
            },
            |cmd, aborted, exhausted| {
                if let Some(ev) = if aborted {
                    parser.abort(cmd, exhausted)
                } else {
                    parser.feed(cmd)
                } {
                    segments.borrow_mut().push(Segment::from(ev));
                }
            },
        );
        Sniffed::Segments(segments.into_inner())
    }
}

/// One ordered piece of a sniffed chunk — see [`GraphicsSniffer::sniff`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Output(Vec<u8>),
    Command {
        control: Control,
        image: Option<Image>,
    },
    Reply(Vec<u8>),
}

impl From<Event> for Segment {
    fn from(event: Event) -> Self {
        match event {
            Event::Command { control, image } => Self::Command { control, image },
            Event::Reply(reply) => Self::Reply(reply),
        }
    }
}

/// The result of [`GraphicsSniffer::sniff`]: either the whole chunk borrowed as
/// output (the graphics-free fast path), or ordered [`Segment`]s.
pub enum Sniffed<'a> {
    /// No graphics in this chunk: the input is output, verbatim and borrowed.
    Plain(&'a [u8]),
    /// Graphics present: apply these in order.
    Segments(Vec<Segment>),
}

/// The kitty action key (`a=`). Only the variants lyrel acts on are named; the
/// rest of the protocol (frames, animation, compose) folds into `Other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// `a=q` — query whether a transmission would succeed. We must reply.
    Query,
    /// `a=t` — transmit only (no display). Rare from our target sender.
    Transmit,
    /// `a=T` — transmit and display.
    TransmitAndDisplay,
    /// `a=p` — display a previously transmitted image.
    Display,
    /// `a=d` — delete image(s)/placement(s).
    Delete,
    /// Any action we don't specifically handle.
    Other,
}

/// The pixel format key (`f=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireFormat {
    /// `f=24` — 3 bytes/pixel RGB.
    Rgb,
    /// `f=32` — 4 bytes/pixel RGBA (the default, and what `terminal-browser` uses).
    Rgba,
    /// `f=100` — a PNG file; pixels stay encoded (decoded client-side).
    Png,
}

/// The transmission medium key (`t=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Medium {
    /// `t=d` — direct: the payload is (base64) the image data itself.
    Direct,
    /// `t=f` — a filesystem path to the data.
    File,
    /// `t=s` — a POSIX shared-memory object name.
    Shared,
    /// `t=t` — a temporary file (deleted after reading).
    TempFile,
}

/// Parsed kitty graphics control keys (the `k=v,k=v` list before the `;`).
///
/// Only the keys lyrel needs are surfaced; unknown keys are ignored so forward
/// protocol additions degrade quietly rather than failing the whole command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Control {
    pub action: Action,
    pub format: WireFormat,
    pub medium: Medium,
    /// `o=z` — payload is zlib-compressed.
    pub compressed: bool,
    /// `i=` — client-assigned image id (0 = unset).
    pub id: u32,
    /// `I=` — client-assigned image number (0 = unset).
    pub number: u32,
    /// `p=` — placement id (0 = unset).
    pub placement: u32,
    /// `s=` — source pixel width.
    pub width: u32,
    /// `v=` — source pixel height.
    pub height: u32,
    /// `c=` — columns to display across (0 = derive from pixels).
    pub cols: u32,
    /// `r=` — rows to display down (0 = derive from pixels).
    pub rows: u32,
    /// `m=1` — more chunks follow.
    pub more: bool,
    /// `d=` — delete target (only meaningful for `a=d`).
    pub delete: u8,
    /// `q=` — suppress responses (1 = errors only, 2 = all).
    pub quiet: u8,
    /// `O=` — byte offset into a file/shm object (file/shm mediums only).
    pub offset: u32,
    /// `S=` — number of bytes to read from a file/shm object (0 = to end).
    pub size: u32,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub offset_x: u32,
    pub offset_y: u32,
    pub z: i32,
    pub no_move: bool,
    /// `U=1` creates an invisible prototype referenced by Unicode grid cells.
    pub virtual_placement: bool,
    pub invalid: bool,
    pub unsupported: bool,
}

impl Default for Control {
    fn default() -> Self {
        // Protocol defaults: a=t, f=32, t=d, no compression.
        Self {
            action: Action::Transmit,
            format: WireFormat::Rgba,
            medium: Medium::Direct,
            compressed: false,
            id: 0,
            number: 0,
            placement: 0,
            width: 0,
            height: 0,
            cols: 0,
            rows: 0,
            more: false,
            delete: 0,
            quiet: 0,
            offset: 0,
            size: 0,
            x: 0,
            y: 0,
            w: 0,
            h: 0,
            offset_x: 0,
            offset_y: 0,
            z: 0,
            no_move: false,
            virtual_placement: false,
            invalid: false,
            unsupported: false,
        }
    }
}

impl Control {
    /// Parse the control section of a `_G` command — the `G…` payload up to (but
    /// not including) the first `;`. The leading `G` is part of the first key
    /// name's position, i.e. the payload is `Ga=T,f=32,…;<data>`; we accept the
    /// whole `G…;…` slice and split on the `;`.
    pub fn parse(payload: &[u8]) -> Option<Self> {
        // Strip the leading `G` identifier.
        let rest = payload.strip_prefix(b"G")?;
        let control = match rest.iter().position(|&b| b == b';') {
            Some(pos) => &rest[..pos],
            None => rest, // no payload section (e.g. `a=d`)
        };
        let mut c = Control::default();
        let mut has_id = false;
        let mut has_number = false;
        for pair in control.split(|&b| b == b',') {
            if pair.is_empty() {
                continue;
            }
            let mut kv = pair.splitn(2, |&b| b == b'=');
            let key = kv.next().unwrap_or(b"");
            let val = kv.next().unwrap_or(b"");
            let num = || parse_u32(val);
            match key {
                b"a" => {
                    c.action = match val {
                        b"q" => Action::Query,
                        b"t" => Action::Transmit,
                        b"T" => Action::TransmitAndDisplay,
                        b"p" => Action::Display,
                        b"d" => Action::Delete,
                        _ => Action::Other,
                    }
                }
                b"f" => {
                    c.format = match val {
                        b"24" => WireFormat::Rgb,
                        b"32" => WireFormat::Rgba,
                        b"100" => WireFormat::Png,
                        _ => {
                            c.unsupported = true;
                            WireFormat::Rgba
                        }
                    }
                }
                b"t" => {
                    c.medium = match val {
                        b"d" => Medium::Direct,
                        b"f" => Medium::File,
                        b"s" => Medium::Shared,
                        b"t" => Medium::TempFile,
                        _ => {
                            c.unsupported = true;
                            Medium::Direct
                        }
                    }
                }
                b"o" => {
                    c.compressed = val == b"z";
                    c.unsupported |= val != b"z";
                }
                b"i" => {
                    has_id = true;
                    c.id = num();
                }
                b"I" => {
                    has_number = true;
                    c.number = num();
                }
                b"p" => c.placement = num(),
                b"s" => c.width = num(),
                b"v" => c.height = num(),
                b"c" => c.cols = num(),
                b"r" => c.rows = num(),
                b"m" => {
                    c.more = val == b"1";
                    c.invalid |= val != b"0" && val != b"1";
                }
                b"d" => {
                    c.delete = val.first().copied().unwrap_or(0);
                    c.invalid |= val.len() != 1;
                }
                // Clamp rather than truncate: `q=256` must not wrap to 0 and
                // turn a request for silence into a request for chatter.
                b"q" => c.quiet = num().min(u8::MAX as u32) as u8,
                b"O" => c.offset = num(),
                b"S" => c.size = num(),
                b"x" => c.x = num(),
                b"y" => c.y = num(),
                b"w" => c.w = num(),
                b"h" => c.h = num(),
                b"X" => c.offset_x = num(),
                b"Y" => c.offset_y = num(),
                b"z" => match std::str::from_utf8(val).ok().and_then(|s| s.parse().ok()) {
                    Some(z) => c.z = z,
                    None => c.invalid = true,
                },
                b"C" => {
                    c.no_move = val == b"1";
                    c.invalid |= val != b"0" && val != b"1";
                }
                b"U" => {
                    c.virtual_placement = val == b"1";
                    c.invalid |= val != b"0" && val != b"1";
                }
                b"P" | b"Q" | b"H" | b"V" => c.unsupported |= val != b"0",
                _ => {} // unknown key: ignore
            }
            if matches!(
                key,
                b"i" | b"I"
                    | b"p"
                    | b"s"
                    | b"v"
                    | b"c"
                    | b"r"
                    | b"x"
                    | b"y"
                    | b"w"
                    | b"h"
                    | b"X"
                    | b"Y"
                    | b"O"
                    | b"S"
                    | b"q"
            ) && std::str::from_utf8(val)
                .ok()
                .and_then(|s| s.parse::<u32>().ok())
                .is_none()
            {
                c.invalid = true;
            }
        }
        c.invalid |= has_id && has_number;
        Some(c)
    }
}

fn parse_u32(bytes: &[u8]) -> u32 {
    let mut n: u32 = 0;
    for &b in bytes {
        if !b.is_ascii_digit() {
            return 0;
        }
        n = n.saturating_mul(10).saturating_add((b - b'0') as u32);
    }
    n
}

/// The `;`-separated data section of a `_G` command (may be empty).
fn payload_data(command: &[u8]) -> &[u8] {
    match command.iter().position(|&b| b == b';') {
        Some(pos) => &command[pos + 1..],
        None => &[],
    }
}

/// A reassembled image transmission, base64-decoded but *not yet* inflated.
///
/// The parser strips base64 inflation. GraphicsState validates raw expansion
/// and PNG pixels before retaining a resource. Raw zlib stays compressed on the
/// socket; an outer PNG zlib wrapper is removed while the PNG itself remains
/// encoded. The desktop worker normalizes the retained content into RGBA8.
///
/// [`data`]: Image::data
/// [`to_rgba8`]: Image::to_rgba8
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// Source pixel dimensions.
    pub width: u32,
    pub height: u32,
    /// The base64-decoded payload: zlib-compressed pixels when [`compressed`],
    /// otherwise raw pixels ([`WireFormat::Rgb`]/[`WireFormat::Rgba`]) or an
    /// encoded PNG file ([`WireFormat::Png`]).
    ///
    /// [`compressed`]: Image::compressed
    pub data: Vec<u8>,
    pub format: WireFormat,
    /// Whether [`data`](Image::data) is still zlib-compressed (`o=z`).
    pub compressed: bool,
}

impl Image {
    /// Inflate and normalize to tightly packed RGBA8 (`width*height*4`), the
    /// form the renderer wants. Runs client-side. Returns `None` for a PNG
    /// (whose decode needs the `image` crate the GUI owns, not this core) or on
    /// a malformed payload; PNG callers should decode [`data`](Image::data)
    /// themselves after checking [`format`](Image::format).
    ///
    /// The inflate is bounded by what `width`/`height` claim the pixels are:
    /// deflate expands ~1000:1 in the limit, so an unbounded `decompress_to_vec`
    /// here would let a payload well inside [`MAX_TRANSMISSION_BASE64`] balloon
    /// into tens of GB and OOM the client — and this runs in the GUI process, so
    /// it would take every pane down, not just the one that received the escape.
    /// The caller checks the inflated length against `width*height*4` anyway, so
    /// capping it up front costs nothing.
    pub fn to_rgba8(&self) -> Option<Vec<u8>> {
        let expected = self.decoded_len()?;
        if expected > super::graphics_state::MAX_IMAGE_DECODED_BYTES {
            return None;
        }
        let raw = if self.compressed {
            miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&self.data, expected).ok()?
        } else {
            self.data.clone()
        };
        if raw.len() != expected {
            return None;
        }
        Some(if self.format == WireFormat::Rgb {
            rgb_to_rgba(&raw)
        } else {
            raw
        })
    }

    /// Consume the pixel buffer on the uncompressed fast path. Normalization
    /// and all size checks stay off the UI thread in the desktop decode worker.
    pub fn take_rgba8(&mut self) -> Option<Vec<u8>> {
        let expected = self.decoded_len()?;
        if expected > super::graphics_state::MAX_IMAGE_DECODED_BYTES {
            return None;
        }
        let data = std::mem::take(&mut self.data);
        let raw = if self.compressed {
            miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&data, expected).ok()?
        } else {
            data
        };
        if raw.len() != expected {
            return None;
        }
        Some(if self.format == WireFormat::Rgb {
            rgb_to_rgba(&raw)
        } else {
            raw
        })
    }

    /// The byte length of this image's *decoded* (pre-`rgb_to_rgba`) pixels —
    /// `width * height * bytes_per_pixel` for the wire format. `None` on overflow
    /// or a zero-sized/PNG image, which have no fixed raw length.
    fn decoded_len(&self) -> Option<usize> {
        // RGB expansion must be admitted by its RGBA allocation, not its smaller
        // wire payload. These helpers also defend desktop frames independently.
        if self.width > super::graphics_state::MAX_IMAGE_DIMENSION
            || self.height > super::graphics_state::MAX_IMAGE_DIMENSION
            || (self.width as usize)
                .checked_mul(self.height as usize)?
                .checked_mul(4)?
                > super::graphics_state::MAX_IMAGE_DECODED_BYTES
        {
            return None;
        }
        let bpp = match self.format {
            WireFormat::Rgb => 3usize,
            WireFormat::Rgba => 4usize,
            WireFormat::Png => return None,
        };
        (self.width as usize)
            .checked_mul(self.height as usize)?
            .checked_mul(bpp)
            .filter(|&n| n != 0)
    }
}

impl MediumTransfer {
    /// Resolve the referenced bytes by reading the file or POSIX shm object,
    /// then unlinking an eligible temporary file or consumed shm object. Plain
    /// files remain the sender's property. Runs on a local daemon reader where
    /// the name resolves on this host. Returns `None` if the name is unusable or
    /// the object can't be read.
    ///
    /// The pixels come back *uncompressed* unless the sender set `o=z`: a sender
    /// that reaches for shared memory does so precisely to avoid the zlib the
    /// inline path forces, so this is the fast path that skips the client-side
    /// inflate entirely.
    #[cfg(unix)]
    pub fn resolve(&self) -> Option<Image> {
        let data = match self.medium {
            Medium::Shared => self.read_shared()?,
            Medium::File | Medium::TempFile => self.read_file()?,
            // Direct never becomes a MediumTransfer.
            Medium::Direct => return None,
        };
        Some(Image {
            width: self.width,
            height: self.height,
            data,
            format: self.format,
            compressed: self.compressed,
        })
    }

    #[cfg(not(unix))]
    pub fn resolve(&self) -> Option<Image> {
        None
    }

    /// Read only the requested bounded region. Descriptor reads, unlike mmap,
    /// fail safely if an untrusted sender truncates an object concurrently.
    #[cfg(unix)]
    fn read_region(&self, mut file: std::fs::File) -> Option<Vec<u8>> {
        use std::io::{Read as _, Seek as _, SeekFrom};
        let meta = file.metadata().ok()?;
        if !meta.is_file() || meta.len() > MAX_IMAGE_BYTES as u64 {
            return None;
        }
        let start = u64::from(self.offset);
        let available = meta.len().checked_sub(start)?;
        let len = if self.size == 0 {
            available
        } else {
            available.min(u64::from(self.size))
        };
        file.seek(SeekFrom::Start(start)).ok()?;
        let mut out = vec![0; len as usize];
        file.read_exact(&mut out).ok()?;
        Some(out)
    }

    #[cfg(unix)]
    fn read_file(&self) -> Option<Vec<u8>> {
        use std::os::unix::ffi::OsStringExt;
        use std::os::unix::fs::OpenOptionsExt as _;
        let path = std::path::PathBuf::from(std::ffi::OsString::from_vec(self.name.clone()));

        // Read through a bounded reader, not `fs::read`. The name is attacker
        // -reachable (any program that can write to the pty picks it), and
        // `fs::read` on `/dev/zero` never returns — on the daemon *reader*
        // thread, which would wedge the pane's whole output path. Refusing
        // anything that isn't a regular file also keeps us off fifos and
        // devices, where the open itself can block.
        let meta = std::fs::metadata(&path).ok()?;
        if !meta.is_file() {
            return None;
        }
        // A path can change after metadata: nonblocking open plus a second
        // descriptor-based check also closes the FIFO replacement race.
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path)
            .ok()?;
        let out = self.read_region(file)?;
        // A temp file is the sender's one-shot handoff: remove it after reading
        // so the browser's per-frame temp files don't pile up. A named `t=f`
        // file is the sender's to manage; leave it.
        //
        // Only unlink inside a known temp directory. `name` is an arbitrary path
        // out of an escape sequence — `cat`ing a hostile file is enough to send
        // one — so an unqualified `remove_file` here would delete anything the
        // user can, `~/.ssh/id_ed25519` included. The spec requires this check
        // for exactly that reason.
        if self.medium == Medium::TempFile
            && path_is_in_temp_dir(&path)
            && path
                .as_os_str()
                .as_encoded_bytes()
                .windows(b"tty-graphics-protocol".len())
                .any(|part| part == b"tty-graphics-protocol")
        {
            let _ = std::fs::remove_file(&path);
        }
        Some(out)
    }

    /// Consume one POSIX shm handoff. A bounded descriptor read avoids SIGBUS
    /// if the producer truncates the object after its size has been checked.
    #[cfg(unix)]
    fn read_shared(&self) -> Option<Vec<u8>> {
        use std::os::fd::FromRawFd as _;
        // A POSIX shm name is a single `/`-prefixed component — no embedded
        // separators, no `..`. Some platforms resolve `shm_open` against the
        // filesystem, where a name like `/../../etc/passwd` would escape the shm
        // namespace and reach a real path we'd then `shm_unlink`.
        if !shm_name_is_wellformed(&self.name) {
            return None;
        }
        // The name must be a C string; kitty shm names look like `/px-…`.
        let cname = std::ffi::CString::new(self.name.clone()).ok()?;
        // SAFETY: shm_open returns an owned descriptor, immediately transferred
        // to File. The object name is a validated CString and is unlinked on
        // every path after a successful open.
        unsafe {
            let fd = libc::shm_open(cname.as_ptr(), libc::O_RDONLY, 0);
            if fd < 0 {
                return None;
            }
            let out = self.read_region(std::fs::File::from_raw_fd(fd));
            // One-shot: the sender expects us to reclaim the object.
            libc::shm_unlink(cname.as_ptr());
            out
        }
    }
}

/// Whether `path` sits inside a directory the platform hands out for temp files,
/// which is the only place a `t=t` handoff may be unlinked.
///
/// Compares *canonicalized* paths so `/tmp/../home/me/.ssh/id_ed25519` — which
/// has the right prefix textually — doesn't pass. On macOS `TMPDIR` is a
/// per-user path under `/var/folders/…` that canonicalizes through the
/// `/private` symlink, so `std::env::temp_dir()` is canonicalized too rather
/// than compared raw.
#[cfg(unix)]
fn path_is_in_temp_dir(path: &std::path::Path) -> bool {
    let Ok(real) = path.canonicalize() else {
        return false;
    };
    // `/dev/shm` is where a `t=t` sender that wanted shm-like semantics without
    // `shm_open` puts its handoff; kitty accepts it alongside the temp dirs.
    let candidates = [
        std::env::temp_dir(),
        std::path::PathBuf::from("/tmp"),
        std::path::PathBuf::from("/dev/shm"),
    ];
    candidates.iter().any(|dir| {
        dir.canonicalize()
            .is_ok_and(|d| real.starts_with(&d) && real != d)
    })
}

/// Whether `name` is a well-formed POSIX shm object name: a leading `/` followed
/// by one non-empty component with no further separators and no `.`/`..`.
#[cfg(unix)]
fn shm_name_is_wellformed(name: &[u8]) -> bool {
    // POSIX caps the name at NAME_MAX; 255 is the floor every platform we build
    // for meets, and no real sender comes close.
    if name.len() < 2 || name.len() > 255 || name[0] != b'/' {
        return false;
    }
    let rest = &name[1..];
    !rest.contains(&b'/') && !rest.contains(&0) && rest != b"." && rest != b".."
}

/// A complete request. GraphicsState owns identity and lifetime decisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Command {
        control: Control,
        image: Option<Image>,
    },
    Reply(Vec<u8>),
}

/// A file/shm transmission the daemon must resolve into pixels. The `name` is
/// the (base64-decoded) payload the sender put after the `;`: a filesystem path
/// for [`Medium::File`]/[`Medium::TempFile`], or a POSIX shm object name for
/// [`Medium::Shared`]. `offset`/`size` bound the region to read (`size == 0`
/// means "to end of object"). All the metadata needed to build the final
/// [`Image`] rides along so the daemon does no parsing of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediumTransfer {
    pub medium: Medium,
    pub name: Vec<u8>,
    pub offset: u32,
    pub size: u32,
    pub width: u32,
    pub height: u32,
    pub format: WireFormat,
    /// Whether the referenced bytes are zlib-compressed (`o=z`). A sender that
    /// picks shm/file for speed sends raw pixels, but honor the flag regardless.
    pub compressed: bool,
}

/// State for reassembling a chunked (`m=1`) direct transmission. Kitty
/// serializes chunked transmissions — only one is in flight at a time — so a
/// single pending accumulator suffices.
#[derive(Default)]
struct Pending {
    control: Control,
    /// Decode each base64 chunk before retaining it. This also accepts senders
    /// that pad each chunk independently without retaining two full encodings.
    data: Vec<u8>,
    encoded_bytes: usize,
}

/// Reassembles and decodes kitty graphics commands emitted by [`ApcTokenizer`].
///
/// The parser owns bounded chunk reassembly, base64 decoding and host-local
/// medium resolution. GraphicsState validates resources and decides all
/// identities and placement lifetimes; desktop decoding stays asynchronous.
#[derive(Default)]
pub struct GraphicsParser {
    pending: Option<Pending>,
    /// Whether the sender shares this host's filesystem (a local pane). Only
    /// then can we honor file/shm transfer, whose names are host-local; a pane
    /// running over SSH must stay on inline `t=d` so the pixels ride the tunnel.
    local: bool,
    work: Option<super::graphics_state::retention::Work>,
    completed_bytes: usize,
}

impl GraphicsParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn new_local(local: bool) -> Self {
        Self {
            local,
            ..Self::default()
        }
    }

    fn cancel_pending(&mut self) {
        self.pending = None;
        if let Some(work) = &mut self.work {
            work.resize(self.completed_bytes);
        }
    }

    fn abort(&mut self, command: &[u8], exhausted: bool) -> Option<Event> {
        let control = self
            .pending
            .take()
            .map(|pending| pending.control)
            .or_else(|| Control::parse(command))?;
        self.cancel_pending();
        response(
            &control,
            control.id,
            if exhausted {
                "ENOSPC:Graphics transfer budget exhausted"
            } else {
                "EINVAL:Interrupted image command"
            },
        )
        .map(Event::Reply)
    }

    pub fn feed(&mut self, command: &[u8]) -> Option<Event> {
        let control = Control::parse(command)?;
        let data = payload_data(command);
        // A delete aborts a partial upload. Other complete commands must not be
        // accidentally swallowed as base64 continuation bytes.
        if control.action == Action::Delete {
            self.cancel_pending();
        }
        if self.pending.is_some() {
            let header = command.split(|b| *b == b';').next().unwrap_or(command);
            let header = header.strip_prefix(b"G").unwrap_or(header);
            let continuation = header
                .split(|b| *b == b',')
                .all(|pair| pair.starts_with(b"m=") || pair.starts_with(b"q=") || pair.is_empty());
            if continuation {
                if header
                    .split(|b| *b == b',')
                    .any(|pair| pair.starts_with(b"q="))
                {
                    self.pending.as_mut().unwrap().control.quiet = control.quiet;
                }
                if control.invalid {
                    let first = self.pending.take().unwrap().control;
                    self.cancel_pending();
                    return response(&first, first.id, "EINVAL:Invalid continuation control")
                        .map(Event::Reply);
                }
                return self.accept_chunk(control, data);
            }
            self.cancel_pending();
        }
        if control.invalid || control.unsupported || control.action == Action::Other {
            self.cancel_pending();
            let status = if control.invalid {
                "EINVAL:Invalid control data"
            } else {
                "ENOTSUPPORTED:Unsupported operation"
            };
            return response(&control, control.id, status).map(Event::Reply);
        }
        match control.action {
            Action::Transmit | Action::TransmitAndDisplay | Action::Query => {
                self.accept_chunk(control, data)
            }
            Action::Display | Action::Delete => {
                if !data.is_empty() || control.more {
                    response(&control, control.id, "EINVAL:Unexpected placement payload")
                        .map(Event::Reply)
                } else {
                    Some(Event::Command {
                        control,
                        image: None,
                    })
                }
            }
            Action::Other => unreachable!(),
        }
    }

    fn accept_chunk(&mut self, control: Control, data: &[u8]) -> Option<Event> {
        let first = self.pending.as_ref().map_or(control, |p| p.control);
        let fail = |status| response(&first, first.id, status).map(Event::Reply);
        if first.medium != Medium::Direct && !(self.local && cfg!(unix)) {
            self.cancel_pending();
            return fail("ENOTSUPPORTED:Host-local transfer unavailable");
        }
        let encoded_bytes = self.pending.as_ref().map_or(0, |p| p.encoded_bytes);
        if data.len() > MAX_TRANSMISSION_BASE64.saturating_sub(encoded_bytes) {
            self.cancel_pending();
            return fail("ENOSPC:Transfer too large");
        }
        let raw_len = if first.format != WireFormat::Png {
            let image = Image {
                width: first.width,
                height: first.height,
                format: first.format,
                compressed: first.compressed,
                data: Vec::new(),
            };
            match image.decoded_len() {
                Some(len) => Some(len),
                None => {
                    self.cancel_pending();
                    return fail("ENOSPC:Decoded image dimensions exceed limit");
                }
            }
        } else {
            None
        };
        // Non-final blocks are independent, complete base64 quanta. Decode
        // directly into the accumulator with exact reservation, avoiding both
        // an accumulated base64 copy. Capacity growth is geometric but never
        // exceeds the transfer bound (plus the final base64 quantum).
        let decoded_capacity = data.len().checked_add(3)?.checked_div(4)?.checked_mul(3)?;
        let buffered = self.pending.as_ref().map_or(0, |p| p.data.len());
        let payload_limit = if first.medium != Medium::Direct {
            4096
        } else if !first.compressed {
            raw_len.unwrap_or(MAX_IMAGE_BYTES)
        } else {
            MAX_IMAGE_BYTES
        };
        if decoded_capacity > payload_limit.saturating_add(2).saturating_sub(buffered) {
            self.cancel_pending();
            return fail(if first.compressed || first.format == WireFormat::Png {
                "ENOSPC:Transfer too large"
            } else {
                "EINVAL:Payload exceeds declared image size"
            });
        }
        let old_capacity = self.pending.as_ref().map_or(0, |p| p.data.capacity());
        let capacity = if buffered + decoded_capacity > old_capacity {
            (buffered + decoded_capacity)
                .max(old_capacity.saturating_mul(2))
                .min(payload_limit.saturating_add(3))
        } else {
            old_capacity
        };
        if self.work.is_none() {
            let retention = super::graphics_state::retention::Retention::shared();
            self.work = Some(retention.work(retention.owner()));
        }
        let transient = if capacity > old_capacity {
            capacity + old_capacity
        } else {
            capacity
        };
        if !self
            .work
            .as_mut()
            .unwrap()
            .resize(self.completed_bytes + transient)
        {
            self.cancel_pending();
            return fail("ENOSPC:Graphics transfer budget exhausted");
        }
        let pending = self.pending.get_or_insert_with(|| Pending {
            control,
            data: Vec::new(),
            encoded_bytes: 0,
        });
        let start = pending.data.len();
        let required = start + decoded_capacity;
        if required > pending.data.capacity() {
            pending.data.reserve_exact(capacity - start);
        }
        self.work
            .as_mut()
            .unwrap()
            .resize(self.completed_bytes + pending.data.capacity());
        pending.data.resize(required, 0);
        let decoded = BASE64.decode_slice(data, &mut pending.data[start..]);
        let Ok(decoded) = decoded else {
            self.cancel_pending();
            return fail("EINVAL:Invalid base64 payload");
        };
        pending.data.truncate(start + decoded);
        pending.encoded_bytes += data.len();
        if control.more {
            return None;
        }
        let retained = self.pending.as_ref().map_or(0, |p| p.data.capacity());
        let resolved = if first.medium == Medium::Direct {
            0
        } else {
            MAX_IMAGE_BYTES
        };
        if !self
            .work
            .as_mut()
            .unwrap()
            .resize(self.completed_bytes + retained + resolved)
        {
            self.cancel_pending();
            return fail("ENOSPC:Graphics transfer budget exhausted");
        }
        let Pending { control, data, .. } = self.pending.take()?;
        let event = self.finalize(control, data);
        self.completed_bytes += match &event {
            Some(Event::Command {
                image: Some(image), ..
            }) => image.data.capacity(),
            _ => 0,
        };
        self.work.as_mut().unwrap().resize(self.completed_bytes);
        event
    }

    fn finalize(&self, control: Control, data: Vec<u8>) -> Option<Event> {
        let fail = |status| response(&control, control.id, status).map(Event::Reply);
        let image = if control.medium == Medium::Direct {
            Image {
                width: control.width,
                height: control.height,
                data,
                format: control.format,
                compressed: control.compressed,
            }
        } else {
            let transfer = MediumTransfer {
                medium: control.medium,
                name: data,
                offset: control.offset,
                size: control.size,
                width: control.width,
                height: control.height,
                format: control.format,
                compressed: control.compressed,
            };
            match transfer.resolve() {
                Some(image) => image,
                None => return fail("EBADF:Failed to read image file"),
            }
        };
        Some(Event::Command {
            control,
            image: Some(image),
        })
    }
}

/// Kitty acknowledgements are correlated with all supplied identities. Anonymous
/// transmissions do not inject unsolicited bytes into the shell's input.
pub(crate) fn response(control: &Control, image_id: u32, status: &str) -> Option<Vec<u8>> {
    let success = status == "OK";
    if control.quiet >= 2 || (success && control.quiet == 1) {
        return None;
    }
    if success && control.action == Action::Delete {
        return None;
    }
    if control.id == 0 && control.number == 0 {
        return None;
    }
    let mut keys = Vec::with_capacity(3);
    if image_id != 0 || control.number == 0 {
        keys.push(format!("i={image_id}"));
    }
    if control.number != 0 {
        keys.push(format!("I={}", control.number));
    }
    if control.placement != 0 && (image_id != 0 || control.number != 0) {
        keys.push(format!("p={}", control.placement));
    }
    Some(format!("\x1b_G{};{status}\x1b\\", keys.join(",")).into_bytes())
}

/// Expand tightly packed RGB into RGBA with an opaque alpha channel.
fn rgb_to_rgba(rgb: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgb.len() / 3 * 4);
    for px in rgb.chunks_exact(3) {
        out.extend_from_slice(px);
        out.push(0xff);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(chunks: &[&[u8]]) -> Vec<Vec<u8>> {
        let mut tok = ApcTokenizer::new();
        let mut out = Vec::new();
        for c in chunks {
            tok.feed(c, |_| {}, |cmd| out.push(cmd.to_vec()));
        }
        out
    }

    /// Feed `chunks` and return the concatenated passthrough stream (input with
    /// every `_G` sequence stripped) alongside the extracted commands.
    fn split(chunks: &[&[u8]]) -> (Vec<u8>, Vec<Vec<u8>>) {
        let mut tok = ApcTokenizer::new();
        let mut pass = Vec::new();
        let mut cmds = Vec::new();
        for c in chunks {
            tok.feed(
                c,
                |b| pass.extend_from_slice(b),
                |cmd| cmds.push(cmd.to_vec()),
            );
        }
        (pass, cmds)
    }

    #[test]
    fn extracts_a_graphics_command_between_esc_underscore_and_st() {
        assert_eq!(
            collect(&[b"\x1b_Ga=T,f=32;AAAA\x1b\\"]),
            vec![b"Ga=T,f=32;AAAA".to_vec()]
        );
    }

    #[test]
    fn non_graphics_apc_is_ignored() {
        // An APC that doesn't start with `G` (e.g. some other program's APC) is
        // discarded, and a graphics command right after is still caught.
        assert_eq!(
            collect(&[b"\x1b_Xhello\x1b\\\x1b_Ga=q;AAAA\x1b\\"]),
            vec![b"Ga=q;AAAA".to_vec()]
        );
    }

    #[test]
    fn command_split_across_reads_is_reassembled() {
        assert_eq!(
            collect(&[b"\x1b_Ga=T,", b"f=32;AA", b"AA\x1b", b"\\"]),
            vec![b"Ga=T,f=32;AAAA".to_vec()]
        );
    }

    #[test]
    fn byte_at_a_time_delivery_crosses_every_state() {
        let stream = b"plain\x1b_Ga=q;AAAA\x1b\\more";
        let chunks: Vec<&[u8]> = stream.chunks(1).collect();
        assert_eq!(collect(&chunks), vec![b"Ga=q;AAAA".to_vec()]);
    }

    #[test]
    fn passthrough_is_input_with_graphics_stripped() {
        let (pass, cmds) = split(&[b"before\x1b_Ga=T;AAAA\x1b\\after"]);
        assert_eq!(pass, b"beforeafter".to_vec());
        assert_eq!(cmds, vec![b"Ga=T;AAAA".to_vec()]);
    }

    #[test]
    fn non_graphics_apc_passes_through_verbatim() {
        // A foreign APC (e.g. tmux) must reach the client's VT parser unchanged,
        // while a graphics command in the same stream is still stripped.
        let (pass, cmds) = split(&[b"\x1b_Xhello\x1b\\\x1b_Ga=q;AAAA\x1b\\end"]);
        assert_eq!(pass, b"\x1b_Xhello\x1b\\end".to_vec());
        assert_eq!(cmds, vec![b"Ga=q;AAAA".to_vec()]);
    }

    #[test]
    fn lone_esc_and_other_escapes_are_preserved_in_passthrough() {
        // A CSI sequence and a bare ESC must survive; only `_G` is removed.
        let (pass, cmds) = split(&[b"a\x1b[31mred\x1b_Ga=d\x1b\\z"]);
        assert_eq!(pass, b"a\x1b[31mredz".to_vec());
        assert_eq!(cmds, vec![b"Ga=d".to_vec()]);
    }

    #[test]
    fn passthrough_survives_being_split_mid_sequence() {
        // Feed the same stream one byte at a time; the passthrough must still be
        // exactly the input minus the graphics command.
        let stream = b"x\x1b_Ga=T;QUJD\x1b\\y\x1b_Zother\x1b\\z";
        let chunks: Vec<&[u8]> = stream.chunks(1).collect();
        let (pass, cmds) = split(&chunks);
        assert_eq!(pass, b"xy\x1b_Zother\x1b\\z".to_vec());
        assert_eq!(cmds, vec![b"Ga=T;QUJD".to_vec()]);
    }

    #[test]
    fn malformed_upload_does_not_swallow_neighboring_text_or_the_next_command() {
        let stream = b"before\x1b_Ga=t,i=7,s=1,v=1;not-base64\x1b\\after\x1b_Ga=p,i=8,p=3\x1b\\end";
        for split_at in 0..=stream.len() {
            let mut sniffer = GraphicsSniffer::new();
            let mut output = Vec::new();
            let mut events = Vec::new();
            for chunk in [&stream[..split_at], &stream[split_at..]] {
                events.extend(sniffer.feed(chunk, |bytes| output.extend_from_slice(bytes)));
            }
            assert_eq!(output, b"beforeafterend");
            assert!(
                matches!(&events[0], Event::Reply(reply) if reply.starts_with(b"\x1b_Gi=7;EINVAL:"))
            );
            assert!(matches!(&events[1], Event::Command { control, image: None }
                if control.id == 8 && control.placement == 3 && control.action == Action::Display));
        }
    }

    #[test]
    fn delete_aborts_a_partial_upload_without_consuming_its_own_selector() {
        let mut parser = GraphicsParser::new();
        assert_eq!(parser.feed(b"Ga=t,i=7,s=1,v=1,m=1;/wAA"), None);
        assert!(
            matches!(parser.feed(b"Ga=d,d=i,i=7"), Some(Event::Command { control, image: None })
            if control.action == Action::Delete && control.delete == b'i')
        );
        match parser.feed(b"Ga=t,i=8,s=1,v=1;/wAA/w==").unwrap() {
            Event::Command {
                control,
                image: Some(image),
            } => {
                assert_eq!(control.id, 8);
                assert_eq!(image.to_rgba8().unwrap(), [255, 0, 0, 255]);
            }
            other => panic!("complete next upload was lost: {other:?}"),
        }
    }

    #[test]
    fn chunked_compressed_rgb_decodes_after_arbitrary_pty_boundaries() {
        let pixels = [0x10, 0x20, 0x30, 0x40, 0x50, 0x60];
        let encoded = BASE64.encode(miniz_oxide::deflate::compress_to_vec_zlib(&pixels, 1));
        let stream = format!(
            "text\x1b_Ga=T,i=9,f=24,s=2,v=1,o=z,m=1;{}\x1b\\\x1b_Gm=0;{}\x1b\\tail",
            &encoded[..4],
            &encoded[4..]
        );
        for split_at in 0..=stream.len() {
            let mut sniffer = GraphicsSniffer::new();
            let mut text = Vec::new();
            let mut events = sniffer.feed(&stream.as_bytes()[..split_at], |bytes| {
                text.extend_from_slice(bytes)
            });
            events.extend(sniffer.feed(&stream.as_bytes()[split_at..], |bytes| {
                text.extend_from_slice(bytes)
            }));
            assert_eq!(text, b"texttail");
            match &events[0] {
                Event::Command {
                    control,
                    image: Some(image),
                } => {
                    assert_eq!(control.id, 9);
                    assert_eq!(
                        image.to_rgba8().unwrap(),
                        [0x10, 0x20, 0x30, 255, 0x40, 0x50, 0x60, 255]
                    );
                }
                other => panic!("image did not survive stream boundaries: {other:?}"),
            }
        }
    }

    #[test]
    fn interrupted_and_oversized_apcs_preserve_following_text() {
        for stream in [
            b"\x1b_Gxx\x1b[31mred".as_slice(),
            b"\x1b_foreign\x1b_Gbad\x1b[31mred",
        ] {
            let (text, _) = split(&[stream]);
            assert!(text.ends_with(b"\x1b[31mred"));
        }
        let mut oversized = b"\x1b_G".to_vec();
        oversized.resize(MAX_APC_PAYLOAD + 4, b'x');
        oversized.extend_from_slice(b"\x1b\\survives\x1b_Ga=p,i=1\x1b\\");
        let (text, commands) = split(&[&oversized]);
        assert_eq!(text, b"survives");
        assert_eq!(commands, [b"Ga=p,i=1".to_vec()]);
    }

    #[test]
    fn inflation_is_bounded_before_a_claimed_huge_image_can_allocate() {
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&vec![0; 4096], 6);
        let mut image = Image {
            width: 1,
            height: 1,
            data: compressed,
            format: WireFormat::Rgba,
            compressed: true,
        };
        assert_eq!(image.to_rgba8(), None);
        assert_eq!(image.take_rgba8(), None);
        let mut exact = Image {
            width: 1,
            height: 1,
            data: vec![1, 2, 3, 4],
            format: WireFormat::Rgba,
            compressed: false,
        };
        assert_eq!(exact.take_rgba8().unwrap(), [1, 2, 3, 4]);
        assert!(exact.data.is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn indirect_media_are_host_local_bounded_and_temp_deletion_is_scoped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tty-graphics-protocol-image.rgba");
        std::fs::write(&path, [1, 2, 3, 4]).unwrap();
        let mut transfer = MediumTransfer {
            medium: Medium::File,
            name: path.clone().into_os_string().into_encoded_bytes(),
            offset: 0,
            size: 4096,
            width: 1,
            height: 1,
            format: WireFormat::Rgba,
            compressed: false,
        };
        assert_eq!(transfer.resolve().unwrap().data, [1, 2, 3, 4]);
        assert!(path.exists());
        transfer.medium = Medium::TempFile;
        assert_eq!(transfer.resolve().unwrap().data, [1, 2, 3, 4]);
        assert!(!path.exists());
        let unmarked = dir.path().join("sender-owned.rgba");
        std::fs::write(&unmarked, [1, 2, 3, 4]).unwrap();
        transfer.name = unmarked.clone().into_os_string().into_encoded_bytes();
        assert_eq!(transfer.resolve().unwrap().data, [1, 2, 3, 4]);
        assert!(
            unmarked.exists(),
            "t=t may unlink only marked temporary paths"
        );
        assert!(!path_is_in_temp_dir(std::path::Path::new(
            "/tmp/../etc/hosts"
        )));
        assert!(!path_is_in_temp_dir(&std::env::temp_dir()));
        transfer.name = b"/dev/zero".to_vec();
        assert!(transfer.resolve().is_none());
        let fifo = dir.path().join("image.fifo");
        let fifo_name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        // A special-file path must fail before open, even with no FIFO writer.
        assert_eq!(unsafe { libc::mkfifo(fifo_name.as_ptr(), 0o600) }, 0);
        transfer.medium = Medium::File;
        transfer.name = fifo.into_os_string().into_encoded_bytes();
        assert!(transfer.resolve().is_none());

        use std::io::Write as _;
        use std::os::fd::FromRawFd as _;
        // Darwin limits POSIX shared-memory names to 31 bytes, including this prefix.
        let shm = std::ffi::CString::new(format!(
            "/lyrel-kitty-{:016x}",
            uuid::Uuid::new_v4().as_u128() as u64
        ))
        .unwrap();
        let fd = unsafe {
            libc::shm_open(
                shm.as_ptr(),
                libc::O_CREAT | libc::O_EXCL | libc::O_RDWR,
                0o600,
            )
        };
        assert!(
            fd >= 0,
            "shm_open failed: {}",
            std::io::Error::last_os_error()
        );
        let mut object = unsafe { std::fs::File::from_raw_fd(fd) };
        object.write_all(&[1, 2, 3, 4]).unwrap();
        transfer.medium = Medium::Shared;
        transfer.name = shm.as_bytes().to_vec();
        assert_eq!(
            transfer.resolve().unwrap().to_rgba8().unwrap(),
            [1, 2, 3, 4]
        );
        assert!(
            unsafe { libc::shm_open(shm.as_ptr(), libc::O_RDONLY, 0) } < 0,
            "a consumed shm handoff must be unlinked"
        );
        for name in [
            b"/../../etc/passwd".as_slice(),
            b"/sub/dir",
            b"no-leading-slash",
            b"/",
            b"/..",
        ] {
            assert!(!shm_name_is_wellformed(name));
        }
        let mut parser = GraphicsParser::new();
        assert!(
            matches!(parser.feed(b"Ga=t,i=7,t=f;L2V0Yy9ob3N0cw=="), Some(Event::Reply(reply))
            if reply.starts_with(b"\x1b_Gi=7;ENOTSUPPORTED:"))
        );
    }
}
