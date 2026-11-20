//! Property-panel widgets: building the two-column `key:: value` panel
//! ([`PropPanel`]), measuring its clickable pills for prepaint hitboxes, and
//! painting the panel + the file-chip block widget. The property construct
//! family, extracted from the former `element.rs` monolith.

use super::*;

/// Measure a property region's rows into a [`PropPanel`] with content-fit
/// columns (like the editor's tables). Values are segmented into plain runs +
/// link pills so the panel matches the reader.
#[allow(clippy::too_many_arguments)]
pub(super) fn build_prop_panel(
    lines: &[&str],
    range: &Range<usize>,
    window: &mut Window,
    font: &Font,
    font_size: Pixels,
    key_color: Hsla,
    value_color: Hsla,
    tag_color: Hsla,
    link_color: Hsla,
    icon_of: Option<&markdown_syntax::PropertyIconFn>,
) -> PropPanel {
    // Reserve room for a leading icon whenever the host resolves any.
    let icon_sz = if icon_of.is_some() {
        font_size * 0.95
    } else {
        px(0.)
    };
    let key_indent = if icon_sz > px(0.) {
        icon_sz + px(6.)
    } else {
        px(0.)
    };
    let mut rows = Vec::new();
    let mut key_w = px(0.);
    let mut val_w = px(0.);
    for &line in &lines[range.start..range.end] {
        let Some((_, k, v)) = zorite_markdown::syntax::prefixed_property(line) else {
            continue;
        };
        key_w = key_w.max(measure_width(window, k, font, font_size));
        let icon = icon_of.and_then(|f| f(k));
        let mut w = px(0.);
        let segs = zorite_markdown::syntax::property_value_segments(v)
            .into_iter()
            .map(|seg| match seg {
                zorite_markdown::syntax::PropSeg::Text(t) => {
                    w += measure_width(window, &t, font, font_size);
                    PanelSeg::Plain(t.into())
                }
                zorite_markdown::syntax::PropSeg::Pill {
                    label,
                    is_tag,
                    target,
                } => {
                    w += measure_width(window, &label, font, font_size)
                        + px(PILL_PAD_X * 2. + PILL_GAP);
                    PanelSeg::Pill {
                        text: label.into(),
                        color: if is_tag { tag_color } else { link_color },
                        target,
                    }
                }
            })
            .collect();
        val_w = val_w.max(w);
        rows.push((SharedString::from(k.to_string()), icon, segs));
    }
    let key_w = key_indent + key_w + px(20.);
    // 10px inner padding on BOTH sides: values start at key_w + 10 (see
    // `paint_prop_panel`), so the width needs 10 + val_w + 10 past key_w or
    // the hover border sits flush against the last value character.
    let width = key_w + val_w + px(20.);
    let row_h = font_size * LINE_HEIGHT_RATIO + px(8.);
    let height = row_h * rows.len() as f32;
    PropPanel {
        rows,
        key_w,
        width,
        row_h,
        height,
        icon_sz,
        key_indent,
        key_color,
        value_color,
        hover_border: key_color,
    }
}

/// Horizontal padding inside a value pill, and the gap after it.
const PILL_PAD_X: f32 = 6.;
const PILL_GAP: f32 = 4.;

/// Window-space bounds of each clickable pill in a property panel at `origin` —
/// the same x-advance `paint_prop_panel` uses. Prepaint inserts a pointer-cursor
/// hitbox per bound; paint records the matching click target.
pub(super) fn prop_pill_bounds(
    p: &PropPanel,
    origin: Point<Pixels>,
    font: &Font,
    font_size: Pixels,
    window: &mut Window,
) -> Vec<Bounds<Pixels>> {
    let line_h = font_size * LINE_HEIGHT_RATIO;
    let pad = px(10.);
    let mut out = Vec::new();
    for (ri, (_key, _icon, segs)) in p.rows.iter().enumerate() {
        let row_top = origin.y + p.row_h * ri as f32;
        let mut x = origin.x + p.key_w + pad;
        for seg in segs {
            match seg {
                PanelSeg::Plain(t) => x += measure_width(window, t, font, font_size),
                PanelSeg::Pill { text, .. } => {
                    let tw = measure_width(window, text, font, font_size);
                    let ph = line_h + px(2.);
                    out.push(Bounds::new(
                        point(x, row_top + (p.row_h - ph) / 2.),
                        size(tw + px(PILL_PAD_X * 2.), ph),
                    ));
                    x += tw + px(PILL_PAD_X * 2. + PILL_GAP);
                }
            }
        }
    }
    out
}

/// Paint a property panel (`Block::Properties`): no grid lines — a muted key
/// column and the value rendered as plain text + colored pills (tags/wiki-links)
/// on each clean row. The row under the pointer gets a rounded hover border, and
/// each pill's bounds + target are recorded (`pill_rects`) so a click can open
/// it; every row's bounds go to `row_rects` for hover change-detection.
#[allow(clippy::too_many_arguments)]
pub(super) fn paint_prop_panel(
    p: &PropPanel,
    origin: Point<Pixels>,
    content_w: Pixels,
    font: &Font,
    font_size: Pixels,
    window: &mut Window,
    cx: &mut App,
    pill_rects: &mut Vec<(Bounds<Pixels>, zorite_markdown::syntax::LinkHit)>,
    row_rects: &mut Vec<(Bounds<Pixels>, usize)>,
    base_row: usize,
) {
    let line_h = font_size * LINE_HEIGHT_RATIO;
    let pad = px(10.);
    let mouse = window.mouse_position();
    // Follow the VALUES, not the keys: a Persian note's property keys are
    // usually Latin (`tags`, `key`), so the key would decide the wrong way.
    // Panel-wide, so the key column doesn't stagger row to row.
    let rtl = p.rows.iter().any(|(_, _, segs)| {
        segs.iter().any(|s| {
            let t = match s {
                PanelSeg::Plain(t) => t,
                PanelSeg::Pill { text, .. } => text,
            };
            zorite_markdown::syntax::content_direction(t).is_rtl()
        })
    });
    // The panel is sized to its content, so mirroring INSIDE it is not enough —
    // it also has to sit on the right, where the rest of an RTL block does.
    let origin = if rtl {
        point(origin.x + content_w - p.width, origin.y)
    } else {
        origin
    };
    // Reflect a box across the panel: everything below is laid out
    // left-to-right and mirrored on the way out, so the two directions can't
    // drift apart.
    let mirror = |x: Pixels, w: Pixels| {
        if rtl {
            origin.x + p.width - (x - origin.x) - w
        } else {
            x
        }
    };
    for (ri, (key, icon, segs)) in p.rows.iter().enumerate() {
        let row_top = origin.y + p.row_h * ri as f32;
        let row_bounds = Bounds::new(point(origin.x, row_top), size(p.width, p.row_h));
        row_rects.push((row_bounds, base_row + ri));
        // Whole-row hover border (Obsidian-style).
        if row_bounds.contains(&mouse) {
            window.paint_quad(PaintQuad {
                bounds: row_bounds,
                corner_radii: Corners::all(px(6.)),
                background: gpui_kit::transparent_black().into(),
                border_widths: Edges::all(px(1.)),
                border_color: p.hover_border,
                border_style: BorderStyle::Solid,
            });
        }
        let ty = row_top + (p.row_h - line_h) / 2.;
        // Optional key icon (host-resolved), then the muted key name inset past it.
        if let Some(path) = icon {
            let ib = Bounds::new(
                point(
                    mirror(origin.x + pad, p.icon_sz),
                    row_top + (p.row_h - p.icon_sz) / 2.,
                ),
                size(p.icon_sz, p.icon_sz),
            );
            let _ = window.paint_svg(
                ib,
                path.clone(),
                None,
                gpui_kit::TransformationMatrix::unit(),
                p.key_color,
                cx,
            );
        }
        let krun = TextRun {
            len: key.len(),
            font: font.clone(),
            color: p.key_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let ks = window
            .text_system()
            .shape_line(key.clone(), font_size, &[krun], None);
        let _ = ks.paint(
            point(mirror(origin.x + pad + p.key_indent, ks.width()), ty),
            line_h,
            gpui_kit::TextAlign::Left,
            None,
            window,
            cx,
        );
        // Value: plain runs painted inline, links as rounded (clickable) pills.
        let mut x = origin.x + p.key_w + pad;
        for seg in segs {
            let (text, color, target) = match seg {
                PanelSeg::Plain(t) => (t, p.value_color, None),
                PanelSeg::Pill {
                    text,
                    color,
                    target,
                } => (text, *color, Some(target)),
            };
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
                .shape_line(text.clone(), font_size, &[run], None);
            let tw = shaped.width();
            if let Some(target) = target {
                let mut bg = color;
                bg.a = 0.16;
                let ph = line_h + px(2.);
                let pw = tw + px(PILL_PAD_X * 2.);
                let pb = Bounds::new(
                    point(mirror(x, pw), row_top + (p.row_h - ph) / 2.),
                    size(pw, ph),
                );
                window.paint_quad(fill(pb, bg).corner_radii(Corners::all(px(6.))));
                let _ = shaped.paint(
                    point(pb.origin.x + px(PILL_PAD_X), ty),
                    line_h,
                    gpui_kit::TextAlign::Left,
                    None,
                    window,
                    cx,
                );
                pill_rects.push((pb, target.clone()));
                x += tw + px(PILL_PAD_X * 2. + PILL_GAP);
            } else {
                let _ = shaped.paint(
                    point(mirror(x, tw), ty),
                    line_h,
                    gpui_kit::TextAlign::Left,
                    None,
                    window,
                    cx,
                );
                x += tw;
            }
        }
    }
}

/// Paint a file chip — a rounded, bordered button with a flat document icon +
/// `label` — filling the row (sized in `shape_document` to include vertical
/// padding), its width fit to the label. Left-click opens it, right-click edits
/// (handled by the mouse handlers via `chip_rows`).
#[allow(clippy::too_many_arguments)]
pub(super) fn paint_chip(
    label: &str,
    link: Hsla,
    bg: Hsla,
    border: Hsla,
    origin: Point<Pixels>,
    row_h: Pixels,
    font: &Font,
    font_size: Pixels,
    window: &mut Window,
    cx: &mut App,
) {
    let text = SharedString::from(label.to_string());
    let run = TextRun {
        len: text.len(),
        font: font.clone(),
        color: link,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let shaped = window
        .text_system()
        .shape_line(text, font_size, &[run], None);
    let pad_x = px(10.);
    let icon_h = font_size * 0.92;
    let icon_w = icon_h * 0.74;
    let gap = px(7.); // between the icon and the label
    let box_w = pad_x * 2. + icon_w + gap + shaped.width();
    window.paint_quad(PaintQuad {
        bounds: Bounds::new(origin, size(box_w, row_h)),
        corner_radii: Corners::all(px(6.)),
        background: bg.into(),
        border_widths: Edges::all(px(1.)),
        border_color: border,
        border_style: BorderStyle::Solid,
    });
    let ix = origin.x + pad_x;
    paint_doc_icon(
        ix,
        origin.y + (row_h - icon_h) / 2.,
        icon_w,
        icon_h,
        link,
        window,
    );
    let line_h = font_size * LINE_HEIGHT_RATIO;
    let _ = shaped.paint(
        point(ix + icon_w + gap, origin.y + (row_h - line_h) / 2.),
        line_h,
        gpui_kit::TextAlign::Left,
        None,
        window,
        cx,
    );
}
