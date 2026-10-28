//! Cell fills, special underlines, cursor and IME overlays.
//! The element determines their order relative to text and graphics.

use gpui_kit::{
    App, BorderStyle, Bounds, Font, Hsla, Pixels, SharedString, TextAlign, TextRun, Window,
    fill, outline, point, px, size,
};
use rio_vt::ansi::CursorShape;

use super::{CellGeom, HELD_BACK_LINK_ALPHA, PaintColors, RenderCell, UnderlineKind};

pub(super) fn paint_backgrounds(window: &mut Window, geom: &CellGeom, buf: &[RenderCell]) {
    for row in 0..geom.rows {
        let mut col = 0;
        while col < geom.cols {
            let cell = &buf[row * geom.cols + col];
            if !cell.draw_bg {
                col += 1;
                continue;
            }
            let bg = cell.bg;
            let start = col;
            while col < geom.cols {
                let c = &buf[row * geom.cols + col];
                if c.spacer || (c.draw_bg && c.bg == bg) {
                    col += 1;
                } else {
                    break;
                }
            }
            window.paint_quad(fill(geom.cell_rect(row, start, col - start), bg));
        }
    }
}

pub(super) fn paint_cell_runs(
    window: &mut Window,
    geom: &CellGeom,
    buf: &[RenderCell],
    color: Hsla,
    mut covered: impl FnMut(&RenderCell) -> bool,
) {
    for row in 0..geom.rows {
        let mut col = 0;
        while col < geom.cols {
            if !covered(&buf[row * geom.cols + col]) {
                col += 1;
                continue;
            }
            let start = col;
            while col < geom.cols {
                let cell = &buf[row * geom.cols + col];
                if covered(cell) || cell.spacer {
                    col += 1;
                } else {
                    break;
                }
            }
            window.paint_quad(fill(geom.cell_rect(row, start, col - start), color));
        }
    }
}

fn for_each_special_underline(
    bounds: Bounds<Pixels>,
    kind: UnderlineKind,
    scale: f32,
    mut draw: impl FnMut(Bounds<Pixels>),
) {
    let scale = scale.max(0.1);
    let snap = |v: f32| (v * scale).round() / scale;
    let device_px = 1. / scale;
    let x0 = snap(bounds.origin.x.as_f32());
    let x1 = snap((bounds.origin.x + bounds.size.width).as_f32());
    let y1 = snap((bounds.origin.y + bounds.size.height).as_f32());
    let line = |y: f32, start: f32, end: f32| {
        Bounds::new(
            point(px(start), px(y)),
            size(px((end - start).max(0.)), px(device_px)),
        )
    };

    match kind {
        UnderlineKind::Double => {
            draw(line(y1 - 4. * device_px, x0, x1));
            draw(line(y1 - 2. * device_px, x0, x1));
        }
        UnderlineKind::Dotted | UnderlineKind::Dashed => {
            // The ink is one device pixel thick, but its rhythm is measured in
            // logical pixels. Otherwise at 2x a dotted underline alternates a
            // single physical pixel on/off and reads as a grey solid line.
            let (on, off) = if kind == UnderlineKind::Dotted {
                (1., 1.)
            } else {
                (3., 2.)
            };
            let mut x = x0;
            while x < x1 {
                draw(line(y1 - 2. * device_px, x, (x + on).min(x1)));
                x += on + off;
            }
        }
        _ => {}
    }
}

pub(super) fn paint_special_underlines(window: &mut Window, geom: &CellGeom, buf: &[RenderCell], scale: f32) {
    for row in 0..geom.rows {
        let row_base = row * geom.cols;
        let mut col = 0;
        while col < geom.cols {
            let cell = &buf[row_base + col];
            if !matches!(
                cell.underline,
                UnderlineKind::Double | UnderlineKind::Dotted | UnderlineKind::Dashed
            ) {
                col += 1;
                continue;
            }
            let kind = cell.underline;
            let color = cell.underline_color.unwrap_or(cell.fg);
            let start = col;
            col += 1;
            while col < geom.cols {
                let next = &buf[row_base + col];
                if next.spacer
                    || (next.underline == kind && next.underline_color.unwrap_or(next.fg) == color)
                {
                    col += 1;
                } else {
                    break;
                }
            }
            for_each_special_underline(
                geom.cell_rect(row, start, col - start),
                kind,
                scale,
                |rect| {
                    window.paint_quad(fill(rect, color));
                },
            );
        }
    }
}

/// What colour a hovered link's underline takes.
///
/// Until the modifier is down the link is only being pointed out, so it is
/// drawn as the same underline held back. A cell carrying an underline of its
/// own keeps the colour it was given: dimming that would be editing the
/// application's output rather than annotating it.
pub(super) fn link_underline_color(cell: &RenderCell, armed: bool) -> Option<Hsla> {
    if armed || cell.underline != UnderlineKind::None || cell.underline_color.is_some() {
        return cell.underline_color;
    }
    Some(cell.fg.opacity(HELD_BACK_LINK_ALPHA))
}

pub(super) fn paint_cursor(
    window: &mut Window,
    geom: &CellGeom,
    cursor: Option<(usize, usize, crate::core::config::CursorStyle)>,
    focused: bool,
    cursor_visible: bool,
    caret: Hsla,
) {
    use crate::core::config::CursorStyle;
    let Some((row, col, style)) = cursor else {
        return;
    };
    let rect = geom.cell_rect(row, col, 1);
    if !focused {
        window.paint_quad(outline(rect, caret, BorderStyle::Solid));
        return;
    }
    if !cursor_visible {
        return;
    }
    match style {
        // A focused block is drawn as reverse video by `invert_cursor_cell`
        // before the glyphs go down, so the caret is opaque and the character
        // under it stays readable. Nothing left to paint here.
        CursorStyle::Block => {}
        CursorStyle::Bar => {
            let w = (geom.cell_width * 0.15).max(px(1.)).min(px(3.));
            let bar = Bounds::new(rect.origin, size(w, rect.size.height));
            window.paint_quad(fill(bar, caret));
        }
        CursorStyle::Underline => {
            let h = (geom.line_height * 0.12).max(px(1.)).min(px(3.));
            let y = rect.origin.y + rect.size.height - h;
            let line = Bounds::new(point(rect.origin.x, y), size(rect.size.width, h));
            window.paint_quad(fill(line, caret));
        }
    }
}

/// Turn the cell under a focused block cursor into reverse video: opaque caret
/// fill, glyph redrawn on top of it. Every other terminal paints a block this
/// way, and a translucent tint would give back most of the contrast the theme
/// conditioned the caret to carry.
pub(super) fn invert_cursor_cell(
    buf: &mut [RenderCell],
    cols: usize,
    row: usize,
    col: usize,
    colors: &PaintColors,
) {
    let ink = crate::ui::presets::caret_ink(colors.caret, colors.default_bg, colors.default_fg);
    // A TUI can park the cursor on the second half of a wide character. The
    // glyph lives on the lead cell, so inverting the spacer would fill half a
    // character with the caret and leave the glyph in its own colour across
    // both halves. Step back to the cell that owns the glyph.
    let col = match buf.get(row * cols + col) {
        Some(c) if c.spacer && col > 0 => col - 1,
        _ => col,
    };
    let Some(cell) = buf.get_mut(row * cols + col) else {
        return;
    };
    cell.bg = colors.caret;
    cell.draw_bg = true;
    cell.fg = ink;
    // Keep the underline's own alpha: an OSC 8 link's held-back dots stay
    // faint under the caret rather than turning into a full-strength line.
    if let Some(under) = cell.underline_color.as_mut() {
        *under = ink.opacity(under.a);
    }
    // A wide character's trailing spacer is absorbed into the lead cell's
    // background run by `paint_backgrounds`, so the fill already covers both
    // columns and only the lead cell carries the glyph.
}

pub(super) fn cursor_style_from_shape(shape: CursorShape) -> crate::core::config::CursorStyle {
    match shape {
        CursorShape::Beam => crate::core::config::CursorStyle::Bar,
        CursorShape::Underline => crate::core::config::CursorStyle::Underline,
        _ => crate::core::config::CursorStyle::Block,
    }
}

/// Whether an IME preedit has anything to draw. `paint_marked` blanks the cell
/// under the cursor to the theme's background before it lays the preedit
/// down, which also covers the block cursor's reverse-video cell. A preedit of
/// nothing but blanks (a composition an IME opened and left empty-looking —
/// Windows IMEs can leave one behind, and gpui there never hears the
/// composition end) would therefore paint a theme-coloured hole with an
/// underline where the character and the caret should be, and hold it on
/// every cell the cursor visits (#966). Nothing to show means nothing to
/// paint: the grid, and the caret on it, stay as they are.
fn preedit_has_ink(marked: &str) -> bool {
    use rio_unicode::UnicodeWidthChar as _;
    marked
        .chars()
        .any(|c| !c.is_whitespace() && !c.is_control() && c.width().unwrap_or(0) > 0)
}

pub(super) fn paint_marked(
    window: &mut Window,
    cx: &mut App,
    geom: &CellGeom,
    cursor: Option<(usize, usize)>,
    marked: &str,
    font_size: Pixels,
    base_font: &Font,
    default_fg: Hsla,
    default_bg: Hsla,
) {
    if !preedit_has_ink(marked) {
        return;
    }
    let Some((row, col)) = cursor else {
        return;
    };
    let x = geom.origin.x + geom.cell_width * (col as f32);
    let y = geom.origin.y + geom.line_height * (row as f32);
    let run = TextRun {
        len: marked.len(),
        font: base_font.clone(),
        color: default_fg,
        background_color: None,
        underline: Some(gpui_kit::UnderlineStyle {
            thickness: px(1.),
            color: Some(default_fg),
            wavy: false,
        }),
        strikethrough: None,
    };
    let shaped = window.text_system().shape_line(
        SharedString::from(marked.to_owned()),
        font_size,
        &[run],
        None,
    );
    let bg_rect = Bounds::new(point(x, y), size(shaped.width, geom.line_height));
    window.paint_quad(fill(bg_rect, default_bg));
    _ = shaped.paint(
        point(x, y),
        geom.line_height,
        TextAlign::Left,
        None,
        window,
        cx,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::colors::to_hsla;
    use super::super::test_support::{caret_colors, cell};
    use rio_vt::config::colors::ColorRgb;

    #[test]
    fn a_block_cursor_turns_its_cell_into_reverse_video() {
        let colors = caret_colors();
        let mut buf = vec![RenderCell::default(); 6];
        buf[3].c = 'o';
        buf[3].fg = to_hsla(ColorRgb { r: 1, g: 2, b: 3 });
        invert_cursor_cell(&mut buf, 3, 1, 0, &colors);

        let cell = &buf[3];
        assert!(cell.draw_bg, "the caret fill has to be painted");
        assert_eq!(cell.bg, colors.caret);
        assert_ne!(
            cell.fg,
            to_hsla(ColorRgb { r: 1, g: 2, b: 3 }),
            "the glyph is redrawn on the caret, not left in its own colour"
        );
        assert_eq!(cell.c, 'o', "the character itself survives");

        for (i, other) in buf.iter().enumerate() {
            if i != 3 {
                assert!(!other.draw_bg, "cell {i} was not the cursor cell");
            }
        }
    }

    #[test]
    fn a_cursor_parked_on_a_wide_char_inverts_the_half_that_holds_the_glyph() {
        let colors = caret_colors();
        let mut buf = vec![RenderCell::default(); 4];
        buf[1].c = '世';
        buf[2].spacer = true;

        // Cursor reported on the trailing spacer: the lead cell is the one
        // that gets the fill, so it can absorb the spacer into its run.
        invert_cursor_cell(&mut buf, 4, 0, 2, &colors);
        assert!(buf[1].draw_bg, "the glyph's own cell carries the caret");
        assert!(!buf[2].draw_bg, "the spacer is absorbed, not painted");
        assert_eq!(buf[1].bg, colors.caret);

        // A spacer in column 0 has no lead to step back to; leave it alone
        // rather than wrapping to the previous row.
        let mut edge = vec![RenderCell::default(); 4];
        edge[0].spacer = true;
        invert_cursor_cell(&mut edge, 4, 0, 0, &colors);
        assert!(edge[0].draw_bg);
    }

    #[test]
    fn caret_ink_takes_whichever_of_the_two_reads_on_the_caret() {
        let colors = caret_colors();
        // A pale caret on a white background: the foreground wins.
        let ink = crate::ui::presets::caret_ink(colors.caret, colors.default_bg, colors.default_fg);
        assert_eq!(ink, colors.default_fg);

        // A caret far from the background: the background wins, which is the
        // conventional reverse-video pairing.
        let deep = to_hsla(ColorRgb {
            r: 0x20,
            g: 0x20,
            b: 0x80,
        });
        let ink = crate::ui::presets::caret_ink(deep, colors.default_bg, colors.default_fg);
        assert_eq!(ink, colors.default_bg);
    }

    #[test]
    fn a_caret_the_colour_of_the_text_still_leaves_its_glyph_readable() {
        // Gruvbox Dark, the theme in #966: the caret is the foreground colour
        // exactly, so drawing the glyph in its own ink on the caret would be
        // cream on cream. Reverse video has to reach for the background.
        let fg = ColorRgb {
            r: 0xeb,
            g: 0xdb,
            b: 0xb2,
        };
        let bg = ColorRgb {
            r: 0x28,
            g: 0x28,
            b: 0x28,
        };
        let colors = PaintColors {
            default_fg: to_hsla(fg),
            default_bg: to_hsla(bg),
            caret: to_hsla(fg),
            fg_rgb: fg,
            bg_rgb: bg,
            ..caret_colors()
        };
        let mut buf = vec![RenderCell::default(); 3];
        buf[1].c = 'v';
        buf[1].fg = to_hsla(fg);
        invert_cursor_cell(&mut buf, 3, 0, 1, &colors);
        assert_eq!(buf[1].bg, colors.caret);
        assert_eq!(buf[1].fg, colors.default_bg);
        assert_eq!(buf[1].c, 'v');
    }

    #[test]
    fn a_preedit_with_nothing_to_show_leaves_the_cursor_cell_alone() {
        // Painting any of these would blank the cell under the cursor —
        // character and block caret both — to the theme background (#966).
        for blank in [
            "",
            " ",
            "  ",
            "\u{3000}",
            "\u{200b}",
            "\t",
            "\u{0}",
            " \u{200d} ",
        ] {
            assert!(!preedit_has_ink(blank), "{blank:?} has nothing to draw");
        }
        // Real compositions still paint, including a leading or trailing
        // blank the IME put between syllables.
        for text in ["n", "ni hao", "你好", " a", "に", "ㅎ", "é"] {
            assert!(preedit_has_ink(text), "{text:?} is a real preedit");
        }
    }

    #[test]
    fn terminal_cursor_shape_maps_to_painted_cursor_style() {
        use crate::core::config::CursorStyle;

        assert_eq!(cursor_style_from_shape(CursorShape::Beam), CursorStyle::Bar);
        assert_eq!(
            cursor_style_from_shape(CursorShape::Underline),
            CursorStyle::Underline
        );
        assert_eq!(
            cursor_style_from_shape(CursorShape::Block),
            CursorStyle::Block
        );
    }

    #[test]
    fn special_underlines_have_distinct_device_pixel_geometry() {
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(12.), px(20.)));
        let ink = |kind| {
            let mut rects = Vec::new();
            for_each_special_underline(bounds, kind, 2., |rect| rects.push(rect));
            rects
        };

        let double = ink(UnderlineKind::Double);
        assert_eq!(
            double.len(),
            2,
            "double underline uses two separate strokes"
        );
        assert_ne!(double[0].origin.y, double[1].origin.y);

        let dotted = ink(UnderlineKind::Dotted);
        let dashed = ink(UnderlineKind::Dashed);
        assert!(
            dotted.len() > dashed.len(),
            "dots repeat more often than dashes"
        );
        assert!(dotted.len() > 1 && dashed.len() > 1);
        assert!(
            dotted.iter().all(|r| r.size.width == px(1.)),
            "each dot is a logical pixel wide at 2x"
        );
        assert!(
            dashed.iter().all(|r| r.size.width <= px(3.)),
            "each dash is at most three logical pixels wide at 2x"
        );
    }

    #[test]
    fn a_link_is_pointed_out_faintly_until_the_modifier_is_down() {
        let mut plain = cell('l');
        plain.fg = gpui_kit::hsla(0., 0., 0.9, 1.);

        assert_eq!(
            link_underline_color(&plain, true),
            None,
            "armed, the underline is the text's own colour"
        );
        let held = link_underline_color(&plain, false).expect("a colour of its own");
        assert!(
            held.a < plain.fg.a,
            "held back, it is the same colour with less of it"
        );
        assert_eq!(
            (held.h, held.s, held.l),
            (plain.fg.h, plain.fg.s, plain.fg.l)
        );

        let mut own = cell('l');
        own.underline = UnderlineKind::Curly;
        own.underline_color = Some(gpui_kit::hsla(0.1, 1., 0.5, 1.));
        assert_eq!(
            link_underline_color(&own, false),
            own.underline_color,
            "a spelling mistake stays the colour the application painted it"
        );
    }
}
