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
    pub anchor_col: i64,
    /// Main screen is zero; alternate screen is one and cleared on entry.
    pub screen: u64,
    pub span_cols: u32,
    pub span_rows: u32,
    /// Retained vertical slice, in cell rows relative to the original anchor.
    pub clip_top: u32,
    pub clip_bottom: u32,
    /// Invisible prototype; grid cells, not this anchor, select its display.
    pub virtual_placement: bool,
    /// Pane-local placement key, never a GUI-only identity or an image frame.
    pub parent: Option<u64>,
    pub relative_x: i32,
    pub relative_y: i32,
    /// True while a virtual root has no retained, valid placeholder cells.
    pub unresolved: bool,
    /// This placement/prototype itself referenced real live content the last
    /// time its screen was active. Observer viewports never enter this fact.
    pub live: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphicsUpdate {
    Resource {
        key: u64,
        image: Arc<Image>,
    },
    /// Atomic selection of an immutable full-frame resource. Placements retain
    /// their root identity; a monotonic revision invalidates stale paint claims.
    Select {
        key: u64,
        frame: u64,
        revision: u64,
    },
    Frame {
        key: u64,
        root: u64,
        number: u32,
    },
    Evicted {
        key: u64,
        width: u32,
        height: u32,
    },
    Place(Placement),
    Screen {
        screen: u64,
        history_origin: i64,
        epoch: u64,
    },
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
    Select {
        key: u64,
        frame: u64,
        revision: u64,
    },
    Frame {
        key: u64,
        root: u64,
        number: u32,
    },
    Evicted {
        key: u64,
        width: u32,
        height: u32,
    },
    Place(Placement),
    Screen {
        screen: u64,
        history_origin: i64,
        epoch: u64,
    },
    Remove {
        placements: Vec<u64>,
        resources: Vec<u64>,
    },
}

impl GraphicsUpdate {
    /// A length-prefixed, small JSON header followed by unexpanded resource
    /// bytes. In particular pixels are never serialized as JSON/base64.
    fn header_pixels(&self) -> (Vec<u8>, &[u8]) {
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
            Self::Select {
                key,
                frame,
                revision,
            } => (
                Metadata::Select {
                    key: *key,
                    frame: *frame,
                    revision: *revision,
                },
                &[],
            ),
            Self::Frame { key, root, number } => (
                Metadata::Frame {
                    key: *key,
                    root: *root,
                    number: *number,
                },
                &[],
            ),
            Self::Evicted { key, width, height } => (
                Metadata::Evicted {
                    key: *key,
                    width: *width,
                    height: *height,
                },
                &[],
            ),
            Self::Place(placement) => (Metadata::Place(placement.clone()), &[]),
            Self::Screen {
                screen,
                history_origin,
                epoch,
            } => (
                Metadata::Screen {
                    screen: *screen,
                    history_origin: *history_origin,
                    epoch: *epoch,
                },
                &[],
            ),
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
        (header, pixels)
    }

    pub fn encode_frame(&self) -> Vec<u8> {
        let (header, pixels) = self.header_pixels();
        Self::join_frame(&header, pixels)
    }

    fn join_frame(header: &[u8], pixels: &[u8]) -> Vec<u8> {
        let mut frame = Vec::with_capacity(4 + header.len() + pixels.len());
        frame.extend_from_slice(&(header.len() as u32).to_le_bytes());
        frame.extend_from_slice(header);
        frame.extend_from_slice(pixels);
        frame
    }

    pub(crate) fn encode_packet(&self) -> Option<crate::daemon::protocol::GraphicsPacket> {
        let retention = retention::Retention::shared();
        let mut header_work = retention.work(retention.owner());
        let header_bytes = match self {
            Self::Remove {
                placements,
                resources,
            } => (placements.len() + resources.len()) * 128 + 1024,
            Self::Place(_) => 4096,
            _ => 1024,
        };
        if !header_work.resize(header_bytes) {
            return None;
        }
        let (header, pixels) = self.header_pixels();
        crate::daemon::protocol::GraphicsPacket::try_encode(4 + header.len() + pixels.len(), || {
            Self::join_frame(&header, pixels)
        })
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
            Metadata::Select {
                key,
                frame: selected,
                revision,
            } if frame.len() == end => Some(Self::Select {
                key,
                frame: selected,
                revision,
            }),
            Metadata::Frame { key, root, number } if frame.len() == end => {
                Some(Self::Frame { key, root, number })
            }
            Metadata::Evicted { key, width, height } if frame.len() == end => {
                Some(Self::Evicted { key, width, height })
            }
            Metadata::Place(placement) if frame.len() == end => Some(Self::Place(placement)),
            Metadata::Screen {
                screen,
                history_origin,
                epoch,
            } if frame.len() == end => Some(Self::Screen {
                screen,
                history_origin,
                epoch,
            }),
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
use super::terminal_resize::ResizeMode;
use crate::daemon::protocol::WinSize;

#[path = "graphics_terminal.rs"]
mod terminal;

#[path = "graphics_placeholder.rs"]
mod placeholder;
pub use placeholder::{PlaceholderSlice, placeholder_prototype, placeholder_row};

/// Encoded source and RGBA-equivalent bounds; these are independent scopes.
pub const MAX_PANE_ENCODED_BYTES: usize = 64 << 20;
pub const MAX_PANE_DECODED_BYTES: usize = 128 << 20;
/// Admits a 3840 × 2160 RGBA screenshot (33,177,600 bytes).
pub const MAX_IMAGE_DECODED_BYTES: usize = 32 << 20;
#[path = "graphics_retention.rs"]
pub(crate) mod retention;
pub use retention::{GRAPHICS_METADATA_BYTES, MAX_DAEMON_METADATA_BYTES, MAX_PANE_METADATA_BYTES};
pub use retention::{
    MAX_DAEMON_DECODED_BYTES, MAX_DAEMON_ENCODED_BYTES, MAX_DAEMON_WORK_BYTES, MAX_PANE_WORK_BYTES,
};
pub const MAX_IMAGE_DIMENSION: u32 = 8192;
pub const MAX_RESOURCES: usize = 1024;
pub const MAX_PLACEMENTS: usize = 4096;
/// Root included. Each frame owns a separately admitted metadata/source lease.
pub const MAX_ANIMATION_FRAMES: u32 = 256;

pub enum GraphicsEffect {
    Update(GraphicsUpdate),
    Frame(crate::daemon::protocol::GraphicsPacket),
    Reply(Vec<u8>),
    Output(Vec<u8>),
}

struct Resource {
    image: Image,
    resident: Arc<std::sync::Mutex<retention::Resident>>,
    id: u32,
    number: u32,
    screen: u64,
    priority: retention::Priority,
    _metadata: retention::MetadataTicket,
    /// Child frames have no protocol image identity of their own.
    root: Option<u64>,
    frame_number: u32,
    selected: u64,
    revision: u64,
}

struct AnchoredPlacement {
    placement: Placement,
    _metadata: retention::MetadataTicket,
    retained: bool,
    // Geometry is carried by Placement; no client-captured second anchor.
}

pub(crate) struct PreparedImage {
    pub image: Arc<Image>,
    pub packet: Option<crate::daemon::protocol::GraphicsPacket>,
    decoded_bytes: usize,
    key: u64,
    metadata: Option<retention::MetadataTicket>,
    placement: Option<(u64, retention::MetadataTicket)>,
    _work: retention::Work,
}
impl PreparedImage {
    pub(crate) fn placement_key(&self) -> Option<u64> {
        self.placement.as_ref().map(|(key, _)| *key)
    }
}

pub(crate) struct PreparedPlacement {
    resource: u64,
    pub key: u64,
    metadata: retention::MetadataTicket,
    pub needs_offer: bool,
}

/// The living pane owns this state. Desktop caches receive ordered mutations,
/// never decide protocol identities or resource/placement deletion lifetimes.
pub struct GraphicsState {
    retention: Arc<retention::Retention>,
    owner: u64,
    wake: Option<std::sync::mpsc::SyncSender<()>>,
    capacity: (usize, usize),
    generation: u64,
    viewport_epoch: u64,
    viewports: BTreeMap<
        u64,
        (
            std::sync::Weak<std::sync::mpsc::Sender<crate::daemon::protocol::DaemonMsg>>,
            crate::daemon::protocol::GraphicsViewport,
        ),
    >,
    resources: BTreeMap<u64, Resource>,
    placements: BTreeMap<u64, AnchoredPlacement>,
    next_key: u64,
    next_image_id: u32,
    size: WinSize,
    resize_mode: ResizeMode,
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
    /// Standard stream PTY semantics, including native SSH on Windows.
    pub fn new(size: WinSize) -> Self {
        Self::with_resize_mode(size, ResizeMode::Raw)
    }

    /// The source PTY determines row anchoring, including for remote displays.
    pub fn with_resize_mode(size: WinSize, resize_mode: ResizeMode) -> Self {
        let retention = retention::Retention::shared();
        let owner = retention.owner();
        Self {
            capacity: (retention.limits.pane_encoded, retention.limits.pane_decoded),
            generation: 0,
            viewport_epoch: 0,
            viewports: BTreeMap::new(),
            retention,
            owner,
            wake: None,
            resources: BTreeMap::new(),
            placements: BTreeMap::new(),
            next_key: 0,
            next_image_id: 0,
            size,
            resize_mode,
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

    pub(crate) fn work_owner(&self) -> u64 {
        self.owner
    }
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
    pub(crate) fn pressure_resource(&mut self, key: u64) -> (bool, Vec<GraphicsEffect>) {
        self.refresh_retention();
        let reclaimed = self
            .resources
            .get(&key)
            .is_some_and(|resource| self.retention.reclaim_specific(&resource.resident));
        (reclaimed, self.drain_evictions())
    }
    pub(crate) fn reserve_resource_key(&mut self) -> u64 {
        self.key()
    }
    pub(crate) fn offer_resources(
        &self,
    ) -> Option<(Vec<(u64, u32, u32, usize, usize)>, retention::Work)> {
        let count = self.resources.len() + self.placements.len();
        let mut work = self.retention.work(self.owner);
        if !work.resize(count * GRAPHICS_METADATA_BYTES) {
            return None;
        }
        let mut offers = Vec::with_capacity(count);
        for (&key, resource) in &self.resources {
            let resident = resource.resident.lock().unwrap();
            offers.push(if let Some(packet) = &resident.packet {
                (
                    key,
                    resource.image.width,
                    resource.image.height,
                    packet.len(),
                    GRAPHICS_METADATA_BYTES,
                )
            } else {
                (key, 0, 0, 0, GRAPHICS_METADATA_BYTES)
            });
        }
        offers.extend(
            self.placements
                .keys()
                .map(|&key| (key, 0, 0, 0, GRAPHICS_METADATA_BYTES)),
        );
        Some((offers, work))
    }

    /// Geometry/text may advance while the desktop reserves capacity. Only
    /// objects that need a lease must still match the offered version and cost.
    /// Replaying their latest positions is atomic with the text snapshot.
    pub(crate) fn matches_offers(&self, offers: &[(u64, u32, u32, usize, usize)]) -> bool {
        if offers.len() != self.resources.len() + self.placements.len() {
            return false;
        }
        let mut offers = offers.iter();
        for (&key, resource) in &self.resources {
            let resident = resource.resident.lock().unwrap();
            let expected = match &resident.packet {
                Some(packet) => (
                    key,
                    resource.image.width,
                    resource.image.height,
                    packet.len(),
                    GRAPHICS_METADATA_BYTES,
                ),
                None => (key, 0, 0, 0, GRAPHICS_METADATA_BYTES),
            };
            if offers.next() != Some(&expected) {
                return false;
            }
        }
        self.placements
            .keys()
            .all(|&key| offers.next() == Some(&(key, 0, 0, 0, GRAPHICS_METADATA_BYTES)))
    }

    pub(crate) fn prepare_image_metadata(
        &mut self,
        control: &Control,
        image: &mut PreparedImage,
    ) -> Result<(), &'static str> {
        self.check_prepared(control, &image.image)?;
        if control.action == Action::Query || image.metadata.is_some() {
            return Ok(());
        }
        let replaced = (control.action != Action::Frame && control.id != 0)
            .then(|| self.lookup(control))
            .flatten();
        image.metadata = replaced
            .and_then(|key| self.resources.get(&key))
            .map(|r| r._metadata.clone())
            .or_else(|| self.retention.metadata(self.owner));
        if image.metadata.is_none() {
            return Err("ENOSPC:Graphics metadata budget exhausted");
        }
        if control.action == Action::TransmitAndDisplay {
            let metadata = self
                .placements
                .values()
                .find(|p| Some(p.placement.resource) == replaced)
                .map(|p| p._metadata.clone())
                .or_else(|| self.retention.metadata(self.owner))
                .ok_or("ENOSPC:Graphics metadata budget exhausted")?;
            image.placement = Some((self.key(), metadata));
        }
        Ok(())
    }

    pub(crate) fn prepare_placement(
        &mut self,
        control: &Control,
    ) -> Result<PreparedPlacement, &'static str> {
        self.relative_parent(control, None)?;
        let resource = self
            .lookup(control)
            .filter(|key| {
                self.resources[key]
                    .resident
                    .lock()
                    .unwrap()
                    .packet
                    .is_some()
            })
            .ok_or("ENOENT:Image not found")?;
        self.geometry(control, &self.resources[&resource].image)
            .ok_or("EINVAL:Invalid placement rectangle")?;
        let existing = (control.placement != 0)
            .then(|| {
                self.placements.iter().find(|(_, p)| {
                    p.placement.resource == resource
                        && p.placement.placement_id == control.placement
                })
            })
            .flatten();
        if let Some((&key, p)) = existing {
            return Ok(PreparedPlacement {
                resource,
                key,
                metadata: p._metadata.clone(),
                needs_offer: false,
            });
        }
        if self.placements.len() >= MAX_PLACEMENTS {
            return Err("ENOSPC:Placement budget exhausted");
        }
        let metadata = self
            .retention
            .metadata(self.owner)
            .ok_or("ENOSPC:Graphics metadata budget exhausted")?;
        Ok(PreparedPlacement {
            resource,
            key: self.key(),
            metadata,
            needs_offer: true,
        })
    }

    pub(crate) fn apply_prepared_placement(
        &mut self,
        control: Control,
        placement: PreparedPlacement,
    ) -> Vec<GraphicsEffect> {
        self.generation += 1;
        self.protect_resources();
        let mut effects = self.drain_evictions();
        if self
            .resources
            .get(&placement.resource)
            .is_some_and(|r| r.resident.lock().unwrap().packet.is_some())
        {
            self.place(
                placement.resource,
                &control,
                &mut effects,
                Some((placement.key, placement.metadata)),
            );
        } else {
            Self::reply(&mut effects, &control, control.id, "ENOENT:Image not found");
        }
        effects.extend(self.finish_layout(false));
        effects
    }

    pub(crate) fn check_prepared(
        &self,
        control: &Control,
        image: &Image,
    ) -> Result<(), &'static str> {
        if control.action == Action::Query {
            return Ok(());
        }
        if control.action == Action::Frame {
            let root = self.lookup(control).ok_or("ENOENT:Image not found")?;
            let resource = &self.resources[&root];
            if control.x != 0
                || control.y != 0
                || control.cols != 0
                || control.rows != 0
                || control.offset_x > 1
                || control.offset_y != 0
                || control.z != 0
            {
                return Err("ENOTSUPPORTED:Partial frame editing and timing unavailable");
            }
            if image.width != resource.image.width || image.height != resource.image.height {
                return Err("EINVAL:Complete frame dimensions must match root");
            }
            if resource.resident.lock().unwrap().packet.is_none() {
                return Err("ENOENT:Image data unavailable");
            }
            if self
                .resources
                .values()
                .filter(|r| r.root == Some(root))
                .count()
                + 1
                >= MAX_ANIMATION_FRAMES as usize
                || self.resources.len() >= MAX_RESOURCES
            {
                return Err("ENOSPC:Animation frame budget exhausted");
            }
            return Ok(());
        }
        if control.action == Action::TransmitAndDisplay {
            self.relative_parent(control, self.lookup(control))?;
        }
        if control.action == Action::TransmitAndDisplay && self.geometry(control, image).is_none() {
            return Err("EINVAL:Invalid placement rectangle");
        }
        let replaced = (control.id != 0).then(|| self.lookup(control)).flatten();
        let placement_full = control.action == Action::TransmitAndDisplay
            && self.placements.len() >= MAX_PLACEMENTS
            && !self
                .placements
                .values()
                .any(|p| Some(p.placement.resource) == replaced);
        let replaced_count = replaced.map_or(0, |root| {
            self.resources
                .iter()
                .filter(|(key, resource)| **key == root || resource.root == Some(root))
                .count()
        });
        if self.resources.len() - replaced_count >= MAX_RESOURCES || placement_full {
            return Err("ENOSPC:Pane graphics budget exhausted");
        }
        Ok(())
    }

    pub(crate) fn prepare_image(
        mut image: Image,
        owner: u64,
        key: u64,
        control: &Control,
    ) -> Result<PreparedImage, &'static str> {
        let retention = retention::Retention::shared();
        let mut work = retention.work(owner);
        let decoded_bytes = validate_image(&mut image, &mut work).map_err(|status| {
            if control.medium == super::kitty_graphics::Medium::Direct {
                status
            } else {
                "EBADF:Failed to read image file"
            }
        })?;
        let image = Arc::new(image);
        let packet = if control.action != Action::Query {
            Some(
                (GraphicsUpdate::Resource {
                    key,
                    image: image.clone(),
                })
                .encode_packet()
                .ok_or("ENOSPC:Graphics transport budget exhausted")?,
            )
        } else {
            None
        };
        Ok(PreparedImage {
            image,
            packet,
            decoded_bytes,
            key,
            metadata: None,
            placement: None,
            _work: work,
        })
    }

    pub(crate) fn set_wake(&mut self, wake: std::sync::mpsc::SyncSender<()>) {
        self.wake = Some(wake);
    }

    pub(crate) fn set_capacity(&mut self, encoded: usize, decoded: usize) -> bool {
        let usage = self.resident_usage(None);
        if usage.0 > encoded || usage.1 > decoded {
            return false;
        }
        self.capacity = (
            encoded.min(self.retention.limits.pane_encoded),
            decoded.min(self.retention.limits.pane_decoded),
        );
        true
    }

    fn resident_usage(&self, excluding: Option<u64>) -> (usize, usize) {
        self.resources
            .iter()
            .filter(|(key, _)| Some(**key) != excluding)
            .fold((0, 0), |mut usage, (_, resource)| {
                let resident = resource.resident.lock().unwrap();
                if let Some(packet) = &resident.packet {
                    usage.0 += packet.len();
                    usage.1 += resident.decoded;
                }
                usage
            })
    }

    fn protect_resources(&self) {
        if self.resources.is_empty() {
            return;
        }
        self.retention
            .protect(self.resources.values().map(|r| &r.resident));
    }

    pub(crate) fn set_viewport(
        &mut self,
        endpoint: u64,
        owner: std::sync::Weak<std::sync::mpsc::Sender<crate::daemon::protocol::DaemonMsg>>,
        viewport: crate::daemon::protocol::GraphicsViewport,
    ) {
        if viewport.epoch != self.viewport_epoch || viewport.screen != self.screen {
            return;
        }
        self.protect_resources();
        self.viewports.insert(endpoint, (owner, viewport));
        self.refresh_retention();
    }

    pub(crate) fn remove_viewport(&mut self, endpoint: u64) {
        self.viewports.remove(&endpoint);
        self.refresh_retention();
    }

    pub(crate) fn retain_viewports(
        &mut self,
        mut attached: impl FnMut(
            &std::sync::Weak<std::sync::mpsc::Sender<crate::daemon::protocol::DaemonMsg>>,
        ) -> bool,
    ) {
        let before = self.viewports.len();
        self.viewports.retain(|_, (owner, _)| attached(owner));
        if self.viewports.len() != before {
            self.refresh_retention();
        }
    }

    fn refresh_retention(&mut self) {
        use retention::Priority;
        self.viewports
            .retain(|_, (owner, _)| owner.strong_count() != 0);
        let live_placeholders = self.placeholder_placements(false);
        let retained_placeholders = self.placeholder_placements(true);
        let top = self.history_origin();
        let bottom = top + i64::from(self.size.rows.max(1));
        let oldest = self.term.grid.lines_evicted() as i64;
        let screen = self.screen;
        let epoch = self.viewport_epoch;
        let visible_range = |viewport: &crate::daemon::protocol::GraphicsViewport| {
            if viewport.epoch != epoch || viewport.screen != screen || viewport.rows == 0 {
                return None;
            }
            let start = viewport.top.unwrap_or(top).clamp(oldest, top);
            Some((start, start + i64::from(viewport.rows), viewport.cols))
        };
        let mut observed_placeholders = BTreeSet::new();
        for (start, end, _) in self
            .viewports
            .values()
            .filter_map(|(_, viewport)| visible_range(viewport))
        {
            observed_placeholders.extend(
                self.placeholder_placements_in(
                    (start - top) as i32,
                    (end.min(bottom) - top) as i32,
                ),
            );
        }
        for resource in self.resources.values_mut() {
            resource.priority = Priority::Unreferenced;
        }
        for (&key, anchored) in &mut self.placements {
            let p = &mut anchored.placement;
            if p.screen == screen {
                let live = if p.virtual_placement {
                    anchored.retained = retained_placeholders.contains(&key);
                    live_placeholders.contains(&key)
                } else {
                    !p.unresolved
                        && p.anchor_col < i64::from(self.size.cols.max(1))
                        && p.anchor_col + i64::from(p.span_cols) > 0
                        && p.anchor_row + i64::from(p.clip_top) < bottom
                        && p.anchor_row + i64::from(p.clip_bottom) > top
                };
                if p.live != live {
                    p.live = live;
                    self.dirty.insert(key);
                }
            }
            // Inactive screens keep each placement's last real live fact,
            // not an observer's transient protection or another placement's.
            let mut priority = if p.live {
                Priority::Protected
            } else if !p.virtual_placement || anchored.retained {
                Priority::History
            } else {
                Priority::Unreferenced
            };
            if p.screen == screen
                && (if p.virtual_placement {
                    observed_placeholders.contains(&key)
                } else {
                    self.viewports
                        .values()
                        .filter_map(|(_, viewport)| visible_range(viewport))
                        .any(|(start, end, cols)| {
                            !p.unresolved
                                && p.anchor_col < i64::from(cols)
                                && p.anchor_col + i64::from(p.span_cols) > 0
                                && p.anchor_row + i64::from(p.clip_top) < end
                                && p.anchor_row + i64::from(p.clip_bottom) > start
                        })
                })
            {
                priority = Priority::Protected;
            }
            if let Some(resource) = self.resources.get_mut(&p.resource) {
                resource.priority = resource.priority.max(priority);
            }
        }
        // Publish only complete reductions: another pane's admission must not
        // observe a live resource temporarily downgraded during the scan.
        for resource in self.resources.values() {
            // Admission and pressure reclaim a frame sequence atomically.
            resource.resident.lock().unwrap().priority = resource
                .root
                .and_then(|root| self.resources.get(&root))
                .map_or(resource.priority, |root| root.priority);
        }
    }

    /// Drain under the pane output lock before any later mutation. A removed
    /// resource has no slot here, so a queued wake cannot resurrect it.
    pub(crate) fn drain_evictions(&mut self) -> Vec<GraphicsEffect> {
        let mut effects = Vec::new();
        let mut obsolete = Vec::new();
        let mut evicted_roots = BTreeSet::new();
        for (&key, resource) in &self.resources {
            let mut resident = resource.resident.lock().unwrap();
            if resident.packet.is_some() {
                continue;
            }
            if resource.root.is_none() && !resident.announced {
                evicted_roots.insert(key);
            }
            if resource.priority == retention::Priority::Unreferenced {
                obsolete.push(key);
            } else if !resident.announced {
                resident.announced = true;
                effects.push(GraphicsEffect::Update(GraphicsUpdate::Evicted {
                    key,
                    width: resource.image.width,
                    height: resource.image.height,
                }));
            }
        }
        let children: Vec<_> = self
            .resources
            .iter()
            .filter_map(|(&key, resource)| {
                resource
                    .root
                    .filter(|root| evicted_roots.contains(root))
                    .map(|_| key)
            })
            .collect();
        for key in &children {
            self.resources.remove(key);
        }
        if !children.is_empty() {
            effects.push(GraphicsEffect::Update(GraphicsUpdate::Remove {
                placements: Vec::new(),
                resources: children,
            }));
        }
        for root in evicted_roots {
            let resource = self.resources.get_mut(&root).unwrap();
            if resource.selected == root {
                continue;
            }
            resource.selected = root;
            resource.revision = resource
                .revision
                .checked_add(1)
                .expect("graphics selection revision exhausted");
            effects.push(GraphicsEffect::Update(GraphicsUpdate::Select {
                key: root,
                frame: root,
                revision: resource.revision,
            }));
        }
        for key in obsolete {
            self.remove_resource(key, &mut effects);
        }
        effects
    }

    pub(crate) fn replay(&self) -> Option<(Vec<GraphicsEffect>, retention::Work)> {
        let mut work = self.retention.work(self.owner);
        if self.resources.is_empty() {
            return Some((Vec::new(), work));
        }
        let count = self.resources.len() * 2 + self.placements.len() + 1;
        if !work.resize(count * std::mem::size_of::<GraphicsEffect>()) {
            return None;
        }
        let mut updates = Vec::with_capacity(count);
        for (&key, resource) in &self.resources {
            let resident = resource.resident.lock().unwrap();
            updates.push(if let Some(packet) = &resident.packet {
                GraphicsEffect::Frame(packet.clone())
            } else {
                GraphicsEffect::Update(GraphicsUpdate::Evicted {
                    key,
                    width: resource.image.width,
                    height: resource.image.height,
                })
            });
        }
        for (&key, resource) in &self.resources {
            if let Some(root) = resource.root {
                updates.push(GraphicsEffect::Update(GraphicsUpdate::Frame {
                    key,
                    root,
                    number: resource.frame_number,
                }));
            } else if resource.revision != 0 {
                updates.push(GraphicsEffect::Update(GraphicsUpdate::Select {
                    key,
                    frame: resource.selected,
                    revision: resource.revision,
                }));
            }
        }
        updates.extend(
            self.placements()
                .cloned()
                .map(|p| GraphicsEffect::Update(GraphicsUpdate::Place(p))),
        );
        updates.push(GraphicsEffect::Update(GraphicsUpdate::Screen {
            screen: self.screen,
            history_origin: self.history_origin(),
            epoch: self.viewport_epoch,
        }));
        Some((updates, work))
    }

    pub fn resize(&mut self, size: WinSize) -> Vec<GraphicsEffect> {
        self.generation += 1;
        self.protect_resources();
        self.resize_layout(size);
        self.finish_layout(true)
    }

    pub fn output(&mut self, bytes: &[u8]) -> Vec<GraphicsEffect> {
        self.generation += 1;
        self.protect_resources();
        let old = (self.screen, self.history_origin(), self.viewport_epoch);
        let mut processor = std::mem::take(&mut self.processor);
        processor.advance(self, bytes);
        self.processor = processor;
        self.expire_history();
        self.finish_layout(
            old.2 != self.viewport_epoch
                || (!self.placements.is_empty()
                    && old != (self.screen, self.history_origin(), self.viewport_epoch)),
        )
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
            (resource.root.is_none()
                && resource.screen == self.screen
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
        self.apply_inner(control, image, None)
    }

    pub(crate) fn apply_prepared(
        &mut self,
        control: Control,
        image: PreparedImage,
    ) -> Vec<GraphicsEffect> {
        self.apply_inner(control, None, Some(image))
    }

    fn apply_inner(
        &mut self,
        control: Control,
        image: Option<Image>,
        prepared: Option<PreparedImage>,
    ) -> Vec<GraphicsEffect> {
        self.generation += 1;
        let mut effects = self.drain_evictions();
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
            Action::Transmit | Action::TransmitAndDisplay | Action::Query | Action::Frame => {
                let prepared = match prepared {
                    Some(prepared) => Ok(prepared),
                    None => match image {
                        Some(image) => {
                            let key = self.key();
                            Self::prepare_image(image, self.owner, key, &control)
                        }
                        None => Err("EINVAL:Missing image data"),
                    },
                };
                let mut prepared = match prepared {
                    Ok(prepared) => prepared,
                    Err(status) => {
                        Self::reply(&mut effects, &control, control.id, status);
                        return effects;
                    }
                };
                if let Err(status) = self.prepare_image_metadata(&control, &mut prepared) {
                    Self::reply(&mut effects, &control, control.id, status);
                    return effects;
                }
                let PreparedImage {
                    image,
                    packet,
                    decoded_bytes,
                    key,
                    metadata,
                    placement: placement_lease,
                    _work: _validation,
                } = prepared;
                if control.action == Action::Query {
                    Self::reply(&mut effects, &control, control.id, "OK");
                    return effects;
                }
                if let Err(status) = self.check_prepared(&control, &image) {
                    Self::reply(&mut effects, &control, control.id, status);
                    return effects;
                }
                let root = if control.action == Action::Frame {
                    self.lookup(&control)
                } else {
                    None
                };
                let replaced = if control.action != Action::Frame && control.id != 0 {
                    self.lookup(&control)
                } else {
                    None
                };
                let replacement = replaced
                    .and_then(|key| self.resources.get(&key))
                    .map(|r| r.resident.clone());
                let metadata_lease = metadata.expect("prepared resource owns its metadata lease");
                self.refresh_retention();
                let packet = packet.expect("transmit preparation encodes its resource packet");
                let Some(resident) = self.retention.admit(
                    self.owner,
                    root.unwrap_or(key),
                    decoded_bytes,
                    replacement.as_ref(),
                    self.wake.clone(),
                    self.capacity,
                    packet.clone(),
                ) else {
                    Self::reply(
                        &mut effects,
                        &control,
                        control.id,
                        "ENOSPC:Graphics retention budget exhausted",
                    );
                    return effects;
                };
                effects.extend(self.drain_evictions());
                let id = if let Some(root) = root {
                    self.resources[&root].id
                } else if control.number != 0 {
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
                let metadata = Image {
                    width: image.width,
                    height: image.height,
                    data: Vec::new(),
                    format: image.format,
                    compressed: image.compressed,
                };
                self.resources.insert(
                    key,
                    Resource {
                        image: metadata,
                        resident,
                        id,
                        number: control.number,
                        screen: self.screen,
                        priority: retention::Priority::Unreferenced,
                        _metadata: metadata_lease,
                        root,
                        frame_number: root.map_or(1, |root| {
                            self.resources
                                .values()
                                .filter(|r| r.root == Some(root))
                                .count() as u32
                                + 2
                        }),
                        selected: key,
                        revision: 0,
                    },
                );
                effects.push(GraphicsEffect::Frame(packet));
                if let Some(root) = root {
                    effects.push(GraphicsEffect::Update(GraphicsUpdate::Frame {
                        key,
                        root,
                        number: self.resources[&key].frame_number,
                    }));
                }
                if control.action == Action::TransmitAndDisplay {
                    self.place(key, &control, &mut effects, placement_lease);
                } else {
                    Self::reply(&mut effects, &control, id, "OK");
                }
            }
            Action::Display => {
                self.protect_resources();
                if let Some(key) = self.lookup(&control).filter(|key| {
                    self.resources[key]
                        .resident
                        .lock()
                        .unwrap()
                        .packet
                        .is_some()
                }) {
                    self.place(key, &control, &mut effects, None);
                } else {
                    Self::reply(&mut effects, &control, control.id, "ENOENT:Image not found");
                }
            }
            Action::Animation => self.select_frame(&control, &mut effects),
            Action::Delete => self.delete(&control, &mut effects),
            Action::Other => unreachable!(),
        }
        effects.extend(self.finish_layout(false));
        effects
    }

    fn select_frame(&mut self, control: &Control, effects: &mut Vec<GraphicsEffect>) {
        let Some(root) = self.lookup(control) else {
            Self::reply(effects, control, control.id, "ENOENT:Image not found");
            return;
        };
        let id = self.resources[&root].id;
        if control.width > 1 || control.height != 0 || control.rows != 0 || control.z != 0 {
            Self::reply(
                effects,
                control,
                id,
                "ENOTSUPPORTED:Automatic playback and timing unavailable",
            );
            return;
        }
        let frame = if control.cols == 0 {
            Some(self.resources[&root].selected)
        } else if control.cols == 1 {
            Some(root)
        } else {
            self.resources.iter().find_map(|(&key, resource)| {
                (resource.root == Some(root) && resource.frame_number == control.cols)
                    .then_some(key)
            })
        };
        let Some(frame) = frame.filter(|frame| {
            self.resources
                .get(frame)
                .is_some_and(|resource| resource.resident.lock().unwrap().packet.is_some())
        }) else {
            Self::reply(effects, control, id, "ENOENT:Animation frame not found");
            return;
        };
        let resource = self.resources.get_mut(&root).unwrap();
        resource.selected = frame;
        resource.revision = resource
            .revision
            .checked_add(1)
            .expect("graphics selection revision exhausted");
        effects.push(GraphicsEffect::Update(GraphicsUpdate::Select {
            key: root,
            frame,
            revision: resource.revision,
        }));
        Self::reply(effects, control, id, "OK");
    }

    /// Validate the prospective graph before quota admission or replacement.
    /// A replacement may deepen existing descendants, so validate every path.
    fn relative_parent(
        &self,
        c: &Control,
        removed_resource: Option<u64>,
    ) -> Result<Option<u64>, &'static str> {
        if c.parent_image == 0 {
            if c.parent_placement != 0 {
                return Err("ENOPARENT:Parent placement not found");
            }
            return Ok(None);
        }
        if c.virtual_placement {
            return Err("EINVAL:Virtual placement cannot be relative");
        }
        let parent = self
            .placements
            .values()
            .rev()
            .find(|p| {
                p.placement.screen == self.screen
                    && p.placement.image_id == c.parent_image
                    && p.placement.placement_id == c.parent_placement
                    && Some(p.placement.resource) != removed_resource
            })
            .map(|p| p.placement.key)
            .ok_or("ENOPARENT:Parent placement not found")?;
        let mut ancestor = Some(parent);
        for _ in 0..=8 {
            let Some(key) = ancestor else { break };
            let p = &self.placements[&key].placement;
            if Some(p.resource) == removed_resource {
                return Err("ENOPARENT:Parent placement not found");
            }
            ancestor = p.parent;
        }
        let replacement = self
            .placements
            .values()
            .find(|p| {
                p.placement.screen == self.screen
                    && c.id != 0
                    && c.placement != 0
                    && p.placement.image_id == c.id
                    && p.placement.placement_id == c.placement
            })
            .map(|p| p.placement.key);
        for start in self
            .placements
            .keys()
            .copied()
            .map(Some)
            .chain(std::iter::once(None))
        {
            let mut current = start;
            let mut visited = [0u64; 10];
            let mut count = 0;
            loop {
                let next = if current.is_none() || current == replacement {
                    Some(parent)
                } else {
                    self.placements
                        .get(&current.unwrap())
                        .and_then(|p| p.placement.parent)
                };
                let Some(next) = next else { break };
                if visited[..count].contains(&next) {
                    return Err("ECYCLE:Placement cycle");
                }
                if count == 8 {
                    // Check for a cycle before depth refusal, including a long
                    // descendant chain closed by this replacement.
                    let mut cursor = Some(next);
                    for _ in 0..=MAX_PLACEMENTS {
                        let Some(key) = cursor else { break };
                        if Some(key) == replacement {
                            return Err("ECYCLE:Placement cycle");
                        }
                        cursor = self.placements.get(&key).and_then(|p| p.placement.parent);
                    }
                    return Err("ETOODEEP:Relative placement chain exceeds eight links");
                }
                visited[count] = next;
                count += 1;
                current = Some(next);
            }
        }
        Ok(Some(parent))
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

    fn place(
        &mut self,
        resource_key: u64,
        control: &Control,
        effects: &mut Vec<GraphicsEffect>,
        reserved_metadata: Option<(u64, retention::MetadataTicket)>,
    ) {
        let resource = &self.resources[&resource_key];
        let parent = match self.relative_parent(control, None) {
            Ok(parent) => parent,
            Err(status) => {
                Self::reply(effects, control, resource.id, status);
                return;
            }
        };
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
        let metadata = reserved_metadata
            .or_else(|| {
                replacement
                    .and_then(|key| self.placements.get(&key))
                    .map(|p| (p.placement.key, p._metadata.clone()))
            })
            .or_else(|| {
                self.retention
                    .metadata(self.owner)
                    .map(|metadata| (self.key(), metadata))
            });
        let Some((key, metadata)) = metadata else {
            Self::reply(
                effects,
                control,
                id,
                "ENOSPC:Graphics metadata budget exhausted",
            );
            return;
        };
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
            anchor_col: self.term.grid.cursor.pos.col.0 as i64,
            screen: self.screen,
            span_cols,
            span_rows,
            clip_top: 0,
            clip_bottom: span_rows,
            virtual_placement: control.virtual_placement,
            parent,
            relative_x: control.relative_x,
            relative_y: control.relative_y,
            unresolved: parent.is_some() || control.virtual_placement,
            live: !control.virtual_placement,
        };
        self.placements.insert(
            key,
            AnchoredPlacement {
                placement: placement.clone(),
                _metadata: metadata,
                retained: !control.virtual_placement,
            },
        );
        effects.push(GraphicsEffect::Update(GraphicsUpdate::Place(placement)));
        effects.push(GraphicsEffect::Update(GraphicsUpdate::Screen {
            screen: self.screen,
            history_origin: self.history_origin(),
            epoch: self.viewport_epoch,
        }));
        if !control.no_move && !control.virtual_placement && parent.is_none() {
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
        drop(resource);
        let mut resources = vec![key];
        resources.extend(
            self.resources
                .iter()
                .filter_map(|(&child, resource)| (resource.root == Some(key)).then_some(child)),
        );
        for key in &resources {
            self.resources.remove(key);
        }
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
            resources,
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
        let cursor_col = self.term.grid.cursor.pos.col.0 as i64;
        let mut affected = Vec::new();
        let mut removed = Vec::new();
        self.placements.retain(|key, p| {
            let p = &p.placement;
            if p.screen != self.screen
                || (p.virtual_placement && !matches!(target, b'i' | b'n' | b'r'))
            {
                return true;
            }
            let row = if target == b'c' {
                cursor_row
            } else {
                history + i64::from(c.y.saturating_sub(1))
            };
            let col = if target == b'c' {
                cursor_col
            } else {
                i64::from(c.x.saturating_sub(1))
            };
            let top = p.anchor_row + i64::from(p.clip_top);
            let bottom = p.anchor_row + i64::from(p.clip_bottom);
            let in_row = row >= top && row < bottom;
            let in_col = col >= p.anchor_col && col < p.anchor_col + i64::from(p.span_cols);
            let selected = match target {
                b'a' => top < history + i64::from(self.size.rows) && bottom > history,
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
                affected.push((p.resource, p.parent.is_some()));
            }
            !selected
        });
        if matches!(target, b'i' | b'n') && c.placement == 0 {
            affected.extend(newest.map(|key| (key, false)));
        }
        if target == b'r' {
            affected.extend(self.resources.iter().filter_map(|(key, r)| {
                (r.screen == self.screen && r.id >= c.x && r.id <= c.y).then_some((*key, false))
            }));
        }
        affected.sort_unstable();
        affected.dedup();
        if !removed.is_empty() {
            effects.push(GraphicsEffect::Update(GraphicsUpdate::Remove {
                placements: removed,
                resources: Vec::new(),
            }));
        }
        for (key, relative) in affected {
            if self
                .placements
                .values()
                .any(|p| p.placement.resource == key)
            {
                continue;
            }
            if let Some(resource) = self.resources.get(&key)
                && resource.root.is_none()
                && (relative || selector.is_ascii_uppercase() || resource.id == 0)
            {
                let _ = resource;
                self.remove_resource(key, effects);
            }
        }
    }
}

fn validate_image(image: &mut Image, work: &mut retention::Work) -> Result<usize, &'static str> {
    if image.data.len() > MAX_IMAGE_BYTES {
        return Err("ENOSPC:Image payload too large");
    }
    // The parser still charges the input for this read. Only a normalized PNG
    // adds a second owned source; keep that source's capacity charged through
    // PreparedImage, including while it is serialized/admitted.
    let mut normalized_capacity = 0;
    if image.format == WireFormat::Png {
        if image.compressed {
            image.data = inflate_png(&image.data, work)?;
            normalized_capacity = image.data.capacity();
            image.compressed = false;
        }
        if image.data.get(..8) != Some(b"\x89PNG\r\n\x1a\n")
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
        .filter(|bytes| *bytes > 0 && *bytes <= MAX_IMAGE_DECODED_BYTES)
        .ok_or("ENOSPC:Decoded image dimensions exceed limit")?;
    if image.format == WireFormat::Png {
        // Header dimensions are checked above, before the PNG library can
        // allocate. Validation discards each decoded row instead of allocating
        // a full second image in the daemon. Ignore ancillary metadata that is
        // irrelevant to terminal pixels and independently cap decoder scratch.
        if !work.resize(normalized_capacity + (4 << 20)) {
            return Err("ENOSPC:Graphics validation budget exhausted");
        }
        let mut decoder = png::Decoder::new_with_limits(
            std::io::Cursor::new(&image.data),
            png::Limits { bytes: 4 << 20 },
        );
        decoder.set_ignore_text_chunk(true);
        decoder.set_ignore_iccp_chunk(true);
        decoder.ignore_checksums(false);
        let mut reader = decoder.read_info().map_err(|_| "EINVAL:Invalid PNG data")?;
        while reader
            .next_row()
            .map_err(|_| "EINVAL:Invalid PNG pixels")?
            .is_some()
        {}
        reader.finish().map_err(|_| "EINVAL:Invalid PNG end")?;
    } else {
        let expected = if image.format == WireFormat::Rgb {
            bytes / 4 * 3
        } else {
            bytes
        };
        if image.compressed {
            if !work.resize(256 << 10) {
                return Err("ENOSPC:Graphics validation budget exhausted");
            }
            validate_zlib_pixels(&image.data, expected)?;
        } else if image.data.len() != expected {
            return Err("EINVAL:Pixel payload does not match dimensions");
        }
    }
    // Row decoding/inflate scratch is gone, but normalized source bytes stay
    // alive until the prepared image and its packet have been consumed.
    work.resize(normalized_capacity);
    Ok(bytes)
}

fn inflate_png(data: &[u8], work: &mut retention::Work) -> Result<Vec<u8>, &'static str> {
    use miniz_oxide::inflate::stream::{InflateState, inflate};
    use miniz_oxide::{DataFormat, MZFlush, MZStatus};

    let state_bytes = std::mem::size_of::<InflateState>();
    if !work.resize(state_bytes) {
        return Err("ENOSPC:Graphics validation budget exhausted");
    }
    let mut state = InflateState::new_boxed(DataFormat::Zlib);
    let mut output = Vec::new();
    let mut consumed = 0;
    loop {
        if output.len() == output.capacity() {
            let old_capacity = output.capacity();
            // One excess byte detects expansion beyond the encoded source
            // boundary, including when the valid PNG exactly fills the limit.
            let capacity = old_capacity
                .max(32 << 10)
                .saturating_mul(2)
                .min(MAX_IMAGE_BYTES + 1);
            // reserve_exact may move the allocation: both old and new source
            // capacities coexist at its peak, in addition to the zlib state.
            if !work.resize(state_bytes + old_capacity + capacity) {
                return Err("ENOSPC:Graphics validation budget exhausted");
            }
            output.reserve_exact(capacity - output.len());
            if !work.resize(state_bytes + output.capacity()) {
                return Err("ENOSPC:Graphics validation budget exhausted");
            }
        }
        let start = output.len();
        output.resize(output.capacity(), 0);
        let result = inflate(
            &mut state,
            &data[consumed..],
            &mut output[start..],
            MZFlush::None,
        );
        consumed += result.bytes_consumed;
        output.truncate(start + result.bytes_written);
        if output.len() > MAX_IMAGE_BYTES {
            return Err("EINVAL:Invalid or excessive compressed PNG");
        }
        match result.status {
            Ok(MZStatus::StreamEnd) if consumed == data.len() => {
                drop(state);
                work.resize(output.capacity());
                return Ok(output);
            }
            Ok(MZStatus::Ok) if result.bytes_consumed != 0 || result.bytes_written != 0 => {}
            _ => return Err("EINVAL:Invalid or excessive compressed PNG"),
        }
    }
}

fn validate_zlib_pixels(data: &[u8], expected: usize) -> Result<(), &'static str> {
    use miniz_oxide::inflate::stream::{InflateState, inflate};
    use miniz_oxide::{DataFormat, MZFlush, MZStatus};

    // Count validated output without a second full pixel allocation in the
    // daemon. One extra byte detects expansion beyond the declared raw length.
    let mut state = InflateState::new_boxed(DataFormat::Zlib);
    let mut scratch = [0; 64 << 10];
    let mut consumed = 0;
    let mut decoded = 0;
    loop {
        let len = scratch.len().min(expected - decoded + 1);
        let result = inflate(
            &mut state,
            &data[consumed..],
            &mut scratch[..len],
            MZFlush::None,
        );
        consumed += result.bytes_consumed;
        decoded += result.bytes_written;
        if decoded > expected {
            return Err("EINVAL:Compressed pixels exceed declared dimensions");
        }
        match result.status {
            Ok(MZStatus::StreamEnd) if decoded == expected && consumed == data.len() => {
                return Ok(());
            }
            Ok(MZStatus::Ok) if result.bytes_consumed != 0 || result.bytes_written != 0 => {}
            _ => return Err("EINVAL:Invalid compressed pixel data or byte count"),
        }
    }
}

#[cfg(test)]
#[path = "graphics_retention_tests.rs"]
mod retention_tests;
