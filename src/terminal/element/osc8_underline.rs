//! An OSC 8 link's resting decoration: iTerm2's faint dotted underline, so a
//! `#1234` that is a link reads as one before the pointer finds it. It rides
//! the SGR 4:4 dotted painter, so it costs what an app's own dotted run does.

use super::{HELD_BACK_LINK_ALPHA, RenderCell, UnderlineKind};

/// A cell the application underlined, or gave an underline colour, keeps
/// its own decoration.
pub(super) fn mark(rc: &mut RenderCell, has_hyperlink: bool) {
    if rc.underline == UnderlineKind::None && rc.underline_color.is_none() && has_hyperlink {
        rc.underline = UnderlineKind::Dotted;
        rc.underline_color = Some(rc.fg.opacity(HELD_BACK_LINK_ALPHA));
        rc.osc8_dots = true;
    }
}

/// Under the pointer the dots give way to the hovered link's solid line.
pub(super) fn unmark(rc: &mut RenderCell) {
    if std::mem::take(&mut rc.osc8_dots) {
        rc.underline = UnderlineKind::None;
        rc.underline_color = None;
    }
}

#[cfg(test)]
mod tests {
    use rio_vt::config::colors::ColorRgb;
    use rio_vt::crosswords::Crosswords;
    use rio_vt::crosswords::pos::{Column, Line, Pos};
    use rio_vt::event::VoidListener;

    use super::super::{
        CellPainter, GlyphStyle, PaintColors, invert_cursor_cell, tests::test_colors,
    };
    use super::*;

    fn linked(sgr: &str) -> Crosswords<VoidListener> {
        super::super::tests::painted_term(&format!(
            "\x1b[{sgr}m\x1b]8;;https://example.com/3328\x1b\\#"
        ))
    }

    fn paint_fixture() -> ([ColorRgb; 256], PaintColors) {
        ([ColorRgb { r: 0, g: 0, b: 0 }; 256], test_colors())
    }

    #[test]
    fn a_hyperlink_rests_under_faint_dots() {
        let (palette, colors) = paint_fixture();
        let rc = CellPainter::new(&linked("0"), &palette, &colors, None)
            .at(Pos::new(Line(0), Column(0)));
        assert_eq!(rc.underline, UnderlineKind::Dotted);
        assert_eq!(
            rc.underline_color,
            Some(rc.fg.opacity(HELD_BACK_LINK_ALPHA))
        );
    }

    #[test]
    fn an_sgr_underlined_link_keeps_its_own_line_only() {
        let (palette, colors) = paint_fixture();
        let rc = CellPainter::new(&linked("4"), &palette, &colors, None)
            .at(Pos::new(Line(0), Column(0)));
        assert_eq!(rc.underline, UnderlineKind::Single);
        assert_eq!(rc.underline_color, None);
        assert!(!rc.osc8_dots);
    }

    #[test]
    fn hovering_swaps_the_dots_for_a_solid_line() {
        let (palette, colors) = paint_fixture();
        let mut rc = CellPainter::new(&linked("0"), &palette, &colors, None)
            .at(Pos::new(Line(0), Column(0)));
        unmark(&mut rc);
        rc.link_hover = true;
        assert_eq!(rc.underline, UnderlineKind::None);
        let solid = GlyphStyle::of(&rc)
            .underline_style()
            .expect("a solid underline");
        assert!(!solid.wavy);
    }

    #[test]
    fn the_dots_stay_faint_under_a_block_cursor() {
        let (palette, colors) = paint_fixture();
        let rc = CellPainter::new(&linked("0"), &palette, &colors, None)
            .at(Pos::new(Line(0), Column(0)));
        let mut buf = vec![rc];
        invert_cursor_cell(&mut buf, 1, 0, 0, &colors);
        let under = buf[0].underline_color.expect("the dots keep a colour");
        assert_eq!(under.a, buf[0].fg.a * HELD_BACK_LINK_ALPHA);
    }
}
