//! Pane-owned graphics resources and placement updates. Pixels are sent once per
//! resource version; placements reference that version without copying pixels.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::kitty_graphics::{Image, WireFormat};

/// One derived display of a resource. Keys are pane-local and never reused.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Placement {
    pub key: u64,
    pub resource: u64,
    pub image_id: u32,
    pub placement_id: u32,
    pub cols: u32,
    pub rows: u32,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub offset_x: u32,
    pub offset_y: u32,
    pub z: i32,
    /// Stable text row, including rows evicted from the retained history ring.
    pub anchor_row: i64,
    pub anchor_col: u32,
    /// Main screen is zero; alternate screen is one and cleared on entry.
    pub screen: u64,
    pub span_cols: u32,
    pub span_rows: u32,
    /// Retained vertical slice, in cell rows relative to the original anchor.
    pub clip_top: u32,
    pub clip_bottom: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphicsUpdate {
    Resource {
        key: u64,
        image: Arc<Image>,
    },
    Place(Placement),
    Screen { screen: u64, history_origin: i64 },
    Remove {
        placements: Vec<u64>,
        resources: Vec<u64>,
    },
}

#[derive(Serialize, Deserialize)]
enum Metadata {
    Resource {
        key: u64,
        width: u32,
        height: u32,
        format: WireFormat,
        compressed: bool,
    },
    Place(Placement),
    Screen { screen: u64, history_origin: i64 },
    Remove {
        placements: Vec<u64>,
        resources: Vec<u64>,
    },
}

impl GraphicsUpdate {
    /// A length-prefixed, small JSON header followed by unexpanded resource
    /// bytes. In particular pixels are never serialized as JSON/base64.
    pub fn encode_frame(&self) -> Vec<u8> {
        let (metadata, pixels): (Metadata, &[u8]) = match self {
            Self::Resource { key, image } => (
                Metadata::Resource {
                    key: *key,
                    width: image.width,
                    height: image.height,
                    format: image.format,
                    compressed: image.compressed,
                },
                &image.data,
            ),
            Self::Place(placement) => (Metadata::Place(placement.clone()), &[]),
            Self::Screen { screen, history_origin } => (Metadata::Screen {
                screen: *screen, history_origin: *history_origin,
            }, &[]),
            Self::Remove {
                placements,
                resources,
            } => (
                Metadata::Remove {
                    placements: placements.clone(),
                    resources: resources.clone(),
                },
                &[],
            ),
        };
        let header = serde_json::to_vec(&metadata).expect("serializable graphics metadata");
        let mut frame = Vec::with_capacity(4 + header.len() + pixels.len());
        frame.extend_from_slice(&(header.len() as u32).to_le_bytes());
        frame.extend_from_slice(&header);
        frame.extend_from_slice(pixels);
        frame
    }

    pub fn decode_frame_owned(mut frame: Vec<u8>) -> Option<Self> {
        let header_len = u32::from_le_bytes(frame.get(..4)?.try_into().ok()?) as usize;
        if header_len > 1 << 20 {
            return None;
        }
        let end = 4usize.checked_add(header_len)?;
        let metadata: Metadata = serde_json::from_slice(frame.get(4..end)?).ok()?;
        match metadata {
            Metadata::Resource {
                key,
                width,
                height,
                format,
                compressed,
            } => {
                frame.drain(..end);
                Some(Self::Resource {
                    key,
                    image: Arc::new(Image {
                        width,
                        height,
                        format,
                        compressed,
                        data: frame,
                    }),
                })
            }
            Metadata::Place(placement) if frame.len() == end => Some(Self::Place(placement)),
            Metadata::Screen { screen, history_origin } if frame.len() == end =>
                Some(Self::Screen { screen, history_origin }),
            Metadata::Remove {
                placements,
                resources,
            } if frame.len() == end => Some(Self::Remove {
                placements,
                resources,
            }),
            _ => None,
        }
    }
}

use std::collections::{BTreeMap, BTreeSet};

use rio_vt::ansi::CursorShape;
use rio_vt::crosswords::grid::Dimensions as _;
use rio_vt::crosswords::{Crosswords, CrosswordsSize};
use rio_vt::event::{VoidListener, WindowId};
use rio_vt::performer::handler::Processor;

use super::kitty_graphics::{Action, Control, MAX_IMAGE_BYTES, response};
use crate::daemon::protocol::WinSize;

#[path = "graphics_terminal.rs"]
mod terminal;

pub const MAX_PANE_ENCODED_BYTES: usize = 128 << 20;
pub const MAX_PANE_DECODED_BYTES: usize = 256 << 20;
pub const MAX_IMAGE_DIMENSION: u32 = 8192;
pub const MAX_RESOURCES: usize = 1024;
pub const MAX_PLACEMENTS: usize = 4096;

pub enum GraphicsEffect {
    Update(GraphicsUpdate),
    Reply(Vec<u8>),
    Output(Vec<u8>),
}

struct Resource {
    image: Arc<Image>,
    id: u32,
    number: u32,
    decoded_bytes: usize,
    screen: u64,
}

struct AnchoredPlacement {
    placement: Placement,
    // Geometry is carried by Placement; no client-captured second anchor.
}

/// The living pane owns this state. Desktop caches receive ordered mutations,
/// never decide protocol identities or resource/placement deletion lifetimes.
pub struct GraphicsState {
    resources: BTreeMap<u64, Resource>,
    placements: BTreeMap<u64, AnchoredPlacement>,
    next_key: u64,
    next_image_id: u32,
    encoded_bytes: usize,
    decoded_bytes: usize,
    size: WinSize,
    term: Crosswords<VoidListener>,
    processor: Processor,
    screen: u64,
    region: std::ops::Range<i32>,
    dirty: BTreeSet<u64>,
    removed: Vec<u64>,
    freed: Vec<u64>,
    replies: Vec<Vec<u8>>,
}

impl GraphicsState {
    pub fn new(size: WinSize) -> Self {
        Self {
            resources: BTreeMap::new(),
            placements: BTreeMap::new(),
            next_key: 0,
            next_image_id: 0,
            encoded_bytes: 0,
            decoded_bytes: 0,
            size,
            term: Crosswords::new(
                CrosswordsSize::new(usize::from(size.cols.max(1)), usize::from(size.rows.max(1))),
                CursorShape::Block,
                VoidListener,
                WindowId::from(0),
                0,
                10_000,
            ),
            processor: Processor::default(),
            screen: 0,
            region: 0..i32::from(size.rows.max(1)),
            dirty: BTreeSet::new(),
            removed: Vec::new(),
            freed: Vec::new(),
            replies: Vec::new(),
        }
    }

    pub fn resize(&mut self, size: WinSize) -> Vec<GraphicsEffect> {
        self.resize_layout(size);
        self.finish_layout(!self.placements.is_empty())
    }

    pub fn output(&mut self, bytes: &[u8]) -> Vec<GraphicsEffect> {
        let old = (self.screen, self.history_origin());
        let mut processor = std::mem::take(&mut self.processor);
        processor.advance(self, bytes);
        self.processor = processor;
        self.expire_history();
        self.finish_layout(!self.placements.is_empty() && old != (self.screen, self.history_origin()))
    }

    fn key(&mut self) -> u64 {
        self.next_key = self
            .next_key
            .checked_add(1)
            .expect("graphics identity exhausted");
        self.next_key
    }

    fn lookup(&self, control: &Control) -> Option<u64> {
        self.resources.iter().rev().find_map(|(key, resource)| {
            (resource.screen == self.screen
                && ((control.id != 0 && resource.id == control.id)
                    || (control.number != 0 && resource.number == control.number)))
                .then_some(*key)
        })
    }

    fn reply(effects: &mut Vec<GraphicsEffect>, control: &Control, id: u32, status: &str) {
        if let Some(reply) = response(control, id, status) {
            effects.push(GraphicsEffect::Reply(reply));
        }
    }

    pub fn apply(&mut self, control: Control, image: Option<Image>) -> Vec<GraphicsEffect> {
        let mut effects = Vec::new();
        if control.invalid || control.unsupported || control.action == Action::Other {
            let status = if control.invalid {
                "EINVAL:Invalid control data"
            } else {
                "ENOTSUPPORTED:Unsupported operation"
            };
            Self::reply(&mut effects, &control, control.id, status);
            return effects;
        }
        match control.action {
            Action::Transmit | Action::TransmitAndDisplay | Action::Query => {
                let Some(mut image) = image else {
                    Self::reply(
                        &mut effects,
                        &control,
                        control.id,
                        "EINVAL:Missing image data",
                    );
                    return effects;
                };
                let decoded_bytes = match validate_image(&mut image) {
                    Ok(bytes) => bytes,
                    Err(status) => {
                        Self::reply(&mut effects, &control, control.id, status);
                        return effects;
                    }
                };
                if control.action == Action::Query {
                    Self::reply(&mut effects, &control, control.id, "OK");
                    return effects;
                }
                // Validate a=T's display before replacing any accepted resource.
                if control.action == Action::TransmitAndDisplay
                    && self.geometry(&control, &image).is_none()
                {
                    Self::reply(
                        &mut effects,
                        &control,
                        control.id,
                        "EINVAL:Invalid placement rectangle",
                    );
                    return effects;
                }
                let replaced = if control.id != 0 {
                    self.lookup(&control)
                } else {
                    None
                };
                let old = replaced.and_then(|key| self.resources.get(&key));
                let old_encoded = old.map_or(0, |r| r.image.data.len());
                let old_decoded = old.map_or(0, |r| r.decoded_bytes);
                let at_placement_limit = control.action == Action::TransmitAndDisplay
                    && self.placements.len() >= MAX_PLACEMENTS
                    && !self
                        .placements
                        .values()
                        .any(|p| Some(p.placement.resource) == replaced);
                if self.resources.len() - usize::from(replaced.is_some()) >= MAX_RESOURCES
                    || self.encoded_bytes - old_encoded + image.data.len() > MAX_PANE_ENCODED_BYTES
                    || self.decoded_bytes - old_decoded + decoded_bytes > MAX_PANE_DECODED_BYTES
                    || at_placement_limit
                {
                    Self::reply(
                        &mut effects,
                        &control,
                        control.id,
                        "ENOSPC:Pane graphics budget exhausted",
                    );
                    return effects;
                }
                let id = if control.number != 0 {
                    loop {
                        self.next_image_id = self.next_image_id.wrapping_add(1).max(1);
                        if !self.resources.values().any(|r| r.id == self.next_image_id) {
                            break self.next_image_id;
                        }
                    }
                } else {
                    control.id
                };
                if let Some(old_key) = replaced {
                    self.remove_resource(old_key, &mut effects);
                }
                let key = self.key();
                let image = Arc::new(image);
                self.encoded_bytes += image.data.len();
                self.decoded_bytes += decoded_bytes;
                self.resources.insert(
                    key,
                    Resource {
                        image: image.clone(),
                        id,
                        number: control.number,
                        decoded_bytes,
                        screen: self.screen,
                    },
                );
                effects.push(GraphicsEffect::Update(GraphicsUpdate::Resource {
                    key,
                    image,
                }));
                if control.action == Action::TransmitAndDisplay {
                    self.place(key, &control, &mut effects);
                } else {
                    Self::reply(&mut effects, &control, id, "OK");
                }
            }
            Action::Display => {
                if let Some(key) = self.lookup(&control) {
                    self.place(key, &control, &mut effects);
                } else {
                    Self::reply(&mut effects, &control, control.id, "ENOENT:Image not found");
                }
            }
            Action::Delete => self.delete(&control, &mut effects),
            Action::Other => unreachable!(),
        }
        effects
    }

    /// Returns intersected source crop and its occupied grid span. Wire c/r
    /// retain the sender's request so the desktop can use its actual cell size.
    fn geometry(&self, c: &Control, image: &Image) -> Option<(u32, u32, u32, u32)> {
        let w = if c.w == 0 {
            image.width.checked_sub(c.x)?
        } else {
            c.w.min(image.width.checked_sub(c.x)?)
        };
        let h = if c.h == 0 {
            image.height.checked_sub(c.y)?
        } else {
            c.h.min(image.height.checked_sub(c.y)?)
        };
        let cw = u32::from(self.size.cell_w.max(1));
        let ch = u32::from(self.size.cell_h.max(1));
        if w == 0
            || h == 0
            || c.offset_x >= cw
            || c.offset_y >= ch
            || c.cols > 65535
            || c.rows > 65535
        {
            return None;
        }
        let (pixel_w, pixel_h) = match (c.cols, c.rows) {
            (0, 0) => (f64::from(w), f64::from(h)),
            (cols, 0) => {
                let width = f64::from(cols * cw - c.offset_x);
                (width, width * f64::from(h) / f64::from(w))
            }
            (0, rows) => {
                let height = f64::from(rows * ch - c.offset_y);
                (height * f64::from(w) / f64::from(h), height)
            }
            (cols, rows) => (
                f64::from(cols * cw - c.offset_x),
                f64::from(rows * ch - c.offset_y),
            ),
        };
        let cols = ((pixel_w + f64::from(c.offset_x)) / f64::from(cw)).ceil() as u32;
        let rows = ((pixel_h + f64::from(c.offset_y)) / f64::from(ch)).ceil() as u32;
        Some((w, h, cols.max(1), rows.max(1)))
    }

    fn place(&mut self, resource_key: u64, control: &Control, effects: &mut Vec<GraphicsEffect>) {
        let resource = &self.resources[&resource_key];
        let id = resource.id;
        let Some((w, h, span_cols, span_rows)) = self.geometry(control, &resource.image) else {
            Self::reply(effects, control, id, "EINVAL:Invalid placement rectangle");
            return;
        };
        let replacement = (id != 0 && control.placement != 0)
            .then(|| {
                self.placements.iter().find_map(|(key, p)| {
                    (p.placement.resource == resource_key
                        && p.placement.placement_id == control.placement)
                        .then_some(*key)
                })
            })
            .flatten();
        if replacement.is_none() && self.placements.len() >= MAX_PLACEMENTS {
            Self::reply(effects, control, id, "ENOSPC:Placement budget exhausted");
            return;
        }
        let key = replacement.unwrap_or_else(|| self.key());
        let placement = Placement {
            key,
            resource: resource_key,
            image_id: id,
            placement_id: if id == 0 { 0 } else { control.placement },
            cols: control.cols,
            rows: control.rows,
            x: control.x,
            y: control.y,
            w,
            h,
            offset_x: control.offset_x,
            offset_y: control.offset_y,
            z: control.z,
            anchor_row: self.history_origin() + i64::from(self.term.grid.cursor.pos.row.0),
            anchor_col: self.term.grid.cursor.pos.col.0 as u32,
            screen: self.screen,
            span_cols,
            span_rows,
            clip_top: 0,
            clip_bottom: span_rows,
        };
        self.placements.insert(
            key,
            AnchoredPlacement {
                placement: placement.clone(),
            },
        );
        effects.push(GraphicsEffect::Update(GraphicsUpdate::Place(placement)));
        effects.push(GraphicsEffect::Update(GraphicsUpdate::Screen {
            screen: self.screen,
            history_origin: self.history_origin(),
        }));
        if !control.no_move {
            // Kitty leaves the cursor on the placement's last row. Do not
            // encode a zero CUD: terminals interpret CSI 0 B as one row.
            let down = span_rows
                .saturating_sub(1)
                .min(u32::from(self.size.rows.max(1)));
            let right = span_cols.min(u32::from(self.size.cols.max(1)));
            let movement = if down == 0 {
                format!("\x1b[{right}C")
            } else {
                format!("\x1b[{down}B\x1b[{right}C")
            }
            .into_bytes();
            let layout = self.output(&movement);
            effects.push(GraphicsEffect::Output(movement));
            effects.extend(layout);
        }
        Self::reply(effects, control, id, "OK");
    }

    fn remove_resource(&mut self, key: u64, effects: &mut Vec<GraphicsEffect>) {
        let Some(resource) = self.resources.remove(&key) else {
            return;
        };
        self.encoded_bytes -= resource.image.data.len();
        self.decoded_bytes -= resource.decoded_bytes;
        let placements: Vec<_> = self
            .placements
            .iter()
            .filter_map(|(k, p)| (p.placement.resource == key).then_some(*k))
            .collect();
        for key in &placements {
            self.placements.remove(key);
        }
        effects.push(GraphicsEffect::Update(GraphicsUpdate::Remove {
            placements,
            resources: vec![key],
        }));
    }

    fn delete(&mut self, c: &Control, effects: &mut Vec<GraphicsEffect>) {
        let selector = if c.delete == 0 { b'a' } else { c.delete };
        let target = selector.to_ascii_lowercase();
        if !matches!(
            target,
            b'a' | b'i' | b'n' | b'c' | b'p' | b'q' | b'r' | b'x' | b'y' | b'z'
        ) {
            Self::reply(
                effects,
                c,
                c.id,
                "ENOTSUPPORTED:Unsupported delete selector",
            );
            return;
        }
        if (matches!(target, b'p' | b'q') && (c.x == 0 || c.y == 0))
            || (target == b'x' && c.x == 0)
            || (target == b'y' && c.y == 0)
            || (target == b'r' && c.x > c.y)
            || (target == b'i' && c.id == 0)
            || (target == b'n' && c.number == 0)
        {
            Self::reply(effects, c, c.id, "EINVAL:Invalid delete selector");
            return;
        }
        let newest = self.lookup(c);
        let history = self.history_origin();
        let cursor_row = history + i64::from(self.term.grid.cursor.pos.row.0);
        let cursor_col = self.term.grid.cursor.pos.col.0 as u32;
        let mut affected = Vec::new();
        let mut removed = Vec::new();
        self.placements.retain(|key, p| {
            let p = &p.placement;
            if p.screen != self.screen { return true; }
            let row = if target == b'c' {
                cursor_row
            } else {
                history + i64::from(c.y.saturating_sub(1))
            };
            let col = if target == b'c' {
                cursor_col
            } else {
                c.x.saturating_sub(1)
            };
            let top = p.anchor_row + i64::from(p.clip_top);
            let bottom = p.anchor_row + i64::from(p.clip_bottom);
            let in_row = row >= top && row < bottom;
            let in_col = col >= p.anchor_col && col < p.anchor_col.saturating_add(p.span_cols);
            let selected = match target {
                b'a' => {
                    top < history + i64::from(self.size.rows) && bottom > history
                }
                b'i' | b'n' => {
                    Some(p.resource) == newest
                        && (c.placement == 0 || p.placement_id == c.placement)
                }
                b'c' | b'p' => in_row && in_col,
                b'q' => in_row && in_col && p.z == c.z,
                b'r' => p.image_id >= c.x && p.image_id <= c.y,
                b'x' => in_col,
                b'y' => in_row,
                b'z' => p.z == c.z,
                _ => false,
            };
            if selected {
                removed.push(*key);
                affected.push(p.resource);
            }
            !selected
        });
        if matches!(target, b'i' | b'n') && c.placement == 0 {
            affected.extend(newest);
        }
        if target == b'r' {
            affected.extend(
                self.resources
                    .iter()
                    .filter_map(|(key, r)| (r.screen == self.screen && r.id >= c.x && r.id <= c.y).then_some(*key)),
            );
        }
        affected.sort_unstable();
        affected.dedup();
        let mut freed = Vec::new();
        for key in affected {
            if self
                .placements
                .values()
                .any(|p| p.placement.resource == key)
            {
                continue;
            }
            if let Some(resource) = self.resources.get(&key)
                && (selector.is_ascii_uppercase() || resource.id == 0)
            {
                self.encoded_bytes -= resource.image.data.len();
                self.decoded_bytes -= resource.decoded_bytes;
                self.resources.remove(&key);
                freed.push(key);
            }
        }
        if !removed.is_empty() || !freed.is_empty() {
            effects.push(GraphicsEffect::Update(GraphicsUpdate::Remove {
                placements: removed,
                resources: freed,
            }));
        }
    }
}

fn validate_image(image: &mut Image) -> Result<usize, &'static str> {
    if image.data.len() > MAX_IMAGE_BYTES {
        return Err("ENOSPC:Image payload too large");
    }
    if image.format == WireFormat::Png {
        if image.compressed
            || image.data.get(..8) != Some(b"\x89PNG\r\n\x1a\n")
            || image.data.get(12..16) != Some(b"IHDR")
        {
            return Err("EINVAL:Invalid PNG header");
        }
        image.width = u32::from_be_bytes(
            image
                .data
                .get(16..20)
                .ok_or("EINVAL:Invalid PNG header")?
                .try_into()
                .unwrap(),
        );
        image.height = u32::from_be_bytes(
            image
                .data
                .get(20..24)
                .ok_or("EINVAL:Invalid PNG header")?
                .try_into()
                .unwrap(),
        );
    }
    if image.width > MAX_IMAGE_DIMENSION || image.height > MAX_IMAGE_DIMENSION {
        return Err("ENOSPC:Image dimensions exceed limit");
    }
    let bytes = (image.width as usize)
        .checked_mul(image.height as usize)
        .and_then(|n| n.checked_mul(4))
        .filter(|bytes| *bytes > 0 && *bytes <= MAX_IMAGE_BYTES)
        .ok_or("ENOSPC:Decoded image dimensions exceed limit")?;
    if image.format != WireFormat::Png && !image.compressed {
        let expected = if image.format == WireFormat::Rgb {
            bytes / 4 * 3
        } else {
            bytes
        };
        if image.data.len() != expected {
            return Err("EINVAL:Pixel payload does not match dimensions");
        }
    }
    Ok(bytes)
}
