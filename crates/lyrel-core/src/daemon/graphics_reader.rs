//! Ordered PTY consumption with desktop admission outside the pane lock.
use super::*;
use crate::core::clipboard::{
    Event as ClipboardEvent, Segment as ClipboardSegment, Sniffed as ClipboardSniffed,
};
use crate::core::glyph_protocol::{GlyphSegment, GlyphSniffed, GlyphSniffer};
use crate::core::kitty_graphics::{Action, Control, Image, Segment, Sniffed, response};

pub(super) enum Part {
    Graphics(Segment),
    Clipboard(ClipboardEvent),
    Glyph(GlyphPart),
    Capability(Vec<u8>),
}

/// One daemon-framed Glyph Protocol body, applied in stream order.
pub(super) enum GlyphPart {
    Command {
        body: Vec<u8>,
        aborted: bool,
        exhausted: bool,
    },
}

pub(super) enum Extracted<'a> {
    Plain(&'a [u8]),
    Parts(Vec<Part>),
}

impl Extracted<'_> {
    /// The exact ordinary PTY bytes, in stream order, before any subscriber
    /// can receive them. Iterate borrowed slices instead of joining a read.
    pub(super) fn output(&self) -> impl Iterator<Item = &[u8]> {
        let (plain, parts): (Option<&[u8]>, &[Part]) = match self {
            Self::Plain(bytes) => (Some(bytes), &[]),
            Self::Parts(parts) => (None, parts),
        };
        plain
            .into_iter()
            .chain(parts.iter().filter_map(|part| match part {
                Part::Graphics(Segment::Output(bytes)) => Some(bytes.as_slice()),
                _ => None,
            }))
    }
}

pub(super) fn extract<'a>(
    raw: &'a [u8],
    clipboard: &mut ClipboardSniffer,
    glyph: &mut GlyphSniffer,
    graphics: &mut GraphicsSniffer,
    locality: &mut impl FnMut() -> Option<bool>,
    discovery: &mut crate::core::xtgettcap::Sniffer,
) -> Extracted<'a> {
    graphics.begin_read();
    use crate::core::xtgettcap::{Segment as DiscoverySegment, Sniffed as DiscoverySniffed};
    match discovery.sniff(raw) {
        DiscoverySniffed::Plain(bytes) => {
            extract_terminal(bytes, clipboard, glyph, graphics, locality)
        }
        DiscoverySniffed::Segments(segments) => {
            let mut parts = Vec::new();
            for segment in segments {
                match segment {
                    DiscoverySegment::Query(bytes) => parts.push(Part::Capability(bytes)),
                    DiscoverySegment::Output(bytes) => {
                        match extract_terminal(&bytes, clipboard, glyph, graphics, locality) {
                            Extracted::Plain(_) => {
                                parts.push(Part::Graphics(Segment::Output(bytes)))
                            }
                            Extracted::Parts(extracted) => parts.extend(extracted),
                        }
                    }
                }
            }
            Extracted::Parts(parts)
        }
    }
}

fn extract_terminal<'a>(
    raw: &'a [u8],
    clipboard: &mut ClipboardSniffer,
    glyph: &mut GlyphSniffer,
    graphics: &mut GraphicsSniffer,
    locality: &mut impl FnMut() -> Option<bool>,
) -> Extracted<'a> {
    match clipboard.sniff(raw) {
        ClipboardSniffed::Plain(bytes) => glyph_then_graphics(bytes, glyph, graphics, locality),
        ClipboardSniffed::Segments(segments) => {
            let mut parts = Vec::new();
            for segment in segments {
                match segment {
                    ClipboardSegment::Output(bytes) => {
                        append_glyph_parts(&mut parts, &bytes, glyph, graphics, locality)
                    }
                    ClipboardSegment::Event(event) => parts.push(Part::Clipboard(event)),
                }
            }
            Extracted::Parts(parts)
        }
    }
}

/// Run the glyph framing first: accepted/rejected bodies leave the stream in
/// order, and every remaining output run is fed to the kitty sniffer. The
/// glyph sniffer's fast path keeps the common no-glyph read zero-copy.
fn glyph_then_graphics<'a>(
    bytes: &'a [u8],
    glyph: &mut GlyphSniffer,
    graphics: &mut GraphicsSniffer,
    locality: &mut impl FnMut() -> Option<bool>,
) -> Extracted<'a> {
    match glyph.sniff(bytes) {
        GlyphSniffed::Plain(bytes) => match graphics.sniff_part(bytes, locality) {
            Sniffed::Plain(bytes) => Extracted::Plain(bytes),
            Sniffed::Segments(segments) => {
                Extracted::Parts(segments.into_iter().map(Part::Graphics).collect())
            }
        },
        GlyphSniffed::Segments(segments) => {
            let mut parts = Vec::new();
            for segment in segments {
                match segment {
                    GlyphSegment::Output(run) => match graphics.sniff_part(&run, locality) {
                        Sniffed::Plain(_) => parts.push(Part::Graphics(Segment::Output(run))),
                        Sniffed::Segments(segments) => {
                            parts.extend(segments.into_iter().map(Part::Graphics))
                        }
                    },
                    GlyphSegment::Command {
                        body,
                        aborted,
                        exhausted,
                    } => parts.push(Part::Glyph(GlyphPart::Command {
                        body,
                        aborted,
                        exhausted,
                    })),
                }
            }
            Extracted::Parts(parts)
        }
    }
}

/// The clipboard tokenizer owns its output segments, so this path copies the
/// runs it forwards instead of borrowing them past the loop.
fn append_glyph_parts(
    parts: &mut Vec<Part>,
    bytes: &[u8],
    glyph: &mut GlyphSniffer,
    graphics: &mut GraphicsSniffer,
    locality: &mut impl FnMut() -> Option<bool>,
) {
    match glyph.sniff(bytes) {
        GlyphSniffed::Plain(run) => match graphics.sniff_part(run, locality) {
            Sniffed::Plain(_) => parts.push(Part::Graphics(Segment::Output(bytes.to_vec()))),
            Sniffed::Segments(segments) => parts.extend(segments.into_iter().map(Part::Graphics)),
        },
        GlyphSniffed::Segments(segments) => {
            for segment in segments {
                match segment {
                    GlyphSegment::Output(run) => match graphics.sniff_part(&run, locality) {
                        Sniffed::Plain(_) => parts.push(Part::Graphics(Segment::Output(run))),
                        Sniffed::Segments(segments) => {
                            parts.extend(segments.into_iter().map(Part::Graphics))
                        }
                    },
                    GlyphSegment::Command {
                        body,
                        aborted,
                        exhausted,
                    } => parts.push(Part::Glyph(GlyphPart::Command {
                        body,
                        aborted,
                        exhausted,
                    })),
                }
            }
        }
    }
}

pub(super) fn text(
    state: &Arc<Mutex<PaneState>>,
    bytes: &[u8],
    gate: &OutputGate,
    writer: &Arc<Mutex<Box<dyn Write + Send>>>,
) {
    let mut state = state.lock().unwrap();
    let effects = state
        .graphics
        .as_mut()
        .map(|graphics| graphics.output(bytes))
        .unwrap_or_default();
    // Taken after the same parse that produced the text, and published after
    // the text frame: a query in this chunk is answered by the consumer's own
    // parse at its exact stream position, while this projection is what a
    // later attach replays once the original bytes leave the ring.
    let colors = state
        .graphics
        .as_mut()
        .and_then(|graphics| graphics.take_color_delta());
    record_output(&mut state, bytes);
    if effects.is_empty() {
        fan_out_output(&mut state, bytes, Vec::new(), gate);
    } else {
        let mut frames = vec![GraphicsFrame::Output(bytes.to_vec())];
        append_graphics_effects(effects, &mut Vec::new(), &mut frames, writer);
        fan_out_output(&mut state, bytes, frames, gate);
    }
    // Prompt marks only stage here; `finish_program_output` applies them
    // once this read has fully fanned out and the probe is safe.
    let (program_frames, program_queries) = collect_program_output(&mut state);
    publish_program_output(&mut state, program_frames);
    if let Some(frame) = colors {
        let PaneState {
            subscriptions,
            graphics,
            ..
        } = &mut *state;
        subscriptions.publish_colors(frame.encode(), gate, graphics);
    }
    let owed = detached_color_scheme_queries(&mut state);
    drop(state);
    answer_color_scheme_queries(writer, owed);
    answer_program_status_queries(writer, program_queries);
}

/// Apply validated report/reset signals to the pane's single store and turn
/// the resulting deltas into ordered frames. Prompt marks only stage here;
/// `finish_program_output` applies them once this read's bytes have fanned
/// out and the locality probe is safe to run.
fn collect_program_output(
    st: &mut PaneState,
) -> (
    Vec<crate::core::program_status::ProgramStatusFrame>,
    Vec<String>,
) {
    let signals = st
        .graphics
        .as_mut()
        .map(GraphicsState::take_program_signals)
        .unwrap_or_default();
    let queries = st
        .graphics
        .as_mut()
        .map(GraphicsState::take_program_status_queries)
        .unwrap_or_default();
    let mut frames = Vec::new();
    for signal in signals {
        let delta = match signal {
            crate::core::graphics_state::ProgramSignal::Report(report) => st.program.report(report),
            // Prompt marks stage for the caller's post-publish pass: deciding
            // whether this prompt is the local shell's can block on a process
            // probe, and that must wait until this read's bytes have fanned
            // out to subscribers.
            crate::core::graphics_state::ProgramSignal::PromptStart => {
                st.pending_prompt_signals = st.pending_prompt_signals.saturating_add(1);
                None
            }
            crate::core::graphics_state::ProgramSignal::Reset => st.program.reset(),
        };
        if let Some(delta) = delta {
            frames.push(crate::core::program_status::ProgramStatusFrame::Update(
                delta,
            ));
        }
    }
    (frames, queries)
}

/// Resolve the prompt marks a read staged while its bytes published. Called
/// only after `consume` has fanned the read out: the locality probe may block
/// on a process query, so it runs once, after subscribers already hold the
/// output. Marks the probe calls local apply to the store; anything else is
/// dropped without prompting a cleanup delta.
pub(super) fn finish_program_output(
    state: &Arc<Mutex<PaneState>>,
    prompt_local: &dyn Fn() -> bool,
) {
    let mut st = state.lock().unwrap();
    let pending = std::mem::take(&mut st.pending_prompt_signals);
    if pending == 0 {
        return;
    }
    let mut frames = Vec::new();
    if prompt_local() {
        for _ in 0..pending {
            if let Some(delta) = st.program.prompt() {
                frames.push(crate::core::program_status::ProgramStatusFrame::Update(
                    delta,
                ));
            }
        }
    }
    publish_program_output(&mut st, frames);
}

fn publish_program_output(
    st: &mut PaneState,
    frames: Vec<crate::core::program_status::ProgramStatusFrame>,
) {
    for frame in frames {
        let PaneState {
            subscriptions,
            graphics,
            ..
        } = st;
        subscriptions.publish_program_status(frame, graphics);
    }
}

fn answer_program_status_queries(writer: &Arc<Mutex<Box<dyn Write + Send>>>, queries: Vec<String>) {
    if queries.is_empty() {
        return;
    }
    if let Ok(mut writer) = writer.lock() {
        for reply in queries {
            let _ = writer.write_all(reply.as_bytes());
        }
        let _ = writer.flush();
    }
}

fn reply(writer: &Arc<Mutex<Box<dyn Write + Send>>>, control: &Control, status: &'static str) {
    if let Some(bytes) = response(control, control.id, status) {
        write_graphics_reply(writer, &bytes);
    }
}

fn command(
    state: &Arc<Mutex<PaneState>>,
    control: Control,
    mut image: Option<Image>,
    source_work: Option<crate::core::graphics_state::retention::Work>,
    gate: &OutputGate,
    writer: &Arc<Mutex<Box<dyn Write + Send>>>,
) -> Vec<u8> {
    if !control.invalid && !control.unsupported && control.action == Action::Display {
        return display(state, control, gate, writer);
    }
    if !control.invalid && !control.unsupported && control.action == Action::Compose {
        image = Some(Image {
            width: 0,
            height: 0,
            data: Vec::new(),
            format: crate::core::kitty_graphics::WireFormat::Rgba,
            compressed: false,
        });
    }
    if control.invalid
        || control.unsupported
        || image.is_none()
        || !matches!(
            control.action,
            Action::Transmit
                | Action::TransmitAndDisplay
                | Action::Query
                | Action::Frame
                | Action::Compose
        )
    {
        let mut state = state.lock().unwrap();
        let Some(graphics) = state.graphics.as_mut() else {
            return Vec::new();
        };
        let effects = graphics.apply(control, image);
        return publish_command(&mut state, effects, gate, writer);
    }
    let image = image.unwrap();
    let resource = control.action != Action::Query;
    let mut admissions = Vec::with_capacity(2);
    let (owner, key, generation, broker, frame) = {
        let mut state = state.lock().unwrap();
        if !state.alive {
            return Vec::new();
        }
        let broker = state.subscriptions.admission().clone();
        let Some(graphics) = state.graphics.as_mut() else {
            return Vec::new();
        };
        let frame = match graphics.capture_frame(&control) {
            Ok(frame) => frame,
            Err(status) => {
                reply(writer, &control, status);
                return Vec::new();
            }
        };
        let key = if resource {
            graphics.reserve_resource_key()
        } else {
            0
        };
        (
            graphics.work_owner(),
            key,
            graphics.generation(),
            broker,
            frame,
        )
    };
    let prepared = match source_work {
        Some(work) => GraphicsState::prepare_image_budgeted(image, key, &control, frame, work),
        // a=c has no incoming source; its retained packet clones keep their
        // canonical leases, and composition reserves new work for this pane.
        None => GraphicsState::prepare_image(image, owner, key, &control, frame),
    };
    let mut image = match prepared {
        Ok(image) => image,
        Err(status) => {
            reply(writer, &control, status);
            return Vec::new();
        }
    };
    let preflight = {
        let mut state = state.lock().unwrap();
        match &mut state.graphics {
            Some(graphics) if graphics.generation() == generation => {
                graphics.prepare_image_metadata(&control, &mut image)
            }
            _ => Err("EINVAL:Image transfer cancelled by session change"),
        }
    };
    if let Err(status) = preflight {
        reply(writer, &control, status);
        return Vec::new();
    }
    if resource {
        let Some(grant) = broker.request_after_release(
            None,
            key,
            image.image.width,
            image.image.height,
            image.packet.as_ref().unwrap().len(),
            crate::core::graphics_state::GRAPHICS_METADATA_BYTES,
            || {
                state
                    .lock()
                    .unwrap()
                    .graphics
                    .as_ref()
                    .map(GraphicsState::generation)
                    == Some(generation)
            },
        ) else {
            let current = state
                .lock()
                .unwrap()
                .graphics
                .as_ref()
                .map(GraphicsState::generation)
                == Some(generation);
            reply(
                writer,
                &control,
                if current {
                    "ENOSPC:Desktop graphics budget exhausted"
                } else {
                    "EINVAL:Image transfer cancelled by session change"
                },
            );
            return Vec::new();
        };
        admissions.push(grant);
        if let Some(placement_key) = image.placement_key() {
            let Some(grant) = broker.request_after_release(
                None,
                placement_key,
                0,
                0,
                0,
                crate::core::graphics_state::GRAPHICS_METADATA_BYTES,
                || {
                    state
                        .lock()
                        .unwrap()
                        .graphics
                        .as_ref()
                        .map(GraphicsState::generation)
                        == Some(generation)
                },
            ) else {
                reply(
                    writer,
                    &control,
                    "ENOSPC:Desktop graphics metadata budget exhausted",
                );
                return Vec::new();
            };
            admissions.push(grant);
        }
    }
    let expected_generation = Some(generation);
    let prepared = image;
    let mut state = state.lock().unwrap();
    if !state.alive
        || state.graphics.as_ref().map(GraphicsState::generation) != expected_generation
        || !admissions.iter().all(|grant| grant.valid())
    {
        reply(
            writer,
            &control,
            "EINVAL:Image transfer cancelled by session change",
        );
        return Vec::new();
    }
    let effects = state
        .graphics
        .as_mut()
        .unwrap()
        .apply_prepared(control, prepared);
    if effects
        .iter()
        .any(|effect| matches!(effect, GraphicsEffect::Frame(_)))
    {
        for grant in admissions {
            grant.commit();
        }
    }
    publish_command(&mut state, effects, gate, writer)
}

fn display(
    state: &Arc<Mutex<PaneState>>,
    control: Control,
    gate: &OutputGate,
    writer: &Arc<Mutex<Box<dyn Write + Send>>>,
) -> Vec<u8> {
    let (placement, generation, broker) = {
        let mut state = state.lock().unwrap();
        let broker = state.subscriptions.admission().clone();
        let Some(graphics) = state.graphics.as_mut() else {
            return Vec::new();
        };
        let placement = match graphics.prepare_placement(&control) {
            Ok(placement) => placement,
            Err(status) => {
                reply(writer, &control, status);
                return Vec::new();
            }
        };
        (placement, graphics.generation(), broker)
    };
    let grant = if placement.needs_offer {
        match broker.request_after_release(
            None,
            placement.key,
            0,
            0,
            0,
            crate::core::graphics_state::GRAPHICS_METADATA_BYTES,
            || {
                state
                    .lock()
                    .unwrap()
                    .graphics
                    .as_ref()
                    .map(GraphicsState::generation)
                    == Some(generation)
            },
        ) {
            Some(grant) => Some(grant),
            None => {
                reply(
                    writer,
                    &control,
                    "ENOSPC:Desktop graphics metadata budget exhausted",
                );
                return Vec::new();
            }
        }
    } else {
        None
    };
    let mut state = state.lock().unwrap();
    if state.graphics.as_ref().map(GraphicsState::generation) != Some(generation)
        || grant.as_ref().is_some_and(|grant| !grant.valid())
    {
        reply(
            writer,
            &control,
            "EINVAL:Placement cancelled by session change",
        );
        return Vec::new();
    }
    let effects = state
        .graphics
        .as_mut()
        .unwrap()
        .apply_prepared_placement(control, placement);
    if effects.iter().any(|effect| {
        matches!(
            effect,
            GraphicsEffect::Update(crate::core::kitty_graphics::GraphicsUpdate::Place(_))
        )
    }) && let Some(grant) = grant
    {
        grant.commit();
    }
    publish_command(&mut state, effects, gate, writer)
}

fn publish_command(
    state: &mut PaneState,
    effects: Vec<GraphicsEffect>,
    gate: &OutputGate,
    writer: &Arc<Mutex<Box<dyn Write + Send>>>,
) -> Vec<u8> {
    let mut pass = Vec::new();
    let mut frames = Vec::new();
    append_graphics_effects(effects, &mut pass, &mut frames, writer);
    record_output(state, &pass);
    fan_out_output(state, &pass, frames, gate);
    pass
}

pub(super) fn consume(
    extracted: Extracted<'_>,
    state: &Arc<Mutex<PaneState>>,
    gate: &OutputGate,
    writer: &Arc<Mutex<Box<dyn Write + Send>>>,
    controller: Option<u64>,
    graphics: &mut GraphicsSniffer,
) {
    match extracted {
        Extracted::Plain(bytes) => {
            text(state, bytes, gate, writer);
        }
        Extracted::Parts(parts) => {
            for part in parts {
                match part {
                    Part::Capability(bytes) => {
                        let program_queries = {
                            let mut state = state.lock().unwrap();
                            let effects = state
                                .graphics
                                .as_mut()
                                .map(|graphics| graphics.output(&bytes))
                                .unwrap_or_default();
                            let (program_frames, program_queries) =
                                collect_program_output(&mut state);
                            let mut frames = Vec::new();
                            append_graphics_effects(effects, &mut Vec::new(), &mut frames, writer);
                            if !frames.is_empty() {
                                fan_out_output(&mut state, &[], frames, gate);
                            }
                            publish_program_output(&mut state, program_frames);
                            program_queries
                        };
                        answer_program_status_queries(writer, program_queries);
                    }
                    Part::Graphics(Segment::Output(bytes)) => {
                        text(state, &bytes, gate, writer);
                    }
                    Part::Graphics(Segment::Command { control, image }) => {
                        let source_work = image.as_ref().map(|image| {
                            if control.iterm2 {
                                graphics.take_osc_source_work(image.data.capacity())
                            } else {
                                graphics.take_source_work(image.data.capacity())
                            }
                        });
                        command(state, control, image, source_work, gate, writer);
                    }
                    Part::Graphics(Segment::Reply(bytes))
                    | Part::Clipboard(ClipboardEvent::Reply(bytes)) => {
                        write_graphics_reply(writer, &bytes)
                    }
                    Part::Glyph(GlyphPart::Command {
                        body,
                        aborted,
                        exhausted,
                    }) => {
                        glyph_reader::consume(state, gate, writer, &body, aborted, exhausted);
                    }
                    Part::Clipboard(ClipboardEvent::Write(write)) => {
                        let mut state = state.lock().unwrap();
                        if controller.is_some()
                            && controller == state.subscriptions.controller_epoch()
                        {
                            fan_out_output(
                                &mut state,
                                &[],
                                vec![GraphicsFrame::ClipboardWrite(write.encode_frame())],
                                gate,
                            );
                        } else {
                            write_graphics_reply(
                                writer,
                                &crate::core::clipboard::response(write.id.as_deref(), "EBUSY"),
                            );
                        }
                    }
                }
            }
        }
    }
}
