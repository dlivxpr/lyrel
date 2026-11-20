//! The prepaint stage: everything inserted into the window BEFORE painting —
//! pointer hitboxes (image grips, checkboxes, chips, folds, links, property
//! pills), the caret/selection/search/IME geometry, the gutter drag grip, and
//! the table hover affordances (zones, thumbs, outlines, resize grips).
//!
//! Split from the former `element.rs` monolith by stage helper; each helper
//! takes this pass's shaped channels by reference (no per-frame cloning) and
//! the [`RangePainter`] centralizes the row-geometry math the selection,
//! caret, and link helpers share (the O(N) row-start array built once per
//! pass — no per-line rescans).

use super::props::prop_pill_bounds;
use super::*;

pub(crate) fn prepaint_editor(
    el: &EditorElement,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) -> PrepaintState {
    let editor = el.editor.read(cx);
    let starts = editor.line_starts();
    // Reveal-on-caret (markers, raw-on-caret widgets, per-construct reveal)
    // applies only while the editor is focused. An unfocused editor — always
    // shown in WYSIWYG mode but not being edited — renders fully, like a
    // reading view. `caret_row = None` + a no-match selection do that.
    let focused = editor.focus_handle.is_focused(window);
    // The active image-resize drag (if any), so a dragged image's grip hitbox
    // tracks its live preview size (copied out — `editor` stays borrowed below).
    let image_resize = editor.affordances.image_resize;
    let style = window.text_style();
    let font = style.font();
    let font_size = style.font_size.to_pixels(window.rem_size());
    let base_lh = font_size * LINE_HEIGHT_RATIO;
    let wrap_width = Some(bounds.size.width);
    let text_color = style.color;

    // Placeholder (uniform) when empty; else shape per line so headings get
    // their own taller rows (W2) and image lines render inline (W4).
    let geometry_row = editor.row_col(editor.cursor_offset()).0;
    let caret_row = focused.then_some(geometry_row);
    let selection = if focused {
        (
            editor.selection.selected_range.start,
            editor.selection.selected_range.end,
        )
    } else {
        (usize::MAX, usize::MAX)
    };
    let sf = window.scale_factor();
    // The measure pass already shaped these exact inputs this frame — use
    // its result instead of shaping the whole document a second time.
    let memo = editor.frame.shape_memo.borrow_mut().take().filter(|m| {
        m.wrap_width == wrap_width
            && m.caret_row == caret_row
            && m.geometry_row == geometry_row
            && m.selection == selection
            && m.font_size == font_size
    });
    let ShapedDoc {
        wrapped,
        heights: line_heights,
        widgets,
        backgrounds,
        tables,
        maps,
        marks,
        inline_maths,
        wrap_rows,
        rtl_rows: rtl_layout,
    } = if let Some(m) = memo {
        m.shaped
    } else if editor.content.is_empty() {
        let w = shape_all(
            window,
            &editor.placeholder,
            font_size,
            font.clone(),
            hsla(0., 0., 0.5, 0.5),
            wrap_width,
        );
        let n = w.len();
        let rows: Vec<usize> = w.iter().map(|l| l.wrap_boundaries().len() + 1).collect();
        ShapedDoc {
            wrapped: w,
            heights: vec![base_lh; n],
            widgets: vec![None; n],
            backgrounds: vec![None; n],
            tables: vec![None; n],
            maps: vec![None; n],
            marks: vec![None; n],
            inline_maths: vec![Vec::new(); n],
            wrap_rows: rows,
            // The placeholder is the host's own string; no RTL layout.
            rtl_rows: (0..n).map(|_| None).collect(),
        }
    } else {
        shape_document(
            window,
            cx,
            &editor.content,
            &font,
            text_color,
            font_size,
            &editor.diagnostics,
            editor.markdown_style.as_ref(),
            wrap_width,
            table_page_width(editor, window),
            editor.soft_wrap,
            caret_row,
            geometry_row,
            editor.block_image.as_ref(),
            editor.block_chip.as_ref(),
            editor.embed_view.as_ref(),
            editor.block_mermaid.as_ref(),
            editor.block_math.as_ref(),
            editor.code_highlight.as_ref(),
            editor.tab_indent,
            editor.block_math_em,
            editor.editing_block.as_ref().map(|eb| {
                let sr = editor.row_col(eb.range.start).0;
                let er = editor
                    .row_col(eb.range.end.saturating_sub(1).max(eb.range.start))
                    .0;
                (sr, er, eb.height)
            }),
            sf,
            selection,
            editor.affordances.image_resize,
            editor.affordances.table_col_resize,
            &editor.scan_data(),
            &editor.frame.shape_caches,
            editor.frame.shape_band.get().map(|(a, b)| (px(a), px(b))),
            &editor.affordances.folded_headings,
        )
    };

    publish_shape_band(editor, &el.editor, bounds, window);
    let line_tops = line_tops_for(&line_heights, &backgrounds, &tables, &wrap_rows);
    let rtl = rtl_row_geometry(bounds.size.width, &backgrounds, &marks, rtl_layout);
    let painter = RangePainter {
        editor,
        starts: &starts,
        wrapped: &wrapped,
        line_heights: &line_heights,
        line_tops: &line_tops,
        bounds,
        maps: &maps,
        tables: &tables,
        backgrounds: &backgrounds,
        marks: &marks,
        rtl: &rtl,
        font: &font,
        font_size,
        base_lh,
    };
    let image_grips = image_grip_hitboxes(
        &widgets,
        image_resize,
        &backgrounds,
        &marks,
        bounds,
        &line_tops,
        window,
    );
    let (checkbox_grips, chip_grips, alert_fold_grips) =
        gutter_pointer_hitboxes(&painter, &widgets, window);
    let (code_chips, code_card_rects) = code_card_chips(&painter, &wrap_rows, window);
    let (heading_chevrons, heading_fold_grips, heading_row_rects) =
        heading_fold_marks(&painter, window);
    let (link_grips, link_rects, inline_image_grips) = link_hitboxes(
        editor,
        &wrapped,
        &widgets,
        &backgrounds,
        &tables,
        &line_heights,
        &maps,
        &inline_maths,
        &starts,
        bounds,
        &line_tops,
        &painter,
        window,
    );
    let prop_pill_grips =
        prop_pill_hitboxes(&widgets, bounds, &line_tops, &font, font_size, window);
    let (grip, grip_hb) = gutter_drag_grip(
        editor,
        bounds,
        &line_tops,
        &line_heights,
        &wrap_rows,
        window,
    );
    let (table_zones, table_thumbs, row_aff, col_aff, caret_cell, col_resize_grips) =
        table_affordances(editor, &tables, &line_tops, &line_heights, bounds, window);
    // Find-match highlights (host-set): the reader's browser-style find
    // colors — soft yellow everywhere, stronger orange on the active
    // match. Behind the text, like the selection.
    let mut search = Vec::new();
    if let Some((ranges, active)) = editor.search.as_ref() {
        for (i, r) in ranges.iter().enumerate() {
            let color: Hsla = if Some(i) == *active {
                rgba(0xFF9500DD).into()
            } else {
                rgba(0xFFD60055).into()
            };
            search.extend(painter.quads(r.start, r.end, color, window));
        }
    }
    // IME composition: underline the marked text, as native text fields
    // do. Without it an unfinished composition looks committed — and the
    // arrow keys the input method keeps for itself seem to stick (#119).
    let marked: Vec<PaintQuad> = editor
        .selection
        .marked_range
        .as_ref()
        .map(|m| {
            painter
                .quads(m.start, m.end, text_color, window)
                .into_iter()
                .map(|q| {
                    let b = q.bounds;
                    fill(
                        Bounds::new(
                            point(b.left(), b.bottom() - px(3.)),
                            size(b.size.width, px(1.5)),
                        ),
                        text_color,
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let (cursor, selections) = caret_and_selection(
        &painter,
        editor,
        &widgets,
        &tables,
        image_resize,
        text_color,
        &font,
        font_size,
        window,
    );
    PrepaintState {
        wrapped,
        wrap_rows,
        grip,
        grip_hb,
        line_tops,
        line_heights,
        widgets,
        backgrounds,
        tables,
        maps,
        marks,
        rtl,
        inline_maths,
        image_grips,
        checkbox_grips,
        code_chips,
        code_card_rects,
        chip_grips,
        alert_fold_grips,
        heading_chevrons,
        heading_fold_grips,
        heading_row_rects,
        link_grips,
        link_rects,
        prop_pill_grips,
        inline_image_grips,
        alert_icons: editor
            .markdown_style
            .as_ref()
            .and_then(|st| st.alert_icons.clone()),
        table_zones,
        table_thumbs,
        row_aff,
        col_aff,
        caret_cell,
        col_resize_grips,
        cursor,
        selections,
        search,
        marked,
    }
}

/// Publish NEXT frame's shaping window: the viewport band in element-local y
/// with a viewport of margin each side, quantized so the band (and with it
/// the measure→prepaint memo) stays stable across small scrolls. Read back
/// one frame stale — both passes of a frame always shape with the same band.
/// When the quantized band CHANGES (e.g. after an off-screen jump), schedule
/// exactly one bounded next-frame repaint of the newly visible rows — no
/// continuous redraws, no missed repaints.
fn publish_shape_band(
    editor: &EditorState,
    entity: &Entity<EditorState>,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    let vh = f32::from(window.viewport_size().height).max(1.);
    let top = -f32::from(bounds.origin.y);
    let q = 512.0;
    let lo = (((top - vh) / q).floor() * q).max(0.);
    let hi = ((top + 2. * vh) / q).ceil() * q;
    if editor.frame.shape_band.replace(Some((lo, hi))) != Some((lo, hi)) {
        // This frame used the previous band; repaint the newly visible rows.
        let entity = entity.clone();
        window.on_next_frame(move |_, cx| {
            entity.update(cx, |_, cx| cx.notify());
        });
    }
}

/// Top offset of each logical line (running sum of variable wrap heights),
/// reserving a gap above/below each code block so its padded box has its own
/// space (no overlap with the adjacent line, no blank line required).
fn line_tops_for(
    line_heights: &[Pixels],
    backgrounds: &[Option<CodeBg>],
    tables: &[Option<TableRow>],
    wrap_rows: &[usize],
) -> Vec<Pixels> {
    let mut line_tops = Vec::with_capacity(line_heights.len());
    let mut y = px(0.);
    for (idx, lh) in line_heights.iter().enumerate() {
        // Code-box pads plus the table gutter rows (see `line_pads`) — baked
        // into line_tops so the caret / click / paint all shift with them,
        // and neither table affordance overlaps the adjacent line.
        let (top_pad, bot_pad) = line_pads(
            backgrounds.get(idx).copied().flatten(),
            tables.get(idx).and_then(Option::as_ref),
        );
        y += top_pad;
        line_tops.push(y);
        y += *lh * wrap_rows[idx] as f32 + bot_pad;
    }
    line_tops
}

/// Right-to-left rows (#66): direction from the SOURCE line — the same
/// `base_direction` the reader uses, so the two views can never disagree
/// about which lines are RTL (markdown markers, digits and whitespace are
/// neutral under UAX #9, so `- سلام` reads RTL). Only plain text rows
/// qualify: a code block is LTR by convention, and a table/image widget row
/// paints its own geometry (flipping those is a follow-up). An all-LTR
/// document leaves every entry `None` and pays nothing.
/// The RTL layout computed while shaping (that is where the row COUNT had to
/// be decided), turned into per-row right-align shifts. Rows are shifted
/// individually: a short last row sits further right than a full one, which
/// is what right-aligned text means.
fn rtl_row_geometry(
    width: Pixels,
    backgrounds: &[Option<CodeBg>],
    marks: &[Option<LineMark>],
    rtl_layout: Vec<Option<(bool, Vec<crate::bidi::paragraph::Row>)>>,
) -> Vec<Option<RtlRow>> {
    // `base_direction` the reader uses, so the two views can never disagree
    // about which lines are RTL (markdown markers, digits and whitespace
    // are neutral under UAX #9, so `- سلام` reads RTL). Only plain text
    // rows qualify: a code block is LTR by convention, and a table/image
    // widget row paints its own geometry (flipping those is a follow-up).
    // An all-LTR document leaves every entry `None` and pays nothing.
    // The RTL layout computed while shaping (that is where the row COUNT
    // had to be decided), turned into per-row right-align shifts. Rows are
    // shifted individually: a short last row sits further right than a full
    // one, which is what right-aligned text means.
    let rtl: Vec<Option<RtlRow>> = rtl_layout
        .into_iter()
        .enumerate()
        .map(|(i, entry)| {
            let (base_rtl, rows) = entry?;
            let inset = row_inset(
                backgrounds.get(i).copied().flatten(),
                marks.get(i).copied().flatten(),
            );
            let shifts = rows
                .iter()
                .map(|r| {
                    if base_rtl {
                        rtl_shift(width, inset, r.width)
                    } else {
                        // Reads left-to-right; it only needed the mapping.
                        px(0.)
                    }
                })
                .collect();
            Some(RtlRow {
                rows,
                shifts,
                base_rtl,
            })
        })
        .collect();
    rtl
}

/// Read-only per-row geometry shared by this pass's range painters — THIS
/// frame's fresh line data (the committed `EditorState` caches lag a frame,
/// which would jump the caret one frame after an edit that hides/reveals
/// markers). Building the row-start array once here keeps every range quad
/// O(N) — no per-line rescans of the document.
struct RangePainter<'a> {
    editor: &'a EditorState,
    /// Source byte offset of each logical line (the row-start array).
    starts: &'a [usize],
    wrapped: &'a [WrappedLine],
    line_heights: &'a [Pixels],
    line_tops: &'a [Pixels],
    bounds: Bounds<Pixels>,
    maps: &'a [Option<std::rc::Rc<Vec<usize>>>],
    tables: &'a [Option<TableRow>],
    backgrounds: &'a [Option<CodeBg>],
    marks: &'a [Option<LineMark>],
    rtl: &'a [Option<RtlRow>],
    font: &'a Font,
    font_size: Pixels,
    base_lh: Pixels,
}
impl RangePainter<'_> {
    /// Row text origin, this frame: the row's inset plus its RTL shift.
    /// Fresh-data twin of `EditorState::row_origin_x` (the committed state
    /// lags a frame — see `disp_col` below).
    fn row_x(&self, row: usize) -> Pixels {
        row_inset(
            self.backgrounds.get(row).copied().flatten(),
            self.marks.get(row).copied().flatten(),
        ) + self
            .rtl
            .get(row)
            .and_then(Option::as_ref)
            .and_then(|r| r.shifts.first().copied())
            .unwrap_or(px(0.))
    }
    /// The row's bidi layout, if it has one.
    fn bidi(&self, row: usize) -> Option<&RtlRow> {
        self.rtl.get(row).and_then(Option::as_ref)
    }
    /// Invert a display→source offset map: the display column for `source_col`.
    /// `None` map → identity (a row shown as full source).
    fn disp_col(&self, row: usize, sc: usize) -> usize {
        display_col_in(self.maps.get(row).and_then(Option::as_ref), sc)
    }
    /// Map a (line-relative) point to a screen point. Captures `bounds` (Copy)
    /// only, so `line_tops` stays free to move into the prepaint state.
    fn to_screen(&self, top: Pixels, p: Point<Pixels>) -> Point<Pixels> {
        point(self.bounds.left() + p.x, self.bounds.top() + top + p.y)
    }
    /// Quads covering source range `s..e`, one per wrap row — the
    /// selection's multi-row geometry, shared with the find-match
    /// highlights (which paint the same shapes in other colors).
    fn quads(&self, s: usize, e: usize, color: Hsla, window: &mut Window) -> Vec<PaintQuad> {
        let s_row = self
            .starts
            .partition_point(|&start| start <= s)
            .saturating_sub(1);
        let e_row = self
            .starts
            .partition_point(|&start| start <= e)
            .saturating_sub(1);
        let right = self.bounds.size.width;
        let mut sels = Vec::new();
        for row in s_row..=e_row {
            let Some(line) = self.wrapped.get(row) else {
                continue;
            };
            let lh = self.line_heights.get(row).copied().unwrap_or(self.base_lh);
            // A collapsed row (hidden marker/fence line, the table's
            // `|---|` separator, a folded body) has no visible text —
            // painting its quad would smear a band over its neighbors.
            if lh <= px(0.5) {
                continue;
            }
            let top = self.line_tops[row];
            let line_start = self.starts[row];
            let a = s.max(line_start) - line_start;
            let line_end = self
                .starts
                .get(row + 1)
                .map_or(self.editor.content.len(), |&s| s - 1);
            let b = e.min(line_end) - line_start;
            // Table row: highlight between the cell positions of the selection
            // ends (not raw-source geometry).
            if let Some(t) = self.tables.get(row).and_then(Option::as_ref) {
                let table_w: Pixels = t.col_widths.iter().copied().sum();
                let tleft = self.editor.table_left(t, row, &self.bounds);
                // Endpoint x's: inside a cell, the exact caret x; at
                // or past the row's cell span — or in an empty cell
                // the shaper can't position — the table's edge, so
                // vacant trailing cells highlight too.
                let first_start = t.cell_ranges.first().map_or(0, |r| r.start);
                let last_end = t.cell_ranges.last().map_or(0, |r| r.end);
                // Clamp to the table's visible band — a wide
                // (scrolling) table's cells extend past the
                // viewport, but its highlight must not.
                let (vis_l, vis_r) =
                    table_visible_band(self.bounds.left(), self.bounds.size.width, t.rtl);
                let mut band = |lo: Pixels, hi: Pixels, y: Pixels, h: Pixels| {
                    let (lo, hi) = (lo.max(vis_l), hi.min(vis_r));
                    if hi > lo {
                        sels.push(fill(
                            Bounds::from_corners(point(lo, y), point(hi, y + h)),
                            color,
                        ));
                    }
                };
                let pa = (a > first_start && a < last_end)
                    .then(|| table_caret_pos(t, a, tleft, self.font, self.font_size, window))
                    .flatten();
                let pb = (b > first_start && b < last_end)
                    .then(|| table_caret_pos(t, b, tleft, self.font, self.font_size, window))
                    .flatten();
                match (pa, pb) {
                    // Both ends inside the SAME (possibly wrapped)
                    // cell: per-wrap-row bands within that cell —
                    // one full-height band would smear the whole row.
                    (Some((xa, ya, ca, _)), Some((xb, yb, cb, _))) if ca == cb => {
                        let pad = px(TABLE_CELL_PAD);
                        let cell_x = tleft + col_offset(&t.col_widths, t.cells.len(), ca, t.rtl);
                        let cw = cell_span_width(&t.col_widths, t.cells.len(), ca);
                        let (cl, cr) = (cell_x + pad, cell_x + cw - pad);
                        let y0 = self.bounds.top() + top + px(6.);
                        if ya == yb {
                            band(
                                xa.min(xb),
                                xa.max(xb).max(xa.min(xb) + px(2.)),
                                y0 + ya,
                                self.base_lh,
                            );
                        } else {
                            band(xa, cr, y0 + ya, self.base_lh);
                            let mut y = ya + self.base_lh;
                            while y < yb {
                                band(cl, cr, y0 + y, self.base_lh);
                                y += self.base_lh;
                            }
                            band(cl, xb, y0 + yb, self.base_lh);
                        }
                    }
                    // Anything else: one full-height band between the
                    // endpoint x's (row edges for out-of-span ends).
                    _ => {
                        let xa = if a <= first_start {
                            tleft
                        } else {
                            pa.map_or(tleft, |(x, ..)| x)
                        };
                        let xb = if b >= last_end {
                            tleft + table_w
                        } else {
                            pb.map_or(tleft + table_w, |(x, ..)| x)
                        };
                        band(xa.min(xb), xa.max(xb), self.bounds.top() + top, lh);
                    }
                }
                continue;
            }
            let inset = self.row_x(row);
            let pa = caret_line_pos(
                line,
                self.bidi(row),
                self.disp_col(row, a),
                lh,
                self.editor.affinity_for_offset(line_start + a),
            )
            .unwrap_or_default();
            let pb = caret_line_pos(
                line,
                self.bidi(row),
                self.disp_col(row, b),
                lh,
                self.editor.affinity_for_offset(line_start + b),
            )
            .unwrap_or_default();
            let pa = point(pa.x + inset, pa.y);
            let pb = point(pb.x + inset, pb.y);
            if pa.y == pb.y {
                // On an RTL row the range's END sits to the LEFT of its
                // start, so order the corners by x. One rect per row is
                // still all this paints: a selection spanning a
                // direction change is visually split (see
                // `VisualMap::rects_for_range`) and needs N — a
                // follow-up.
                let (x0, x1) = (pa.x.min(pb.x), pa.x.max(pb.x));
                sels.push(fill(
                    Bounds::from_corners(
                        self.to_screen(top, point(x0, pa.y)),
                        self.to_screen(top, point(x1.max(x0 + px(2.)), pb.y + lh)),
                    ),
                    color,
                ));
            } else if let Some(r) = self.bidi(row) {
                // RTL, spanning wrap rows: the selection runs the other
                // way. It leaves the first row at its LEFT edge,
                // continues from the right on each following row, and
                // ends part-way across the last. Rows are banded to
                // their own extent, not the content width — a
                // right-aligned row doesn't span it.
                let row_of =
                    |y: Pixels| (f32::from(y) / f32::from(lh).max(1.0)).round().max(0.) as usize;
                let (ka, kb) = (row_of(pa.y), row_of(pb.y));
                let mut band = |k: usize, from: Option<Pixels>, to: Option<Pixels>| {
                    let (l, rt) = r.row_extent(k);
                    let (l, rt) = (l + inset, rt + inset);
                    let (x0, x1) = (from.unwrap_or(l), to.unwrap_or(rt));
                    if x1 > x0 {
                        sels.push(fill(
                            Bounds::from_corners(
                                self.to_screen(top, point(x0, lh * k)),
                                self.to_screen(top, point(x1, lh * k + lh)),
                            ),
                            color,
                        ));
                    }
                };
                // The start is the row's RIGHT-hand portion: from the
                // row's left edge up to the caret x.
                band(ka, None, Some(pa.x));
                for k in (ka + 1)..kb.min(r.row_count()) {
                    band(k, None, None);
                }
                // ...and the last row from the end x rightwards.
                band(kb, Some(pb.x), None);
            } else {
                // First wrap row: start x → right edge.
                sels.push(fill(
                    Bounds::from_corners(
                        self.to_screen(top, pa),
                        self.to_screen(top, point(right, pa.y + lh)),
                    ),
                    color,
                ));
                // Full middle wrap rows.
                let mut yy = pa.y + lh;
                while yy < pb.y {
                    sels.push(fill(
                        Bounds::from_corners(
                            self.to_screen(top, point(px(0.), yy)),
                            self.to_screen(top, point(right, yy + lh)),
                        ),
                        color,
                    ));
                    yy += lh;
                }
                // Last wrap row: left edge → end x.
                sels.push(fill(
                    Bounds::from_corners(
                        self.to_screen(top, point(px(0.), pb.y)),
                        self.to_screen(top, point(pb.x, pb.y + lh)),
                    ),
                    color,
                ));
            }
        }
        sels
    }
}

// ===== Inline image corner grips =====
/// Corner-grip hitboxes for each inline image, in `widgets` order (matching
/// the order paint walks them) — hitboxes must be inserted during prepaint,
/// but the resize cursor is set during paint via these. Mirrors the paint's
/// image-bounds math (row inset + IMG_ROW_PAD, live drag size) exactly so
/// the grip pins to the painted corner (incl. list-item images, which inset
/// past their bullet).
fn image_grip_hitboxes(
    widgets: &[Option<Block>],
    image_resize: Option<ImageResize>,
    backgrounds: &[Option<CodeBg>],
    marks: &[Option<LineMark>],
    bounds: Bounds<Pixels>,
    line_tops: &[Pixels],
    window: &mut Window,
) -> Vec<Hitbox> {
    // Corner-grip hitboxes for each inline image, in `widgets` order (matching
    // the order paint walks them) — hitboxes must be inserted during prepaint,
    // but the resize cursor is set during paint via these. Mirrors the paint's
    // image-bounds math (row inset + IMG_ROW_PAD, live drag size) exactly so
    // the grip pins to the painted corner (incl. list-item images, which inset
    // past their bullet).
    let mut image_grips = Vec::new();
    for (i, w) in widgets.iter().enumerate() {
        if let Some(Block::Image(img)) = w
            && img.resizable
        {
            let inset = row_inset(
                backgrounds.get(i).copied().flatten(),
                marks.get(i).copied().flatten(),
            );
            let (img_w, img_h) = image_display_size(img, image_resize, i);
            let img_bounds = Bounds::new(
                point(
                    bounds.origin.x + inset,
                    bounds.origin.y + line_tops[i] + px(IMG_ROW_PAD / 2.),
                ),
                size(img_w, img_h),
            );
            let grip = EditorState::image_grip(img_bounds);
            image_grips.push(window.insert_hitbox(grip, HitboxBehavior::Normal));
        }
    }

    image_grips
}

// ===== Gutter pointer hitboxes: task checkboxes, file chips, fold chevrons =====
/// Pointer-cursor hitboxes for clickable gutter checkboxes + file chips, so
/// the cursor flips to a hand over them. Bounds mirror the paint math; keyed
/// by line so paint sets the cursor on each (see `set_cursor_style`).
fn gutter_pointer_hitboxes(
    frame: &RangePainter<'_>,
    widgets: &[Option<Block>],
    window: &mut Window,
) -> (
    Vec<(usize, Hitbox)>,
    Vec<(usize, Hitbox)>,
    Vec<(usize, Hitbox)>,
) {
    let RangePainter {
        marks,
        line_heights,
        rtl,
        bounds,
        line_tops,
        font_size,
        ..
    } = *frame;
    // Pointer-cursor hitboxes for clickable gutter checkboxes + file chips, so
    // the cursor flips to a hand over them. Bounds mirror the paint math; keyed
    // by line so paint sets the cursor on each (see `set_cursor_style`).
    let mut checkbox_grips = Vec::new();
    let mut chip_grips = Vec::new();
    let mut alert_fold_grips = Vec::new();
    for (i, lh) in line_heights.iter().enumerate() {
        if let Some(LineMark::Check { bullet_x, .. }) = marks.get(i).copied().flatten() {
            let sz = font_size * CHECKBOX_SCALE;
            let pad = px(4.);
            let is_rtl = rtl
                .get(i)
                .and_then(Option::as_ref)
                .is_some_and(|r| r.base_rtl);
            let bx = bounds.origin.x + checkbox_x(bullet_x, sz, bounds.size.width, is_rtl);
            let by = bounds.origin.y + line_tops[i] + (*lh - sz) / 2.;
            let hit = Bounds::new(
                point(bx - pad, by - pad),
                size(sz + pad * 2., sz + pad * 2.),
            );
            checkbox_grips.push((i, window.insert_hitbox(hit, HitboxBehavior::Normal)));
        }
        if let Some(LineMark::Alert {
            fold: Some(_),
            chevron_x,
            ..
        }) = marks.get(i).copied().flatten()
        {
            // A generous box around the chevron (its glyph is ~an em wide).
            let pad = px(4.);
            let hit = Bounds::new(
                point(
                    bounds.origin.x + chevron_x - pad,
                    bounds.origin.y + line_tops[i],
                ),
                size(font_size + pad * 2., *lh),
            );
            alert_fold_grips.push((i, window.insert_hitbox(hit, HitboxBehavior::Normal)));
        }
        if matches!(
            widgets.get(i).and_then(Option::as_ref),
            Some(Block::Chip { .. })
        ) {
            let hit = Bounds::new(
                point(bounds.origin.x, bounds.origin.y + line_tops[i]),
                size(bounds.size.width, *lh),
            );
            chip_grips.push((i, window.insert_hitbox(hit, HitboxBehavior::Normal)));
        }
    }
    (checkbox_grips, chip_grips, alert_fold_grips)
}

// ===== Code-card chrome: language tag + Copy button =====
/// Code-card chrome (Cditor-inspired, issue #16): a language tag + Copy
/// button at each code block's top-right, inside the padded box. Laid out
/// here (paint draws at these bounds; hitboxes flip the cursor).
fn code_card_chips(
    frame: &RangePainter<'_>,
    wrap_rows: &[usize],
    window: &mut Window,
) -> (Vec<CodeChip>, Vec<(usize, Bounds<Pixels>)>) {
    let RangePainter {
        editor,
        backgrounds,
        line_tops,
        line_heights,
        bounds,
        font,
        starts,
        ..
    } = *frame;
    // Code-card chrome (Cditor-inspired, issue #16): a language tag + Copy
    // button at each code block's top-right, inside the padded box. Laid
    // out here (paint draws at these bounds; hitboxes flip the cursor).
    let mut code_chips = Vec::new();
    let mut code_card_rects = Vec::new();
    if let Some(st) = editor
        .markdown_style
        .as_ref()
        .filter(|_| !editor.content.is_empty())
    {
        let chip_fs = px(13.);
        let chip_h = px(20.);
        let shape = |window: &mut Window, text: &SharedString, color: Hsla| {
            let run = TextRun {
                len: text.len(),
                font: font.clone(),
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            window
                .text_system()
                .shape_line(text.clone(), chip_fs, &[run], None)
                .width()
        };
        for (i, bg) in backgrounds.iter().enumerate() {
            let Some(cb) = bg.as_ref().filter(|cb| cb.top) else {
                continue;
            };
            // The card's full bounds (for hover tracking): first body line's
            // padded top through the last block line's padded bottom.
            let mut last = i;
            while last + 1 < backgrounds.len() && backgrounds[last + 1].is_some() {
                last += 1;
                if backgrounds[last].as_ref().is_some_and(|b| b.bottom) {
                    break;
                }
            }
            let (top_pad, _) = code_pads(Some(*cb));
            let (_, last_bot_pad) = code_pads(backgrounds[last]);
            let card_top = bounds.origin.y + line_tops[i] - top_pad;
            let card_bottom = bounds.origin.y
                + line_tops[last]
                + line_heights[last] * wrap_rows[last] as f32
                + last_bot_pad;
            let card = Bounds::new(
                point(bounds.origin.x, card_top),
                size(cb.width, card_bottom - card_top),
            );
            code_card_rects.push((i, card));
            // The chrome itself is hover-revealed: only the hovered card
            // lays out (and hit-tests) its chips.
            if editor.affordances.code_chip_hover != Some(i) {
                continue;
            }
            // The opening fence row: this row if the fence is revealed, else
            // the nearest ``` row above (hidden fences collapse to height 0).
            let Some(fence_row) = (0..=i).rev().find(|&r| {
                starts.get(r).is_some_and(|&s| {
                    let end = starts.get(r + 1).map_or(editor.content.len(), |&s| s - 1);
                    editor.content[s..end].trim_start().starts_with("```")
                })
            }) else {
                continue;
            };
            let lang = editor
                .code_block_at(fence_row)
                .map(|(l, _)| l)
                .unwrap_or_default();
            let lang_text: SharedString = if lang.is_empty() {
                "text ▾".into()
            } else {
                format!("{lang} ▾").into()
            };
            let copy_text: SharedString = editor.labels.code_copy.clone();
            let lang_w = shape(window, &lang_text, st.quote) + px(12.);
            let copy_w = shape(window, &copy_text, st.quote) + px(12.);
            let right = bounds.origin.x + cb.width - px(6.);
            let y = bounds.origin.y + line_tops[i] - top_pad + px(3.);
            let copy_bounds = Bounds::new(point(right - copy_w, y), size(copy_w, chip_h));
            let lang_bounds = Bounds::new(
                point(right - copy_w - px(4.) - lang_w, y),
                size(lang_w, chip_h),
            );
            let lang_hb = window.insert_hitbox(lang_bounds, HitboxBehavior::Normal);
            let copy_hb = window.insert_hitbox(copy_bounds, HitboxBehavior::Normal);
            code_chips.push(CodeChip {
                lang_text,
                copy_text,
                lang_bounds,
                copy_bounds,
                fence_row,
                // The popover surface (opaque) — the card tint is
                // translucent, and the labels must cover the code text.
                bg: st.popover_bg,
                fg: st.quote,
                lang_hb,
                copy_hb,
            });
        }
    }
    (code_chips, code_card_rects)
}

// ===== Heading fold chevrons =====
/// Heading fold chevrons: every heading row gets a hover-tracking rect; a
/// chevron (+ its hand-cursor hitbox) only when that row is hovered or
/// already folded — one on every heading would clutter.
fn heading_fold_marks(
    frame: &RangePainter<'_>,
    window: &mut Window,
) -> (
    Vec<(usize, bool, Pixels)>,
    Vec<(usize, Hitbox)>,
    Vec<(usize, Bounds<Pixels>)>,
) {
    let RangePainter {
        editor,
        wrapped,
        backgrounds,
        marks,
        line_heights,
        starts,
        bounds,
        line_tops,
        font_size,
        ..
    } = *frame;
    // Heading fold chevrons: every heading row gets a hover-tracking rect;
    // a chevron (+ its hand-cursor hitbox) only when that row is hovered
    // or already folded — one on every heading would clutter.
    let mut heading_chevrons = Vec::new();
    let mut heading_fold_grips = Vec::new();
    let mut heading_row_rects = Vec::new();
    if editor.markdown_style.is_some() && !editor.content.is_empty() {
        for (i, line_shaped) in wrapped.iter().enumerate() {
            // A fence's `# comment` line isn't a heading; folded rows
            // (height 0, inside an outer fold) can't anchor a chevron.
            if backgrounds.get(i).and_then(Option::as_ref).is_some() {
                continue;
            }
            let (Some(&start), Some(&lh)) = (starts.get(i), line_heights.get(i)) else {
                continue;
            };
            // Reuse this pass's index instead of rescanning the source for every row.
            let end = starts.get(i + 1).map_or(editor.content.len(), |&s| s - 1);
            let line = &editor.content[start..end];
            if markdown_syntax::line_heading_level(line).is_none() || lh == px(0.) {
                continue;
            }
            let top = bounds.origin.y + line_tops[i];
            heading_row_rects.push((
                i,
                Bounds::new(point(bounds.origin.x, top), size(bounds.size.width, lh)),
            ));
            let folded = editor.affordances.folded_headings.contains(line.trim());
            if !folded && editor.affordances.heading_hover_row != Some(i) {
                continue;
            }
            // Past the heading text (its mark inset + shaped width), capped
            // inside the right edge for a wrapped heading.
            let inset = marks
                .get(i)
                .copied()
                .flatten()
                .map_or(px(0.), LineMark::inset);
            let x = (inset + line_shaped.width() + px(10.)).min(bounds.size.width - px(20.));
            heading_chevrons.push((i, folded, x));
            let hit = Bounds::new(
                point(bounds.origin.x + x - px(4.), top),
                size(font_size + px(8.), lh),
            );
            heading_fold_grips.push((i, window.insert_hitbox(hit, HitboxBehavior::Normal)));
        }
    }
    (heading_chevrons, heading_fold_grips, heading_row_rects)
}

// ===== Inline link + inline-image pointer hitboxes =====
/// Pointer-cursor hitboxes over inline links (`[[wiki]]` / `#tag` /
/// `[text](url)`), so hovering shows a hand like the reading view.
/// Geometry from this frame's shaping: source range → display cols (the
/// row's offset map) → kerned x via position_for_index. A link that crosses
/// a wrap boundary gets a box on every visual row. Widgets/code/table rows
/// have their own image and chip machinery. Inline images on this line get a
/// pointer-cursor hitbox too (they open a preview).
#[allow(clippy::too_many_arguments)]
fn link_hitboxes(
    editor: &EditorState,
    wrapped: &[WrappedLine],
    widgets: &[Option<Block>],
    backgrounds: &[Option<CodeBg>],
    tables: &[Option<TableRow>],
    line_heights: &[Pixels],
    maps: &[Option<std::rc::Rc<Vec<usize>>>],
    inline_maths: &[Vec<InlineMath>],
    starts: &[usize],
    bounds: Bounds<Pixels>,
    line_tops: &[Pixels],
    painter: &RangePainter,
    window: &mut Window,
) -> (
    Vec<Hitbox>,
    Vec<(Bounds<Pixels>, zorite_markdown::syntax::LinkHit)>,
    Vec<Hitbox>,
) {
    // Pointer-cursor hitboxes over inline links (`[[wiki]]` / `#tag` /
    // `[text](url)`), so hovering shows a hand like the reading view.
    // Geometry from this frame's shaping: source range → display cols (the
    // row's offset map) → kerned x via position_for_index. A link that
    // crosses a wrap boundary gets a box on every visual row.
    // Widgets/code/table rows have their own image and chip machinery.
    let mut link_grips = Vec::new();
    let mut link_rects: Vec<(Bounds<Pixels>, zorite_markdown::syntax::LinkHit)> = Vec::new();
    let mut inline_image_grips = Vec::new();
    if editor.markdown_style.is_some() && !editor.content.is_empty() {
        for (i, line_shaped) in wrapped.iter().enumerate() {
            if widgets.get(i).and_then(Option::as_ref).is_some()
                || backgrounds.get(i).and_then(Option::as_ref).is_some()
                || tables.get(i).and_then(Option::as_ref).is_some()
            {
                continue;
            }
            let (Some(&start), Some(lh)) = (starts.get(i), line_heights.get(i)) else {
                continue;
            };
            let end = starts.get(i + 1).map_or(editor.content.len(), |&s| s - 1);
            let line = &editor.content[start..end];
            let inset = painter.row_x(i);
            // The reference-count badge over a hidden ` ^id` anchor is
            // clickable too (skipped on the caret's line, where the raw
            // anchor is revealed for editing).
            let badge = editor
                .markdown_style
                .as_ref()
                .and_then(|st| st.block_ref_count.as_ref())
                .filter(|_| editor.row_col(editor.selection.selected_range.start).0 != i)
                .and_then(|f| {
                    zorite_markdown::syntax::block_id(line)
                        .filter(|(_, id)| f(id) > 0)
                        .map(|(at, _)| at..line.len())
                });
            // `None` = the badge (clickable, but not a link to preview).
            for (range, hit) in markdown_syntax::links(line)
                .into_iter()
                .filter(|(_, hit)| matches!(hit, markdown_syntax::LinkHit::Url(_)))
                .map(|(r, h)| (r, Some(h)))
                .chain(badge.map(|r| (r, None)))
            {
                let map = maps.get(i).and_then(Option::as_ref);
                let d1 = display_col_in(map, range.start);
                let d2 = display_col_in(map, range.end);
                let (Some(p1), Some(p2)) = (
                    caret_line_pos(
                        line_shaped,
                        painter.bidi(i),
                        d1,
                        *lh,
                        WrapAffinity::Downstream,
                    ),
                    caret_line_pos(
                        line_shaped,
                        painter.bidi(i),
                        d2,
                        *lh,
                        WrapAffinity::Upstream,
                    ),
                ) else {
                    continue;
                };
                if d1 >= d2 {
                    continue; // fully hidden (e.g. collapsed markers)
                }
                let origin = point(bounds.origin.x + inset, bounds.origin.y + line_tops[i]);
                let hit_ref = hit;
                if p1.y == p2.y {
                    // An RTL link ends to the LEFT of where it starts, so
                    // the box spans min→max x rather than p1→p2.
                    let hit = Bounds::new(
                        point(origin.x + p1.x.min(p2.x), origin.y + p1.y),
                        size((p2.x - p1.x).abs(), *lh),
                    );
                    link_grips.push(window.insert_hitbox(hit, HitboxBehavior::Normal));
                    if let Some(h) = hit_target(&hit_ref) {
                        link_rects.push((hit, h));
                    }
                } else {
                    // Wrapped: head runs to the row's end, tail from its row's start.
                    let head = Bounds::new(
                        point(origin.x + p1.x, origin.y + p1.y),
                        size((line_shaped.width() - p1.x).max(px(0.)), *lh),
                    );
                    let tail = Bounds::new(point(origin.x, origin.y + p2.y), size(p2.x, *lh));
                    link_grips.push(window.insert_hitbox(head, HitboxBehavior::Normal));
                    link_grips.push(window.insert_hitbox(tail, HitboxBehavior::Normal));
                    let first_row = (f32::from(p1.y) / f32::from(*lh)).round() as usize;
                    let last_row = (f32::from(p2.y) / f32::from(*lh)).round() as usize;
                    for row in first_row + 1..last_row {
                        let middle = Bounds::new(
                            point(origin.x, origin.y + *lh * row),
                            size(line_shaped.width(), *lh),
                        );
                        link_grips.push(window.insert_hitbox(middle, HitboxBehavior::Normal));
                        if let Some(h) = hit_target(&hit_ref) {
                            link_rects.push((middle, h));
                        }
                    }
                    if let Some(h) = hit_target(&hit_ref) {
                        link_rects.push((head, h.clone()));
                        link_rects.push((tail, h));
                    }
                }
            }
            // Inline images on this line get a pointer-cursor hitbox (they
            // open a preview) — bounds mirror the paint math (inset + the
            // spacer's wrap-row position, centered in the row).
            for im in inline_maths.get(i).into_iter().flatten() {
                if !im.latex.is_empty() {
                    continue; // a `$…$` formula, not an image
                }
                if let Some(p) = line_shaped.position_for_index(im.display_off, *lh) {
                    let hit = Bounds::new(
                        point(
                            bounds.origin.x + inset + p.x,
                            bounds.origin.y + line_tops[i] + p.y + (*lh - im.height) / 2.,
                        ),
                        size(im.width, im.height),
                    );
                    inline_image_grips.push(window.insert_hitbox(hit, HitboxBehavior::Normal));
                }
            }
        }
    }
    (link_grips, link_rects, inline_image_grips)
}

// ===== Property-panel pill hitboxes =====
/// Pointer cursor over property-panel pills: a panel is a widget on its
/// region's first line, so measure each pill (the same x-advance paint
/// uses) and insert a hitbox — the cursor is set during paint.
fn prop_pill_hitboxes(
    widgets: &[Option<Block>],
    bounds: Bounds<Pixels>,
    line_tops: &[Pixels],
    font: &Font,
    font_size: Pixels,
    window: &mut Window,
) -> Vec<Hitbox> {
    // Pointer cursor over property-panel pills: a panel is a widget on its
    // region's first line, so measure each pill (the same x-advance paint
    // uses) and insert a hitbox — the cursor is set during paint.
    let mut prop_pill_grips = Vec::new();
    for (i, w) in widgets.iter().enumerate() {
        if let Some(Block::Properties(p)) = w.as_ref() {
            let origin = point(bounds.origin.x, bounds.origin.y + line_tops[i]);
            for b in prop_pill_bounds(p, origin, font, font_size, window) {
                prop_pill_grips.push(window.insert_hitbox(b, HitboxBehavior::Normal));
            }
        }
    }
    prop_pill_grips
}

// ===== Gutter drag grip (block reorder) =====
/// Gutter drag grip (Cditor/Notion-style block reorder): hovering a line
/// reveals a six-dot handle in the left margin; pressing it grabs the line's
/// block (see \`EditorState::drag_block_rows\`). Skipped while a drag is live
/// — the grip would chase the pointer. While a drag is live, a viewport-sized
/// hitbox (inserted after the editor's own, so it sits on top) keeps the
/// closed-hand cursor everywhere — otherwise the text I-beam takes over
/// mid-drag.
fn gutter_drag_grip(
    editor: &EditorState,
    bounds: Bounds<Pixels>,
    line_tops: &[Pixels],
    line_heights: &[Pixels],
    wrap_rows: &[usize],
    window: &mut Window,
) -> (Option<(usize, Bounds<Pixels>)>, Option<Hitbox>) {
    // Gutter drag grip (Cditor/Notion-style block reorder): hovering a
    // line reveals a six-dot handle in the left margin; pressing it grabs
    // the line's block (see `drag_block_rows`). Skipped while a drag is
    // live — the grip would chase the pointer.
    let mouse = window.mouse_position();
    let mut grip: Option<(usize, Bounds<Pixels>)> = None;
    let mut grip_hb: Option<Hitbox> = None;
    // While a drag is live, a viewport-sized hitbox (inserted after the
    // editor's own, so it sits on top) keeps the closed-hand cursor
    // everywhere — otherwise the text I-beam takes over mid-drag.
    if editor.affordances.line_drag.is_some() {
        let all = Bounds::new(gpui_kit::Point::default(), window.viewport_size());
        grip_hb = Some(window.insert_hitbox(all, HitboxBehavior::Normal));
    }
    let gl = grip_left(bounds.origin.x, editor.affordances.grip_inset);
    if editor.markdown_style.is_some()
        && editor.affordances.line_drag.is_none()
        && mouse.x >= gl - px(4.)
        && mouse.x <= bounds.origin.x + bounds.size.width
    {
        let y = mouse.y - bounds.origin.y;
        let row = (0..line_tops.len()).find(|&i| {
            let h = line_heights[i] * wrap_rows[i] as f32;
            h > px(0.5) && y >= line_tops[i] && y < line_tops[i] + h
        });
        if let Some(row) = row {
            let sz = px(14.);
            let rect = Bounds::new(
                point(
                    gl,
                    bounds.origin.y + line_tops[row] + (line_heights[row] - sz) / 2.,
                ),
                size(sz, sz),
            );
            grip_hb = Some(window.insert_hitbox(rect, HitboxBehavior::Normal));
            grip = Some((row, rect));
        }
    }
    (grip, grip_hb)
}

// ===== Table affordances: hover zones, scroll thumbs, caret cell, resize grips =====
/// Table interaction (issue #16, Cditor-style): the hovered row/column gets
/// an ACCENT OUTLINE (no fill) plus a small pill ON the table border — "+"
/// inserts after it, "−" deletes it. The caret's cell is outlined too. Paint
/// shows/cursors them only while hovered; on_mouse_down hit-tests the
/// committed pill rects. A border drag (or the pointer on a resize band) owns
/// the interaction — the row/column outlines + pills would just flicker
/// under it.
fn table_affordances(
    editor: &EditorState,
    tables: &[Option<TableRow>],
    line_tops: &[Pixels],
    line_heights: &[Pixels],
    bounds: Bounds<Pixels>,
    window: &mut Window,
) -> (
    Vec<(Bounds<Pixels>, usize)>,
    Vec<(TableThumb, Hitbox)>,
    Option<TableAffordance>,
    Option<TableAffordance>,
    Option<(Bounds<Pixels>, Hsla)>,
    Vec<ColResizeGrip>,
) {
    // Table interaction (issue #16, Cditor-style): the hovered row/column
    // gets an ACCENT OUTLINE (no fill) plus a small pill ON the table border
    // — "+" inserts after it, "−" deletes it. The caret's cell is outlined
    // too. Paint shows/cursors them only while hovered; on_mouse_down
    // hit-tests the committed pill rects.
    let mouse = window.mouse_position();
    let mut table_zones: Vec<(Bounds<Pixels>, usize)> = Vec::new();
    let mut table_thumbs: Vec<(TableThumb, Hitbox)> = Vec::new();
    let mut row_aff: Option<TableAffordance> = None;
    let mut col_aff: Option<TableAffordance> = None;
    let mut col_resize_grips: Vec<ColResizeGrip> = Vec::new();
    let dragging_col = editor.affordances.table_col_resize;
    let mut caret_cell: Option<(Bounds<Pixels>, Hsla)> = None;
    let caret_pos = editor.caret_table_cell_pos();
    let mut tbl_top: Option<Pixels> = None;
    let mut tbl_header = 0usize;
    // Pill geometry: thickness across the border, length along it.
    const PILL_ACROSS: f32 = 16.;
    const PILL_ALONG: f32 = 36.;
    for (i, slot) in tables.iter().enumerate() {
        let Some(t) = slot else { continue };
        if t.is_header {
            tbl_top = Some(bounds.origin.y + line_tops[i]);
            tbl_header = i;
        }
        if t.is_last && !t.col_widths.is_empty() {
            let top = tbl_top.unwrap_or(bounds.origin.y + line_tops[i]);
            let bottom = bounds.origin.y + line_tops[i] + line_heights[i];
            let width: Pixels = t.col_widths.iter().copied().sum();
            // A scrolled wide table shifts every affordance with its content.
            let left = editor.table_left(t, i, &bounds);
            let accent = editor.markdown_style.as_ref().map_or(t.border, |s| s.link);
            let g = px(PILL_ACROSS);
            let zone = Bounds::new(
                point(left - g, top - g),
                size(width + g * 2., (bottom - top) + g * 2.),
            );
            // Clip to the viewport: a scrolled wide table's CONTENT rect
            // slides left with its scroll offset, but the visible grid
            // stays put — hover and the wheel target what's on screen.
            table_zones.push((zone.intersect(&bounds), tbl_header));

            // The scroll thumb under a wide table: geometry + a
            // hand-cursor hitbox here; paint draws it and mouse-down
            // drags it (see `on_mouse_down`).
            let avail = bounds.size.width - px(TABLE_GUTTER);
            if width > avail {
                let th_w = (avail / f32::from(width) * f32::from(avail)).max(px(24.));
                let range = f32::from(width - avail);
                let sx = editor.table_sx(tbl_header, width, avail);
                // Track left is FIXED (the gutter edge) — `left` is the
                // scrolled content edge and would drift the thumb. An RTL
                // table's band sits on the other side and its scroll runs
                // the other way, so the thumb starts at the track's right.
                let (track, _) = table_visible_band(bounds.origin.x, bounds.size.width, t.rtl);
                let frac = f32::from(sx) / range;
                let frac = if t.rtl { 1. - frac } else { frac };
                let th_x = track + (avail - th_w) * frac;
                let mut th_c = t.border;
                th_c.a = (th_c.a * 1.5).min(0.8);
                let rect = Bounds::new(point(th_x, bottom - px(4.)), size(th_w, px(3.)));
                let grab = Bounds::new(
                    rect.origin - point(px(4.), px(6.)),
                    rect.size + size(px(8.), px(12.)),
                );
                table_thumbs.push((
                    TableThumb {
                        rect,
                        grab,
                        header: tbl_header,
                        // Negative on RTL: the thumb runs against the
                        // scroll offset there (see `frac` above), so a
                        // drag right must decrease it.
                        factor: range / f32::from(avail - th_w).max(1.)
                            * if t.rtl { -1. } else { 1. },
                        color: th_c,
                    },
                    window.insert_hitbox(grab, HitboxBehavior::Normal),
                ));
            }

            // The caret's cell (this table only), outlined like Cditor's.
            if let Some((crow, ccell, _)) = caret_pos
                && crow >= tbl_header
                && crow <= i
                && let Some(rt) = tables.get(crow).and_then(Option::as_ref)
                && !rt.col_widths.is_empty()
            {
                let cc = ccell.min(rt.col_widths.len() - 1);
                let x: Pixels = left + col_offset(&rt.col_widths, rt.cells.len(), cc, rt.rtl);
                let rect = Bounds::new(
                    point(x, bounds.origin.y + line_tops[crow]),
                    size(
                        cell_span_width(&rt.col_widths, rt.cells.len(), cc),
                        line_heights[crow],
                    ),
                );
                caret_cell = Some((rect, accent));
            }

            // Column-resize grips: a slim band on each column's right
            // border (hover-gated; kept while a drag on this table is live).
            if zone.contains(&mouse) || dragging_col.is_some_and(|r| r.header_row == tbl_header) {
                for (col, &cw) in t.col_widths.iter().enumerate() {
                    // The grip sits on the edge where this column ENDS: its
                    // left on an RTL table, its right otherwise.
                    let cx = left + col_offset(&t.col_widths, t.cells.len(), col, t.rtl);
                    let xacc = if t.rtl { cx } else { cx + cw };
                    let band = Bounds::new(point(xacc - px(3.), top), size(px(6.), bottom - top));
                    col_resize_grips.push(ColResizeGrip {
                        band,
                        hit: window.insert_hitbox(band, HitboxBehavior::Normal),
                        header_row: tbl_header,
                        col,
                        width: f32::from(cw),
                        x: xacc,
                        top,
                        bottom,
                        accent,
                    });
                }
            }
            if zone.contains(&mouse) {
                // Hovered BODY row → outline + a vertical pill on the border
                // the reserved gutter is on: the left one, or an RTL
                // table's right one (#66) — the pill must land in empty
                // gutter, not over the note column's edge.
                let pill_x = if t.rtl { left + width } else { left };
                for line in tbl_header..=i {
                    let Some(rt) = tables.get(line).and_then(Option::as_ref) else {
                        continue;
                    };
                    if rt.is_separator || rt.is_header {
                        continue;
                    }
                    let rtop = bounds.origin.y + line_tops[line];
                    let rh = line_heights[line];
                    if mouse.y >= rtop && mouse.y < rtop + rh {
                        let ph = px(PILL_ALONG).min(rh - px(2.));
                        let py0 = rtop + (rh - ph) / 2.;
                        let pill = Bounds::new(point(pill_x - g / 2., py0), size(g, ph));
                        let plus = Bounds::new(pill.origin, size(g, ph / 2.));
                        let minus =
                            Bounds::new(point(pill.origin.x, py0 + ph / 2.), size(g, ph / 2.));
                        row_aff = Some(TableAffordance {
                            outline: Bounds::new(point(left, rtop), size(width, rh)),
                            plus,
                            minus,
                            plus_hit: window.insert_hitbox(plus, HitboxBehavior::Normal),
                            minus_hit: window.insert_hitbox(minus, HitboxBehavior::Normal),
                            row: line,
                            col: 0,
                            accent,
                        });
                        break;
                    }
                }
                // Hovered column → outline + a horizontal pill on the top border.
                // Not while the pointer is ON the row's pill — the column
                // highlight under it is noise when you're about to click.
                let on_row_pill = row_aff
                    .as_ref()
                    .is_some_and(|a| a.plus.contains(&mouse) || a.minus.contains(&mouse));
                if !on_row_pill && mouse.x >= left && mouse.x < left + width {
                    for (col, &cw) in t.col_widths.iter().enumerate() {
                        let colx = left + col_offset(&t.col_widths, t.cells.len(), col, t.rtl);
                        if (mouse.x >= colx && mouse.x < colx + cw) || col + 1 == t.col_widths.len()
                        {
                            let pw = px(PILL_ALONG).min(cw - px(2.));
                            let px0 = colx + (cw - pw) / 2.;
                            let pill = Bounds::new(point(px0, top - g / 2.), size(pw, g));
                            let plus = Bounds::new(pill.origin, size(pw / 2., g));
                            let minus =
                                Bounds::new(point(px0 + pw / 2., pill.origin.y), size(pw / 2., g));
                            col_aff = Some(TableAffordance {
                                outline: Bounds::new(point(colx, top), size(cw, bottom - top)),
                                plus,
                                minus,
                                plus_hit: window.insert_hitbox(plus, HitboxBehavior::Normal),
                                minus_hit: window.insert_hitbox(minus, HitboxBehavior::Normal),
                                row: tbl_header,
                                col,
                                accent,
                            });
                            break;
                        }
                    }
                }
            }
            tbl_top = None;
        }
    }
    // A border drag (or the pointer on a resize band) owns the interaction —
    // the row/column outlines + pills would just flicker under it.
    if dragging_col.is_some() || col_resize_grips.iter().any(|gr| gr.band.contains(&mouse)) {
        row_aff = None;
        col_aff = None;
    }
    (
        table_zones,
        table_thumbs,
        row_aff,
        col_aff,
        caret_cell,
        col_resize_grips,
    )
}

// ===== Caret + selection geometry =====
/// The caret quad + selection quads. Caret/selection positioning must use
/// THIS frame's fresh per-row data — `editor.frame.offset_maps`/`line_insets`
/// aren't committed until paint, so the method forms would lag a frame (a
/// one-frame caret jump after an edit that hides/reveals markers).
#[allow(clippy::too_many_arguments)]
fn caret_and_selection(
    painter: &RangePainter,
    editor: &EditorState,
    widgets: &[Option<Block>],
    tables: &[Option<TableRow>],
    image_resize: Option<ImageResize>,
    text_color: Hsla,
    font: &Font,
    font_size: Pixels,
    window: &mut Window,
) -> (Option<PaintQuad>, Vec<PaintQuad>) {
    let (cursor, mut selections) = if editor.content.is_empty() {
        let c = fill(
            Bounds::new(
                point(painter.bounds.left(), painter.bounds.top()),
                size(px(CARET_WIDTH), painter.base_lh),
            ),
            text_color,
        );
        (Some(c), Vec::new())
    } else if editor.selection.selected_range.is_empty() {
        let (row, col) = editor.row_col(editor.cursor_offset());
        let lh = painter
            .line_heights
            .get(row)
            .copied()
            .unwrap_or(painter.base_lh);
        let top = painter.line_tops.get(row).copied().unwrap_or(px(0.));
        // Caret on an image row: the picture stays rendered (a Word-style
        // atomic object), so paint an image-height caret parked at its edge
        // — before the picture at the line's start, after it anywhere else —
        // instead of a text caret placed by the hidden source glyphs.
        if let Some(Block::Image(img)) = widgets.get(row).and_then(Option::as_ref) {
            let inset = painter.row_x(row);
            let (img_w, img_h) = image_display_size(img, image_resize, row);
            let x = painter.bounds.left() + inset + if col == 0 { px(-3.) } else { img_w + px(3.) };
            let c = fill(
                Bounds::new(
                    point(x, painter.bounds.top() + top + px(IMG_ROW_PAD / 2.)),
                    size(px(CARET_WIDTH), img_h),
                ),
                text_color,
            );
            (Some(c), Vec::new())
        } else if let Some(t) = tables.get(row).and_then(Option::as_ref)
            && let Some((x, y_off, _, _)) = table_caret_pos(
                t,
                col,
                editor.table_left(t, row, &painter.bounds),
                font,
                font_size,
                window,
            )
        {
            // Top pad + the wrap row's y (0 for unwrapped cells).
            let y = painter.bounds.top() + top + px(6.) + y_off;
            let c = fill(
                Bounds::new(point(x, y), size(px(CARET_WIDTH), painter.base_lh)),
                text_color,
            );
            (Some(c), Vec::new())
        } else {
            let p = painter
                .wrapped
                .get(row)
                .and_then(|l| {
                    caret_line_pos(
                        l,
                        painter.bidi(row),
                        painter.disp_col(row, col),
                        lh,
                        editor.selection.caret_affinity,
                    )
                })
                .unwrap_or_default();
            let inset = painter.row_x(row);
            // A row can be taller than its text — grown to fit an inline
            // formula, or breathing like a list item (LIST_ROW_GAP) — with
            // the glyphs centered in it. The caret matches the TEXT height
            // (the row's shaped font size × the ratio, so headings keep
            // their taller carets), centered the same way, not the row.
            let text_lh = painter
                .wrapped
                .get(row)
                .map_or(painter.base_lh, |l| {
                    l.unwrapped_layout.font_size * LINE_HEIGHT_RATIO
                })
                .min(lh);
            let c = fill(
                Bounds::new(
                    painter.to_screen(top, point(p.x + inset, p.y + (lh - text_lh) / 2.)),
                    size(px(CARET_WIDTH), text_lh),
                ),
                text_color,
            );
            (Some(c), Vec::new())
        }
    } else {
        // Selection tint = the theme accent at low alpha (fallback: a fixed blue).
        let color = editor
            .markdown_style
            .as_ref()
            .map_or(rgba(0x3b82f640).into(), |s| {
                let mut c = s.link;
                c.a = 0.25;
                c
            });
        (
            None,
            painter.quads(
                editor.selection.selected_range.start,
                editor.selection.selected_range.end,
                color,
                window,
            ),
        )
    };
    for selection in &editor.selection.extra_selections {
        if selection.is_empty() {
            let (row, col) = editor.row_col(selection.start);
            if let Some(line) = painter.wrapped.get(row) {
                let lh = painter
                    .line_heights
                    .get(row)
                    .copied()
                    .unwrap_or(painter.base_lh);
                if let Some(p) = line_pos(line, painter.bidi(row), painter.disp_col(row, col), lh) {
                    selections.push(fill(
                        Bounds::new(
                            painter.to_screen(
                                painter.line_tops[row],
                                point(p.x + painter.row_x(row), p.y),
                            ),
                            size(px(CARET_WIDTH), lh),
                        ),
                        text_color,
                    ));
                }
            }
        } else {
            let mut color = editor
                .markdown_style
                .as_ref()
                .map_or(text_color, |style| style.link);
            color.a = 0.25;
            selections.extend(painter.quads(selection.start, selection.end, color, window));
        }
    }
    (cursor, selections)
}

// ===== The previewable target of a link range =====
/// The previewable target of a link range: a real link, or `None` for the
/// reference-count badge that rides along in the same loop.
pub(super) fn hit_target(
    hit: &Option<zorite_markdown::syntax::LinkHit>,
) -> Option<zorite_markdown::syntax::LinkHit> {
    hit.clone()
}
