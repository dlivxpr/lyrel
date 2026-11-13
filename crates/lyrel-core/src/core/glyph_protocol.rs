//! Daemon-side Glyph Protocol authority.
//!
//! The pane's PTY reader owns one [`GlyphSniffer`] and one [`GlyphSession`].
//! The sniffer frames `ESC _ 25a1 … ESC \` messages with the same
//! [`ApcTokenizer`] the kitty-graphics path uses — one state machine, two
//! lifted prefixes — and the session validates and retains accepted
//! registrations. The official `rio-vt` APC parser and `rio-graphics` glyf
//! decoder do the protocol and outline work; nothing here re-implements them.
//!
//! Accepted `r`/`c` bodies are re-emitted into the ordered output stream so a
//! rendering consumer derives its picture from the daemon's decision, not from
//! its own parse of the producer's bytes. Rejected or malformed bodies are
//! stripped. `q` is answered by the daemon from the controller's actual font
//! coverage plus the registry bit captured at the request's stream position.
//!
//! This is the first monochrome slice: `s` stays silent until the follow-on
//! layout, restoration, retained-frame and full-client gates pass, so the
//! support-negotiation path never claims more than this build renders.

use std::collections::VecDeque;

use rio_graphics::glyph::glyf_decode;
use rio_graphics::glyph::glyph_registry::{GlyphRegistry, StoredPayload};
use rio_vt::ansi::glyph_protocol::{self, GlyphCommand, GlyphPayload, RegisterError, ReplyMode};

use super::kitty_graphics::ApcTokenizer;

/// The largest complete `25a1` APC body the daemon buffers. Base64 of the
/// protocol's 64 KiB decoded limit is ~86 KiB; this leaves room for the
/// control parameters and stops a hostile sender before it allocates more.
/// A body past this is rejected as `payload_too_large`, which is always true
/// at this size (its decoded payload alone would exceed 64 KiB).
pub(crate) const MAX_APC_BODY: usize = 128 * 1024;

/// Pending frontend-bound queries per pane. Each is one codepoint and a token;
/// a controller that never answers (or a detached pane) cannot grow the queue.
pub(crate) const MAX_PENDING_QUERIES: usize = 64;

/// What the pane should do with one complete (or aborted) glyph APC body.
pub(crate) enum GlyphOutcome {
    /// Nothing to forward and nothing to answer.
    Ignore,
    /// An accepted body is forwarded to consumers at this stream position;
    /// a rejected one is stripped. `reply` is the daemon's answer to write,
    /// if the request's reply level allows one.
    Pass {
        forward: bool,
        reply: Option<Vec<u8>>,
    },
    /// A frontend-bound query: the daemon asks the controller for actual font
    /// coverage and writes the single reply itself. `glossary` is the registry
    /// bit at the request's stream position.
    Query { cp: u32, glossary: bool },
}

/// The pane-local accepted glossary. Lazily allocated: a pane that never sees
/// Glyph Protocol traffic pays nothing.
#[derive(Default)]
pub(crate) struct GlyphSession {
    registry: Option<GlyphRegistry>,
}

impl GlyphSession {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn contains(&self, cp: u32) -> bool {
        self.registry.as_ref().is_some_and(|r| r.contains(cp))
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.registry.as_ref().map_or(0, GlyphRegistry::len)
    }

    pub(crate) fn clear_all(&mut self) {
        if let Some(registry) = &self.registry {
            registry.clear_all();
        }
    }

    pub(crate) fn clear_one(&mut self, cp: u32) {
        if let Some(registry) = &self.registry {
            registry.clear_one(cp);
        }
    }

    /// Apply one framed body. `aborted` means the APC was interrupted before
    /// its terminator; `exhausted` means it passed [`MAX_APC_BODY`] and the
    /// caller kept only a bounded control prefix.
    pub(crate) fn handle(&mut self, body: &[u8], aborted: bool, exhausted: bool) -> GlyphOutcome {
        if aborted {
            return GlyphOutcome::Ignore;
        }
        if exhausted {
            return oversize_outcome(body);
        }
        match glyph_protocol::parse(body) {
            Ok(GlyphCommand::Support) => {
                // This slice does not advertise `fmt=glyf`: the follow-on
                // layout/restoration/retained/full-client gates are not met.
                GlyphOutcome::Ignore
            }
            Ok(GlyphCommand::Query { cp }) => GlyphOutcome::Query {
                cp,
                glossary: self.contains(cp),
            },
            Ok(GlyphCommand::Register {
                cp,
                payload,
                reply,
                width,
                layout,
            }) => self.register(cp, payload, reply, width, layout),
            Ok(GlyphCommand::Clear { cp }) => {
                match cp {
                    Some(cp) => self.clear_one(cp),
                    None => self.clear_all(),
                }
                GlyphOutcome::Pass {
                    forward: true,
                    reply: Some(format_clear_ok(cp)),
                }
            }
            Err(glyph_protocol::ParseError::RegisterFailed { cp, reason, reply }) => {
                let reply = reply
                    .emit_error()
                    .then(|| format_register_error(cp, reason));
                GlyphOutcome::Pass {
                    forward: false,
                    reply,
                }
            }
            Err(glyph_protocol::ParseError::ClearOutOfNamespace) => GlyphOutcome::Pass {
                forward: false,
                reply: Some(format_clear_error_out_of_namespace()),
            },
            // Malformed framing is consumed and answered with silence, per the
            // protocol's "ignore unknown/malformed APC" rule.
            Err(glyph_protocol::ParseError::Malformed(_))
            | Err(glyph_protocol::ParseError::NotGlyphProtocol) => GlyphOutcome::Ignore,
        }
    }

    fn register(
        &mut self,
        cp: u32,
        payload: GlyphPayload,
        reply: ReplyMode,
        width: u8,
        layout: rio_graphics::glyph_layout::GlyphLayout,
    ) -> GlyphOutcome {
        let GlyphPayload::Glyf { glyf, upm } = payload else {
            // Only `glyf` is accepted by this build. The wire parser already
            // bounded the colour container; rejecting it here is the
            // advertised-format rule (§3.3).
            let reply = reply
                .emit_error()
                .then(|| format_register_error(cp, RegisterError::MalformedPayload));
            return GlyphOutcome::Pass {
                forward: false,
                reply,
            };
        };
        let result = glyf_decode::decode(&glyf).map_err(translate_decode);
        let result = result.and_then(|_| {
            self.registry
                .get_or_insert_with(GlyphRegistry::new)
                .register(cp, StoredPayload::Glyf { glyf }, upm, width, layout)
                .map(|_| ())
                .map_err(|_| RegisterError::OutOfNamespace)
        });
        match result {
            Ok(()) => GlyphOutcome::Pass {
                forward: true,
                reply: reply.emit_success().then(|| format_register_ok(cp)),
            },
            Err(reason) => GlyphOutcome::Pass {
                forward: false,
                reply: reply
                    .emit_error()
                    .then(|| format_register_error(cp, reason)),
            },
        }
    }
}

fn translate_decode(error: glyf_decode::DecodeError) -> RegisterError {
    match error {
        glyf_decode::DecodeError::Composite => RegisterError::CompositeUnsupported,
        glyf_decode::DecodeError::Hinted => RegisterError::HintingUnsupported,
        glyf_decode::DecodeError::Malformed => RegisterError::MalformedPayload,
        glyf_decode::DecodeError::TooLarge => RegisterError::OutlineTooLarge,
    }
}

/// `payload_too_large` for a body whose payload never fit the buffer. The
/// caller keeps the control prefix, so `cp=`/`reply=` are still readable.
fn oversize_outcome(body: &[u8]) -> GlyphOutcome {
    if !body.starts_with(b"25a1;r;") && !body.starts_with(b"25a1;r") {
        return GlyphOutcome::Ignore;
    }
    let mut cp = None;
    let mut reply = ReplyMode::All;
    for part in body.split(|byte| *byte == b';') {
        if let Some(raw) = part.strip_prefix(b"cp=") {
            cp = parse_hex_cp(raw);
        } else if let Some(raw) = part.strip_prefix(b"reply=") {
            reply = match raw {
                b"0" => ReplyMode::None,
                b"2" => ReplyMode::ErrorsOnly,
                _ => ReplyMode::All,
            };
        }
    }
    let Some(cp) = cp else {
        return GlyphOutcome::Ignore;
    };
    GlyphOutcome::Pass {
        forward: false,
        reply: reply
            .emit_error()
            .then(|| format_register_error(cp, RegisterError::PayloadTooLarge)),
    }
}

fn parse_hex_cp(raw: &[u8]) -> Option<u32> {
    if raw.is_empty() || raw.len() > 6 {
        return None;
    }
    let mut value = 0u32;
    for byte in raw {
        value = value.checked_mul(16)?;
        value += match byte {
            b'0'..=b'9' => u32::from(byte - b'0'),
            b'a'..=b'f' => u32::from(byte - b'a') + 10,
            b'A'..=b'F' => u32::from(byte - b'A') + 10,
            _ => return None,
        };
    }
    Some(value)
}

fn reason_code(reason: RegisterError) -> &'static str {
    match reason {
        RegisterError::OutOfNamespace => "out_of_namespace",
        RegisterError::CompositeUnsupported => "composite_unsupported",
        RegisterError::HintingUnsupported => "hinting_unsupported",
        RegisterError::MalformedPayload => "malformed_payload",
        RegisterError::PayloadTooLarge => "payload_too_large",
        RegisterError::OutlineTooLarge => "outline_too_large",
    }
}

fn format_register_ok(cp: u32) -> Vec<u8> {
    format!("\x1b_25a1;r;cp={cp:x};status=0\x1b\\").into_bytes()
}

fn format_register_error(cp: u32, reason: RegisterError) -> Vec<u8> {
    format!(
        "\x1b_25a1;r;cp={cp:x};status=1;reason={}\x1b\\",
        reason_code(reason)
    )
    .into_bytes()
}

fn format_clear_ok(cp: Option<u32>) -> Vec<u8> {
    match cp {
        Some(cp) => format!("\x1b_25a1;c;cp={cp:x};status=0\x1b\\").into_bytes(),
        None => b"\x1b_25a1;c;status=0\x1b\\".to_vec(),
    }
}

fn format_clear_error_out_of_namespace() -> Vec<u8> {
    b"\x1b_25a1;c;status=1;reason=out_of_namespace\x1b\\".to_vec()
}

/// One framed glyph segment: passthrough bytes or a complete body.
pub(crate) enum GlyphSegment {
    Output(Vec<u8>),
    Command {
        body: Vec<u8>,
        aborted: bool,
        exhausted: bool,
    },
}

/// The glyph tokenizer's result: the whole chunk borrowed when no glyph APC is
/// present, otherwise ordered segments. Only the glyph-free fast path stays
/// borrowed; a chunk that actually carries an APC copies its output runs.
pub(crate) enum GlyphSniffed<'a> {
    Plain(&'a [u8]),
    Segments(Vec<GlyphSegment>),
}

/// Streams `_25a1` APCs out of raw PTY output using the shared APC framing.
pub(crate) struct GlyphSniffer {
    tokenizer: ApcTokenizer,
}

impl GlyphSniffer {
    pub(crate) fn new() -> Self {
        Self {
            tokenizer: ApcTokenizer::glyph(),
        }
    }

    pub(crate) fn sniff<'a>(&mut self, bytes: &'a [u8]) -> GlyphSniffed<'a> {
        if self.tokenizer.ground() && !might_contain_glyph(bytes) {
            return GlyphSniffed::Plain(bytes);
        }
        let segments = std::cell::RefCell::new(Vec::new());
        self.tokenizer.feed_lifted(
            bytes,
            |run| {
                if !run.is_empty() {
                    segments
                        .borrow_mut()
                        .push(GlyphSegment::Output(run.to_vec()));
                }
            },
            |body, aborted, exhausted| {
                segments.borrow_mut().push(GlyphSegment::Command {
                    body: body.to_vec(),
                    aborted,
                    exhausted,
                });
            },
        );
        GlyphSniffed::Segments(segments.into_inner())
    }
}

fn might_contain_glyph(bytes: &[u8]) -> bool {
    const PREFIX: &[u8] = b"\x1b_25a1";
    bytes.last() == Some(&0x1b)
        || memchr::memmem::find(bytes, PREFIX).is_some()
        || (1..PREFIX.len())
            .any(|len| bytes.len() >= len && bytes[bytes.len() - len..] == PREFIX[..len])
}

#[cfg(test)]
#[path = "glyph_protocol_tests.rs"]
mod tests;

/// Pending frontend-bound query state, owned by the pane.
pub(crate) struct PendingQuery {
    pub token: u64,
    pub cp: u32,
    pub epoch: u64,
    pub glossary: bool,
}

/// Bounded queue of queries awaiting a controller answer.
#[derive(Default)]
pub(crate) struct PendingQueries {
    entries: VecDeque<PendingQuery>,
}

impl PendingQueries {
    pub(crate) fn push(&mut self, query: PendingQuery) {
        if self.entries.len() == MAX_PENDING_QUERIES {
            self.entries.pop_front();
        }
        self.entries.push_back(query);
    }

    pub(crate) fn take(&mut self, token: u64, cp: u32) -> Option<PendingQuery> {
        let at = self
            .entries
            .iter()
            .position(|query| query.token == token && query.cp == cp)?;
        self.entries.remove(at)
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}
