//! The paint stage: drawing the rows + widgets and committing the frame's hit
//! map. Split from the former `element.rs` monolith by Markdown construct
//! family: line backgrounds (code cards, quote bars, rules), gutter
//! decorations (nesting guides, list bullets, task checkboxes, alert markers,
//! heading/callout fold chevrons), the widget dispatch (tables, images, file
//! chips, Mermaid status, property panels), text + inline `$…$` formulas,
//! then the table affordance chrome, the gutter drag grip, the window-level
//! drag listeners, the code-card chips, and finally the per-frame commit
//! (the fresh geometry moves into `EditorState` by take, never by per-frame
//! clone of the layout vectors).

use super::props::{paint_chip, paint_prop_panel};
use super::*;

pub(crate) fn paint_editor(
    el: &EditorElement,
    bounds: Bounds<Pixels>,
    prepaint: &mut PrepaintState,
    window: &mut Window,
    cx: &mut App,
) {
    let focus = el.editor.read(cx).focus_handle.clone();
    window.handle_input(
        &focus,
        ElementInputHandler::new(bounds, el.editor.clone()),
        cx,
    );

    let style = window.text_style();
    let font = style.font();
    let text_color = style.color;
    let font_size = style.font_size.to_pixels(window.rem_size());
    let base_lh = font_size * LINE_HEIGHT_RATIO;
    let viewport_h = window.viewport_size().height;

    // Pass 1: paint fenced-code cards, table shading, and inline-code/highlight
    // run backgrounds BEFORE search/selection quads so selections stay visible
    // over code blocks and highlighted text.
    for (i, ((line, top), lh)) in prepaint
        .wrapped
        .iter()
        .zip(prepaint.line_tops.iter())
        .zip(prepaint.line_heights.iter())
        .enumerate()
    {
        let origin = point(bounds.origin.x, bounds.origin.y + *top);
        let advance = *lh * prepaint.wrap_rows.get(i).copied().unwrap_or(1) as f32;
        if origin.y + advance + px(64.) < px(0.) || origin.y - px(64.) > viewport_h {
            continue;
        }
        let rtl = prepaint
            .rtl
            .get(i)
            .and_then(Option::as_ref)
            .filter(|r| r.base_rtl);
        let shift = rtl
            .and_then(|r| r.shifts.first().copied())
            .unwrap_or(px(0.));
        paint_code_card_bg(i, prepaint, origin, lh, window);
        if let Some(t) = prepaint.tables.get(i).and_then(Option::as_ref) {
            paint_table_row_cell_bg(i, t, origin, bounds, lh, &el.editor, window, cx);
        } else if prepaint.widgets.get(i).and_then(Option::as_ref).is_none() {
            paint_text_line_bg(i, line, rtl, shift, origin, lh, prepaint, window, cx);
        }
    }

    for quad in prepaint.search.drain(..) {
        window.paint_quad(quad);
    }
    for sel in prepaint.selections.drain(..) {
        window.paint_quad(sel);
    }
    for underline in prepaint.marked.drain(..) {
        window.paint_quad(underline);
    }

    // Inline-image resize: the accent color for the corner grips, and the
    // active drag (if any) so the dragged image paints at its live width.
    let grip_color = el
        .editor
        .read(cx)
        .markdown_style
        .as_ref()
        .map_or(text_color, |s| s.link);
    let image_resize = el.editor.read(cx).affordances.image_resize;
    // Window-space bounds of each painted image + its logical line, collected
    // for the next frame's grip hit-testing (committed below).
    let mut image_rects: Vec<(usize, Bounds<Pixels>)> = Vec::new();
    // Window-space bounds of each inline `$…$` formula + its absolute range and LaTeX, for
    // the next frame's click-to-edit hit-testing + seating the structural editor.
    let mut inline_math_rects: Vec<(Range<usize>, SharedString, Bounds<Pixels>)> = Vec::new();
    // Property-panel pill bounds + targets (click-to-open) and row bounds
    // (hover change-detection), committed for the next frame's handlers.
    let mut prop_pill_rects: Vec<(Bounds<Pixels>, zorite_markdown::syntax::LinkHit)> = Vec::new();
    let mut prop_row_rects: Vec<(Bounds<Pixels>, usize)> = Vec::new();
    // The span being structurally edited (if any): skip painting its raster — the seated
    // editor overlays its spot.
    let editing_inline = el
        .editor
        .read(cx)
        .editing_inline
        .as_ref()
        .map(|e| e.range.clone());
    // Window-space box bounds of each painted task checkbox + its line, for the
    // next frame's click-to-toggle hit-testing (committed below).
    let mut checkbox_rects: Vec<(usize, Bounds<Pixels>)> = Vec::new();
    // A foldable callout's chevron bounds (from this paint), so a click can
    // flip its fold char — the checkbox-toggle pattern.
    let mut alert_fold_rects: Vec<(usize, Bounds<Pixels>)> = Vec::new();
    let mut heading_fold_rects: Vec<(usize, Bounds<Pixels>)> = Vec::new();
    // Logseq-style list nesting guides: `outline` holds the bullet x of each
    // active ancestor level, so a faint vertical line can drop from each down
    // through its descendants. Popped on dedent, reset off the list.
    let mut outline: Vec<Pixels> = Vec::new();
    for (i, ((line, top), lh)) in prepaint
        .wrapped
        .iter()
        .zip(prepaint.line_tops.iter())
        .zip(prepaint.line_heights.iter())
        .enumerate()
    {
        let origin = point(bounds.origin.x, bounds.origin.y + *top);
        // Right-to-left row (#66): its text right-aligns by `shift`, and
        // every gutter decoration mirrors to the other side so the markers
        // sit where the text now starts — matching the reader.
        // Only a line that READS right-to-left mirrors its gutter: an
        // English line with an Arabic tail has an `RtlRow` (for the bidi
        // maps) but keeps its bullet, guides and rule on the left.
        let rtl = prepaint
            .rtl
            .get(i)
            .and_then(Option::as_ref)
            .filter(|r| r.base_rtl);
        // Row 0's shift: the gutter decorations mirror against the line as
        // a whole, and the per-row shifts only move the text.
        let shift = rtl
            .and_then(|r| r.shifts.first().copied())
            .unwrap_or(px(0.));
        let content_w = bounds.size.width;
        paint_nesting_guides(
            prepaint.marks.get(i).copied().flatten(),
            rtl,
            content_w,
            origin,
            lh,
            &mut outline,
            window,
        );
        // Windowed paint: a line fully outside the window's viewport paints
        // nothing. The guide bookkeeping above still runs — a visible list
        // row's ancestor guides can start above the viewport. The slack
        // covers code-box pads and table affordances that overhang the row.
        let advance = *lh * prepaint.wrap_rows.get(i).copied().unwrap_or(1) as f32;
        if origin.y + advance + px(64.) < px(0.) || origin.y - px(64.) > viewport_h {
            continue;
        }
        paint_quote_bar(i, prepaint, rtl, content_w, origin, lh, window);
        paint_heading_chevron(
            i,
            prepaint,
            origin,
            lh,
            text_color,
            &font,
            font_size,
            window,
            cx,
            &mut heading_fold_rects,
        );
        paint_alert_marker(
            i,
            prepaint,
            rtl,
            content_w,
            origin,
            lh,
            &font,
            font_size,
            window,
            cx,
            &mut alert_fold_rects,
        );
        paint_rule(i, prepaint, content_w, origin, lh, window);
        paint_list_bullet(
            i, prepaint, rtl, content_w, origin, lh, &font, font_size, window, cx,
        );
        paint_checkbox(
            i,
            prepaint,
            rtl,
            content_w,
            origin,
            lh,
            font_size,
            window,
            &mut checkbox_rects,
        );
        if let Some(t) = prepaint.tables.get(i).and_then(Option::as_ref) {
            paint_table_row_cell(
                i, t, prepaint, origin, bounds, lh, &font, font_size, base_lh, text_color,
                &el.editor, window, cx,
            );
        } else if let Some(Block::Image(w)) = prepaint.widgets.get(i).and_then(Option::as_ref) {
            paint_block_image(
                i,
                w,
                prepaint,
                origin,
                bounds,
                image_resize,
                grip_color,
                window,
                &mut image_rects,
            );
        } else if let Some(Block::Chip {
            label,
            link,
            bg,
            border,
            ..
        }) = prepaint.widgets.get(i).and_then(Option::as_ref)
        {
            // File chip (e.g. a PDF embed): a rounded button with the label.
            paint_chip(
                label, *link, *bg, *border, origin, *lh, &font, font_size, window, cx,
            );
            if let Some(hb) = prepaint
                .chip_grips
                .iter()
                .find_map(|(l, hb)| (*l == i).then_some(hb))
            {
                window.set_cursor_style(CursorStyle::PointingHand, hb);
            }
        } else if let Some(Block::MermaidStatus {
            text, line_height, ..
        }) = prepaint.widgets.get(i).and_then(Option::as_ref)
        {
            paint_mermaid_status(text, *line_height, origin, window, cx);
        } else if let Some(Block::Properties(p)) = prepaint.widgets.get(i).and_then(Option::as_ref)
        {
            paint_prop_panel(
                p,
                origin,
                content_w,
                &font,
                font_size,
                window,
                cx,
                &mut prop_pill_rects,
                &mut prop_row_rects,
                i,
            );
        } else {
            paint_text_line(
                i,
                line,
                rtl,
                shift,
                origin,
                lh,
                prepaint,
                &editing_inline,
                window,
                cx,
                &mut inline_math_rects,
            );
        }
    }

    if focus.is_focused(window)
        && let Some(cursor) = prepaint.cursor.take()
    {
        window.paint_quad(cursor);
    }
    // Zones are committed every frame so on_mouse_move knows where the tables are.
    let (
        table_row_add_rects,
        table_col_add_rects,
        table_row_del,
        table_col_del,
        table_col_resize_rects,
    ) = paint_table_chrome(prepaint, &el.editor, window, cx);
    paint_gutter_drag(&el.editor, bounds, prepaint, text_color, window, cx);
    install_drag_listeners(
        &el.editor,
        prepaint.grip,
        prepaint.grip_hb.clone(),
        el.editor.read(cx).markdown_style.is_some(),
        el.editor.read(cx).affordances.line_drag.is_some(),
        window,
    );
    let code_chip_rects = paint_code_card_chips(prepaint, &font, text_color, window, cx);
    // Hovering an inline link shows a hand, like the reading view (the
    // hitboxes come from prepaint; cursor styles must be set during paint).
    if el.editor.read(cx).host_links {
        for hb in &prepaint.link_grips {
            window.set_cursor_style(CursorStyle::PointingHand, hb);
        }
    }
    for hb in &prepaint.prop_pill_grips {
        window.set_cursor_style(CursorStyle::PointingHand, hb);
    }
    for hb in &prepaint.inline_image_grips {
        window.set_cursor_style(CursorStyle::PointingHand, hb);
    }
    let reveal = commit_frame(
        &el.editor,
        bounds,
        prepaint,
        cx,
        image_rects,
        inline_math_rects,
        prop_pill_rects,
        prop_row_rects,
        checkbox_rects,
        alert_fold_rects,
        heading_fold_rects,
        table_row_add_rects,
        table_col_add_rects,
        table_row_del,
        table_col_del,
        table_col_resize_rects,
        code_chip_rects,
        base_lh,
        font_size,
        &font,
    );
    if let Some((key, offset)) = reveal {
        let entity = el.editor.clone();
        window.on_next_frame(move |_, cx| {
            entity.update(cx, |editor, cx| {
                if editor.frame.revealed_caret == Some(key) && !editor.frame.reveal_requested {
                    editor.frame.scroll.set_offset(offset);
                    cx.notify();
                }
            });
        });
    }
    let editor = el.editor.read(cx);
    let viewport = editor.frame.scroll_viewport;
    let viewport_h = f32::from(viewport.size.height);
    let content_h = f32::from(editor.frame.scroll.max_offset().y + viewport.size.height);
    if viewport.size.width > px(0.) && content_h > viewport_h {
        let track_w = px(4.);
        let track_right = viewport.right() - px(8.);
        let track = Bounds::new(
            point(track_right - track_w, viewport.top()),
            size(track_w, viewport.size.height),
        );
        window.paint_quad(fill(track, text_color.opacity(0.08)));
        let thumb_h = (viewport_h * viewport_h / content_h).max(24.);
        let max_offset = content_h - viewport_h;
        let scrolled = (-f32::from(editor.frame.scroll.offset().y)).clamp(0., max_offset);
        let thumb = Bounds::new(
            point(
                track_right - track_w,
                viewport.top() + px(scrolled / max_offset * (viewport_h - thumb_h)),
            ),
            size(track_w, px(thumb_h)),
        );
        window.paint_quad(fill(thumb, text_color.opacity(0.35)).corner_radii(Corners::all(px(2.))));
    }
}

// ===== Gutter decorations: nesting guides, quote bar, fold chevrons, rules, bullets, checkboxes =====
/// Nesting guides for a list/task row: a thin vertical line at each ancestor
/// bullet's x, spanning this row (contiguous rows stack into a continuous
/// guide). The `outline` stack is the caller's — it spans the whole row walk
/// so a visible row's guides can start above the viewport.
fn paint_nesting_guides(
    mark: Option<LineMark>,
    rtl: Option<&RtlRow>,
    content_w: Pixels,
    origin: Point<Pixels>,
    lh: &Pixels,
    outline: &mut Vec<Pixels>,
    window: &mut Window,
) {
    // Nesting guides for a list/task row: a thin vertical line at each
    // ancestor bullet's x, spanning this row (contiguous rows stack into a
    // continuous guide).
    match mark {
        Some(LineMark::List {
            bullet_x, color, ..
        })
        | Some(LineMark::Check {
            bullet_x, color, ..
        }) => {
            while outline.last().is_some_and(|&x| x >= bullet_x) {
                outline.pop();
            }
            let guide = Hsla {
                a: color.a * 0.5,
                ..color
            };
            for &gx in outline.iter() {
                let g = gx + px(3.);
                let g = if rtl.is_some() {
                    rtl_marker_x(content_w, g, px(1.))
                } else {
                    g
                };
                window.paint_quad(fill(
                    Bounds::new(point(origin.x + g, origin.y), size(px(1.), *lh)),
                    guide,
                ));
            }
            outline.push(bullet_x);
        }
        _ => outline.clear(),
    }
}

/// Fenced code block: one rounded, content-fit box (sized to the widest line,
/// like a table). The first line rounds + pads the top, the last rounds +
/// pads the bottom; the pad fills the layout gap reserved for it (see
/// `code_pads`), so the caret geometry stays text-height and the box never
/// overlaps an adjacent line.
fn paint_code_card_bg(
    i: usize,
    prepaint: &PrepaintState,
    origin: Point<Pixels>,
    lh: &Pixels,
    window: &mut Window,
) {
    // Fenced code block: one rounded, content-fit box (sized to the
    // widest line, like a table). The first line rounds + pads the top, the
    // last rounds + pads the bottom; the pad fills the layout gap reserved
    // for it (see `code_pads`), so the caret geometry stays text-height and
    // the box never overlaps an adjacent line.
    if let Some(cb) = prepaint.backgrounds.get(i).copied().flatten() {
        let r = px(6.);
        let z = px(0.);
        let (top_pad, bot_pad) = code_pads(Some(cb));
        let corners = Corners {
            top_left: if cb.top { r } else { z },
            top_right: if cb.top { r } else { z },
            bottom_left: if cb.bottom { r } else { z },
            bottom_right: if cb.bottom { r } else { z },
        };
        let box_origin = point(origin.x, origin.y - top_pad);
        // A soft-wrapped source row owns every one of its visual rows;
        // painting only the first leaves the continuation outside the card.
        let box_size = size(
            cb.width,
            *lh * prepaint.wrap_rows[i] as f32 + top_pad + bot_pad,
        );
        window.paint_quad(fill(Bounds::new(box_origin, box_size), cb.color).corner_radii(corners));
    }
}

/// Blockquote: a muted 2px left border down the line (the body is inset past
/// it by QUOTE_INSET). RTL: the rule belongs on the right, where the text
/// starts — the reader's `border_r_2`.
fn paint_quote_bar(
    i: usize,
    prepaint: &PrepaintState,
    rtl: Option<&RtlRow>,
    content_w: Pixels,
    origin: Point<Pixels>,
    lh: &Pixels,
    window: &mut Window,
) {
    // Blockquote: a muted 2px left border down the line (the body is inset
    // past it by QUOTE_INSET).
    if let Some(LineMark::Quote { bar, .. }) = prepaint.marks.get(i).copied().flatten() {
        // RTL: the rule belongs on the right, where the text starts —
        // the reader's `border_r_2` (see `zorite-markdown`'s blockquote).
        let bx = origin.x + rtl.map_or(px(0.), |_| content_w - px(2.));
        window.paint_quad(fill(
            Bounds::new(point(bx, origin.y), size(px(2.), *lh)),
            bar,
        ));
    }
}

/// Heading fold chevron (hovered or folded headings only): painted past the
/// heading text, muted; clicking toggles the fold (rects committed for
/// on_mouse_down, like the callout chevron).
#[allow(clippy::too_many_arguments)]
fn paint_heading_chevron(
    i: usize,
    prepaint: &PrepaintState,
    origin: Point<Pixels>,
    lh: &Pixels,
    text_color: Hsla,
    font: &Font,
    font_size: Pixels,
    window: &mut Window,
    cx: &mut App,
    rects: &mut Vec<(usize, Bounds<Pixels>)>,
) {
    // Heading fold chevron (hovered or folded headings only): painted
    // past the heading text, muted; clicking toggles the fold (rects
    // committed for on_mouse_down, like the callout chevron).
    if let Some(&(_, folded, cx0)) = prepaint.heading_chevrons.iter().find(|(r, _, _)| *r == i) {
        let glyph: SharedString = if folded { "▸" } else { "▾" }.into();
        let mut muted = text_color;
        muted.a *= 0.45;
        let run = TextRun {
            len: glyph.len(),
            font: font.clone(),
            color: muted,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shaped = window
            .text_system()
            .shape_line(glyph, font_size, &[run], None);
        let gx = origin.x + cx0;
        let _ = shaped.paint(
            point(gx, origin.y),
            *lh,
            gpui_kit::TextAlign::Left,
            None,
            window,
            cx,
        );
        rects.push((
            i,
            Bounds::new(point(gx, origin.y), size(shaped.width(), *lh)),
        ));
        if let Some(hb) = prepaint
            .heading_fold_grips
            .iter()
            .find_map(|(l, hb)| (*l == i).then_some(hb))
        {
            window.set_cursor_style(CursorStyle::PointingHand, hb);
        }
    }
}

/// Alert marker line: the colored bar plus a bold label ("Note", …) where the
/// hidden `[!NOTE]` marker was. An RTL callout mirrors like the rest of the
/// row: the rule on the right, then the icon, then the label running
/// leftward. A foldable callout also paints its chevron; clicking flips the
/// fold char in the source (rects committed for on_mouse_down).
#[allow(clippy::too_many_arguments)]
fn paint_alert_marker(
    i: usize,
    prepaint: &PrepaintState,
    rtl: Option<&RtlRow>,
    content_w: Pixels,
    origin: Point<Pixels>,
    lh: &Pixels,
    font: &Font,
    font_size: Pixels,
    window: &mut Window,
    cx: &mut App,
    rects: &mut Vec<(usize, Bounds<Pixels>)>,
) {
    // Alert marker line: the colored bar plus a bold label ("Note", …)
    // where the hidden `[!NOTE]` marker was.
    if let Some(LineMark::Alert {
        bar,
        label,
        kind,
        fold,
        chevron_x,
        ..
    }) = prepaint.marks.get(i).copied().flatten()
    {
        // An RTL callout mirrors like the rest of the row: the rule on
        // the right, then the icon, then the label running leftward.
        // Each box is reflected on its own, which reverses their order
        // exactly as intended.
        let amirror = |x: Pixels, w: Pixels| {
            if rtl.is_some() {
                origin.x + content_w - (x - origin.x) - w
            } else {
                x
            }
        };
        window.paint_quad(fill(
            Bounds::new(
                point(amirror(origin.x, px(2.)), origin.y),
                size(px(2.), *lh),
            ),
            bar,
        ));
        // Foldable callout: a chevron after the label; clicking it flips
        // the `-`/`+` in the source (rects committed for on_mouse_down).
        if let Some(folded) = fold {
            let glyph: SharedString = if folded { "▸" } else { "▾" }.into();
            let run = TextRun {
                len: glyph.len(),
                font: font.clone(),
                color: bar,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let shaped = window
                .text_system()
                .shape_line(glyph, font_size, &[run], None);
            let cx0 = amirror(origin.x + chevron_x, shaped.width());
            let _ = shaped.paint(
                point(cx0, origin.y),
                *lh,
                gpui_kit::TextAlign::Left,
                None,
                window,
                cx,
            );
            rects.push((
                i,
                Bounds::new(point(cx0, origin.y), size(shaped.width(), *lh)),
            ));
            if let Some(hb) = prepaint
                .alert_fold_grips
                .iter()
                .find_map(|(l, hb)| (*l == i).then_some(hb))
            {
                window.set_cursor_style(CursorStyle::PointingHand, hb);
            }
        }
        // Icon (when the host supplies asset paths), then the bold label.
        let mut label_x = origin.x + font_size * (QUOTE_INSET / 16.);
        if let Some(icons) = &prepaint.alert_icons {
            let sz = font_size;
            let icon_bounds = Bounds::new(
                point(amirror(label_x, sz), origin.y + (*lh - sz) / 2.),
                size(sz, sz),
            );
            let _ = window.paint_svg(
                icon_bounds,
                icons.get(kind),
                None,
                gpui_kit::TransformationMatrix::unit(),
                bar,
                cx,
            );
            label_x += sz + font_size * (6. / 16.);
        }
        let label_font = Font {
            weight: FontWeight::BOLD,
            ..font.clone()
        };
        let run = TextRun {
            len: label.len(),
            font: label_font,
            color: bar,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shaped = window
            .text_system()
            .shape_line(label.into(), font_size, &[run], None);
        let _ = shaped.paint(
            point(amirror(label_x, shaped.width()), origin.y),
            *lh,
            gpui_kit::TextAlign::Left,
            None,
            window,
            cx,
        );
    }
}

/// Thematic break: a 1px full-width divider centered in the row.
fn paint_rule(
    i: usize,
    prepaint: &PrepaintState,
    content_w: Pixels,
    origin: Point<Pixels>,
    lh: &Pixels,
    window: &mut Window,
) {
    // Thematic break: a 1px full-width divider centered in the row.
    if let Some(LineMark::Rule(c)) = prepaint.marks.get(i).copied().flatten() {
        let y = origin.y + (*lh - px(1.)) / 2.;
        let w = content_w;
        window.paint_quad(fill(Bounds::new(point(origin.x, y), size(w, px(1.))), c));
    }
}

/// List item: a muted bullet (`•`) or number (`N.`) glyph where the hidden
/// source marker began (`bullet_x`); the body is inset to the measured prefix
/// width so it lines up with the raw line. An RTL row mirrors the marker to
/// the side the text starts at.
#[allow(clippy::too_many_arguments)]
fn paint_list_bullet(
    i: usize,
    prepaint: &PrepaintState,
    rtl: Option<&RtlRow>,
    content_w: Pixels,
    origin: Point<Pixels>,
    lh: &Pixels,
    font: &Font,
    font_size: Pixels,
    window: &mut Window,
    cx: &mut App,
) {
    // List item: a muted bullet (`•`) or number (`N.`) glyph where the
    // hidden source marker began (`bullet_x`); the body is inset to the
    // measured prefix width so it lines up with the raw line.
    if let Some(LineMark::List {
        bullet_x,
        ordered,
        num,
        level,
        color,
        ..
    }) = prepaint.marks.get(i).copied().flatten()
    {
        let glyph: SharedString = if ordered {
            zorite_markdown::syntax::ordered_marker(level, num).into()
        } else {
            "•".into()
        };
        let run = TextRun {
            len: glyph.len(),
            font: font.clone(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shaped = window
            .text_system()
            .shape_line(glyph, font_size, &[run], None);
        let bx = if rtl.is_some() {
            rtl_marker_x(content_w, bullet_x, shaped.width())
        } else {
            bullet_x
        };
        let _ = shaped.paint(
            point(origin.x + bx, origin.y),
            *lh,
            gpui_kit::TextAlign::Left,
            None,
            window,
            cx,
        );
    }
}

/// Task item: a crisp cap-height box (custom-drawn, not a font glyph so it
/// reads at the text's size) with a checkmark when done. Done: a solid
/// accent fill with a white check (Notion-style). Open: an empty outline.
#[allow(clippy::too_many_arguments)]
fn paint_checkbox(
    i: usize,
    prepaint: &PrepaintState,
    rtl: Option<&RtlRow>,
    content_w: Pixels,
    origin: Point<Pixels>,
    lh: &Pixels,
    font_size: Pixels,
    window: &mut Window,
    rects: &mut Vec<(usize, Bounds<Pixels>)>,
) {
    // Task item: a crisp cap-height box (custom-drawn, not a font glyph so
    // it reads at the text's size) with a checkmark when done.
    if let Some(LineMark::Check {
        bullet_x,
        checked,
        color,
        accent,
        ..
    }) = prepaint.marks.get(i).copied().flatten()
    {
        let sz = font_size * CHECKBOX_SCALE;
        let bx = origin.x + checkbox_x(bullet_x, sz, content_w, rtl.is_some());
        let by = origin.y + (*lh - sz) / 2.; // vertically centered on the line
        let box_bounds = Bounds::new(point(bx, by), size(sz, sz));
        rects.push((i, box_bounds));
        if let Some(hb) = prepaint
            .checkbox_grips
            .iter()
            .find_map(|(l, hb)| (*l == i).then_some(hb))
        {
            window.set_cursor_style(CursorStyle::PointingHand, hb);
        }
        // Done: a solid accent fill with a white check (Notion-style).
        // Open: an empty outline.
        window.paint_quad(PaintQuad {
            bounds: box_bounds,
            corner_radii: Corners::all(px(3.)),
            background: if checked {
                accent.into()
            } else {
                hsla(0., 0., 0., 0.).into()
            },
            border_widths: Edges::all(px(1.5)),
            border_color: if checked { accent } else { color },
            border_style: BorderStyle::Solid,
        });
        if checked {
            let s = f32::from(sz);
            let mut pb = PathBuilder::stroke(px(1.6));
            pb.move_to(point(bx + px(s * 0.24), by + px(s * 0.52)));
            pb.line_to(point(bx + px(s * 0.42), by + px(s * 0.70)));
            pb.line_to(point(bx + px(s * 0.76), by + px(s * 0.28)));
            if let Ok(path) = pb.build() {
                window.paint_path(path, gpui_kit::white());
            }
        }
    }
}

// ===== Tables: the grid row + the hover chrome =====
/// One table row: a wide table keeps natural columns, scrolled by sx and
/// clipped to the viewport — the row paints shifted under a mask. The header
/// row of a Grid-style table paints the whole table's rounded outer border
/// (one box around all its rows, matching the reading view), under its own
/// full-height mask.
#[allow(clippy::too_many_arguments)]
fn paint_table_row_cell_bg(
    i: usize,
    t: &TableRow,
    origin: Point<Pixels>,
    bounds: Bounds<Pixels>,
    lh: &Pixels,
    entity: &Entity<EditorState>,
    window: &mut Window,
    cx: &mut App,
) {
    let avail = bounds.size.width - px(TABLE_GUTTER);
    let (tleft, _) = table_visible_band(origin.x, bounds.size.width, t.rtl);
    let content_left = entity.read(cx).table_left(t, i, &bounds);
    let g = px(TABLE_GUTTER);
    let mask = gpui_kit::ContentMask {
        bounds: Bounds::new(point(tleft, origin.y - g), size(avail, *lh + g * 2.)),
    };
    window.with_content_mask(Some(mask), |window| {
        paint_table_row_bg(t, point(content_left, origin.y), *lh, window);
    });
}

#[allow(clippy::too_many_arguments)]
fn paint_table_row_cell(
    i: usize,
    t: &TableRow,
    prepaint: &PrepaintState,
    origin: Point<Pixels>,
    bounds: Bounds<Pixels>,
    lh: &Pixels,
    font: &Font,
    font_size: Pixels,
    base_lh: Pixels,
    text_color: Hsla,
    entity: &Entity<EditorState>,
    window: &mut Window,
    cx: &mut App,
) {
    // A wide table keeps natural columns, scrolled by sx and
    // clipped to the viewport — rows paint shifted under a mask.
    let table_w: Pixels = t.col_widths.iter().copied().sum();
    let avail = bounds.size.width - px(TABLE_GUTTER);
    // The clip band, on the side this table's gutter ISN'T (#66).
    let (tleft, _) = table_visible_band(origin.x, bounds.size.width, t.rtl);
    let content_left = entity.read(cx).table_left(t, i, &bounds);
    let g = px(TABLE_GUTTER);
    let mask = gpui_kit::ContentMask {
        bounds: Bounds::new(point(tleft, origin.y - g), size(avail, *lh + g * 2.)),
    };
    // The header row paints the whole table's rounded outer border
    // (one box around all its rows, matching the reading view) — for
    // the Grid style only; the others are box-less. It spans the
    // WHOLE table's height, so it gets its own full-height mask
    // (the per-row mask below would clip it to the header band).
    if t.is_header && matches!(t.style, markdown_syntax::TableStyle::Grid) {
        let mut total_h = px(0.);
        for j in i..prepaint.tables.len() {
            match prepaint.tables[j].as_ref() {
                Some(tr) => {
                    total_h += prepaint.line_heights[j];
                    if tr.is_last {
                        break;
                    }
                }
                None => break,
            }
        }
        let border_mask = gpui_kit::ContentMask {
            bounds: Bounds::new(point(tleft, origin.y - g), size(avail, total_h + g * 2.)),
        };
        window.with_content_mask(Some(border_mask), |window| {
            window.paint_quad(PaintQuad {
                bounds: Bounds::new(point(content_left, origin.y), size(table_w, total_h)),
                corner_radii: Corners::all(px(6.)),
                background: hsla(0., 0., 0., 0.).into(),
                border_widths: Edges::all(px(1.)),
                border_color: t.border,
                border_style: BorderStyle::Solid,
            });
        });
    }
    window.with_content_mask(Some(mask), |window| {
        paint_table_row(
            t,
            point(content_left, origin.y),
            *lh,
            font,
            font_size,
            base_lh,
            text_color,
            window,
            cx,
        );
    });
}

/// Table "+" affordances (issue #16): while the pointer is over a table's
/// hover zone, paint its add-row (below) + add-column (right) strips, cursor
/// them (unconditionally, so gpui applies the hand from its cached map as
/// the pointer moves onto a strip), and commit their rects for on_mouse_down.
/// The hovered strip fills; on_mouse_move drives the repaints (the editor
/// otherwise only repaints on the caret blink). The caret's cell gets a
/// quiet accent outline (under the hover outlines), hovered row/column get an
/// accent outline + a border pill with "+" / "−" (Cditor-style — no fill
/// highlight), and column-resize grips draw their border in the accent while
/// hovered or dragged.
fn paint_table_chrome(
    prepaint: &PrepaintState,
    entity: &Entity<EditorState>,
    window: &mut Window,
    cx: &App,
) -> (
    Vec<(Bounds<Pixels>, usize)>,
    Vec<(Bounds<Pixels>, usize, usize)>,
    Option<(Bounds<Pixels>, usize)>,
    Option<(Bounds<Pixels>, usize, usize)>,
    Vec<(Bounds<Pixels>, usize, usize, f32)>,
) {
    let mouse = window.mouse_position();
    let mut table_row_add_rects: Vec<(Bounds<Pixels>, usize)> = Vec::new();
    let mut table_col_add_rects: Vec<(Bounds<Pixels>, usize, usize)> = Vec::new();
    let mut table_row_del = None;
    let mut table_col_del = None;
    // The caret's cell: a quiet accent outline (under the hover outlines).
    if let Some((rect, accent)) = prepaint.caret_cell {
        let mut c = accent;
        c.a *= 0.55;
        paint_table_outline(rect, c, window);
    }
    // Hovered row/column: accent outline + a border pill with "+" / "−"
    // (Cditor-style — no fill highlight). Rects are committed for
    // on_mouse_down's hit-tests.
    if let Some(a) = &prepaint.row_aff {
        paint_table_outline(a.outline, a.accent, window);
        paint_table_pill(a, false, mouse, window);
        window.set_cursor_style(CursorStyle::PointingHand, &a.plus_hit);
        window.set_cursor_style(CursorStyle::PointingHand, &a.minus_hit);
        table_row_add_rects.push((a.plus, a.row));
        table_row_del = Some((a.minus, a.row));
    }
    if let Some(a) = &prepaint.col_aff {
        paint_table_outline(a.outline, a.accent, window);
        paint_table_pill(a, true, mouse, window);
        window.set_cursor_style(CursorStyle::PointingHand, &a.plus_hit);
        window.set_cursor_style(CursorStyle::PointingHand, &a.minus_hit);
        table_col_add_rects.push((a.plus, a.row, a.col));
        table_col_del = Some((a.minus, a.row, a.col));
    }
    // Column-resize grips: a resize cursor over each border band; the
    // hovered/dragged border draws in the accent color.
    let mut table_col_resize_rects: Vec<(Bounds<Pixels>, usize, usize, f32)> = Vec::new();
    let dragging = entity.read(cx).affordances.table_col_resize;
    for gr in &prepaint.col_resize_grips {
        window.set_cursor_style(CursorStyle::ResizeLeftRight, &gr.hit);
        table_col_resize_rects.push((gr.band, gr.header_row, gr.col, gr.width));
        let active = dragging.is_some_and(|d| d.header_row == gr.header_row && d.col == gr.col)
            || (dragging.is_none() && gr.band.contains(&mouse));
        if active {
            window.paint_quad(fill(
                Bounds::new(
                    point(gr.x - px(1.), gr.top),
                    size(px(2.), gr.bottom - gr.top),
                ),
                gr.accent,
            ));
        }
    }

    // Wide-table scroll thumbs (from prepaint): the slim track under a
    // wide table's last row, with a hand cursor over its grab band —
    // draggable (see `on_mouse_down`).
    for (thumb, hb) in &prepaint.table_thumbs {
        window.paint_quad(fill(thumb.rect, thumb.color).corner_radii(Corners::all(px(1.5))));
        window.set_cursor_style(CursorStyle::PointingHand, hb);
    }
    (
        table_row_add_rects,
        table_col_add_rects,
        table_row_del,
        table_col_del,
        table_col_resize_rects,
    )
}

// ===== Block widgets: inline image, file chip, Mermaid status, property panel =====
/// Inline image (W4a): paint the decoded image instead of source, inset to
/// the row's gutter so a list-item image sits past its bullet (painted above,
/// like any list row). While its grip is being dragged, preview the live
/// width (aspect-preserved from the saved size) instead of the saved
/// `{width=N}` — the source isn't rewritten until release. A draggable
/// corner grip (accent square) + the resize cursor over it, via the hitbox
/// inserted in prepaint; the bounds are recorded for the next frame's grip
/// hit-testing. Honor the block's horizontal alignment within the content
/// width (display math centers by default).
#[allow(clippy::too_many_arguments)]
fn paint_block_image(
    i: usize,
    w: &BlockImg,
    prepaint: &PrepaintState,
    origin: Point<Pixels>,
    bounds: Bounds<Pixels>,
    image_resize: Option<ImageResize>,
    grip_color: Hsla,
    window: &mut Window,
    rects: &mut Vec<(usize, Bounds<Pixels>)>,
) {
    // Inline image (W4a): paint the decoded image instead of source,
    // inset to the row's gutter so a list-item image sits past its
    // bullet (painted above, like any list row).
    let inset = row_inset(
        prepaint.backgrounds.get(i).copied().flatten(),
        prepaint.marks.get(i).copied().flatten(),
    );
    // While this image's grip is being dragged, preview the live width
    // (aspect-preserved from the saved size) instead of the saved
    // `{width=N}` — the source isn't rewritten until release.
    let (img_w, img_h) = image_display_size(w, image_resize, i);
    // Honor the block's horizontal alignment within the content width. Display math
    // centers by default; left/right come from its `<!-- math:ALIGN -->` marker. A
    // real image is always `Left` (it sits at the row's inset).
    let slack = bounds.size.width - img_w;
    let img_x = match w.align {
        _ if slack <= px(0.) => origin.x + inset,
        MathAlign::Left => origin.x + inset,
        MathAlign::Center => origin.x + px(f32::from(slack) / 2.0),
        MathAlign::Right => origin.x + slack,
    };
    let img_bounds = Bounds::new(
        point(img_x, origin.y + px(IMG_ROW_PAD / 2.)),
        size(img_w, img_h),
    );
    let _ = window.paint_image(
        img_bounds,
        img_bounds,
        Corners::default(),
        w.img.clone(),
        0,
        false,
    );
    // A draggable corner grip (accent square) + the resize cursor over it,
    // via the hitbox inserted in prepaint. Recorded in `image_rects` for the
    // next frame's grip hit-testing. Skipped for non-resizable blocks (math),
    // keeping `image_grips` parallel to `image_rects`.
    if w.resizable {
        let grip = EditorState::image_grip(img_bounds);
        window.paint_quad(fill(grip, grip_color).corner_radii(Corners::all(px(3.))));
        if let Some(hitbox) = prepaint.image_grips.get(rects.len()) {
            window.set_cursor_style(CursorStyle::ResizeLeftRight, hitbox);
        }
        rects.push((i, img_bounds));
    }
}

/// A Mermaid block's loading/error status: its shaped message lines paint on
/// the block's opening row (the raw source stays editable underneath).
fn paint_mermaid_status(
    text: &std::rc::Rc<Vec<WrappedLine>>,
    line_height: Pixels,
    origin: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let mut y = origin.y;
    for line in text.iter() {
        let _ = line.paint(
            point(origin.x, y),
            line_height,
            gpui_kit::TextAlign::Left,
            None,
            window,
            cx,
        );
        y += line_height * (line.wrap_boundaries().len() + 1) as f32;
    }
}

/// A plain text row (or the raw-on-caret reveal): run backgrounds (the
/// inline-code tint) paint separately from the glyphs. An RTL row paints its
/// own rows in reading order (`paint_row` draws the run backgrounds too).
/// Inline `$…$` formulas paint their typeset rasters over their spacers,
/// centered on the text row; each formula's window bounds are recorded for
/// click-to-edit (the one being edited shows the seated editor instead of
/// its raster).
#[allow(clippy::too_many_arguments)]
fn paint_text_line_bg(
    i: usize,
    line: &WrappedLine,
    rtl: Option<&RtlRow>,
    shift: Pixels,
    origin: Point<Pixels>,
    lh: &Pixels,
    prepaint: &PrepaintState,
    window: &mut Window,
    cx: &mut App,
) {
    let inset = row_inset(
        prepaint.backgrounds.get(i).copied().flatten(),
        prepaint.marks.get(i).copied().flatten(),
    );
    let text_origin = point(origin.x + inset + shift, origin.y);
    if let Some(r) = rtl {
        for (k, row) in r.rows.iter().enumerate() {
            crate::bidi::paragraph::paint_row_background(
                row,
                point(
                    origin.x + inset + r.shifts.get(k).copied().unwrap_or(px(0.)),
                    origin.y + *lh * k,
                ),
                *lh,
                window,
                cx,
            );
        }
    } else {
        let _ = line.paint_background(
            text_origin,
            *lh,
            gpui_kit::TextAlign::Left,
            None,
            window,
            cx,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_text_line(
    i: usize,
    line: &WrappedLine,
    rtl: Option<&RtlRow>,
    shift: Pixels,
    origin: Point<Pixels>,
    lh: &Pixels,
    prepaint: &PrepaintState,
    editing_inline: &Option<Range<usize>>,
    window: &mut Window,
    cx: &mut App,
    rects: &mut Vec<(Range<usize>, SharedString, Bounds<Pixels>)>,
) {
    // Code blocks + gutter marks inset their text (kept in sync with
    // `EditorState::line_inset` / the fresh prepaint inset).
    let inset = row_inset(
        prepaint.backgrounds.get(i).copied().flatten(),
        prepaint.marks.get(i).copied().flatten(),
    );
    let text_origin = point(origin.x + inset + shift, origin.y);
    if let Some(r) = rtl {
        // Our own rows, painted in reading order — gpui's wrapping
        // is not used for this line at all (#66). Each row is
        // right-aligned on its own, so a short last row sits
        // further right than a full one. Backgrounds were painted in Pass 1.
        for (k, row) in r.rows.iter().enumerate() {
            crate::bidi::paragraph::paint_row_text(
                row,
                point(
                    origin.x + inset + r.shifts.get(k).copied().unwrap_or(px(0.)),
                    origin.y + *lh * k,
                ),
                *lh,
                window,
                cx,
            );
        }
    } else {
        let _ = line.paint(
            text_origin,
            *lh,
            gpui_kit::TextAlign::Left,
            None,
            window,
            cx,
        );
    }
    // Inline `$…$` formulas: paint each typeset raster over its spacer, centered on
    // the text row. Record each formula's window bounds for click-to-edit; the one
    // being edited shows the seated editor instead of its raster.
    for im in prepaint.inline_maths.get(i).into_iter().flatten() {
        // The spacer's LEFT edge and which row it landed on. On an
        // RTL row `display_off` is its RIGHT edge — anchoring there
        // paints the formula over the words beside it and leaves
        // the reserved gap empty — so the row's map gives the
        // spacer's actual visual box instead.
        let seat = match rtl {
            Some(r) => {
                let (k, local) = r.row_of(im.display_off);
                r.rows.get(k).and_then(|rr| {
                    let end = local + im.len;
                    rr.map.rects_for_range(local..end).first().map(|&(x0, _)| {
                        (
                            inset + r.shifts.get(k).copied().unwrap_or(px(0.)) + px(x0),
                            *lh * k,
                        )
                    })
                })
            }
            None => line
                .position_for_index(im.display_off, *lh)
                .map(|p| (inset + p.x, p.y)),
        };
        if let Some((x, row_y)) = seat {
            let x = origin.x + x;
            // Center the formula in the (grown-to-fit) wrap row.
            let y = origin.y + row_y + (*lh - im.height) / 2.0;
            let b = Bounds::new(point(x, y), size(im.width, im.height));
            rects.push((im.source.clone(), im.latex.clone(), b));
            if editing_inline.as_ref() != Some(&im.source) {
                let _ = window.paint_image(b, b, Corners::default(), im.img.clone(), 0, false);
            }
        }
    }
}

// ===== Gutter drag: drop boundary bar / six-dot grip + window-level listeners =====
/// Gutter drag grip: six muted dots on the hovered line; while a drag is
/// live, a full-width accent bar marks the drop boundary instead. Each dot
/// snaps to the DEVICE pixel grid: at fractional scale factors (Windows
/// 100/125/150%) the 2.5px dots otherwise straddle pixel boundaries at
/// per-dot subpixel phases and antialias to visibly different sizes.
fn paint_gutter_drag(
    entity: &Entity<EditorState>,
    bounds: Bounds<Pixels>,
    prepaint: &PrepaintState,
    text_color: Hsla,
    window: &mut Window,
    cx: &App,
) {
    let ed = entity.read(cx);
    let accent = ed.markdown_style.as_ref().map_or(text_color, |s| s.link);
    let mut dot_c = ed.markdown_style.as_ref().map_or(text_color, |s| s.marker);
    dot_c.a = (dot_c.a * 1.6).min(0.9);
    if let Some((bs, be, t)) = ed.affordances.line_drag {
        let y = prepaint.line_tops.get(t).copied().unwrap_or_else(|| {
            let i = prepaint.line_tops.len().saturating_sub(1);
            prepaint.line_tops.last().copied().unwrap_or(px(0.))
                + prepaint.line_heights.get(i).copied().unwrap_or(px(0.))
                    * prepaint.wrap_rows.get(i).copied().unwrap_or(1) as f32
        });
        // No bar when the boundary is a no-op (inside the block).
        if !(t >= bs && t <= be + 1) {
            window.paint_quad(fill(
                Bounds::new(
                    point(bounds.origin.x, bounds.origin.y + y - px(1.)),
                    size(bounds.size.width, px(2.)),
                ),
                accent,
            ));
        }
        if let Some(hb) = &prepaint.grip_hb {
            window.set_cursor_style(CursorStyle::ClosedHand, hb);
        }
    } else if let Some((_, rect)) = prepaint.grip {
        // Snap each dot to the DEVICE pixel grid: at fractional scale
        // factors (Windows 100/125/150%) the 2.5px dots otherwise
        // straddle pixel boundaries at per-dot subpixel phases and
        // antialias to visibly different sizes (macOS @2x masked it).
        let scale = window.scale_factor();
        let snap = |v: Pixels| px((f32::from(v) * scale).round() / scale);
        let d = snap(px(2.5));
        let gap = px(4.5);
        for col in 0..2 {
            for dr in 0..3 {
                let dot = Bounds::new(
                    point(
                        snap(rect.origin.x + px(2.) + gap * col as f32),
                        snap(rect.origin.y + px(1.) + gap * dr as f32),
                    ),
                    size(d, d),
                );
                window.paint_quad(fill(dot, dot_c).corner_radii(Corners::all(d * 0.5)));
            }
        }
        if let Some(hb) = &prepaint.grip_hb {
            window.set_cursor_style(CursorStyle::OpenHand, hb);
        }
    }
}

/// The window-level drag listeners. The grip sits OUTSIDE the editor div's
/// bounds, so the div's own mouse listeners never see a press on it — start
/// the drag from a window-level listener instead, gated on the grip's
/// HITBOX (not the bare rect): is_hovered respects occlusion, so a
/// dialog/menu/popover covering the gutter blocks the drag from starting
/// through it. The hovered-grip-row tracker and the drag's boundary move /
/// drop land here too (the gutter is outside the div, where its own handlers
/// never fire) — both paths take/compare the same state, so double delivery
/// is a no-op.
fn install_drag_listeners(
    entity: &Entity<EditorState>,
    grip: Option<(usize, Bounds<Pixels>)>,
    grip_hb: Option<Hitbox>,
    styled: bool,
    dragging: bool,
    window: &mut Window,
) {
    // The grip sits OUTSIDE the editor div's bounds, so the div's own
    // mouse listeners never see a press on it — start the drag from a
    // window-level listener instead, gated on the grip's HITBOX (not the
    // bare rect): is_hovered respects occlusion, so a dialog/menu/popover
    // covering the gutter blocks the drag from starting through it.
    if let (Some((row, _)), Some(hb)) = (grip, grip_hb.clone()) {
        let editor = entity.clone();
        window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
            if phase == gpui_kit::DispatchPhase::Bubble
                && e.button == MouseButton::Left
                && hb.is_hovered(window)
            {
                editor.update(cx, |ed, cx| {
                    let (bs, be) = ed.drag_block_rows(row);
                    ed.affordances.line_drag = Some((bs, be, bs));
                    ed.selection.is_selecting = false;
                    ed.overlay_menu.dismiss();
                    cx.notify();
                });
                cx.stop_propagation();
            }
        });
    }
    // Track the hovered grip row from a window-level listener — the
    // gutter sits outside the div, so its own on_mouse_move never fires
    // there. Every editor's listener sees every pointer move, so the
    // common miss (pointer nowhere near this editor) must stay cheap: a
    // read-only check first (grip_hover_row_at bails on the x band and
    // y range in O(1)), entity update only on an actual change — and no
    // listener at all for raw-view editors.
    if styled {
        let editor = entity.clone();
        window.on_mouse_event(move |e: &MouseMoveEvent, phase, _w, cx| {
            if phase == gpui_kit::DispatchPhase::Bubble {
                let ed = editor.read(cx);
                let gr = ed.grip_hover_row_at(e.position);
                if gr != ed.affordances.grip_hover_row {
                    editor.update(cx, |ed, cx| {
                        ed.affordances.grip_hover_row = gr;
                        cx.notify();
                    });
                }
            }
        });
    }
    // While a drag is live the pointer usually travels down the GUTTER —
    // still outside the div, where its on_mouse_move / on_mouse_up never
    // fire — so track the boundary and land the drop from window-level
    // listeners too (the div's own handlers cover in-bounds travel; both
    // paths take/compare the same state, so double delivery is a no-op).
    if dragging {
        let editor = entity.clone();
        window.on_mouse_event(move |e: &MouseMoveEvent, phase, _w, cx| {
            if phase == gpui_kit::DispatchPhase::Bubble {
                editor.update(cx, |ed, cx| {
                    if let Some((bs, be, t)) = ed.affordances.line_drag {
                        let b = ed.snap_drop_boundary(ed.drop_boundary_at(e.position));
                        if b != t {
                            ed.affordances.line_drag = Some((bs, be, b));
                            cx.notify();
                        }
                    }
                });
            }
        });
        let editor = entity.clone();
        window.on_mouse_event(move |e: &MouseUpEvent, phase, _w, cx| {
            if phase == gpui_kit::DispatchPhase::Bubble && e.button == MouseButton::Left {
                editor.update(cx, |ed, cx| {
                    if let Some((bs, be, t)) = ed.affordances.line_drag.take() {
                        ed.apply_line_drag(bs, be, t, cx);
                        cx.notify();
                    }
                });
            }
        });
    }
}

// ===== Code-card chips + the per-frame commit =====
/// Code-card chrome: the language tag + Copy button laid out in prepaint.
/// Each label sits on an opaque pill of the card color so it stays readable
/// over a long first line; hover brightens it.
fn paint_code_card_chips(
    prepaint: &PrepaintState,
    font: &Font,
    text_color: Hsla,
    window: &mut Window,
    cx: &mut App,
) -> Vec<CodeChipHit> {
    let mut code_chip_rects = Vec::new();
    for chip in &prepaint.code_chips {
        for (bounds_, text, hb) in [
            (chip.lang_bounds, &chip.lang_text, &chip.lang_hb),
            (chip.copy_bounds, &chip.copy_text, &chip.copy_hb),
        ] {
            let hovered = hb.is_hovered(window);
            window.paint_quad(PaintQuad {
                bounds: bounds_,
                corner_radii: Corners::all(px(4.)),
                background: chip.bg.into(),
                border_widths: Edges::all(px(0.)),
                border_color: chip.fg,
                border_style: BorderStyle::Solid,
            });
            let color = if hovered { text_color } else { chip.fg };
            let run = TextRun {
                len: text.len(),
                font: font.clone(),
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let shaped = window
                .text_system()
                .shape_line(text.clone(), px(13.), &[run], None);
            let _ = shaped.paint(
                point(
                    bounds_.origin.x + (bounds_.size.width - shaped.width()) / 2.,
                    bounds_.origin.y + px(1.),
                ),
                bounds_.size.height - px(2.),
                gpui_kit::TextAlign::Left,
                None,
                window,
                cx,
            );
            window.set_cursor_style(CursorStyle::PointingHand, hb);
        }
        code_chip_rects.push(CodeChipHit {
            lang: chip.lang_bounds,
            copy: chip.copy_bounds,
            fence_row: chip.fence_row,
        });
    }
    code_chip_rects
}

/// Commit this frame's geometry: the fresh layout vectors move into
/// `EditorState` by `std::mem::take` (never per-frame clones), the hit maps are
/// replaced with what this pass painted, and a pending caret reveal computes
/// its scroll offset from the just-committed geometry. Returns the reveal
/// request (caret key + target offset) for the caller's next-frame scroll.
#[allow(clippy::too_many_arguments)]
fn commit_frame(
    entity: &Entity<EditorState>,
    bounds: Bounds<Pixels>,
    prepaint: &mut PrepaintState,
    cx: &mut App,
    image_rects: Vec<(usize, Bounds<Pixels>)>,
    inline_math_rects: Vec<(Range<usize>, SharedString, Bounds<Pixels>)>,
    prop_pill_rects: Vec<(Bounds<Pixels>, zorite_markdown::syntax::LinkHit)>,
    prop_row_rects: Vec<(Bounds<Pixels>, usize)>,
    checkbox_rects: Vec<(usize, Bounds<Pixels>)>,
    alert_fold_rects: Vec<(usize, Bounds<Pixels>)>,
    heading_fold_rects: Vec<(usize, Bounds<Pixels>)>,
    table_row_add_rects: Vec<(Bounds<Pixels>, usize)>,
    table_col_add_rects: Vec<(Bounds<Pixels>, usize, usize)>,
    table_row_del: Option<(Bounds<Pixels>, usize)>,
    table_col_del: Option<(Bounds<Pixels>, usize, usize)>,
    table_col_resize_rects: Vec<(Bounds<Pixels>, usize, usize, f32)>,
    code_chip_rects: Vec<CodeChipHit>,
    base_lh: Pixels,
    font_size: Pixels,
    font: &Font,
) -> Option<((u64, usize, WrapAffinity), Point<Pixels>)> {
    let wrapped = std::mem::take(&mut prepaint.wrapped);
    let line_tops = std::mem::take(&mut prepaint.line_tops);
    let line_heights = std::mem::take(&mut prepaint.line_heights);
    let offset_maps = std::mem::take(&mut prepaint.maps);
    let widget_rows: Vec<bool> = prepaint
        .widgets
        .iter()
        .enumerate()
        .map(|(i, w)| w.is_some() || prepaint.tables.get(i).is_some_and(Option::is_some))
        .collect();
    let table_rows = std::mem::take(&mut prepaint.tables);
    let line_insets: Vec<Pixels> = prepaint
        .backgrounds
        .iter()
        .zip(prepaint.marks.iter())
        .map(|(bg, mark)| row_inset(*bg, *mark))
        .collect();
    let chip_rows: Vec<Option<(SharedString, bool)>> = prepaint
        .widgets
        .iter()
        .map(|w| match w {
            Some(Block::Chip { src, wiki, .. }) => Some((src.clone(), *wiki)),
            _ => None,
        })
        .collect();
    entity.update(cx, |editor, _| {
        editor.frame.wrapped = wrapped;
        editor.frame.line_tops = line_tops;
        editor.frame.line_heights = line_heights;
        editor.frame.wrap_rows = std::mem::take(&mut prepaint.wrap_rows);
        editor.frame.widget_rows = widget_rows;
        editor.frame.offset_maps = offset_maps;
        editor.affordances.chip_rows = chip_rows;
        editor.frame.line_insets = line_insets;
        editor.frame.rtl_rows = std::mem::take(&mut prepaint.rtl);
        editor.frame.table_rows = table_rows;
        editor.affordances.image_rects = image_rects;
        editor.affordances.inline_math_rects = inline_math_rects;
        editor.affordances.prop_pill_rects = prop_pill_rects;
        editor.affordances.link_rects = std::mem::take(&mut prepaint.link_rects);
        editor.affordances.prop_row_rects = prop_row_rects;
        editor.affordances.checkbox_rects = checkbox_rects;
        editor.affordances.table_thumbs = prepaint.table_thumbs.iter().map(|(t, _)| *t).collect();
        editor.affordances.code_chip_rects = code_chip_rects;
        editor.affordances.table_col_resize_rects = table_col_resize_rects;
        editor.affordances.code_card_rects = std::mem::take(&mut prepaint.code_card_rects);
        editor.affordances.alert_fold_rects = alert_fold_rects;
        editor.affordances.heading_fold_rects = heading_fold_rects;
        editor.affordances.heading_row_rects = std::mem::take(&mut prepaint.heading_row_rects);
        editor.affordances.table_row_add_rects = table_row_add_rects;
        editor.affordances.table_col_add_rects = table_col_add_rects;
        editor.affordances.table_hover_zones = std::mem::take(&mut prepaint.table_zones);
        editor.affordances.table_row_del = table_row_del;
        editor.affordances.table_col_del = table_col_del;
        editor.frame.last_bounds = Some(bounds);
        editor.frame.compensated.set(false);
        editor.frame.last_paint_gen = editor.frame.content_gen;
        editor.frame.line_height = base_lh;
        editor.frame.font_size = font_size;
        editor.frame.paint_font = Some(font.clone());
        if !editor.frame.reveal_requested {
            return None;
        }
        let key = (
            editor.frame.content_gen,
            editor.cursor_offset(),
            editor.selection.caret_affinity,
        );
        let caret = editor.range_to_bounds(&(key.1..key.1))?;
        editor.frame.reveal_requested = false;
        editor.frame.revealed_caret = Some(key);
        let viewport = editor.frame.scroll.bounds();
        let mut offset = editor.frame.scroll.offset();
        let previous = offset;
        let margin = px(4.);
        if caret.top() < viewport.top() + margin {
            offset.y += viewport.top() + margin - caret.top();
        } else if caret.bottom() > viewport.bottom() - margin {
            offset.y -= caret.bottom() - viewport.bottom() + margin;
        }
        if caret.left() < viewport.left() + margin {
            offset.x += viewport.left() + margin - caret.left();
        } else if caret.right() > viewport.right() - margin {
            offset.x -= caret.right() - viewport.right() + margin;
        }
        offset.x = offset.x.min(px(0.));
        offset.y = offset.y.min(px(0.));
        (offset != previous).then_some((key, offset))
    })
}
