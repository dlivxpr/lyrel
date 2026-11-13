//! Pane-session-local dynamic colours (OSC 4/10/11/12 and their resets).
//!
//! A program may set the indexed palette and the default foreground,
//! background and cursor for its pane. Those values are an override layer over
//! the live lyrel theme: they never touch the workbench theme or user
//! configuration, they belong to the pane session, and a reset reveals whatever
//! the current theme paints rather than a snapshot of the theme at set time.
//!
//! The state is a fixed 259-slot table — 256 indexed entries plus the three
//! named defaults — so an untrusted program can neither grow it nor make the
//! daemon retain anything beyond a small, accounted frame. Nothing here is
//! persisted: a replacement shell after daemon death starts empty, and the
//! only cross-image carrier is the explicit live-handoff record.

/// Indexed palette slots (`OSC 4`).
pub const INDEXED_SLOTS: usize = 256;
/// `OSC 10` default foreground.
pub const DEFAULT_FG: u16 = 256;
/// `OSC 11` default background.
pub const DEFAULT_BG: u16 = 257;
/// `OSC 12` cursor colour.
pub const CURSOR: u16 = 258;
/// Every slot a program may override.
pub const COLOR_SLOTS: usize = 259;

const FORMAT: u8 = 1;
const MODE_DELTA: u8 = 0;
const MODE_SNAPSHOT: u8 = 1;
const MAX_OPS: usize = COLOR_SLOTS + 1;

const TAG_SET: u8 = 0;
const TAG_RESET: u8 = 1;
const TAG_RESET_INDEXED: u8 = 2;

/// One effective override layer. `None` means "the current default", which is
/// resolved by the renderer and by query replies against the live theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneColors {
    slots: [Option<[u8; 3]>; COLOR_SLOTS],
}

impl Default for PaneColors {
    fn default() -> Self {
        Self {
            slots: [None; COLOR_SLOTS],
        }
    }
}

impl PaneColors {
    /// Set one slot. Out-of-contract indices are ignored rather than clamped:
    /// the parser only produces 0..=258, and a wider value must not silently
    /// land on another colour.
    pub fn set(&mut self, index: u16, rgb: [u8; 3]) -> bool {
        let Some(slot) = self.slots.get_mut(index as usize) else {
            return false;
        };
        let changed = *slot != Some(rgb);
        *slot = Some(rgb);
        changed
    }

    /// Remove one override. `OSC 104` resets indexed entries, `OSC 110/111/112`
    /// the named defaults.
    pub fn reset(&mut self, index: u16) -> bool {
        let Some(slot) = self.slots.get_mut(index as usize) else {
            return false;
        };
        slot.take().is_some()
    }

    /// `OSC 104` with no arguments: every indexed slot back to the theme.
    pub fn reset_indexed(&mut self) -> bool {
        let mut changed = false;
        for slot in &mut self.slots[..INDEXED_SLOTS] {
            changed |= slot.take().is_some();
        }
        changed
    }

    pub fn get(&self, index: u16) -> Option<[u8; 3]> {
        self.slots.get(index as usize).copied().flatten()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(Option::is_none)
    }

    pub fn iter_set(&self) -> impl Iterator<Item = (u16, [u8; 3])> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| slot.map(|rgb| (index as u16, rgb)))
    }

    /// The sparse form used by the live-handoff record.
    pub fn sparse(&self) -> Vec<(u16, [u8; 3])> {
        self.iter_set().collect()
    }

    pub fn from_sparse(entries: impl IntoIterator<Item = (u16, [u8; 3])>) -> Self {
        let mut colors = Self::default();
        for (index, rgb) in entries {
            colors.set(index, rgb);
        }
        colors
    }

    pub fn apply(&mut self, op: ColorOp) {
        match op {
            ColorOp::Set { index, rgb } => {
                self.set(index, rgb);
            }
            ColorOp::Reset { index } => {
                self.reset(index);
            }
            ColorOp::ResetIndexed => {
                self.reset_indexed();
            }
        }
    }

    /// The operations that turn `self` into `newer`. A wholesale indexed clear
    /// is sent as one op so `OSC 104` cannot cost 256 frames' worth of bytes.
    pub fn diff(&self, newer: &Self) -> Vec<ColorOp> {
        let indexed_cleared = (0..INDEXED_SLOTS).all(|index| newer.slots[index].is_none())
            && (0..INDEXED_SLOTS).any(|index| self.slots[index].is_some());
        let mut ops = Vec::new();
        if indexed_cleared {
            ops.push(ColorOp::ResetIndexed);
        }
        for index in 0..COLOR_SLOTS {
            if index < INDEXED_SLOTS && indexed_cleared {
                continue;
            }
            match (self.slots[index], newer.slots[index]) {
                (Some(old), Some(new)) if old == new => {}
                (_, Some(rgb)) => ops.push(ColorOp::Set {
                    index: index as u16,
                    rgb,
                }),
                (Some(_), None) => ops.push(ColorOp::Reset {
                    index: index as u16,
                }),
                (None, None) => {}
            }
        }
        ops
    }
}

/// One ordered change to the override layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorOp {
    Set { index: u16, rgb: [u8; 3] },
    Reset { index: u16 },
    ResetIndexed,
}

/// An ordered colour projection: a full snapshot for a fresh consumer, or the
/// deltas produced by live output. Encoded outside JSON so its size is exact
/// and charged to the pane's output gate like every other byte.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColorFrame {
    snapshot: bool,
    ops: Vec<ColorOp>,
}

impl ColorFrame {
    pub fn snapshot(colors: &PaneColors) -> Self {
        Self {
            snapshot: true,
            ops: colors
                .iter_set()
                .map(|(index, rgb)| ColorOp::Set { index, rgb })
                .collect(),
        }
    }

    pub fn delta(ops: Vec<ColorOp>) -> Self {
        Self {
            snapshot: false,
            ops,
        }
    }

    pub fn is_snapshot(&self) -> bool {
        self.snapshot
    }

    pub fn ops(&self) -> &[ColorOp] {
        &self.ops
    }

    pub fn apply(&self, colors: &mut PaneColors) {
        if self.snapshot {
            *colors = PaneColors::default();
        }
        for op in &self.ops {
            colors.apply(*op);
        }
    }

    /// `[format, mode, count u16 le, ops…]`. At most 260 operations, so the
    /// frame cannot exceed 1.6 KiB however much a program writes.
    pub fn encode(&self) -> Vec<u8> {
        let count = self.ops.len().min(MAX_OPS);
        let mut out = Vec::with_capacity(4 + count * 6);
        out.push(FORMAT);
        out.push(if self.snapshot {
            MODE_SNAPSHOT
        } else {
            MODE_DELTA
        });
        out.extend_from_slice(&(count as u16).to_le_bytes());
        for op in self.ops.iter().take(count) {
            match *op {
                ColorOp::Set { index, rgb } => {
                    out.push(TAG_SET);
                    out.extend_from_slice(&index.to_le_bytes());
                    out.extend_from_slice(&rgb);
                }
                ColorOp::Reset { index } => {
                    out.push(TAG_RESET);
                    out.extend_from_slice(&index.to_le_bytes());
                }
                ColorOp::ResetIndexed => out.push(TAG_RESET_INDEXED),
            }
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let (&format, rest) = bytes.split_first()?;
        if format != FORMAT {
            return None;
        }
        let (&mode, rest) = rest.split_first()?;
        if mode != MODE_DELTA && mode != MODE_SNAPSHOT {
            return None;
        }
        if rest.len() < 2 {
            return None;
        }
        let count = usize::from(u16::from_le_bytes([rest[0], rest[1]]));
        if count > MAX_OPS {
            return None;
        }
        let mut rest = &rest[2..];
        let mut ops = Vec::with_capacity(count);
        for _ in 0..count {
            let (&tag, tail) = rest.split_first()?;
            match tag {
                TAG_SET => {
                    if tail.len() < 5 {
                        return None;
                    }
                    let index = u16::from_le_bytes([tail[0], tail[1]]);
                    if usize::from(index) >= COLOR_SLOTS {
                        return None;
                    }
                    ops.push(ColorOp::Set {
                        index,
                        rgb: [tail[2], tail[3], tail[4]],
                    });
                    rest = &tail[5..];
                }
                TAG_RESET => {
                    if tail.len() < 2 {
                        return None;
                    }
                    let index = u16::from_le_bytes([tail[0], tail[1]]);
                    if usize::from(index) >= COLOR_SLOTS {
                        return None;
                    }
                    ops.push(ColorOp::Reset { index });
                    rest = &tail[2..];
                }
                TAG_RESET_INDEXED => {
                    ops.push(ColorOp::ResetIndexed);
                    rest = tail;
                }
                _ => return None,
            }
        }
        if !rest.is_empty() {
            return None;
        }
        Some(Self {
            snapshot: mode == MODE_SNAPSHOT,
            ops,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(r: u8, g: u8, b: u8) -> [u8; 3] {
        [r, g, b]
    }

    #[test]
    fn an_out_of_contract_index_is_ignored_not_clamped() {
        let mut colors = PaneColors::default();
        assert!(!colors.set(COLOR_SLOTS as u16, rgb(1, 2, 3)));
        assert!(!colors.set(u16::MAX, rgb(1, 2, 3)));
        assert!(colors.is_empty());
        assert_eq!(colors.get(DEFAULT_FG), None);
        assert!(colors.set(DEFAULT_FG, rgb(1, 2, 3)));
        assert_eq!(colors.get(DEFAULT_FG), Some(rgb(1, 2, 3)));
        assert!(colors.reset(DEFAULT_FG));
        assert!(!colors.reset(DEFAULT_FG));
    }

    #[test]
    fn a_diff_is_the_minimal_ordered_change_and_compresses_a_full_clear() {
        let mut before = PaneColors::default();
        before.set(1, rgb(1, 1, 1));
        before.set(2, rgb(2, 2, 2));
        before.set(DEFAULT_BG, rgb(3, 3, 3));
        let mut after = before;
        after.set(1, rgb(9, 9, 9));
        after.reset(2);
        assert_eq!(
            before.diff(&after),
            vec![
                ColorOp::Set {
                    index: 1,
                    rgb: rgb(9, 9, 9)
                },
                ColorOp::Reset { index: 2 },
            ]
        );

        let mut cleared = after;
        cleared.reset_indexed();
        let ops = after.diff(&cleared);
        assert_eq!(ops.first(), Some(&ColorOp::ResetIndexed));
        assert!(
            !ops.iter()
                .any(|op| matches!(op, ColorOp::Reset { index } if *index < INDEXED_SLOTS as u16)),
            "a wholesale clear must not be spelled as 256 resets"
        );
        assert_eq!(
            ops.last(),
            Some(&ColorOp::Set {
                index: DEFAULT_BG,
                rgb: rgb(3, 3, 3)
            }),
            "the named defaults are not part of OSC 104"
        );
    }

    #[test]
    fn a_snapshot_frame_round_trips_and_replaces_older_state() {
        let mut colors = PaneColors::default();
        colors.set(4, rgb(0x11, 0x22, 0x33));
        colors.set(DEFAULT_FG, rgb(0xaa, 0xbb, 0xcc));
        let frame = ColorFrame::snapshot(&colors);
        let decoded = ColorFrame::decode(&frame.encode()).expect("decode");
        assert!(decoded.is_snapshot());
        assert_eq!(decoded, frame);

        let mut target = PaneColors::default();
        target.set(7, rgb(1, 2, 3));
        target.set(CURSOR, rgb(4, 5, 6));
        decoded.apply(&mut target);
        assert_eq!(target, colors, "a snapshot replaces stale slots");
    }

    #[test]
    fn a_delta_frame_round_trips_in_order() {
        let frame = ColorFrame::delta(vec![
            ColorOp::Set {
                index: 1,
                rgb: rgb(1, 2, 3),
            },
            ColorOp::Reset { index: 1 },
            ColorOp::Set {
                index: DEFAULT_BG,
                rgb: rgb(4, 5, 6),
            },
            ColorOp::ResetIndexed,
        ]);
        let decoded = ColorFrame::decode(&frame.encode()).expect("decode");
        assert!(!decoded.is_snapshot());
        assert_eq!(decoded, frame);
        let mut colors = PaneColors::default();
        decoded.apply(&mut colors);
        assert_eq!(colors.get(1), None);
        assert_eq!(colors.get(DEFAULT_BG), Some(rgb(4, 5, 6)));
    }

    #[test]
    fn malformed_frames_are_refused_before_they_touch_state() {
        let mut bytes = ColorFrame::delta(vec![ColorOp::Set {
            index: 1,
            rgb: rgb(1, 2, 3),
        }])
        .encode();
        assert!(ColorFrame::decode(&bytes).is_some());
        bytes[0] = 9;
        assert_eq!(ColorFrame::decode(&bytes), None, "unknown format");
        let mut bytes = ColorFrame::delta(vec![]).encode();
        bytes[1] = 7;
        assert_eq!(ColorFrame::decode(&bytes), None, "unknown mode");
        let mut bytes = ColorFrame::delta(vec![]).encode();
        bytes[2..4].copy_from_slice(&(MAX_OPS as u16 + 1).to_le_bytes());
        assert_eq!(ColorFrame::decode(&bytes), None, "op count over the bound");
        let mut bytes = ColorFrame::delta(vec![ColorOp::Set {
            index: 1,
            rgb: rgb(1, 2, 3),
        }])
        .encode();
        bytes[4] = 0xff;
        assert_eq!(ColorFrame::decode(&bytes), None, "unknown op tag");
        let mut bytes = ColorFrame::delta(vec![ColorOp::Set {
            index: 1,
            rgb: rgb(1, 2, 3),
        }])
        .encode();
        bytes.pop();
        assert_eq!(ColorFrame::decode(&bytes), None, "truncated op");
        let mut bytes = ColorFrame::delta(vec![ColorOp::Set {
            index: COLOR_SLOTS as u16,
            rgb: rgb(1, 2, 3),
        }])
        .encode();
        // Re-encode accepts it (set ignores), but decode must refuse the
        // out-of-contract slot rather than map it onto another colour.
        assert!(ColorFrame::decode(&bytes).is_none());
        bytes[4] = TAG_RESET;
        assert!(ColorFrame::decode(&bytes).is_none());
    }

    #[test]
    fn a_frame_never_exceeds_its_bound() {
        let mut colors = PaneColors::default();
        for index in 0..COLOR_SLOTS as u16 {
            colors.set(index, rgb(1, 2, 3));
        }
        let snapshot = ColorFrame::snapshot(&colors).encode();
        assert_eq!(snapshot.len(), 4 + COLOR_SLOTS * 6);
        assert!(snapshot.len() < 2048);
        let mut clear = colors;
        clear.reset_indexed();
        let delta = ColorFrame::delta(colors.diff(&clear)).encode();
        assert!(delta.len() <= 4 + MAX_OPS * 6);
        assert!(
            delta.len() < snapshot.len(),
            "a full indexed reset must be smaller than the state it clears"
        );
    }
}
