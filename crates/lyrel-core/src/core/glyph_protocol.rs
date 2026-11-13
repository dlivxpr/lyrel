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

use std::collections::{HashSet, VecDeque};

use rio_graphics::glyph::glyf_decode;
use rio_graphics::glyph::glyph_registry::{
    GLOSSARY_CAPACITY, GlyphRegistry, StoredPayload, is_pua,
};
use rio_graphics::glyph_layout::{GlyphAlignH, GlyphAlignV, GlyphLayout, GlyphSize};
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

/// One registration's decoded `glyf` limit, the same bound the wire parser and
/// [`glyf_decode::decode`] enforce.
pub(crate) const MAX_GLYF_BYTES: usize = 64 * 1024;

const DECODED_BYTES_PER_POINT: usize = 12;
const MAX_RETAINED_WIRE_BYTES: usize = 4 * 1024 * 1024;
const MAX_RETAINED_DECODED_BYTES: usize = 8 * 1024 * 1024;
const FRAME_MAGIC: &[u8; 4] = b"GLS1";

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

/// One accepted registration as the ring-independent projection carries it.
#[derive(Debug, Clone, PartialEq)]
pub struct RetainedGlyph {
    pub cp: u32,
    pub upm: u16,
    pub width: u8,
    pub layout: GlyphLayout,
    pub glyf: Vec<u8>,
}

/// A full authoritative glossary snapshot, sent after the replayed text so an
/// older replayed mutation cannot overwrite it. The generation is the
/// session's monotonic mutation counter.
#[derive(Debug, Clone, PartialEq)]
pub struct GlyphFrame {
    generation: u64,
    glyphs: Vec<RetainedGlyph>,
}

impl GlyphFrame {
    pub fn snapshot(generation: u64, glyphs: Vec<RetainedGlyph>) -> Self {
        Self { generation, glyphs }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn glyphs(&self) -> &[RetainedGlyph] {
        &self.glyphs
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(
            16 + self
                .glyphs
                .iter()
                .map(|glyph| glyph.glyf.len() + 40)
                .sum::<usize>(),
        );
        out.extend_from_slice(FRAME_MAGIC);
        out.extend_from_slice(&self.generation.to_le_bytes());
        out.extend_from_slice(&(self.glyphs.len() as u32).to_le_bytes());
        for glyph in &self.glyphs {
            out.extend_from_slice(&glyph.cp.to_le_bytes());
            out.extend_from_slice(&glyph.upm.to_le_bytes());
            out.push(glyph.width);
            let mut flags = size_tag(glyph.layout.size) << 3;
            if glyph.layout.aw.is_some() {
                flags |= 1;
            }
            if glyph.layout.lh.is_some() {
                flags |= 2;
            }
            if glyph.layout.ymin.is_some() {
                flags |= 4;
            }
            out.push(flags);
            out.push(align_h_tag(glyph.layout.align_h) | (align_v_tag(glyph.layout.align_v) << 2));
            if let Some(aw) = glyph.layout.aw {
                out.extend_from_slice(&aw.to_le_bytes());
            }
            if let Some(lh) = glyph.layout.lh {
                out.extend_from_slice(&lh.to_le_bytes());
            }
            if let Some(ymin) = glyph.layout.ymin {
                out.extend_from_slice(&ymin.to_le_bytes());
            }
            for pad in glyph.layout.pad {
                out.extend_from_slice(&pad.to_bits().to_le_bytes());
            }
            out.extend_from_slice(&(glyph.glyf.len() as u32).to_le_bytes());
            out.extend_from_slice(&glyph.glyf);
        }
        out
    }

    pub fn decode(raw: &[u8]) -> Option<Self> {
        let mut at = 0usize;
        if frame_bytes(raw, &mut at, 4)? != FRAME_MAGIC {
            return None;
        }
        let generation = u64::from_le_bytes(frame_bytes(raw, &mut at, 8)?.try_into().ok()?);
        let count = u32::from_le_bytes(frame_bytes(raw, &mut at, 4)?.try_into().ok()?) as usize;
        if count > GLOSSARY_CAPACITY {
            return None;
        }
        let mut glyphs = Vec::with_capacity(count);
        let mut seen = HashSet::with_capacity(count);
        for _ in 0..count {
            let cp = u32::from_le_bytes(frame_bytes(raw, &mut at, 4)?.try_into().ok()?);
            if !is_pua(cp) || !seen.insert(cp) {
                return None;
            }
            let upm = u16::from_le_bytes(frame_bytes(raw, &mut at, 2)?.try_into().ok()?);
            let width = frame_bytes(raw, &mut at, 1)?[0];
            if !matches!(width, 1 | 2) {
                return None;
            }
            let flags = frame_bytes(raw, &mut at, 1)?[0];
            if flags & !0x3f != 0 {
                return None;
            }
            let align = frame_bytes(raw, &mut at, 1)?[0];
            if align & !0x1f != 0 {
                return None;
            }
            let size = size_from_tag((flags >> 3) & 0x07)?;
            let align_h = align_h_from_tag(align & 0x03)?;
            let align_v = align_v_from_tag((align >> 2) & 0x07)?;
            let aw = if flags & 1 != 0 {
                Some(u16::from_le_bytes(
                    frame_bytes(raw, &mut at, 2)?.try_into().ok()?,
                ))
            } else {
                None
            };
            let lh = if flags & 2 != 0 {
                Some(u16::from_le_bytes(
                    frame_bytes(raw, &mut at, 2)?.try_into().ok()?,
                ))
            } else {
                None
            };
            let ymin = if flags & 4 != 0 {
                Some(i16::from_le_bytes(
                    frame_bytes(raw, &mut at, 2)?.try_into().ok()?,
                ))
            } else {
                None
            };
            let mut pad = [0f32; 4];
            for slot in &mut pad {
                let value = f32::from_bits(u32::from_le_bytes(
                    frame_bytes(raw, &mut at, 4)?.try_into().ok()?,
                ));
                if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                    return None;
                }
                *slot = value;
            }
            let len = u32::from_le_bytes(frame_bytes(raw, &mut at, 4)?.try_into().ok()?) as usize;
            if len > MAX_GLYF_BYTES {
                return None;
            }
            let glyf = frame_bytes(raw, &mut at, len)?.to_vec();
            glyphs.push(RetainedGlyph {
                cp,
                upm,
                width,
                layout: GlyphLayout {
                    aw,
                    lh,
                    ymin,
                    size,
                    align_h,
                    align_v,
                    pad,
                },
                glyf,
            });
        }
        if at != raw.len() {
            return None;
        }
        Some(Self { generation, glyphs })
    }
}

fn frame_bytes<'a>(raw: &'a [u8], at: &mut usize, len: usize) -> Option<&'a [u8]> {
    let end = at.checked_add(len)?;
    let bytes = raw.get(*at..end)?;
    *at = end;
    Some(bytes)
}

fn size_tag(size: GlyphSize) -> u8 {
    match size {
        GlyphSize::Height => 0,
        GlyphSize::Advance => 1,
        GlyphSize::Contain => 2,
        GlyphSize::Cover => 3,
        GlyphSize::Stretch => 4,
    }
}

fn size_from_tag(tag: u8) -> Option<GlyphSize> {
    Some(match tag {
        0 => GlyphSize::Height,
        1 => GlyphSize::Advance,
        2 => GlyphSize::Contain,
        3 => GlyphSize::Cover,
        4 => GlyphSize::Stretch,
        _ => return None,
    })
}

fn align_h_tag(align: GlyphAlignH) -> u8 {
    match align {
        GlyphAlignH::Start => 0,
        GlyphAlignH::Center => 1,
        GlyphAlignH::End => 2,
    }
}

fn align_h_from_tag(tag: u8) -> Option<GlyphAlignH> {
    Some(match tag {
        0 => GlyphAlignH::Start,
        1 => GlyphAlignH::Center,
        2 => GlyphAlignH::End,
        _ => return None,
    })
}

fn align_v_tag(align: GlyphAlignV) -> u8 {
    match align {
        GlyphAlignV::Start => 0,
        GlyphAlignV::Center => 1,
        GlyphAlignV::End => 2,
        GlyphAlignV::Baseline => 3,
    }
}

fn align_v_from_tag(tag: u8) -> Option<GlyphAlignV> {
    Some(match tag {
        0 => GlyphAlignV::Start,
        1 => GlyphAlignV::Center,
        2 => GlyphAlignV::End,
        3 => GlyphAlignV::Baseline,
        _ => return None,
    })
}

#[derive(Clone, Copy)]
struct GlyphLimits {
    wire: usize,
    decoded: usize,
}

impl Default for GlyphLimits {
    fn default() -> Self {
        Self {
            wire: MAX_RETAINED_WIRE_BYTES,
            decoded: MAX_RETAINED_DECODED_BYTES,
        }
    }
}

/// The pane-local accepted glossary. Lazily allocated: a pane that never sees
/// Glyph Protocol traffic pays nothing.
#[derive(Default)]
pub(crate) struct GlyphSession {
    registry: Option<GlyphRegistry>,
    /// Bumps on every accepted mutation; part of the projection's staleness
    /// check.
    generation: u64,
    limits: GlyphLimits,
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
        if let Some(registry) = &self.registry
            && registry.len() > 0
        {
            registry.clear_all();
            self.generation = self.generation.wrapping_add(1);
        }
    }

    pub(crate) fn clear_one(&mut self, cp: u32) {
        if let Some(registry) = &self.registry
            && registry.contains(cp)
        {
            registry.clear_one(cp);
            self.generation = self.generation.wrapping_add(1);
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
        match self.admit(cp, &glyf, upm, width, layout) {
            Ok(()) => {
                self.generation = self.generation.wrapping_add(1);
                GlyphOutcome::Pass {
                    forward: true,
                    reply: reply.emit_success().then(|| format_register_ok(cp)),
                }
            }
            Err(reason) => GlyphOutcome::Pass {
                forward: false,
                reply: reply
                    .emit_error()
                    .then(|| format_register_error(cp, reason)),
            },
        }
    }

    /// Ordered full projection of the current glossary. The caller sends it
    /// after the replayed text; its generation lets a consumer reject a stale
    /// snapshot.
    pub(crate) fn frame(&self) -> GlyphFrame {
        let mut entries: Vec<(u64, RetainedGlyph)> = Vec::new();
        if let Some(registry) = &self.registry {
            for index in 0..GLOSSARY_CAPACITY as u16 {
                let Some(cp) = registry.cp_for_index(index) else {
                    continue;
                };
                let Some(glyph) = registry.get(cp) else {
                    continue;
                };
                let StoredPayload::Glyf { glyf } = &*glyph.payload else {
                    continue;
                };
                entries.push((
                    glyph.insertion_id,
                    RetainedGlyph {
                        cp,
                        upm: glyph.upm,
                        width: glyph.width,
                        layout: glyph.layout,
                        glyf: glyf.clone(),
                    },
                ));
            }
        }
        entries.sort_by_key(|(insertion, _)| *insertion);
        GlyphFrame::snapshot(
            self.generation,
            entries.into_iter().map(|(_, glyph)| glyph).collect(),
        )
    }

    /// Rebuild a session from its handoff projection. Any entry the decoded
    /// state could not admit rejects the whole frame rather than restoring a
    /// partial glossary the application never saw.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(crate) fn from_frame(frame: GlyphFrame) -> Option<Self> {
        let mut session = Self::new();
        for glyph in frame.glyphs() {
            session
                .admit(glyph.cp, &glyph.glyf, glyph.upm, glyph.width, glyph.layout)
                .ok()?;
        }
        session.generation = frame.generation();
        Some(session)
    }

    /// Validate against both the per-registration and whole-glossary budgets
    /// before allocating decoded points, then store the exact accepted bytes.
    fn admit(
        &mut self,
        cp: u32,
        glyf: &[u8],
        upm: u16,
        width: u8,
        layout: GlyphLayout,
    ) -> Result<(), RegisterError> {
        if glyf.len() > MAX_GLYF_BYTES {
            return Err(RegisterError::PayloadTooLarge);
        }
        let points = glyf_decode::peek_point_count(glyf).map_err(translate_decode)?;
        if points > glyf_decode::MAX_DECODED_POINTS {
            return Err(RegisterError::OutlineTooLarge);
        }
        let decoded = points.saturating_mul(DECODED_BYTES_PER_POINT);
        let (old_wire, old_decoded) = self.entry_sizes(cp);
        let (evicted_wire, evicted_decoded) = self.fifo_eviction_sizes(cp);
        let (total_wire, total_decoded) = self.totals();
        let projected_wire = total_wire
            .saturating_sub(old_wire)
            .saturating_sub(evicted_wire)
            .saturating_add(glyf.len());
        let projected_decoded = total_decoded
            .saturating_sub(old_decoded)
            .saturating_sub(evicted_decoded)
            .saturating_add(decoded);
        if projected_wire > self.limits.wire || projected_decoded > self.limits.decoded {
            return Err(RegisterError::OutlineTooLarge);
        }
        glyf_decode::decode(glyf).map_err(translate_decode)?;
        self.registry
            .get_or_insert_with(GlyphRegistry::new)
            .register(
                cp,
                StoredPayload::Glyf {
                    glyf: glyf.to_vec(),
                },
                upm,
                width,
                layout,
            )
            .map(|_| ())
            .map_err(|_| RegisterError::OutOfNamespace)
    }

    fn entry_sizes(&self, cp: u32) -> (usize, usize) {
        self.registry
            .as_ref()
            .and_then(|registry| registry.get(cp))
            .map_or((0, 0), |glyph| payload_sizes(&glyph.payload))
    }

    /// The registry evicts the oldest insertion when a new codepoint arrives at
    /// capacity. Account for that entry in the projected total so a valid
    /// FIFO admission is not refused for space the same call frees.
    fn fifo_eviction_sizes(&self, cp: u32) -> (usize, usize) {
        let Some(registry) = &self.registry else {
            return (0, 0);
        };
        if registry.contains(cp) || registry.len() < GLOSSARY_CAPACITY {
            return (0, 0);
        }
        self.oldest_sizes(registry)
    }

    fn oldest_sizes(&self, registry: &GlyphRegistry) -> (usize, usize) {
        let mut oldest: Option<(u64, (usize, usize))> = None;
        for index in 0..GLOSSARY_CAPACITY as u16 {
            let Some(cp) = registry.cp_for_index(index) else {
                continue;
            };
            let Some(glyph) = registry.get(cp) else {
                continue;
            };
            let sizes = payload_sizes(&glyph.payload);
            if oldest.is_none_or(|(insertion, _)| glyph.insertion_id < insertion) {
                oldest = Some((glyph.insertion_id, sizes));
            }
        }
        oldest.map_or((0, 0), |(_, sizes)| sizes)
    }

    fn totals(&self) -> (usize, usize) {
        let Some(registry) = &self.registry else {
            return (0, 0);
        };
        let mut wire = 0usize;
        let mut decoded = 0usize;
        for index in 0..GLOSSARY_CAPACITY as u16 {
            let Some(cp) = registry.cp_for_index(index) else {
                continue;
            };
            let Some(glyph) = registry.get(cp) else {
                continue;
            };
            let (entry_wire, entry_decoded) = payload_sizes(&glyph.payload);
            wire = wire.saturating_add(entry_wire);
            decoded = decoded.saturating_add(entry_decoded);
        }
        (wire, decoded)
    }
}

fn payload_sizes(payload: &StoredPayload) -> (usize, usize) {
    let StoredPayload::Glyf { glyf } = payload else {
        return (0, 0);
    };
    let decoded = glyf_decode::peek_point_count(glyf).map_or(MAX_GLYF_BYTES, |points| {
        points.saturating_mul(DECODED_BYTES_PER_POINT)
    });
    (glyf.len(), decoded)
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
