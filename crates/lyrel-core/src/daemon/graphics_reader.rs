//! Ordered PTY consumption with desktop admission outside the pane lock.
use super::*;
use crate::core::clipboard::{
    Event as ClipboardEvent, Segment as ClipboardSegment, Sniffed as ClipboardSniffed,
};
use crate::core::kitty_graphics::{Action, Control, Image, Segment, Sniffed, response};
use std::borrow::Cow;

enum Part {
    Graphics(Segment),
    Clipboard(ClipboardEvent),
}
enum Extracted<'a> {
    Plain(&'a [u8]),
    Parts(Vec<Part>),
}

fn extract<'a>(
    raw: &'a [u8],
    clipboard: &mut ClipboardSniffer,
    graphics: &mut GraphicsSniffer,
    locality: &mut impl FnMut() -> Option<bool>,
) -> Extracted<'a> {
    graphics.begin_read();
    match clipboard.sniff(raw) {
        ClipboardSniffed::Plain(bytes) => match graphics.sniff_part(bytes, locality) {
            Sniffed::Plain(bytes) => Extracted::Plain(bytes),
            Sniffed::Segments(segments) => {
                Extracted::Parts(segments.into_iter().map(Part::Graphics).collect())
            }
        },
        ClipboardSniffed::Segments(segments) => {
            let mut parts = Vec::new();
            for segment in segments {
                match segment {
                    ClipboardSegment::Output(bytes) => {
                        match graphics.sniff_part(&bytes, locality) {
                            Sniffed::Plain(_) => parts.push(Part::Graphics(Segment::Output(bytes))),
                            Sniffed::Segments(segments) => {
                                parts.extend(segments.into_iter().map(Part::Graphics))
                            }
                        }
                    }
                    ClipboardSegment::Event(event) => parts.push(Part::Clipboard(event)),
                }
            }
            Extracted::Parts(parts)
        }
    }
}

fn text(
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
    record_output(&mut state, bytes);
    if effects.is_empty() {
        fan_out_output(&mut state, bytes, Vec::new(), gate);
    } else {
        let mut frames = vec![GraphicsFrame::Output(bytes.to_vec())];
        append_graphics_effects(effects, &mut Vec::new(), &mut frames, writer);
        fan_out_output(&mut state, bytes, frames, gate);
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
    gate: &OutputGate,
    writer: &Arc<Mutex<Box<dyn Write + Send>>>,
) -> Vec<u8> {
    if !control.invalid && !control.unsupported && control.action == Action::Display {
        return display(state, control, gate, writer);
    }
    if !control.invalid && !control.unsupported && control.action == Action::Compose {
        image = Some(Image { width: 0, height: 0, data: Vec::new(), format: crate::core::kitty_graphics::WireFormat::Rgba, compressed: false });
    }
    if control.invalid
        || control.unsupported
        || image.is_none()
        || !matches!(
            control.action,
            Action::Transmit | Action::TransmitAndDisplay | Action::Query | Action::Frame | Action::Compose
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
            Err(status) => { reply(writer, &control, status); return Vec::new(); }
        };
        let key = if resource {
            graphics.reserve_resource_key()
        } else {
            0
        };
        (graphics.work_owner(), key, graphics.generation(), broker, frame)
    };
    let mut image = match GraphicsState::prepare_image(image, owner, key, &control, frame) {
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

pub(super) fn consume<'a>(
    raw: &'a [u8],
    clipboard: &mut ClipboardSniffer,
    graphics: &mut GraphicsSniffer,
    state: &Arc<Mutex<PaneState>>,
    gate: &OutputGate,
    writer: &Arc<Mutex<Box<dyn Write + Send>>>,
    controller: Option<u64>,
    locality: &mut impl FnMut() -> Option<bool>,
) -> Cow<'a, [u8]> {
    match extract(raw, clipboard, graphics, locality) {
        Extracted::Plain(bytes) => {
            text(state, bytes, gate, writer);
            Cow::Borrowed(bytes)
        }
        Extracted::Parts(parts) => {
            let mut pass = Vec::new();
            for part in parts {
                match part {
                    Part::Graphics(Segment::Output(bytes)) => {
                        text(state, &bytes, gate, writer);
                        pass.extend(bytes);
                    }
                    Part::Graphics(Segment::Command { control, image }) => {
                        pass.extend(command(state, control, image, gate, writer))
                    }
                    Part::Graphics(Segment::Reply(bytes))
                    | Part::Clipboard(ClipboardEvent::Reply(bytes)) => {
                        write_graphics_reply(writer, &bytes)
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
            Cow::Owned(pass)
        }
    }
}
