//! 负责有界回放字节、几何分段与截断处的终端模式，不编排 pane 会话状态。

use std::collections::VecDeque;

use crate::core::term_modes::TerminalModes;
use crate::daemon::protocol::{DaemonMsg, WinSize};

pub(super) const RING_CAP: usize = 8 * 1024 * 1024;
const MAX_RING_SEGMENTS: usize = 64;

pub(super) struct ReplayRing {
    segments: VecDeque<RingSegment>,
    len: usize,
    /// Bytes ever appended, never reset — the mark the scrollback writer uses to
    /// tell a ring that has moved from one that has not. Comparing lengths would
    /// not do it: a ring at its cap stays exactly `RING_CAP` long no matter how
    /// much output flows through it, which is precisely the busy pane whose
    /// snapshot is most stale.
    appended: u64,
    /// Where the front of the ring stands in the escape grammar: every
    /// evicted byte is folded through it, so it is the state a parser that had
    /// read the whole stream would be in at the first byte still held.
    head: HeadCut,
    /// The terminal state at the same point: every evicted byte is folded
    /// through this too, so it holds what a client replaying the ring has to
    /// be put in before the first byte still held. See
    /// [`TerminalModes::replay_prelude`].
    head_modes: TerminalModes,
}

struct RingSegment {
    size: WinSize,
    bytes: VecDeque<u8>,
}

impl RingSegment {
    fn empty(size: WinSize) -> Self {
        Self {
            size,
            bytes: VecDeque::new(),
        }
    }

    fn to_vec(&self) -> Vec<u8> {
        let (a, b) = self.bytes.as_slices();
        let mut out = Vec::with_capacity(self.bytes.len());
        out.extend_from_slice(a);
        out.extend_from_slice(b);
        out
    }
}

/// Just enough of the VT escape grammar to tell whether a byte stands inside a
/// sequence — the state transitions `vte` makes, minus everything it
/// dispatches. A string (OSC, DCS, SOS, PM, APC) ends at an `ESC`, which also
/// opens whatever follows it (a `\` makes that ST); an OSC also ends at BEL;
/// CAN and SUB abort anything.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
enum HeadCut {
    #[default]
    Ground,
    Esc,
    EscIntermediate,
    Csi,
    Str {
        osc: bool,
    },
}

impl HeadCut {
    fn step(&mut self, b: u8) {
        *self = match (*self, b) {
            (_, 0x18 | 0x1a) => Self::Ground,
            (_, 0x1b) => Self::Esc,
            (Self::Ground, _) => Self::Ground,
            (Self::Esc, b'[') => Self::Csi,
            (Self::Esc, b']') => Self::Str { osc: true },
            (Self::Esc, b'P' | b'X' | b'^' | b'_') => Self::Str { osc: false },
            (Self::Esc | Self::EscIntermediate, 0x20..=0x2f) => Self::EscIntermediate,
            // C0 controls execute without ending the sequence, as in `vte`.
            (state @ (Self::Esc | Self::EscIntermediate | Self::Csi), 0x00..=0x1f | 0x7f) => state,
            (Self::Esc | Self::EscIntermediate, _) => Self::Ground,
            (Self::Csi, 0x40..=0x7e) => Self::Ground,
            (Self::Csi, _) => Self::Csi,
            (Self::Str { osc: true }, 0x07) => Self::Ground,
            (state @ Self::Str { .. }, _) => state,
        };
    }

    fn fold(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.step(b);
        }
    }

    /// Whether a replay could start at `next`: outside every sequence, and not
    /// on a UTF-8 continuation byte, which never begins a character.
    fn at_boundary(self, next: u8) -> bool {
        self == Self::Ground && !(0x80..=0xbf).contains(&next)
    }
}

impl ReplayRing {
    /// 自创建以来实际追加的字节标记，不计入恢复时提供的种子。
    pub(super) fn appended_mark(&self) -> u64 {
        self.appended
    }

    /// 仅序列化已逐出字节留下的模式，供进程接管沿用原有格式。
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(super) fn head_restore_bytes(&self) -> Option<Vec<u8>> {
        self.head_modes.restore_bytes()
    }

    /// 只补齐保留字节无法自行恢复的终端模式。
    pub(super) fn replay_prelude(&self, modes: &TerminalModes) -> Option<Vec<u8>> {
        modes.replay_prelude(&self.head_modes, &self.modes())
    }

    pub(super) fn new(size: WinSize) -> Self {
        Self {
            segments: VecDeque::from([RingSegment::empty(size)]),
            len: 0,
            appended: 0,
            head: HeadCut::default(),
            head_modes: TerminalModes::new(),
        }
    }

    /// A ring that starts with output some earlier pane produced.
    ///
    /// Used when a pane is restored from disk: the saved segments go in ahead
    /// of anything the new shell writes, so a client attaching sees the screen
    /// it lost and then the new prompt below it. The seeded bytes are counted
    /// into `len` — they are subject to the same cap as live output, and the
    /// cap is far above what a snapshot can hold — but not into `appended`,
    /// which exists to answer "has this pane produced anything since the last
    /// snapshot?" and would otherwise answer yes for a pane that never ran.
    pub(super) fn seeded(segments: Vec<crate::daemon::scrollback::Segment>, size: WinSize) -> Self {
        let mut ring = Self::new(size);
        if segments.is_empty() {
            return ring;
        }
        ring.segments.clear();
        for seg in segments {
            ring.len += seg.bytes.len();
            ring.segments.push_back(RingSegment {
                size: seg.size,
                bytes: seg.bytes.into(),
            });
        }
        // Whatever the shell writes from here was written at *this* pane's
        // size, which is not necessarily the size the snapshot was taken at.
        ring.resize(size);
        ring
    }

    /// A ring carried across a handoff, with the pane's modes folded back
    /// from it: `head` is the carried [`Self::head_modes`], and the pane's
    /// modes are that plus every byte the ring still holds.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(super) fn adopted(
        segments: Vec<crate::daemon::scrollback::Segment>,
        size: WinSize,
        head: Option<&[u8]>,
    ) -> (Self, TerminalModes) {
        let mut ring = Self::seeded(segments, size);
        if let Some(bytes) = head {
            ring.head_modes.feed(bytes);
        }
        let modes = ring.modes_from(ring.head_modes.clone());
        (ring, modes)
    }

    /// The geometry new output would be recorded at.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(super) fn tail_size(&self) -> WinSize {
        self.segments
            .back()
            .map(|seg| seg.size)
            .expect("ring always has a tail")
    }

    /// Full live history for Unix process handoff, not disk persistence.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(super) fn snapshot(&self) -> Vec<crate::daemon::scrollback::Segment> {
        self.segments
            .iter()
            .filter(|seg| !seg.bytes.is_empty())
            .map(|seg| crate::daemon::scrollback::Segment {
                size: seg.size,
                bytes: seg.to_vec(),
            })
            .collect()
    }

    /// Copy the same byte suffix as `snapshot` followed by `trim_to`, without
    /// allocating the discarded live history. Storage deliberately keeps its
    /// existing raw-byte cut; live replay's escape/UTF-8 boundary is unchanged.
    pub(super) fn persistence_snapshot(&self) -> Vec<crate::daemon::scrollback::Segment> {
        let mut skip = self
            .len
            .saturating_sub(crate::daemon::scrollback::SNAPSHOT_CAP);
        let mut segments = Vec::new();
        for seg in &self.segments {
            if seg.bytes.is_empty() {
                continue;
            }
            if skip >= seg.bytes.len() {
                skip -= seg.bytes.len();
                continue;
            }
            let (a, b) = seg.bytes.as_slices();
            let mut bytes = Vec::with_capacity(seg.bytes.len() - skip);
            bytes.extend_from_slice(&a[skip.min(a.len())..]);
            bytes.extend_from_slice(&b[skip.saturating_sub(a.len())..]);
            segments.push(crate::daemon::scrollback::Segment {
                size: seg.size,
                bytes,
            });
            skip = 0;
        }
        segments
    }

    fn tail(&mut self) -> &mut RingSegment {
        self.segments.back_mut().expect("ring always has a tail")
    }

    /// The size the pane is at now: the newest segment's.
    pub(super) fn size(&self) -> WinSize {
        self.segments.back().expect("ring always has a tail").size
    }

    pub(super) fn resize(&mut self, size: WinSize) {
        let tail = self.tail();
        if tail.size == size {
            return;
        }
        if tail.bytes.is_empty() {
            tail.size = size;
            return;
        }
        if self.segments.len() >= MAX_RING_SEGMENTS {
            let old = self.segments.pop_front().expect("len >= cap");
            let head = self.segments.front_mut().expect("cap >= 2");
            let mut merged = old.bytes;
            merged.extend(head.bytes.drain(..));
            head.bytes = merged;
        }
        self.segments.push_back(RingSegment::empty(size));
    }

    pub(super) fn append(&mut self, bytes: &[u8]) {
        self.appended = self.appended.saturating_add(bytes.len() as u64);
        if bytes.len() >= RING_CAP {
            let size = self.tail().size;
            // Everything held goes, and so does the front of `bytes`; both
            // still pass through `head` so it knows where the kept part begins.
            for seg in self.segments.drain(..) {
                let (a, b) = seg.bytes.as_slices();
                self.head.fold(a);
                self.head.fold(b);
                self.head_modes.feed(a);
                self.head_modes.feed(b);
            }
            let kept = bytes.len() - RING_CAP;
            self.head.fold(&bytes[..kept]);
            self.head_modes.feed(&bytes[..kept]);
            let mut tail = RingSegment::empty(size);
            tail.bytes.extend(&bytes[kept..]);
            self.segments.push_back(tail);
            self.len = RING_CAP;
            self.evict(0);
            return;
        }
        self.tail().bytes.extend(bytes);
        self.len += bytes.len();
        let overflow = self.len.saturating_sub(RING_CAP);
        if overflow > 0 {
            self.evict(overflow);
        }
    }

    /// Drop `n` bytes from the front, then keep dropping until the front is a
    /// place a parser can start from (#857).
    ///
    /// The cap alone falls wherever it falls — through an OSC, a CSI, or the
    /// middle of a three-byte CJK character — and a replay starts the client's
    /// emulator in its ground state, so the tail of a sequence was painted as
    /// text (`33;C` out of `ESC ] 133;C BEL`) and half a character as a
    /// replacement glyph. Moving on to the end of the sequence costs nothing
    /// visible: the client would not have painted any of it.
    fn evict(&mut self, mut n: usize) {
        loop {
            let head = &mut self.head;
            let Some(front) = self.segments.front_mut() else {
                return;
            };
            let mut take = 0;
            for &b in front.bytes.iter() {
                if n == 0 && head.at_boundary(b) {
                    break;
                }
                head.step(b);
                n = n.saturating_sub(1);
                take += 1;
            }
            let (a, b) = front.bytes.as_slices();
            let in_a = take.min(a.len());
            self.head_modes.feed(&a[..in_a]);
            self.head_modes.feed(&b[..take - in_a]);
            front.bytes.drain(..take);
            self.len -= take;
            if !front.bytes.is_empty() || self.segments.len() == 1 {
                return;
            }
            self.segments.pop_front();
        }
    }

    pub(super) fn replay(&self, send: &mut impl FnMut(DaemonMsg)) {
        for seg in &self.segments {
            send(DaemonMsg::Size(seg.size));
            send(DaemonMsg::Snapshot(seg.to_vec()));
        }
    }

    /// The modes a replay of this ring switches on by itself — the same fold
    /// the pane keeps, over the bytes that are actually left.
    ///
    /// Folded rather than remembered per mode because the front of the ring
    /// moves: a `?1049h` the cap fell through is evicted whole (see
    /// [`Self::evict`]), and the answer here has to be the one the client's
    /// emulator will reach from what is left.
    pub(super) fn modes(&self) -> TerminalModes {
        self.modes_from(TerminalModes::new())
    }

    /// `start` with every byte the ring holds folded into it.
    fn modes_from(&self, mut modes: TerminalModes) -> TerminalModes {
        for seg in &self.segments {
            // Both halves of the deque, in order: `feed` carries a sequence
            // across calls, so the split is invisible to the fold.
            let (a, b) = seg.bytes.as_slices();
            modes.feed(a);
            modes.feed(b);
        }
        modes
    }

    #[cfg(test)]
    pub(super) fn flatten(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.len);
        for seg in &self.segments {
            out.extend(seg.to_vec());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn ws(cols: u16, rows: u16) -> WinSize {
        WinSize {
            cols,
            rows,
            cell_w: 8,
            cell_h: 16,
        }
    }

    #[test]
    fn ring_under_cap_keeps_all() {
        let mut ring = ReplayRing::new(ws(80, 24));
        ring.append(b"hello ");
        ring.append(b"world");
        assert_eq!(ring.flatten(), b"hello world");
    }

    #[test]
    fn ring_over_cap_drops_oldest() {
        let mut ring = ReplayRing::new(ws(80, 24));
        ring.append(&vec![b'a'; RING_CAP]);
        assert_eq!(ring.len, RING_CAP);
        ring.append(&vec![b'b'; 100]);
        assert_eq!(ring.len, RING_CAP);
        let flat = ring.flatten();
        assert_eq!(&flat[..RING_CAP - 100], &vec![b'a'; RING_CAP - 100][..]);
        assert_eq!(&flat[RING_CAP - 100..], &vec![b'b'; 100][..]);
    }

    /// Issue #857's `33;C`: Pi marks every streamed line with
    /// `ESC ] 133;B BEL ESC ] 133;C BEL`, and a pane that streams long enough
    /// has the cap fall through one of them sooner or later. A replay begins
    /// in the client's ground state, so whatever tail the cut left in front
    /// was painted as text — here `33;C`, next to the line it marked.
    ///
    /// Every cut, at every byte of the line: the head has to land where a
    /// fresh parser can start, which is a sequence's `ESC` or the first byte
    /// of a whole character.
    #[test]
    fn ring_eviction_never_leaves_a_sequence_tail_or_half_a_character_in_front() {
        let line = "\x1b]133;B\x07\x1b]133;C\x07下一步。\r\n".as_bytes();
        // The two marks, then each character (the CJK ones three bytes wide),
        // then the end of the line, where the filler starts.
        let starts = [0, 8, 16, 19, 22, 25, 28, 29, 30];
        assert_eq!(*starts.last().unwrap(), line.len());
        for cut in 0..=line.len() {
            let mut ring = ReplayRing::new(ws(80, 24));
            ring.append(line);
            ring.append(&vec![b'.'; RING_CAP - line.len() + cut]);
            let head = *starts.iter().find(|&&s| s >= cut).unwrap();
            let flat = ring.flatten();
            assert_eq!(
                &flat[..line.len() - head],
                &line[head..],
                "a cut at byte {cut} must move the head on to byte {head}"
            );
            assert_eq!(ring.len, flat.len());
        }
    }

    /// A string sequence the cap falls into is evicted up to its terminator
    /// however far that is — the client would not have shown a byte of it —
    /// and the state carries across appends, so a later eviction still knows
    /// it is inside one.
    #[test]
    fn ring_eviction_carries_an_open_sequence_across_appends() {
        let mut ring = ReplayRing::new(ws(80, 24));
        ring.append(b"\x1b]52;c;");
        ring.append(&vec![b'Q'; RING_CAP - 7]);
        // Cuts two bytes into the OSC: the rest of it goes too.
        ring.append(b"\x07ok");
        assert!(ring.flatten().starts_with(b"ok"), "the payload is gone");
        assert_eq!(ring.len, 2);
    }

    #[test]
    fn ring_giant_chunk_starts_at_a_sequence_boundary() {
        let mut ring = ReplayRing::new(ws(80, 24));
        let mut big = b"\x1b]133;C\x07".to_vec();
        big.extend(vec![b'x'; RING_CAP - 4]);
        ring.append(&big);
        let flat = ring.flatten();
        assert!(flat.iter().all(|&b| b == b'x'), "no `C BEL` left in front");
        assert_eq!(ring.len, RING_CAP - 4);
    }

    #[test]
    fn ring_giant_chunk_keeps_tail() {
        let mut ring = ReplayRing::new(ws(100, 24));
        ring.append(b"seed");
        ring.resize(ws(80, 24));
        let mut big = vec![b'x'; RING_CAP];
        big.extend_from_slice(b"TAIL");
        ring.append(&big);
        assert_eq!(ring.len, RING_CAP);
        assert_eq!(ring.segments.len(), 1);
        assert_eq!(&ring.flatten()[RING_CAP - 4..], b"TAIL");
    }

    #[test]
    fn ring_resize_splits_replay_into_geometry_segments() {
        let mut ring = ReplayRing::new(ws(100, 24));
        ring.append(b"wide bytes");
        ring.resize(ws(80, 24));
        ring.append(b"narrow bytes");

        let (tx, rx) = mpsc::channel();
        ring.replay(&mut |m| {
            let _ = tx.send(m);
        });
        assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Size(s)) if s == ws(100, 24)));
        assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(b)) if b == b"wide bytes"));
        assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Size(s)) if s == ws(80, 24)));
        assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(b)) if b == b"narrow bytes"));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn a_seeded_ring_replays_the_old_screen_at_the_size_it_was_written() {
        use crate::daemon::scrollback::Segment;

        let mut ring = ReplayRing::seeded(
            vec![Segment {
                size: ws(100, 24),
                bytes: b"what the dead pane had on it".to_vec(),
            }],
            ws(80, 30),
        );
        ring.append(b"the new shell's prompt");

        let (tx, rx) = mpsc::channel();
        ring.replay(&mut |m| {
            let _ = tx.send(m);
        });
        assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Size(s)) if s == ws(100, 24)));
        assert!(
            matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(b)) if b == b"what the dead pane had on it"),
            "restored output has to be replayed at the width it was produced at, or the client \
             rewraps it to whatever the window happens to be now"
        );
        assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Size(s)) if s == ws(80, 30)));
        assert!(
            matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(b)) if b == b"the new shell's prompt")
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn seeding_does_not_make_an_untouched_pane_look_like_it_produced_output() {
        use crate::daemon::scrollback::Segment;

        let ring = ReplayRing::seeded(
            vec![Segment {
                size: ws(80, 24),
                bytes: b"restored".to_vec(),
            }],
            ws(80, 24),
        );
        assert_eq!(
            ring.appended_mark(),
            0,
            "the mark answers 'has this pane written anything since the last snapshot', and \
             bytes it was handed at birth are not an answer of yes"
        );
    }

    #[test]
    fn a_handoff_keeps_the_modes_the_ring_dropped_off_its_front() {
        use crate::daemon::scrollback::Segment;

        let mut head = TerminalModes::new();
        head.feed(b"\x1b[?1049h\x1b[?2031h\x1b[>1u");
        let (ring, modes) = ReplayRing::adopted(
            vec![Segment {
                size: ws(80, 24),
                bytes: b"long after startup".to_vec(),
            }],
            ws(80, 24),
            head.restore_bytes().as_deref(),
        );

        assert!(modes.is_on(crate::core::term_modes::COLOR_SCHEME_UPDATES));
        let prelude = ring.replay_prelude(&modes).expect("a prelude");
        let has = |needle: &[u8]| prelude.windows(needle.len()).any(|w| w == needle);
        assert!(has(b"\x1b[?2031h"));
        assert!(
            has(b"\x1b[>1u"),
            "the keyboard flags come back from the head fold, so it has to cross the exec too"
        );
    }

    #[test]
    fn an_empty_seed_leaves_an_ordinary_ring() {
        let mut ring = ReplayRing::seeded(Vec::new(), ws(80, 24));
        ring.append(b"output");
        assert_eq!(
            ring.segments.len(),
            1,
            "the ring always has exactly one tail"
        );
        assert_eq!(ring.flatten(), b"output");
    }

    #[test]
    fn ring_idle_resizes_collapse_and_replay_ends_at_current_size() {
        let mut ring = ReplayRing::new(ws(100, 24));
        ring.append(b"bytes");
        ring.resize(ws(100, 24));
        assert_eq!(ring.segments.len(), 1);
        ring.resize(ws(90, 24));
        ring.resize(ws(80, 30));
        assert_eq!(ring.segments.len(), 2);

        let (tx, rx) = mpsc::channel();
        ring.replay(&mut |m| {
            let _ = tx.send(m);
        });
        assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Size(s)) if s == ws(100, 24)));
        assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(b)) if b == b"bytes"));
        assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Size(s)) if s == ws(80, 30)));
        assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(b)) if b.is_empty()));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn ring_eviction_drops_emptied_segments() {
        let mut ring = ReplayRing::new(ws(100, 24));
        ring.append(b"old");
        ring.resize(ws(80, 24));
        ring.append(&vec![b'n'; RING_CAP - 2]);
        assert_eq!(ring.segments.len(), 2, "two bytes of the old segment left");
        ring.append(b"nn");
        assert_eq!(ring.segments.len(), 1, "the emptied old segment is gone");
        assert_eq!(ring.len, RING_CAP);

        let (tx, rx) = mpsc::channel();
        ring.replay(&mut |m| {
            let _ = tx.send(m);
        });
        assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Size(s)) if s == ws(80, 24)));
    }

    #[test]
    fn ring_caps_segment_count_by_merging_oldest() {
        let mut ring = ReplayRing::new(ws(100, 24));
        let rounds = MAX_RING_SEGMENTS + 10;
        for i in 0..rounds {
            ring.append(format!("seg{i:02} ").as_bytes());
            ring.resize(ws(101 + i as u16, 24));
        }
        assert_eq!(ring.segments.len(), MAX_RING_SEGMENTS);

        let flat = String::from_utf8(ring.flatten()).unwrap();
        let expect: String = (0..rounds).map(|i| format!("seg{i:02} ")).collect();
        assert_eq!(flat, expect);

        let head = ring.segments.front().unwrap();
        assert_eq!(head.size, ws(111, 24));
        assert!(
            String::from_utf8(head.to_vec())
                .unwrap()
                .ends_with("seg11 ")
        );
    }

    #[test]
    fn persisted_suffix_replays_exact_bytes_and_geometry_at_each_cut() {
        use crate::daemon::scrollback::{SNAPSHOT_CAP, decode, encode};

        let boundary = "\x1b]133;C\x07下一步\x1b[31mend\x1b[0m".as_bytes();
        for cut in 0..=boundary.len() {
            let mut ring = ReplayRing::new(ws(100, 24));
            // Force the storage cap (not the live replay cap) through every
            // byte of an escape sequence and UTF-8 character.
            ring.append(boundary);
            ring.append(&vec![b'x'; SNAPSHOT_CAP - boundary.len() + cut - 4]);
            ring.resize(ws(71, 30));
            ring.append(b"tail");
            // An empty current-size tail must not become a stored segment.
            ring.resize(ws(90, 40));
            let mark = ring.appended_mark();
            let stored = ring.persistence_snapshot();
            let (saved, title) = decode(&encode(&stored, Some("恢复标题"))).unwrap();
            assert_eq!(title.as_deref(), Some("恢复标题"));
            let restored = ReplayRing::seeded(saved, ws(90, 40));
            let mut messages = Vec::new();
            restored.replay(&mut |msg| messages.push(msg));
            let mut first = boundary[cut..].to_vec();
            first.extend(vec![b'x'; SNAPSHOT_CAP - boundary.len() + cut - 4]);
            assert!(matches!(&messages[0], DaemonMsg::Size(s) if *s == ws(100, 24)));
            assert!(
                matches!(&messages[1], DaemonMsg::Snapshot(b) if *b == first),
                "cut {cut}"
            );
            assert!(matches!(&messages[2], DaemonMsg::Size(s) if *s == ws(71, 30)));
            assert!(matches!(&messages[3], DaemonMsg::Snapshot(b) if b == b"tail"));
            assert!(matches!(&messages[4], DaemonMsg::Size(s) if *s == ws(90, 40)));
            assert!(matches!(&messages[5], DaemonMsg::Snapshot(b) if b.is_empty()));
            assert_eq!(messages.len(), 6);
            assert_eq!(
                ring.appended_mark(),
                mark,
                "saving must not consume live output"
            );
            assert!(
                ring.flatten().starts_with(boundary),
                "the live replay must remain complete"
            );
        }
    }

    #[test]
    fn persisted_wrapped_history_keeps_both_deque_slices_in_order() {
        use crate::daemon::scrollback::{SNAPSHOT_CAP, decode, encode};

        let mut ring = ReplayRing::new(ws(80, 24));
        ring.append(&vec![b'a'; RING_CAP]);
        for _ in 0..64 {
            ring.append(&vec![b'c'; SNAPSHOT_CAP / 2]);
        }
        ring.append(&vec![b'd'; SNAPSHOT_CAP / 2]);
        let stored = ring.persistence_snapshot();
        let (saved, _) = decode(&encode(&stored, None)).unwrap();
        let restored = ReplayRing::seeded(saved, ws(80, 24));
        let mut messages = Vec::new();
        restored.replay(&mut |msg| messages.push(msg));
        let mut expected = vec![b'c'; SNAPSHOT_CAP / 2];
        expected.extend(vec![b'd'; SNAPSHOT_CAP / 2]);
        assert!(matches!(&messages[0], DaemonMsg::Size(s) if *s == ws(80, 24)));
        assert!(matches!(&messages[1], DaemonMsg::Snapshot(b) if *b == expected));
        assert_eq!(messages.len(), 2);
    }
}
