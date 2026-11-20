//! The editor's layout frame + shaping caches: the wrapped lines and per-line
//! geometry from the last paint, the scroll handle + viewport, the shaping
//! memo/caches, and the paint generation counters. Everything here is
//! derived presentation state — invalidated together on any content or
//! geometry change (see [`FrameState::invalidate_geometry`]) and repopulated
//! by the next measure/prepaint/paint pass.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui_kit::{Bounds, Font, Pixels, ScrollHandle, WrappedLine, px};

use crate::{RtlRow, ScanData, ShapeCaches, ShapeMemo, TableRow, WrapAffinity};

/// Layout frame + shaping caches for one [`EditorState`](crate::EditorState)
/// presentation. All fields are derived from the document + viewport; none is
/// an editing authority.
pub(crate) struct FrameState {
    /// Widest laid-out line from the last layout pass, excluding document
    /// padding. With soft wrap off this is the horizontal scroll extent; the
    /// renderer gives the scroll content this minimum width so unwrapped lines
    /// can be read by scrolling instead of being clipped.
    pub(crate) natural_width: Cell<Pixels>,
    pub(crate) scroll: ScrollHandle,
    /// Real viewport bounds published by the host; the editor's scroll handle
    /// spans content height, so a scrollbar cannot infer the visible extent.
    pub(crate) scroll_viewport: Bounds<Pixels>,
    pub(crate) scrollbar_dragging: bool,
    /// Last paint's wrapped lines (one per logical line) and each line's top
    /// offset relative to the editor's top — both used for hit-testing and
    /// cursor/IME positioning.
    pub(crate) wrapped: Vec<WrappedLine>,
    pub(crate) line_tops: Vec<Pixels>,
    /// Per-logical-line wrap-row count (from the last paint). Geometry reads
    /// this, not `wrapped[i].wrap_boundaries()` — a windowed-out line's entry
    /// in `wrapped` is an empty placeholder.
    pub(crate) wrap_rows: Vec<usize>,
    /// Per-logical-line wrap-row height. Variable so a heading (bigger font)
    /// gets a taller row (W2); `line_height` is the base/fallback for the
    /// empty doc and any row without a recorded height.
    pub(crate) line_heights: Vec<Pixels>,
    /// Per-logical-line table-grid row (from the last paint), so a click /
    /// Tab / caret hit-tests against cells instead of the raw source line.
    pub(crate) table_rows: Vec<Option<TableRow>>,
    /// Per-logical-line flag: this row is painted as an inline image (W4), so
    /// a click on it places the caret at the line start instead of
    /// hit-testing source text. From the last paint.
    pub(crate) widget_rows: Vec<bool>,
    /// Per-logical-line display→source byte map for rows with hidden markers
    /// (W6); `None` when the painted text equals the source. From the last
    /// paint.
    pub(crate) offset_maps: Vec<Option<Rc<Vec<usize>>>>,
    /// Per-logical-line horizontal text inset (and so the caret/selection/
    /// hit-test inset): non-zero for fenced code blocks and gutter marks
    /// (blockquotes, lists). From the last paint.
    pub(crate) line_insets: Vec<Pixels>,
    /// Per-logical-line right-to-left geometry (#66), `None` on every LTR row
    /// so an LTR document pays nothing. From the last paint. See [`RtlRow`].
    pub(crate) rtl_rows: Vec<Option<RtlRow>>,
    pub(crate) last_bounds: Option<Bounds<Pixels>>,
    pub(crate) line_height: Pixels,
    /// Font size from the last paint. Hit-testing that runs during event
    /// dispatch (e.g. table-cell clicks) must measure at this size — the
    /// window's text-style stack is unwound there, so `window.text_style()`
    /// would report the root size, not the host wrapper's.
    pub(crate) font_size: Pixels,
    /// The font of the last paint, for the same reason: event-time
    /// `text_style()` reports the ROOT font, whose family/metrics can differ
    /// from what the table was painted with (column auto-fit measured with
    /// the wrong glyph widths and wrapped cells).
    pub(crate) paint_font: Option<Font>,
    /// See [`ShapeMemo`] — `RefCell` because the measure closure holds only a
    /// read borrow of the editor.
    pub(crate) shape_memo: RefCell<Option<ShapeMemo>>,
    /// See [`ScanData`].
    pub(crate) scan_cache: RefCell<Option<(u64, Rc<ScanData>)>>,
    /// Cross-frame shaping caches — line runs, table column widths, and table
    /// wrap rows (see [`ShapeCaches`]). Capacity-capped in `shape_document`.
    pub(crate) shape_caches: ShapeCaches,
    /// The shaping window (element-local y, quantized), set after each
    /// prepaint from the painted bounds — one frame stale by design, so the
    /// measure pass and prepaint always shape with the SAME band and the
    /// measure→prepaint memo keeps hitting.
    pub(crate) shape_band: Cell<Option<(f32, f32)>>,
    /// Latch: the scroll compensator fired since the last paint. Measure can
    /// run several times before a paint commits fresh `line_tops`; without
    /// this, one async height change compensates once per measure call.
    pub(crate) compensated: Cell<bool>,
    /// `content_gen` as of the last paint — a measure with the SAME generation
    /// but different heights means an async (non-edit) height change, the
    /// scroll-anchoring trigger.
    pub(crate) last_paint_gen: u64,
    /// Bumped on every content mutation — cheap staleness key for caches
    /// (the UTF-16 conversion anchor below; a shape cache later).
    pub(crate) content_gen: u64,
    /// Resume point for UTF-8↔UTF-16 conversion: `(generation, utf8, utf16)`
    /// of the last converted offset. IME composition fires conversions many
    /// times per keystroke, clustered near the caret — resuming from the
    /// anchor makes them O(distance) instead of O(document) (CJK latency
    /// grew with note size; found auditing against Cditor's per-block IME).
    pub(crate) utf16_anchor: Cell<(u64, usize, usize)>,
    pub(crate) revealed_caret: Option<(u64, usize, WrapAffinity)>,
    /// Only local navigation/editing requests reveal; observing another view
    /// never does.
    pub(crate) reveal_requested: bool,
}

impl FrameState {
    pub(crate) fn new(scroll: ScrollHandle, content_gen: u64) -> Self {
        Self {
            natural_width: Cell::new(px(0.)),
            scroll,
            scroll_viewport: Bounds::default(),
            scrollbar_dragging: false,
            wrapped: Vec::new(),
            line_tops: Vec::new(),
            wrap_rows: Vec::new(),
            line_heights: Vec::new(),
            table_rows: Vec::new(),
            widget_rows: Vec::new(),
            offset_maps: Vec::new(),
            line_insets: Vec::new(),
            rtl_rows: Vec::new(),
            last_bounds: None,
            line_height: px(20.),
            font_size: px(16.),
            paint_font: None,
            shape_memo: RefCell::new(None),
            scan_cache: RefCell::new(None),
            shape_caches: ShapeCaches::default(),
            shape_band: Cell::new(None),
            compensated: Cell::new(false),
            last_paint_gen: 0,
            content_gen,
            utf16_anchor: Cell::new((0, 0, 0)),
            revealed_caret: None,
            reveal_requested: false,
        }
    }

    /// Drop every geometry/shaping derivative of the last paint. Called on
    /// any content mutation or geometry-affecting config change; the next
    /// measure/prepaint/paint pass rebuilds them.
    pub(crate) fn invalidate_geometry(&mut self) {
        self.revealed_caret = None;
        self.last_bounds = None;
        self.wrapped.clear();
        self.line_tops.clear();
        self.table_rows.clear();
        self.offset_maps.clear();
        self.rtl_rows.clear();
        *self.shape_memo.borrow_mut() = None;
        *self.scan_cache.borrow_mut() = None;
        self.utf16_anchor.set((self.content_gen, 0, 0));
    }

    /// Ask the next paint to scroll the caret into view (a local
    /// navigation/edit), and cancel any in-flight reveal keyed to the stale
    /// geometry.
    pub(crate) fn request_caret_reveal(&mut self) {
        self.reveal_requested = true;
        self.revealed_caret = None;
    }
}
