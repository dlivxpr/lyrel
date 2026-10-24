//! Pane-owned graphics resources and placement updates. Pixels are sent once per
//! resource version; placements reference that version without copying pixels.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::kitty_graphics::{Image, WireFormat};

/// One derived display of a resource. Keys are pane-local and never reused.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphicsUpdate {
    Resource { key: u64, image: Arc<Image> },
    Place(Placement),
    Remove { placements: Vec<u64>, resources: Vec<u64> },
}

#[derive(Serialize, Deserialize)]
enum Metadata {
    Resource { key: u64, width: u32, height: u32, format: WireFormat, compressed: bool },
    Place(Placement),
    Remove { placements: Vec<u64>, resources: Vec<u64> },
}

impl GraphicsUpdate {
    /// A length-prefixed, small JSON header followed by unexpanded resource
    /// bytes. In particular pixels are never serialized as JSON/base64.
    pub fn encode_frame(&self) -> Vec<u8> {
        let (metadata, pixels): (Metadata, &[u8]) = match self {
            Self::Resource { key, image } => (Metadata::Resource {
                key: *key, width: image.width, height: image.height,
                format: image.format, compressed: image.compressed,
            }, &image.data),
            Self::Place(placement) => (Metadata::Place(placement.clone()), &[]),
            Self::Remove { placements, resources } => (Metadata::Remove {
                placements: placements.clone(), resources: resources.clone(),
            }, &[]),
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
        if header_len > 1 << 20 { return None; }
        let end = 4usize.checked_add(header_len)?;
        let metadata: Metadata = serde_json::from_slice(frame.get(4..end)?).ok()?;
        match metadata {
            Metadata::Resource { key, width, height, format, compressed } => {
                frame.drain(..end);
                Some(Self::Resource { key, image: Arc::new(Image {
                    id: 0, number: 0, placement: 0, cols: 0, rows: 0,
                    width, height, format, compressed, data: frame,
                }) })
            }
            Metadata::Place(placement) if frame.len() == end => Some(Self::Place(placement)),
            Metadata::Remove { placements, resources } if frame.len() == end =>
                Some(Self::Remove { placements, resources }),
            _ => None,
        }
    }
}
