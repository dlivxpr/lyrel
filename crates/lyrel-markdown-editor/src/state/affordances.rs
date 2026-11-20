//! The editor's hit map + affordance/drag state: the window-space rects the
//! last paint committed (table hover zones + strips + delete handles,
//! checkboxes, code-card chrome, folds, links, property rows, math formulas),
//! the pointer's hover tracking, and the in-progress drag transitions
//! (image corner-grip resize, table column resize, gutter block reorder,
//! wide-table thumb scroll). Hit-tested on mouse events; rewritten every
//! paint.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use gpui_kit::{Bounds, Pixels, Point, SharedString, px};

use crate::{CodeChipHit, ImageResize, TableColResize, TableThumb};

/// Hit map + affordance/drag state for one
/// [`EditorState`](crate::EditorState) presentation. Everything here is
/// derived from the last paint pass — pointer hit-testing and hover
/// repainting read it; drag transitions mutate it between presses.
pub(crate) struct AffordanceState {
    /// Hover-revealed "+" add-row / add-column strips for each table (issue
    /// #16), each paired with the table row to seat the caret in before
    /// inserting. From the last paint, committed only while the table is
    /// hovered; hit-tested on mouse-down.
    pub(crate) table_row_add_rects: Vec<(Bounds<Pixels>, usize)>,
    pub(crate) table_col_add_rects: Vec<(Bounds<Pixels>, usize, usize)>,
    /// Each table's hover zone (grid + a thin margin) with its header row,
    /// committed every paint so `on_mouse_move` can repaint when the
    /// pointer's table-affordance region changes (the editor otherwise only
    /// repaints on the caret blink) and `on_scroll_wheel` can hit-test the
    /// PAINTED table.
    pub(crate) table_hover_zones: Vec<(Bounds<Pixels>, usize)>,
    /// The affordance region the pointer was last in — `(table index, 0 = zone
    /// / 1 = below strip / 2 = right strip)` — so the repaint fires only on
    /// change.
    pub(crate) table_hover_region: Option<(usize, u8)>,
    /// Committed delete-handle rects (issue #16): the hovered row's "−"
    /// `(bounds, row)` and the hovered column's "−" `(bounds, row, col)`,
    /// hit-tested on click.
    pub(crate) table_row_del: Option<(Bounds<Pixels>, usize)>,
    pub(crate) table_col_del: Option<(Bounds<Pixels>, usize, usize)>,
    /// The table cell `(row, col)` the pointer was last over, so
    /// `on_mouse_move` repaints the delete handles when it changes.
    pub(crate) table_hover_cell: Option<(usize, usize)>,
    /// An in-progress table column-border drag (drag-to-resize, issue #16):
    /// the column resizes live; release persists `cols=` into the table's
    /// marker line. `None` = no drag.
    pub(crate) table_col_resize: Option<TableColResize>,
    /// Last-paint column-resize grip bands: `(band, header row, column, width)`.
    pub(crate) table_col_resize_rects: Vec<(Bounds<Pixels>, usize, usize, f32)>,
    /// The band index the pointer is on (repaint-on-change for its accent
    /// line).
    pub(crate) table_resize_hover: Option<usize>,
    /// Horizontal scroll of each table, keyed by its header row. Columns are
    /// fitted to the note width and wrap, so this is the fallback for a page
    /// too narrow for every column's floor. Keys drift on edits above a
    /// table; entries are clamped at use, so a stale one is a harmless
    /// partial offset. (ponytail: no eviction, the map stays tiny)
    pub(crate) table_scroll_x: HashMap<usize, f32>,
    /// Last-paint wide-table scroll thumbs (padded grab rects), so the thumb
    /// is mouse-draggable, not just an indicator.
    pub(crate) table_thumbs: Vec<TableThumb>,
    /// A live thumb drag: `(header row, grab x, scroll offset at grab)`.
    pub(crate) table_thumb_drag: Option<(usize, Pixels, f32)>,
    /// Per-logical-line `src` for rows painted as a file chip (from the last
    /// paint), so a left-click can open it and a right-click can edit it.
    pub(crate) chip_rows: Vec<Option<(SharedString, bool)>>,
    /// Window-space painted bounds of each inline image, with its logical
    /// line index (from the last paint), so a press near a corner can start a
    /// resize and know which `![](src)` line to rewrite. One entry per
    /// rendered image.
    pub(crate) image_rects: Vec<(usize, Bounds<Pixels>)>,
    /// The in-progress corner-grip drag, if any (see [`ImageResize`]). While
    /// set, that image paints at the live width and other mouse handling is
    /// suppressed.
    pub(crate) image_resize: Option<ImageResize>,
    /// Window-space bounds of each painted task checkbox, with its logical
    /// line — so a click on the box toggles `[ ]`↔`[x]` instead of placing
    /// the caret.
    pub(crate) checkbox_rects: Vec<(usize, Bounds<Pixels>)>,
    /// Painted code-card chrome bounds from the last frame (lang tag + Copy
    /// per code block, keyed by the opening-fence row) — clicks route here
    /// before caret placement.
    pub(crate) code_chip_rects: Vec<CodeChipHit>,
    /// Full card bounds of each code block from the last paint (`(first body
    /// line, rect)`), for hover tracking — the chrome is hover-revealed.
    pub(crate) code_card_rects: Vec<(usize, Bounds<Pixels>)>,
    /// The hovered code block's first body line, if any (chrome shows there).
    pub(crate) code_chip_hover: Option<usize>,
    /// Painted chevron bounds of foldable callouts (`(line, rect)`, from the
    /// last paint) — a click flips the marker's `-`/`+` fold char.
    pub(crate) alert_fold_rects: Vec<(usize, Bounds<Pixels>)>,
    /// An active gutter block drag (Notion/Cditor-style reorder): the grabbed
    /// block's first + last rows and the current drop boundary (a row index;
    /// `== rows` drops at the document end).
    pub(crate) line_drag: Option<(usize, usize, usize)>,
    /// The row whose gutter grip the pointer hovers (mirrors prepaint's
    /// computation) — tracked so hover changes repaint the grip.
    pub(crate) grip_hover_row: Option<usize>,
    /// Extra left offset for the drag grip — the host sets its line-number
    /// gutter's width here so the grip sits beside the numbers, not on them.
    pub(crate) grip_inset: Pixels,
    /// Window-space painted bounds of each inline `$…$` formula + its
    /// absolute byte range and inner LaTeX (from the last paint), so a click
    /// can open its structural editor and the seated editor can be positioned
    /// at the formula's spot.
    pub(crate) inline_math_rects: Vec<(Range<usize>, SharedString, Bounds<Pixels>)>,
    /// Painted bounds + target of each property-panel pill (from the last
    /// paint), so a left-click opens it (`OpenWikiLink` / `OpenLink`).
    pub(crate) prop_pill_rects: Vec<(Bounds<Pixels>, zorite_markdown::syntax::LinkHit)>,
    /// Inline links' painted boxes + targets from the last paint (the same
    /// geometry as the hand-cursor hitboxes), for hover → `HoverLink`.
    pub(crate) link_rects: Vec<(Bounds<Pixels>, zorite_markdown::syntax::LinkHit)>,
    /// The link under the pointer, if any — `HoverLink` fires on change.
    pub(crate) hovered_link: Option<(zorite_markdown::syntax::LinkHit, Bounds<Pixels>)>,
    /// A link the editor itself is about to open on mouse-up (so a drag
    /// doesn't also trigger the host's navigation).
    pub(crate) pending_link: Option<(SharedString, Point<Pixels>, u64)>,
    /// Each painted property-panel row's bounds + its source line (for hover
    /// borders and the right-click property menu).
    pub(crate) prop_row_rects: Vec<(Bounds<Pixels>, usize)>,
    /// The property row the pointer was last over — drives `on_mouse_move`'s
    /// repaint-on-change (like the table hover).
    pub(crate) prop_hover_row: Option<usize>,
    /// Collapsed headings, keyed by the heading's trimmed source line
    /// (`## Goals`). View-local — markdown has no heading-fold syntax (unlike
    /// callouts' `-`/`+`), so folds live for the editor's lifetime and a key
    /// self-heals by vanishing when its heading text is edited.
    pub(crate) folded_headings: HashSet<String>,
    /// Painted chevron bounds of heading folds (`(line, rect)`, from the last
    /// paint) — a click toggles that heading in `folded_headings`.
    pub(crate) heading_fold_rects: Vec<(usize, Bounds<Pixels>)>,
    /// Window-space bounds of every heading's first visual row (from the last
    /// paint) — `on_mouse_move` hit-tests these for the hover chevron.
    pub(crate) heading_row_rects: Vec<(usize, Bounds<Pixels>)>,
    /// The heading line the pointer was last over — its chevron shows on
    /// hover (a fold chevron on every heading would clutter). Drives
    /// `on_mouse_move`'s repaint-on-change, like the property-row hover.
    pub(crate) heading_hover_row: Option<usize>,
}

impl AffordanceState {
    pub(crate) fn new() -> Self {
        Self {
            table_row_add_rects: Vec::new(),
            table_col_add_rects: Vec::new(),
            table_hover_zones: Vec::new(),
            table_hover_region: None,
            table_row_del: None,
            table_col_del: None,
            table_hover_cell: None,
            table_col_resize: None,
            table_col_resize_rects: Vec::new(),
            table_resize_hover: None,
            table_scroll_x: HashMap::new(),
            table_thumbs: Vec::new(),
            table_thumb_drag: None,
            chip_rows: Vec::new(),
            image_rects: Vec::new(),
            image_resize: None,
            checkbox_rects: Vec::new(),
            code_chip_rects: Vec::new(),
            code_card_rects: Vec::new(),
            code_chip_hover: None,
            alert_fold_rects: Vec::new(),
            line_drag: None,
            grip_hover_row: None,
            grip_inset: px(0.),
            inline_math_rects: Vec::new(),
            prop_pill_rects: Vec::new(),
            link_rects: Vec::new(),
            hovered_link: None,
            pending_link: None,
            prop_row_rects: Vec::new(),
            prop_hover_row: None,
            folded_headings: HashSet::new(),
            heading_fold_rects: Vec::new(),
            heading_row_rects: Vec::new(),
            heading_hover_row: None,
        }
    }
}
