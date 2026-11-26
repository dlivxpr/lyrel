//! The editor element + its per-stage pipeline, split from the former
//! `element.rs` monolith by responsibility:
//!
//! - [`shape`]: the WYSIWYG shaping pass — per-line layout, block-family
//!   pre-scans (tables, fenced code, images/Mermaid, math, callouts,
//!   properties), run caches, and the quantized viewport-band shaping window.
//! - [`prepaint`]: pointer hitboxes + caret/selection/table-affordance geometry.
//! - [`paint`]: row + widget painters, window-level drag listeners, the
//!   per-frame hit-map commit, and the horizontal scrollbar.
//! - [`props`]: the `key:: value` property panel.
//!
//! `EditorElement` stays the thin coordinator: `request_layout` measures the
//! content height (publishing the shape memo), `prepaint` builds the geometry
//! state, and `paint` draws + commits.

use super::*;

mod paint;
mod prepaint;
mod props;
mod shape;

// Re-exported so the crate root's doc link [`shape_document`] keeps resolving
// through `use element::*;` (the crate root also links [`shape_all`], defined
// directly in this module).
pub(crate) use shape::shape_document;

/// The custom element that lays out + paints the editor's wrapped lines, cursor,
/// and selection, and wires the input handler. Height is content-driven via a
/// measured layout (it depends on the resolved width once soft-wrap is applied).
pub(crate) struct EditorElement {
    pub(crate) editor: Entity<EditorState>,
}

pub(crate) struct PrepaintState {
    wrapped: Vec<WrappedLine>,
    /// Per-line wrap-row count (see [`EditorState::wrap_rows`]).
    wrap_rows: Vec<usize>,
    /// The hovered line's gutter drag grip (line, rect) + its cursor hitbox.
    grip: Option<(usize, Bounds<Pixels>)>,
    grip_hb: Option<Hitbox>,
    /// Top offset of each logical line relative to the editor's top.
    line_tops: Vec<Pixels>,
    /// Per-logical-line wrap-row height (variable for headings + images).
    line_heights: Vec<Pixels>,
    /// `Some` for a line painted as an inline image instead of its source text.
    widgets: Vec<Option<Block>>,
    /// Per-line fenced-code-block background (rounded full-width box).
    backgrounds: Vec<Option<CodeBg>>,
    /// `Some` for a line painted as a table-grid row instead of source.
    tables: Vec<Option<TableRow>>,
    /// Per-line display→source byte map for marker-hidden rows (W6).
    maps: Vec<Option<std::rc::Rc<Vec<usize>>>>,
    /// Per-line gutter decoration (blockquote / list / checkbox).
    marks: Vec<Option<LineMark>>,
    /// Per-line right-to-left geometry (#66); `None` on LTR rows. Committed to
    /// [`EditorState::rtl_rows`] in paint, like `line_insets`.
    rtl: Vec<Option<RtlRow>>,
    /// Per-line inline `$…$` formulas (image + display offset + source range), painted over
    /// their spacers in the shaped text.
    inline_maths: Vec<Vec<InlineMath>>,
    /// Corner-grip hitbox for each painted inline image, in `widgets` order — so
    /// paint can set the resize cursor over each (hitboxes must be inserted in
    /// prepaint). Parallels the images paint walks, indexed by image count.
    image_grips: Vec<Hitbox>,
    /// Pointer-cursor hitboxes for task checkboxes, keyed by line.
    checkbox_grips: Vec<(usize, Hitbox)>,
    /// Per-code-block chrome (lang tag + Copy), laid out here so paint and the
    /// click rects agree. Hover-revealed: only the hovered card's chips.
    code_chips: Vec<CodeChip>,
    /// Every code card's full bounds (`(first body line, rect)`), committed for
    /// the hover tracking in `on_mouse_move`.
    code_card_rects: Vec<(usize, Bounds<Pixels>)>,
    /// Pointer-cursor hitboxes over foldable-callout chevrons, keyed by line.
    alert_fold_grips: Vec<(usize, Hitbox)>,
    /// Heading fold chevrons to paint: `(line, folded, x)` — x is line-local,
    /// past the heading text. Only hovered or already-folded headings get one.
    heading_chevrons: Vec<(usize, bool, Pixels)>,
    /// Pointer-cursor hitboxes over heading fold chevrons, keyed by line.
    heading_fold_grips: Vec<(usize, Hitbox)>,
    /// Window-space bounds of every heading's first visual row, for
    /// `on_mouse_move`'s hover tracking (committed to the editor in paint).
    heading_row_rects: Vec<(usize, Bounds<Pixels>)>,
    /// Pointer-cursor hitboxes over inline links (`[[wiki]]` / `#tag` /
    /// `[text](url)`), so hovering a clickable link shows a hand.
    link_grips: Vec<Hitbox>,
    /// The links' boxes + targets, committed to the editor for hover → `HoverLink`.
    link_rects: Vec<(Bounds<Pixels>, zorite_markdown::syntax::LinkHit)>,
    /// Pointer-cursor hitboxes over clickable property-panel pills, so hovering
    /// a pill shows a hand (like `link_grips`).
    prop_pill_grips: Vec<Hitbox>,
    /// Pointer-cursor hitboxes over inline images (they open a preview on
    /// click, so hovering shows a hand rather than the text caret).
    inline_image_grips: Vec<Hitbox>,
    /// Icon asset paths for alert marker lines, cloned from the style so the
    /// paint can draw them next to the labels.
    alert_icons: Option<markdown_syntax::AlertIcons>,
    /// Per-table hover zones (the grid plus pill reach) — repaint gating.
    table_zones: Vec<(Bounds<Pixels>, usize)>,
    table_thumbs: Vec<(TableThumb, Hitbox)>,
    /// Hovered-row / hovered-column border pills + outlines (issue #16).
    row_aff: Option<TableAffordance>,
    col_aff: Option<TableAffordance>,
    /// The caret's cell, outlined in the accent color (Cditor-style).
    caret_cell: Option<(Bounds<Pixels>, Hsla)>,
    /// Column-resize grip bands over the hovered table's vertical borders:
    /// `(band, hitbox, header row, column, current width, border x, table top,
    /// table bottom, accent)`.
    col_resize_grips: Vec<ColResizeGrip>,
    cursor: Option<PaintQuad>,
    selections: Vec<PaintQuad>,
    /// Find-match highlight quads, painted beneath the selection.
    search: Vec<PaintQuad>,
    /// The underline under IME marked text (a composition still in progress).
    marked: Vec<PaintQuad>,
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        _: &mut App,
    ) -> (LayoutId, ()) {
        // Height depends on the resolved width (soft-wrap), so measure it: shape
        // the content at the available width and count wrapped rows.
        let editor = self.editor.clone();
        let layout_entity = editor.clone();
        // Capture the ambient text style NOW, while the host wrapper's style is
        // on the window's style stack (gpui's Text element does the same). The
        // measure closure runs later, in the layout pass, where the stack is
        // unwound and `text_style()` reverts to the root size — measuring at a
        // different size than paint leaves the element shorter than its painted
        // text (days/pages overlapped at >16px text sizes).
        let text_style = window.text_style();
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let id = window.request_measured_layout(style, move |known, available, window, cx| {
            let editor = editor.read(cx);
            let base_lh = font_size * LINE_HEIGHT_RATIO;
            let wrap_width = match available.width {
                AvailableSpace::Definite(w) => Some(w),
                _ => known.width,
            };
            let height = if editor.content.is_empty() {
                // Placeholder rows at the base size.
                let rows = shape_all(
                    window,
                    &editor.placeholder,
                    font_size,
                    text_style.font(),
                    text_style.color,
                    wrap_width,
                )
                .iter()
                .map(|line| line.wrap_boundaries().len() + 1)
                .sum::<usize>()
                .max(1);
                base_lh * rows as f32
            } else {
                // Sum of per-line (variable) heights × each line's wrap rows.
                // Reveal-on-caret applies only while focused (matches prepaint).
                let focused = editor.focus_handle.is_focused(window);
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
                let scan = editor.scan_data();
                let shaped = shape_document(
                    window,
                    cx,
                    &editor.content,
                    &text_style.font(),
                    text_style.color,
                    font_size,
                    &editor.diagnostics,
                    editor.markdown_style.as_ref(),
                    wrap_width,
                    table_page_width(editor, window),
                    editor.soft_wrap,
                    caret_row,
                    geometry_row,
                    editor.block_image.as_ref(),
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
                    &scan,
                    &editor.frame.shape_caches,
                    editor.frame.shape_band.get().map(|(a, b)| (px(a), px(b))),
                    &editor.affordances.folded_headings,
                );
                natural_width_publish(editor, &shaped, &layout_entity, window);
                let (new_tops, walked, needed) = measure_line_tops(
                    &shaped.heights,
                    &shaped.backgrounds,
                    &shaped.tables,
                    &shaped.wrap_rows,
                );
                let total = walked.max(base_lh).max(needed);
                compensate_scroll(editor, &new_tops, total, wrap_width, sf);
                // Hand the shaping to this frame's prepaint (see ShapeMemo).
                *editor.frame.shape_memo.borrow_mut() = Some(ShapeMemo {
                    wrap_width,
                    caret_row,
                    geometry_row,
                    selection,
                    font_size,
                    shaped,
                });
                total
            };
            let width = wrap_width.or(known.width).unwrap_or(px(0.));
            size(width, height)
        });
        (id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> PrepaintState {
        prepaint::prepaint_editor(self, bounds, window, cx)
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        paint::paint_editor(self, bounds, prepaint, window, cx)
    }
}

// ===== The plain shaping entry (the placeholder + measurement runs) =====

/// Shape `text` into wrapped lines at `wrap_width` (one [`WrappedLine`] per
/// logical line, each carrying its own wrap boundaries). Empty on a shaping
/// error, so the editor degrades to blank rather than panicking.
pub(crate) fn shape_all(
    window: &mut Window,
    text: &SharedString,
    font_size: Pixels,
    font: Font,
    color: Hsla,
    wrap_width: Option<Pixels>,
) -> Vec<WrappedLine> {
    let run = TextRun {
        len: text.len(),
        font,
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let runs: &[TextRun] = if text.is_empty() {
        &[]
    } else {
        std::slice::from_ref(&run)
    };
    window
        .text_system()
        .shape_text(text.clone(), font_size, runs, wrap_width, None)
        .map(|lines| lines.into_vec())
        .unwrap_or_default()
}

// ===== Shared row metrics (shaping, prepaint, and paint must agree on
// these — ONE definition each, used by all three stages) =====

/// An inline image's painted size: the saved `BlockImg` size, unless its grip is
/// being dragged (`resize.line == line`), in which case the live drag width wins
/// and the height scales with it (aspect preserved). Used by both the prepaint
/// (grip hitbox) and the paint (image + grip), so the preview stays consistent.
fn image_display_size(w: &BlockImg, resize: Option<ImageResize>, line: usize) -> (Pixels, Pixels) {
    match resize {
        Some(r) if r.line == line => (px(r.width), w.height * (r.width / f32::from(w.width))),
        _ => (w.width, w.height),
    }
}

/// Horizontal text inset for a row from its decorations: [`CODE_INSET`] inside a
/// fenced code block, else the gutter mark's inset (blockquote/list), else zero.
/// At most one applies per line.
fn row_inset(bg: Option<CodeBg>, mark: Option<LineMark>) -> Pixels {
    if bg.is_some() {
        px(CODE_INSET)
    } else {
        mark.map_or(px(0.), LineMark::inset)
    }
}

/// The reserved vertical gap above (`.0`) and below (`.1`) a row, from its
/// code-block background: [`CODE_PAD`] above the block's first line and below its
/// last. Added to the line tops + total height so the padded box has real layout
/// space and never overlaps adjacent lines.
fn code_pads(bg: Option<CodeBg>) -> (Pixels, Pixels) {
    match bg {
        Some(cb) => (
            if cb.top { px(CODE_PAD) } else { px(0.) },
            if cb.bottom { px(CODE_PAD) } else { px(0.) },
        ),
        None => (px(0.), px(0.)),
    }
}

/// A row's full reserved gap above (`.0`) and below (`.1`): the code-block pads
/// plus a table's gutter rows (above the header for the column "−" handles,
/// below the last row for the add-row "+" strip). ONE function shared by the
/// measured layout and prepaint so they can never disagree — when they did
/// (measure missed the table gutters), the element laid out ~2×TABLE_GUTTER
/// shorter per table than it painted, and clicks over the shortfall never
/// reached the editor (the add-row "+" strip's dead bottom half).
fn line_pads(bg: Option<CodeBg>, table: Option<&TableRow>) -> (Pixels, Pixels) {
    let (mut top, mut bot) = code_pads(bg);
    if let Some(t) = table {
        if t.is_header {
            top += px(TABLE_GUTTER);
        }
        if t.is_last {
            bot += px(TABLE_GUTTER);
        }
    }
    (top, bot)
}

/// The WYSIWYG layout pass needs the visible page width inside the horizontal
/// scroll owner so tables fit it even when unwrapped lines make the scroll
/// content wider.
/// line can render as the image (W4). Returns, per logical line: the shaped
/// source [`WrappedLine`], its row height, and `Some(BlockImg)` when it paints
/// as an image. `md` drives the per-line size + inline styling (`None` = the
/// raw view: base size, no widgets); `diagnostics` are clipped + shifted to
/// each line. The caret's line (`caret_row`) always shows source, so an image
/// stays editable ("raw on caret"). A single line always shapes to one wrapped
/// line (incl. empty), so the counts match the logical lines and blank rows
/// stay positionable.
#[allow(clippy::too_many_arguments)]
fn table_page_width(editor: &EditorState, window: &Window) -> Option<Pixels> {
    let viewport = editor.frame.scroll_viewport.size.width;
    (viewport > px(0.))
        .then(|| (viewport - window.rem_size() * (editor.document_inset.0 * 2.)).max(px(1.)))
}

/// Publish the document's natural (unwrapped) width for the horizontal scroll
/// owner: the widest shaped line, with tables contributing their fitted grid
/// instead of their unwrapped source row. Only a real change schedules the
/// next-frame notify that re-renders with the new minimum width.
fn natural_width_publish(
    editor: &EditorState,
    shaped: &ShapedDoc,
    layout_entity: &Entity<EditorState>,
    window: &mut Window,
) {
    // Without soft wrap the widest shaped line is the document's
    // horizontal extent. Tables contribute their fitted grid, not
    // their (unwrapped) source row, or every table would widen the
    // page. Publish the value so render can give the scroll content
    // that minimum width; the next frame picks it up.
    if !editor.soft_wrap {
        let natural = shaped
            .wrapped
            .iter()
            .enumerate()
            .map(
                |(i, line)| match shaped.tables.get(i).and_then(Option::as_ref) {
                    Some(t) if !t.is_separator && !t.col_widths.is_empty() => {
                        t.col_widths.iter().copied().sum::<Pixels>() + px(TABLE_GUTTER)
                    }
                    Some(_) => px(0.),
                    None => line.width(),
                },
            )
            .fold(px(0.), Pixels::max);
        if (editor.frame.natural_width.replace(natural) - natural).abs() > px(0.5) {
            let editor = layout_entity.clone();
            window.on_next_frame(move |_, cx| {
                editor.update(cx, |_, cx| cx.notify());
            });
        }
    } else if editor.frame.natural_width.replace(px(0.)) != px(0.) {
        let editor = layout_entity.clone();
        window.on_next_frame(move |_, cx| {
            editor.update(cx, |_, cx| cx.notify());
        });
    }
}

/// The measured layout's `line_tops` walk — the SAME per-line pads and wrap-row
/// math as prepaint ([`line_pads`]), so the element lays out exactly as tall as
/// it paints. Returns the per-line tops, the walked total (`y`), and the extra
/// space a trailing table's add-row strip needs.
fn measure_line_tops(
    heights: &[Pixels],
    backgrounds: &[Option<CodeBg>],
    tables: &[Option<TableRow>],
    rows: &[usize],
) -> (Vec<Pixels>, Pixels, Pixels) {
    let mut y = px(0.);
    let mut needed = px(0.);
    let mut new_tops: Vec<Pixels> = Vec::with_capacity(heights.len());
    for (i, h) in heights.iter().enumerate() {
        let tbl = tables.get(i).and_then(Option::as_ref);
        let (top, bot) = line_pads(backgrounds[i], tbl);
        y += top;
        new_tops.push(y);
        let row_h = *h * rows[i] as f32;
        // A table's hover "+" add-row strip paints just below its
        // last row (see `TableAdds`). Keep that space inside the
        // element for a table near the document's end — outside
        // the element the strip's hitbox is masked off and its
        // clicks never reach the bounds-gated mouse handlers
        // (the add-column strip never has this problem: it
        // borrows horizontal space, which always exists).
        if let Some(t) = tbl
            && t.is_last
            && !t.col_widths.is_empty()
        {
            needed = needed.max(y + row_h + (*h * 0.75).max(px(12.)));
        }
        y += row_h + bot;
    }
    (new_tops, y, needed)
}

/// Scroll anchoring (Cditor's anchor-restore): the SAME content generation as
/// the last paint means no edit happened — so a height difference here is an
/// ASYNC change (a math/mermaid/image raster arriving, collapsing raw lines to
/// a rendered block). If the first changed row sits above the window's
/// viewport, shift this presentation's scroll offset during measure, before
/// the scroll container places its children.
fn compensate_scroll(
    editor: &EditorState,
    new_tops: &[Pixels],
    total: Pixels,
    wrap_width: Option<Pixels>,
    sf: f32,
) {
    // ONLY at the real, final width: taffy also measures at
    // intrinsic (None / min-content) widths, whose unwrapped
    // heights diverge wildly from the wrapped layout at a narrow
    // window — a compensation from one of those yanks the scroll
    // offset by thousands of px and fights the user's scrolling.
    if let Some(last) = editor.frame.last_bounds
        && editor.frame.content_gen == editor.frame.last_paint_gen
        && !editor.frame.line_tops.is_empty()
        && wrap_width.is_some_and(|w| (w - last.size.width).abs() < px(1.))
    {
        let n = new_tops.len().min(editor.frame.line_tops.len());
        let viewport_top = editor.frame.scroll.bounds().top();
        let anchor = editor
            .frame
            .line_tops
            .iter()
            .position(|top| last.origin.y + *top >= viewport_top)
            .unwrap_or(n);
        // Changes below the visible row cannot cancel or amplify
        // an independent asynchronous height change above it.
        let delta = new_tops
            .get(anchor)
            .zip(editor.frame.line_tops.get(anchor))
            .map_or(total - last.size.height, |(new, old)| *new - *old);
        // Layout rounds the painted bounds to device pixels, so
        // `last` can differ from `total` by just under one device
        // px with nothing changed (seen: 0.9998 at 1x). Compensating
        // that noise on every re-render crept the feed (#122);
        // 1.5 device px leaves margin over it.
        if delta.abs() >= px(1.5 / sf) {
            let j = (0..n)
                .find(|&k| (new_tops[k] - editor.frame.line_tops[k]).abs() > px(0.5))
                .unwrap_or(n);
            // Debug tap (ZORITE_WINDOW_DEBUG=1): a height change
            // with NO edit and NO width change means a cache
            // mismatch or an async raster — log which line moved.
            if std::env::var_os("ZORITE_WINDOW_DEBUG").is_some() {
                let line_txt = editor
                    .content
                    .split('\n')
                    .nth(j)
                    .unwrap_or("")
                    .chars()
                    .take(60)
                    .collect::<String>();
                eprintln!(
                    "[wdbg] gen={} delta={:?} first_changed_row={} old_top={:?} new_top={:?} line={:?}",
                    editor.frame.content_gen,
                    delta,
                    j,
                    editor.frame.line_tops.get(j),
                    new_tops.get(j),
                    line_txt
                );
            }
            let changed_y = editor
                .frame
                .line_tops
                .get(j)
                .copied()
                .unwrap_or(last.size.height);
            if last.origin.y + changed_y < viewport_top && !editor.frame.compensated.get() {
                editor.frame.compensated.set(true);
                let scroll = &editor.frame.scroll;
                let mut offset = scroll.offset();
                offset.y -= delta;
                scroll.set_offset(offset);
            }
        }
    }
}
