//! XTGETTCAP discovery: the bounded framing filter, its wire segment values,
//! and the pure response policy.
//!
//! One self-contained filter ([`Sniffer`]) strips complete, cancelled and
//! oversized discovery requests out of a raw pane stream before any
//! daemon/desktop/CLI mirror Processor sees it. Only bounded live queries
//! reach the XTGETTCAP owner, which answers them with [`response`]. The filter
//! is graphics-free: it lifts just the DCS framing the discovery check needs,
//! so `lyrel-cli` and the desktop mirror can share the daemon's exact
//! fragmentation, cancellation and oversize semantics without linking
//! terminal-image authority (or `rio-vt`, which pulls `rio-graphics`).
//!
//! The filter is used two ways. The Session Daemon sniffs every PTY read and
//! answers the queries itself; a non-owning terminal mirror (CLI capture, the
//! desktop's remote reader) instead drops the queries and keeps only the
//! `Output` segments. Both feed the same [`Sniffer`].
//!
//! The discovery buffer is bounded independently of the daemon's graphics
//! retention budget: one request holds at most [`MAX_DISCOVERY_PAYLOAD`]
//! buffered bytes (with [`MAX_DCS_HEADER`] held before classification), and a
//! dropped request drains without buffering. That bound used to be charged to
//! the shared graphics work budget only because the filter shared the
//! graphics APC tokenizer's growth path; the framing it keeps is identical,
//! and bounding discovery by its own fixed cap keeps the graphics budget
//! reserved for pixel payloads.

use memchr::{memchr, memchr3, memmem};

/// One discovery request's buffered ceiling: the DCS header and the encoded
/// capability names together may not exceed this many bytes. A request that
/// crosses it is drained (with its terminator) and reported cancelled.
const MAX_DISCOVERY_PAYLOAD: usize = 8192;

/// Ceiling on a DCS header whose final byte has not classified it yet. A
/// header that crosses it without dispatching is drained, exactly like an
/// oversized payload.
const MAX_DCS_HEADER: usize = 128;

/// Rio's parser (the classification authority this filter mirrors) tracks at
/// most four DCS intermediates; a fifth sets its ignore flag without extending
/// the list, and `hook` still sees the same prefix. Keep the same cut so the
/// discovery decision cannot diverge from the parser the owner answers with.
const MAX_INTERMEDIATES: usize = 4;

/// Rio has already decoded the names and generated one DCS segment per name.
/// Never inherit its values, product identity or capability table: decide by
/// decoded name and generate only values whose full product path is verified.
pub fn response(generated: &str) -> Vec<u8> {
    let mut reply = Vec::with_capacity(generated.len());
    for segment in generated.split_terminator("\x1b\\") {
        let Some(fields) = segment
            .strip_prefix("\x1bP1+r")
            .or_else(|| segment.strip_prefix("\x1bP0+r"))
        else {
            continue;
        };
        let name = fields.split('=').next().unwrap_or_default();
        let mut decoded = [0; 6];
        let bytes = name.as_bytes();
        let valid = bytes.len().is_multiple_of(2)
            && bytes.len() <= decoded.len() * 2
            && bytes
                .chunks_exact(2)
                .zip(decoded.iter_mut())
                .all(|(pair, slot)| {
                    let digit = |byte: u8| match byte {
                        b'0'..=b'9' => Some(byte - b'0'),
                        b'a'..=b'f' => Some(byte - b'a' + 10),
                        b'A'..=b'F' => Some(byte - b'A' + 10),
                        _ => None,
                    };
                    match (digit(pair[0]), digit(pair[1])) {
                        (Some(hi), Some(lo)) => {
                            *slot = hi * 16 + lo;
                            true
                        }
                        _ => false,
                    }
                });
        let value: Option<&[u8]> = if valid {
            match &decoded[..bytes.len() / 2] {
                b"Co" | b"colors" => Some(b"323536"),
                b"RGB" => Some(b"382F382F38"),
                _ => None,
            }
        } else {
            None
        };
        reply.extend_from_slice(if value.is_some() {
            b"\x1bP1+r"
        } else {
            b"\x1bP0+r"
        });
        reply.extend_from_slice(name.as_bytes());
        if let Some(value) = value {
            reply.push(b'=');
            reply.extend_from_slice(value);
        }
        reply.extend_from_slice(b"\x1b\\");
    }
    reply
}

/// One framed piece of the sniffed stream: plain terminal output to forward,
/// or a complete discovery request the XTGETTCAP owner answers.
pub enum Segment {
    Output(Vec<u8>),
    Query(Vec<u8>),
}

pub enum Sniffed<'a> {
    Plain(&'a [u8]),
    Segments(Vec<Segment>),
}

/// Strips complete, cancelled and oversized XTGETTCAP discovery requests
/// before *any* daemon/desktop/CLI Processor. Only bounded live queries reach
/// the owner. Framing survives PTY read boundaries, including split openers,
/// terminators, cancellation and a later escape interrupting an unfinished
/// request; passthrough keeps every other byte verbatim.
#[derive(Default)]
pub struct Sniffer {
    /// Header bytes of the candidate request, then its names while it stays
    /// within [`MAX_DISCOVERY_PAYLOAD`].
    buf: Vec<u8>,
    state: State,
    header: Header,
}

#[derive(Default, Clone, Copy, PartialEq)]
enum State {
    /// Not inside an escape sequence; bytes are passthrough.
    #[default]
    Ground,
    /// Held one `ESC`; a following `P` opens a discovery candidate.
    Esc,
    /// Classifying the DCS header between `ESC P` and its final byte.
    DcsStart,
    /// Buffering a classified discovery request (stripped from passthrough).
    Payload,
    /// Saw `ESC` inside a discovery request — a following `\` completes it.
    PayloadEsc,
    /// Draining an abandoned/oversized request (emitted nowhere).
    Drop,
    /// Saw `ESC` inside a drained request — a following `\` is its terminator.
    DropEsc,
    /// Forwarding a non-discovery DCS verbatim.
    Pass,
    /// Saw `ESC` inside a passthrough DCS — a following `\` is its terminator.
    PassEsc,
}

impl Sniffer {
    /// Frame one chunk of raw output. The common no-discovery chunk returns
    /// borrowed as [`Sniffed::Plain`] without allocating; a chunk that holds
    /// any `\x1bP` opener (or ends while holding an `ESC`) takes the slow path
    /// and returns ordered [`Segment`]s.
    pub fn sniff<'a>(&mut self, bytes: &'a [u8]) -> Sniffed<'a> {
        if self.at_boundary()
            && bytes.last() != Some(&0x1b)
            && memmem::find(bytes, b"\x1bP").is_none()
        {
            return Sniffed::Plain(bytes);
        }
        let mut segments = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            match self.state {
                State::Ground => match memchr(0x1b, &bytes[i..]) {
                    Some(off) => {
                        push_output(&mut segments, &bytes[i..i + off]);
                        self.state = State::Esc;
                        i += off + 1;
                    }
                    None => {
                        push_output(&mut segments, &bytes[i..]);
                        return Sniffed::Segments(segments);
                    }
                },
                State::Esc => match bytes[i] {
                    b'P' => {
                        self.begin_command();
                        i += 1;
                    }
                    // A run of ESCs: the earlier one was a lone ESC
                    // (passthrough); keep the newest one held.
                    0x1b => {
                        push_output(&mut segments, b"\x1b");
                        i += 1;
                    }
                    // The ESC began some other escape: emit it and re-examine
                    // this byte from ground (it is not an ESC, so it joins the
                    // run).
                    _ => {
                        push_output(&mut segments, b"\x1b");
                        self.state = State::Ground;
                    }
                },
                State::DcsStart => {
                    let byte = bytes[i];
                    if matches!(byte, 0x18 | 0x1a | 0x9c) {
                        // CAN, SUB and a bare ST abandon the candidate before
                        // it classifies; the cancellation reaches the terminal.
                        self.buf.clear();
                        push_output(&mut segments, b"\x18");
                        self.state = State::Ground;
                        i += 1;
                    } else if byte == 0x1b {
                        self.buf.clear();
                        push_output(&mut segments, b"\x18");
                        self.state = State::Esc;
                        i += 1;
                    } else {
                        self.header.advance(byte);
                        if byte < 0x20 || byte == 0x7f {
                            // Forward an executed C0 once, not again in the
                            // owner's query Processor.
                            push_output(&mut segments, &bytes[i..i + 1]);
                            i += 1;
                        } else {
                            self.buf.push(byte);
                            i += 1;
                            match self.header.query {
                                Some(true) => self.state = State::Payload,
                                Some(false) => {
                                    // Not discovery: the whole DCS belongs to
                                    // the terminal, opener included.
                                    let buffered = std::mem::take(&mut self.buf);
                                    push_output(&mut segments, b"\x1bP");
                                    push_output(&mut segments, &buffered);
                                    self.state = State::Pass;
                                }
                                None => {
                                    if self.buf.len() >= MAX_DCS_HEADER {
                                        push_output(&mut segments, b"\x18");
                                        self.buf.clear();
                                        self.state = State::Drop;
                                    }
                                }
                            }
                        }
                    }
                }
                State::Payload => match boundary(&bytes[i..]) {
                    Some(off) => {
                        let run = &bytes[i..i + off];
                        if run.len() <= MAX_DISCOVERY_PAYLOAD.saturating_sub(self.buf.len()) {
                            self.buf.extend_from_slice(run);
                            match bytes[i + off] {
                                0x1b => self.state = State::PayloadEsc,
                                // A bare ST completes the request.
                                0x9c => {
                                    self.finish(&mut segments);
                                    self.state = State::Ground;
                                }
                                // CAN or SUB cancels it.
                                _ => {
                                    self.cancel(&mut segments);
                                    self.state = State::Ground;
                                }
                            }
                            i += off + 1;
                        } else {
                            log::debug!(
                                "dropping a discovery request whose payload passed {MAX_DISCOVERY_PAYLOAD} bytes"
                            );
                            self.cancel(&mut segments);
                            // Oversized: abandon and let the drop state consume
                            // the terminator we just found (leave `i` on it).
                            self.state = State::Drop;
                            i += off;
                        }
                    }
                    None => {
                        let run = &bytes[i..];
                        if run.len() <= MAX_DISCOVERY_PAYLOAD.saturating_sub(self.buf.len()) {
                            self.buf.extend_from_slice(run);
                        } else {
                            log::debug!(
                                "dropping a discovery request whose payload passed {MAX_DISCOVERY_PAYLOAD} bytes"
                            );
                            self.cancel(&mut segments);
                            self.state = State::Drop;
                        }
                        return Sniffed::Segments(segments);
                    }
                },
                State::PayloadEsc => match bytes[i] {
                    b'\\' => {
                        self.finish(&mut segments);
                        self.state = State::Ground;
                        i += 1;
                    }
                    // A second ESC cancels the would-be terminator; the escape
                    // itself is re-examined.
                    0x1b => {
                        self.cancel(&mut segments);
                        self.state = State::Esc;
                        i += 1;
                    }
                    // A new `ESC P` re-opens: the next byte decides discovery
                    // again.
                    b'P' => {
                        self.cancel(&mut segments);
                        self.begin_command();
                        i += 1;
                    }
                    // The escape interrupted the request: its bytes stay
                    // stripped, but the escape itself belongs to the terminal,
                    // so forward it and re-examine this byte from ground.
                    _ => {
                        self.cancel(&mut segments);
                        push_output(&mut segments, b"\x1b");
                        self.state = State::Ground;
                    }
                },
                State::Drop => match boundary(&bytes[i..]) {
                    Some(off) => {
                        self.state = if bytes[i + off] == 0x1b {
                            State::DropEsc
                        } else {
                            State::Ground
                        };
                        i += off + 1;
                    }
                    None => return Sniffed::Segments(segments),
                },
                State::DropEsc => match bytes[i] {
                    b'\\' => {
                        self.state = State::Ground;
                        i += 1;
                    }
                    0x1b => i += 1,
                    b'P' => {
                        self.begin_command();
                        i += 1;
                    }
                    // As in `PayloadEsc`: the dropped request's bytes stay
                    // stripped, but the escape that interrupted it has to reach
                    // the terminal intact.
                    _ => {
                        push_output(&mut segments, b"\x1b");
                        self.state = State::Ground;
                    }
                },
                State::Pass => match boundary(&bytes[i..]) {
                    Some(off) => {
                        push_output(&mut segments, &bytes[i..i + off]);
                        if bytes[i + off] == 0x1b {
                            self.state = State::PassEsc;
                        } else {
                            // CAN, SUB or a bare ST ends the passthrough DCS;
                            // the terminator is part of its bytes.
                            push_output(&mut segments, &bytes[i + off..i + off + 1]);
                            self.state = State::Ground;
                        }
                        i += off + 1;
                    }
                    None => {
                        push_output(&mut segments, &bytes[i..]);
                        return Sniffed::Segments(segments);
                    }
                },
                State::PassEsc => match bytes[i] {
                    b'\\' => {
                        push_output(&mut segments, b"\x1b\\");
                        self.state = State::Ground;
                        i += 1;
                    }
                    0x1b => {
                        push_output(&mut segments, b"\x1b");
                        i += 1;
                    }
                    // A new opener aborts the passthrough DCS; report the
                    // cancellation, then classify the fresh candidate.
                    b'P' => {
                        push_output(&mut segments, b"\x18");
                        self.begin_command();
                        i += 1;
                    }
                    _ => {
                        push_output(&mut segments, b"\x1b");
                        self.state = State::Ground;
                    }
                },
            }
        }
        Sniffed::Segments(segments)
    }

    fn at_boundary(&self) -> bool {
        matches!(self.state, State::Ground)
    }

    /// A new candidate request after `ESC P`: clear the buffer and re-arm the
    /// header classifier.
    fn begin_command(&mut self) {
        self.buf.clear();
        self.header = Header::default();
        self.state = State::DcsStart;
    }

    /// The buffered request terminated whole: report the cancellation that
    /// ends any preceding control string, then hand the owner the query.
    fn finish(&mut self, segments: &mut Vec<Segment>) {
        segments.push(Segment::Output(vec![0x18]));
        let mut query = Vec::with_capacity(self.buf.len() + 4);
        query.extend_from_slice(b"\x1bP");
        query.extend_from_slice(&self.buf);
        query.extend_from_slice(b"\x1b\\");
        segments.push(Segment::Query(query));
        self.buf.clear();
    }

    /// The buffered request terminated cancelled or oversized: only the
    /// cancellation marker reaches the terminal, never partial query bytes.
    fn cancel(&mut self, segments: &mut Vec<Segment>) {
        segments.push(Segment::Output(vec![0x18]));
        self.buf.clear();
    }
}

/// Discovery framing's terminator scan: `ESC`, CAN, SUB or a bare ST ends a
/// control string; the caller decides what each terminator means.
fn boundary(bytes: &[u8]) -> Option<usize> {
    match (memchr3(0x1b, 0x18, 0x1a, bytes), memchr(0x9c, bytes)) {
        (Some(control), Some(st)) => Some(control.min(st)),
        (control, st) => control.or(st),
    }
}

/// Append a passthrough run, coalescing with a preceding output segment.
fn push_output(segments: &mut Vec<Segment>, run: &[u8]) {
    if let Some(Segment::Output(bytes)) = segments.last_mut() {
        bytes.extend_from_slice(run);
    } else if !run.is_empty() {
        segments.push(Segment::Output(run.to_vec()));
    }
}

/// The DCS header classification, mirroring the discovery decision the live
/// terminal's `rio-vt` parser makes: a request is discovery when its DCS
/// carries the `+` intermediate and a `q` final. Parameter bytes never affect
/// the decision; a header byte outside the parameter/intermediate/final sets
/// (C0, DEL and high bytes) is left for the terminal and never classifies.
#[derive(Default)]
struct Header {
    state: HeaderState,
    intermediates: [u8; MAX_INTERMEDIATES],
    len: usize,
    query: Option<bool>,
}

#[derive(Default, Clone, Copy)]
enum HeaderState {
    #[default]
    Entry,
    Param,
    Intermediate,
    /// The parser gave up on this header; no final byte can classify it.
    Ignore,
}

impl Header {
    fn advance(&mut self, byte: u8) {
        if self.query.is_some() {
            return;
        }
        match self.state {
            HeaderState::Ignore => {}
            HeaderState::Entry => match byte {
                0x00..=0x17 | 0x19 | 0x1c..=0x1f => {}
                0x20..=0x2f => self.collect(byte, HeaderState::Intermediate),
                0x30..=0x3b => self.state = HeaderState::Param,
                0x3c..=0x3f => self.collect(byte, HeaderState::Param),
                0x40..=0x7e => self.hook(byte),
                _ => {}
            },
            HeaderState::Param => match byte {
                0x00..=0x17 | 0x19 | 0x1c..=0x1f => {}
                0x20..=0x2f => self.collect(byte, HeaderState::Intermediate),
                0x30..=0x3b => {}
                0x3c..=0x3f => self.state = HeaderState::Ignore,
                0x40..=0x7e => self.hook(byte),
                _ => {}
            },
            HeaderState::Intermediate => match byte {
                0x00..=0x17 | 0x19 | 0x1c..=0x1f => {}
                0x20..=0x2f => self.collect(byte, HeaderState::Intermediate),
                0x30..=0x3f => self.state = HeaderState::Ignore,
                0x40..=0x7e => self.hook(byte),
                _ => {}
            },
        }
    }

    fn collect(&mut self, byte: u8, next: HeaderState) {
        if self.len < MAX_INTERMEDIATES {
            self.intermediates[self.len] = byte;
            self.len += 1;
        }
        self.state = next;
    }

    /// Rio fires `hook` even when its ignore flag is set, and the discovery
    /// check reads only the intermediates and the final byte.
    fn hook(&mut self, action: u8) {
        self.query = Some(&self.intermediates[..self.len] == b"+" && action == b'q');
    }
}

#[cfg(test)]
mod tests {
    use super::{Segment, Sniffed, Sniffer};

    /// Concatenate the plain output and the query bytes a chunked stream
    /// frames into, so tests can assert both sides of the contract.
    fn consume(chunks: impl IntoIterator<Item = Vec<u8>>) -> (Vec<u8>, Vec<u8>) {
        let mut sniffer = Sniffer::default();
        let mut plain = Vec::new();
        let mut queries = Vec::new();
        for chunk in chunks {
            let segments = match sniffer.sniff(&chunk) {
                Sniffed::Plain(bytes) => vec![Segment::Output(bytes.to_vec())],
                Sniffed::Segments(segments) => segments,
            };
            for segment in segments {
                match segment {
                    Segment::Output(bytes) => plain.extend_from_slice(&bytes),
                    Segment::Query(bytes) => queries.extend_from_slice(&bytes),
                }
            }
        }
        (plain, queries)
    }

    #[test]
    fn discovery_request_is_framed_whole_for_the_owner() {
        let (plain, queries) = consume([b"before\x1bP+q436f;524742\x1b\\after".to_vec()]);
        assert_eq!(plain, b"before\x18after");
        assert_eq!(queries, b"\x1bP+q436f;524742\x1b\\");
    }

    #[test]
    fn fragmentation_cancellation_and_next_text_share_one_contract() {
        let stream = b"before\x1bP+q436f\x18after_CAN\x1bP+q524742\x1aafter_SUB\x1bP+q436f\x1b[0mafter_ESC\x1bP0+q636f6c6f7273;544e\x1b\\after_query";
        for split in 0..=stream.len() {
            let (plain, queries) = consume([stream[..split].to_vec(), stream[split..].to_vec()]);
            assert_eq!(queries, b"\x1bP0+q636f6c6f7273;544e\x1b\\", "split {split}");
            assert_eq!(
                plain, b"before\x18after_CAN\x18after_SUB\x18\x1b[0mafter_ESC\x18after_query",
                "split {split}"
            );
        }
        let (plain, queries) = consume(stream.iter().map(|byte| vec![*byte]));
        assert_eq!(queries, b"\x1bP0+q636f6c6f7273;544e\x1b\\");
        assert_eq!(
            plain,
            b"before\x18after_CAN\x18after_SUB\x18\x1b[0mafter_ESC\x18after_query"
        );
        let (plain, queries) = consume(b"\x1bP\n+q436f\x1b\\".iter().map(|byte| vec![*byte]));
        assert_eq!(plain, b"\n\x18");
        assert_eq!(queries, b"\x1bP+q436f\x1b\\");
        let (plain, queries) = consume(b"\x1bP+q436f\x1b\x1b\\".iter().map(|byte| vec![*byte]));
        assert!(
            queries.is_empty(),
            "a second ESC cancels the first terminator"
        );
        assert_eq!(plain, b"\x18\x1b\\");
    }

    #[test]
    fn limit_applies_before_every_consumer_and_recovers() {
        let stream = format!(
            "\x1bP+q{}\x1b\\after_oversize\x1bP+q436f\x1b\\",
            "436f;".repeat(2000)
        );
        for size in [1, 113, 8192, 65536] {
            let (plain, queries) = consume(stream.as_bytes().chunks(size).map(<[u8]>::to_vec));
            assert_eq!(plain, b"\x18after_oversize\x18", "chunk {size}");
            assert_eq!(queries, b"\x1bP+q436f\x1b\\");
        }
        let at_limit = format!("\x1bP+q436f{}\x1b\\", ";".repeat(8186));
        let (plain, queries) = consume([at_limit.clone().into_bytes()]);
        assert_eq!(plain, b"\x18");
        assert_eq!(queries, at_limit.into_bytes());
        let past_limit = format!("\x1bP+q436f{}\x1b\\ordinary", ";".repeat(8187));
        let (plain, queries) = consume([past_limit.into_bytes()]);
        assert_eq!(plain, b"\x18ordinary");
        assert!(queries.is_empty());
        let header_overflow = format!("\x1bP{}+q436f\x1b\\ordinary", "0".repeat(128));
        let (plain, queries) = consume([header_overflow.into_bytes()]);
        assert_eq!(plain, b"\x18ordinary");
        assert!(queries.is_empty());
        let non_discovery = b"\x1bP=1s\x1b\\ordinary\x1bP1;2qabc\x1b\\";
        let (plain, _) = consume(non_discovery.iter().map(|byte| vec![*byte]));
        assert_eq!(
            plain, non_discovery,
            "other DCS protocols are not discovery"
        );
    }

    #[test]
    fn response_verifies_names_before_answering() {
        assert_eq!(
            super::response(
                "\x1bP1+r436F\x1b\\\x1bP1+r524742\x1b\\\x1bP0+r544e\x1b\\\x1bP1+r756e6b6e6f776e\x1b\\"
            ),
            b"\x1bP1+r436F=323536\x1b\\\x1bP1+r524742=382F382F38\x1b\\\x1bP0+r544e\x1b\\\x1bP0+r756e6b6e6f776e\x1b\\"
        );
    }
}
