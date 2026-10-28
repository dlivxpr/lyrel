//! Small deterministic fixtures shared by rendering behavior tests.

use gpui_kit::Hsla;
use rio_vt::config::colors::ColorRgb;
use rio_vt::crosswords::Crosswords;

use super::colors::to_hsla;
use super::{PaintColors, RenderCell};

pub(super) fn caret_colors() -> PaintColors {
    PaintColors {
        default_fg: to_hsla(ColorRgb {
            r: 17,
            g: 17,
            b: 17,
        }),
        default_bg: to_hsla(ColorRgb {
            r: 255,
            g: 255,
            b: 255,
        }),
        caret: to_hsla(ColorRgb {
            r: 0xf5,
            g: 0xa1,
            b: 0x5c,
        }),
        selection_bg: Hsla::default(),
        match_bg: Hsla::default(),
        current_match_bg: Hsla::default(),
        legible_dim: true,
        fg_rgb: ColorRgb {
            r: 17,
            g: 17,
            b: 17,
        },
        bg_rgb: ColorRgb {
            r: 255,
            g: 255,
            b: 255,
        },
    }
}

pub(super) fn cell(c: char) -> RenderCell {
    RenderCell {
        c,
        ..RenderCell::default()
    }
}

pub(super) fn test_colors() -> PaintColors {
    let fg = ColorRgb {
        r: 10,
        g: 10,
        b: 10,
    };
    let bg = ColorRgb {
        r: 250,
        g: 250,
        b: 245,
    };
    let wash = |a: f32| {
        let mut c = to_hsla(ColorRgb {
            r: 0x4a,
            g: 0x43,
            b: 0x39,
        });
        c.a = a;
        c
    };
    PaintColors {
        default_fg: to_hsla(fg),
        default_bg: to_hsla(bg),
        caret: to_hsla(fg),
        selection_bg: wash(0.55),
        match_bg: wash(0.32),
        current_match_bg: wash(0.85),
        fg_rgb: fg,
        bg_rgb: bg,
        legible_dim: true,
    }
}

pub(super) fn painted_term(input: &str) -> Crosswords<rio_vt::event::VoidListener> {
    let mut term =
        crate::terminal::remote::test_term(crate::terminal::size::TermSize::new(80, 24));
    rio_vt::performer::handler::Processor::default().advance(&mut term, input.as_bytes());
    term
}
