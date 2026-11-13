use gpui_kit::Global;
use rio_vt::config::colors::ColorRgb;

#[derive(Debug, Clone)]
pub struct ActivePalette {
    pub ansi16: [ColorRgb; 16],
    pub sel_bg: ColorRgb,
}

impl Global for ActivePalette {}

pub fn hsla_to_rgb(c: gpui_kit::Hsla) -> ColorRgb {
    let rgba = gpui_kit::Rgba::from(c);
    ColorRgb {
        r: (rgba.r * 255.0).round().clamp(0.0, 255.0) as u8,
        g: (rgba.g * 255.0).round().clamp(0.0, 255.0) as u8,
        b: (rgba.b * 255.0).round().clamp(0.0, 255.0) as u8,
    }
}

const DARK_ANSI16: [(u8, u8, u8); 16] = [
    (0x2c, 0x2a, 0x26),
    (0xec, 0x6a, 0x78),
    (0x8f, 0xbf, 0x6e),
    (0xe0, 0xb0, 0x72),
    (0x6f, 0xa8, 0xe6),
    (0xc0, 0x8a, 0xdf),
    (0x5f, 0xc2, 0xc9),
    (0xd2, 0xcf, 0xc8),
    (0x6b, 0x66, 0x5d),
    (0xf5, 0x86, 0x8f),
    (0xa8, 0xd9, 0x8a),
    (0xef, 0xc7, 0x8a),
    (0x8f, 0xc0, 0xf5),
    (0xd2, 0xa6, 0xec),
    (0x84, 0xd6, 0xdc),
    (0xf6, 0xf3, 0xec),
];

pub fn build() -> [ColorRgb; 256] {
    let mut p = [ColorRgb { r: 0, g: 0, b: 0 }; 256];

    for (i, (r, g, b)) in DARK_ANSI16.iter().enumerate() {
        p[i] = ColorRgb {
            r: *r,
            g: *g,
            b: *b,
        };
    }

    let steps = [0u8, 95, 135, 175, 215, 255];
    let mut idx = 16;
    for r in 0..6 {
        for g in 0..6 {
            for b in 0..6 {
                p[idx] = ColorRgb {
                    r: steps[r],
                    g: steps[g],
                    b: steps[b],
                };
                idx += 1;
            }
        }
    }

    for i in 0..24 {
        let v = 8 + i as u8 * 10;
        p[232 + i] = ColorRgb { r: v, g: v, b: v };
    }

    p
}

/// The 256-entry table a pane resolves indexed colours through: the fixed
/// base with the active theme's sixteen ANSI slots overlaid, including the
/// `theme_legible_palette` contrast rescue already folded into `ansi16` at
/// theme-application time. Painting, prompt backdrops and query replies must
/// share this one view — a query answered from the bare base would report
/// colours the cells never wear once a preset owns the first sixteen slots.
pub fn effective(
    base: &[ColorRgb; 256],
    active: Option<&ActivePalette>,
    overrides: &lyrel_core::core::pane_colors::PaneColors,
) -> [ColorRgb; 256] {
    let mut table = *base;
    if let Some(active) = active {
        table[..16].copy_from_slice(&active.ansi16);
    }
    for (index, rgb) in overrides.iter_set() {
        if let Some(slot) = table.get_mut(usize::from(index)) {
            *slot = rgb_color(rgb);
        }
    }
    table
}

pub fn rgb_color(rgb: [u8; 3]) -> ColorRgb {
    ColorRgb {
        r: rgb[0],
        g: rgb[1],
        b: rgb[2],
    }
}

/// The effective named defaults: an explicit pane override, otherwise the
/// live theme's value. A reset therefore reveals the current theme rather
/// than any colour captured when the override was set.
pub fn defaults(
    overrides: &lyrel_core::core::pane_colors::PaneColors,
    default_fg: ColorRgb,
    default_bg: ColorRgb,
    caret: ColorRgb,
) -> (ColorRgb, ColorRgb, ColorRgb) {
    use lyrel_core::core::pane_colors::{CURSOR, DEFAULT_BG, DEFAULT_FG};
    (
        overrides
            .get(DEFAULT_FG)
            .map(rgb_color)
            .unwrap_or(default_fg),
        overrides
            .get(DEFAULT_BG)
            .map(rgb_color)
            .unwrap_or(default_bg),
        overrides.get(CURSOR).map(rgb_color).unwrap_or(caret),
    )
}

/// Resolve the same indexed/default/cursor colours as the painter. `table` is
/// already the effective table (`effective`), so every indexed slot has the
/// theme and the pane overrides folded in; the named defaults fall back to the
/// live theme.
pub fn query_color(
    index: usize,
    table: &[ColorRgb; 256],
    overrides: &lyrel_core::core::pane_colors::PaneColors,
    default_fg: ColorRgb,
    default_bg: ColorRgb,
    caret: ColorRgb,
) -> ColorRgb {
    use lyrel_core::core::pane_colors::{CURSOR, DEFAULT_BG, DEFAULT_FG};
    match index {
        i if i < 256 => table[i],
        i if i == DEFAULT_FG as usize => overrides
            .get(DEFAULT_FG)
            .map(rgb_color)
            .unwrap_or(default_fg),
        i if i == DEFAULT_BG as usize => overrides
            .get(DEFAULT_BG)
            .map(rgb_color)
            .unwrap_or(default_bg),
        i if i == CURSOR as usize => overrides.get(CURSOR).map(rgb_color).unwrap_or(caret),
        _ => default_fg,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsla_to_rgb_round_trips_a_known_color() {
        let rgb = hsla_to_rgb(gpui_kit::rgb(0x123456).into());
        assert_eq!((rgb.r, rgb.g, rgb.b), (0x12, 0x34, 0x56));
        let black = hsla_to_rgb(gpui_kit::rgb(0x000000).into());
        assert_eq!((black.r, black.g, black.b), (0, 0, 0));
        let white = hsla_to_rgb(gpui_kit::rgb(0xffffff).into());
        assert_eq!((white.r, white.g, white.b), (255, 255, 255));
    }

    #[test]
    fn build_lays_out_the_256_color_cube_and_ramp() {
        let p = build();
        for (i, (r, g, b)) in DARK_ANSI16.iter().enumerate() {
            assert_eq!((p[i].r, p[i].g, p[i].b), (*r, *g, *b));
        }
        assert_eq!((p[16].r, p[16].g, p[16].b), (0, 0, 0));
        assert_eq!((p[231].r, p[231].g, p[231].b), (255, 255, 255));
        assert_eq!(p[232].r, 8);
        assert_eq!(p[255].r, 8 + 23 * 10);
        assert_eq!(p[240].r, p[240].g);
        assert_eq!(p[240].g, p[240].b);
    }

    #[test]
    fn effective_overlays_only_the_sixteen_theme_slots() {
        use lyrel_core::core::pane_colors::PaneColors;
        let base = build();
        let mut ansi16 = [ColorRgb { r: 0, g: 0, b: 0 }; 16];
        ansi16[1] = ColorRgb { r: 1, g: 2, b: 3 };
        let active = ActivePalette {
            ansi16,
            sel_bg: ColorRgb { r: 0, g: 0, b: 0 },
        };

        let none = effective(&base, None, &PaneColors::default());
        assert_eq!(none, base, "no theme overlay keeps the base table");

        let overlaid = effective(&base, Some(&active), &PaneColors::default());
        assert_eq!(overlaid[1], ansi16[1]);
        for i in [0usize, 2, 8, 15] {
            assert_eq!(overlaid[i], ansi16[i]);
        }
        assert_eq!(overlaid[16], base[16], "the cube is the theme's to keep");
        assert_eq!(overlaid[255], base[255]);
    }

    #[test]
    fn pane_overrides_win_over_the_theme_and_only_where_set() {
        use lyrel_core::core::pane_colors::{DEFAULT_BG, DEFAULT_FG, PaneColors};
        let base = build();
        let mut ansi16 = [ColorRgb { r: 0, g: 0, b: 0 }; 16];
        ansi16[1] = ColorRgb { r: 1, g: 2, b: 3 };
        let active = ActivePalette {
            ansi16,
            sel_bg: ColorRgb { r: 0, g: 0, b: 0 },
        };
        let mut overrides = PaneColors::default();
        overrides.set(1, [9, 8, 7]);
        overrides.set(DEFAULT_BG, [6, 5, 4]);

        let table = effective(&base, Some(&active), &overrides);
        assert_eq!((table[1].r, table[1].g, table[1].b), (9, 8, 7));
        assert_eq!(table[2], ansi16[2], "unset slots keep the theme");
        assert_eq!(table[200], base[200]);

        let fg = ColorRgb {
            r: 10,
            g: 11,
            b: 12,
        };
        let bg = ColorRgb {
            r: 20,
            g: 21,
            b: 22,
        };
        let caret = ColorRgb {
            r: 30,
            g: 31,
            b: 32,
        };
        assert_eq!(query_color(1, &table, &overrides, fg, bg, caret).r, 9);
        assert_eq!(query_color(2, &table, &overrides, fg, bg, caret), ansi16[2]);
        assert_eq!(query_color(256, &table, &overrides, fg, bg, caret), fg);
        assert_eq!(
            query_color(257, &table, &overrides, fg, bg, caret).g,
            5,
            "a default-background reset target is the current theme only when unset"
        );
        assert_eq!(query_color(258, &table, &overrides, fg, bg, caret), caret);
        let (fg, bg, caret) = defaults(&overrides, fg, bg, caret);
        assert_eq!((fg.r, bg.g, caret.b), (10, 5, 32));
        assert_eq!(overrides.get(DEFAULT_FG), None);
    }

    #[test]
    fn query_color_mirrors_the_painter_named_and_fallback_rules() {
        use lyrel_core::core::pane_colors::PaneColors;
        let table = build();
        let overrides = PaneColors::default();
        let fg = ColorRgb {
            r: 10,
            g: 11,
            b: 12,
        };
        let bg = ColorRgb {
            r: 20,
            g: 21,
            b: 22,
        };
        let caret = ColorRgb {
            r: 30,
            g: 31,
            b: 32,
        };

        assert_eq!(
            query_color(200, &table, &overrides, fg, bg, caret),
            table[200]
        );
        assert_eq!(query_color(256, &table, &overrides, fg, bg, caret), fg);
        assert_eq!(query_color(257, &table, &overrides, fg, bg, caret), bg);
        assert_eq!(query_color(258, &table, &overrides, fg, bg, caret), caret);
        assert_eq!(
            query_color(259, &table, &overrides, fg, bg, caret),
            fg,
            "past the named slots the painter resolves the default foreground"
        );
    }
}
