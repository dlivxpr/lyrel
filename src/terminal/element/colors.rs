//! Frame-local color resolution and pane dimming; no retained render state.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{Hsla, Rgba};
use rio_vt::config::colors::{AnsiColor, ColorRgb, NamedColor};

use super::{DIM_OPACITY, PaintColors, RenderCell};
use crate::core::config::Config;

fn pack_rgb(c: ColorRgb) -> u32 {
    (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32
}

fn unpack_rgb(n: u32) -> ColorRgb {
    ColorRgb {
        r: (n >> 16) as u8,
        g: (n >> 8) as u8,
        b: n as u8,
    }
}

pub(super) fn to_hsla(c: ColorRgb) -> Hsla {
    Rgba {
        r: c.r as f32 / 255.,
        g: c.g as f32 / 255.,
        b: c.b as f32 / 255.,
        a: 1.,
    }
    .into()
}

pub(super) fn resolve(
    color: AnsiColor,
    palette: &[ColorRgb; 256],
    default_fg: ColorRgb,
    default_bg: ColorRgb,
) -> (ColorRgb, bool) {
    match color {
        AnsiColor::Spec(rgb) => (rgb, false),
        AnsiColor::Indexed(i) => (palette[i as usize], false),
        AnsiColor::Named(named) => match named {
            NamedColor::Foreground => (default_fg, true),
            NamedColor::Background => (default_bg, true),
            other => {
                let idx = other as usize;
                if idx < 256 {
                    (palette[idx], false)
                } else {
                    (default_fg, true)
                }
            }
        },
    }
}

/// SGR 2's colour: the ink at `DIM_OPACITY` over its cell, or — on a light
/// cell where that fade would be illegible — the opaque ink
/// `presets::legible_dim` solved for. `legible` is Settings → Appearance's
/// palette switch; off keeps the plain fade everywhere.
pub(super) fn dim_fg(fg: Hsla, under: ColorRgb, legible: bool) -> Hsla {
    let mut faded = fg;
    faded.a *= DIM_OPACITY;
    if !legible {
        return faded;
    }
    // A faint run is one (ink, background) pair repeated across the row, and
    // the rescue is a contrast bisection — remember the last answer so a
    // screen of dim text costs one solve per colour change, not one per cell.
    thread_local! {
        static LAST: std::cell::Cell<Option<(u32, u32, Option<u32>)>> =
            const { std::cell::Cell::new(None) };
    }
    let (ink, bg) = (pack_rgb(crate::terminal::palette::hsla_to_rgb(fg)), pack_rgb(under));
    let solved = LAST.with(|last| match last.get() {
        Some((i, b, s)) if i == ink && b == bg => s,
        _ => {
            let s = crate::ui::presets::legible_dim(ink, bg, DIM_OPACITY);
            last.set(Some((ink, bg, s)));
            s
        }
    });
    match solved {
        Some(c) => Hsla {
            a: fg.a,
            ..to_hsla(unpack_rgb(c))
        },
        None => faded,
    }
}

fn active_selection_bg(cx: &gpui_kit::App) -> ColorRgb {
    match cx.try_global::<crate::terminal::palette::ActivePalette>() {
        Some(a) => a.sel_bg,
        None => ColorRgb {
            r: 0x4a,
            g: 0x43,
            b: 0x39,
        },
    }
}

/// Blends a finished cell's colours toward `under` for a dimmed pane. The
/// blend has to happen on the *cell*, not on the palette: truecolour cells
/// (the direct `38;2;…`/`48;2;…` SGR a prompt like starship emits for its
/// segments) carry their own `Spec` colour that a palette-level dim would
/// never see, leaving a truecolour prompt at full brightness while indexed
/// content around it dims.
pub(super) fn dim_cell(mut rc: RenderCell, dim: f32, under: Rgba) -> RenderCell {
    rc.fg = blend_toward(rc.fg, dim, under);
    rc.bg = blend_toward(rc.bg, dim, under);
    if let Some(u) = rc.underline_color.as_mut() {
        *u = blend_toward(*u, dim, under);
    }
    rc
}

/// What a search match is washed with.
///
/// The theme's accent, not the terminal palette's selection colour. A hit and a
/// selection are different things, and washing both from the same tint left the
/// only difference between them a few percent of luminance — on a grid that is
/// already grey on grey, that reads as "slightly dirty background", not as "the
/// thing you searched for". The accent is the one colour the terminal surface
/// has nothing else in, and `legible_accent` has already floored it at 3:1
/// against the background, so the wash always has somewhere to travel.
fn match_tint(cx: &gpui_kit::App) -> u32 {
    match cx.try_global::<crate::ui::presets::ActiveAccent>() {
        Some(a) => a.0,
        None => pack_rgb(active_selection_bg(cx)),
    }
}

/// The under-colour a dimmed pane blends its content toward: the window
/// background as it actually sits in the frame, i.e. the active preset fill
/// premultiplied by the window's own opacity. Dimming by blending every colour
/// toward this value (instead of alpha-multiplying each primitive) keeps the
/// composite of stacked layers — a powerline separator path over its segment
/// quad, text over a tint — exactly as dimmed as any single layer, so the
/// decorations of a prompt stay continuous when the pane goes inactive.
///
/// The fill comes from the active preset rather than the flat
/// `theme().background` token because the workspace root paints the preset
/// (see `theme::workspace_background`): with a gradient or wallpaper preset
/// the actual backdrop is not the theme token, and blending toward the wrong
/// colour would leave a tinted slab inside the dimmed pane. Solid fills match
/// exactly; gradients are approximated by their midpoint stop; a wallpaper
/// image rides over the fill at low opacity, so the fill stays the best
/// available target. Cells whose background is not painted (the default
/// terminal background) intentionally keep showing the un-dimmed window
/// background through them, so the backdrop material stays visible.
///
/// Two trade-offs follow from painting opaque, pre-blended colours instead of
/// the old element-opacity style. On a translucent window the desktop no
/// longer shows through a dimmed pane's painted cells — the old style left
/// them at alpha `dim` and let the backdrop contribute; the new style paints
/// them opaque, blended toward `fill × window_opacity`, which ignores the
/// backdrop's own contribution. And only the terminal element's cells are
/// dimmed: the search bar, completion menu and integration notice render
/// outside the grid and stay at full brightness, where the old style faded
/// the whole `TerminalView` — the same "the fading is worn by what it holds"
/// rule the drag grip follows.
pub(super) fn dim_under(cx: &gpui_kit::App) -> Rgba {
    // Mirror `workspace_background`: the active preset fill when a preset is
    // installed, the theme token otherwise. The alpha is the window opacity
    // in both arms, so the premultiply is uniform.
    match cx.try_global::<crate::ui::presets::ActiveBackground>() {
        Some(bg) => fill_under(&bg.fill, bg.opacity),
        None => theme_under(cx.theme().background),
    }
}

/// The premultiplied under-colour for an active preset's fill at the window's
/// own opacity. Solid fills are exact; gradients are approximated by their
/// midpoint stop (a wallpaper image rides over the fill at low opacity, so
/// the fill stays the best available target).
fn fill_under(fill: &crate::ui::presets::Fill, opacity: Option<f32>) -> Rgba {
    let packed = match fill {
        crate::ui::presets::Fill::Solid(c) => *c,
        crate::ui::presets::Fill::Vertical { top, bottom } => {
            crate::ui::presets::mix(*top, *bottom, 0.5)
        }
        crate::ui::presets::Fill::Horizontal { left, right } => {
            crate::ui::presets::mix(*left, *right, 0.5)
        }
    };
    premultiplied(packed, opacity.unwrap_or(1.))
}

/// The premultiplied under-colour for the theme background token, used when
/// no preset is installed — the `None` arm of `workspace_background`.
fn theme_under(bg: Hsla) -> Rgba {
    let bg = Rgba::from(bg);
    Rgba {
        r: bg.r * bg.a,
        g: bg.g * bg.a,
        b: bg.b * bg.a,
        a: 1.,
    }
}

/// The premultiplied under-colour for a packed fill colour at the window's
/// own opacity — exactly the colour a `Solid` workspace fill paints over the
/// OS backdrop, ignoring the backdrop's own contribution.
fn premultiplied(packed: u32, opacity: f32) -> Rgba {
    Rgba {
        r: ((packed >> 16) & 0xff) as f32 / 255. * opacity,
        g: ((packed >> 8) & 0xff) as f32 / 255. * opacity,
        b: (packed & 0xff) as f32 / 255. * opacity,
        a: 1.,
    }
}

/// Blends `c` toward `under` in RGB space, keeping `c`'s own alpha. `under`
/// is passed pre-converted to `Rgba` because it is a frame constant: every
/// cell blends fg, bg and an optional underline colour against it, and the
/// HSL↔RGB round trip would otherwise run once per colour per cell. Linear
/// in the composited result: painting `blend_toward(a)` over `blend_toward(b)`
/// equals `blend_toward(a over b)`, which is exactly what a uniform opacity
/// of `dim` over the window background would produce.
fn blend_toward(c: Hsla, dim: f32, under: Rgba) -> Hsla {
    let c = Rgba::from(c);
    Rgba {
        r: dim * c.r + (1. - dim) * under.r,
        g: dim * c.g + (1. - dim) * under.g,
        b: dim * c.b + (1. - dim) * under.b,
        a: c.a,
    }
    .into()
}

impl PaintColors {
    pub(in crate::terminal) fn resolve(theme: &gpui_kit::component::Theme, cx: &gpui_kit::App) -> Self {
        let default_fg = theme.foreground;
        let default_bg = theme.background;
        let caret = theme.caret;
        let selection_bg = {
            let mut c = default_fg;
            c.a = 0.24;
            c
        };
        // Opaque, and solved for a contrast against the background rather than
        // a fixed alpha: the selection tint is only 24% away from the
        // background to begin with, so a 0.32 alpha landed 7% off it in every
        // theme — on a white one the matches were all but invisible. How far
        // the wash may travel is the theme's to say (`match_wash_targets`),
        // since the glyph on top of it pays for every step.
        let fg_rgb = crate::terminal::palette::hsla_to_rgb(default_fg);
        let bg_rgb = crate::terminal::palette::hsla_to_rgb(default_bg);
        let bg_packed = pack_rgb(bg_rgb);
        let tint = match_tint(cx);
        let (hit_target, current_target) =
            crate::ui::presets::match_wash_targets(bg_packed, pack_rgb(fg_rgb));
        let match_bg = to_hsla(unpack_rgb(crate::ui::presets::wash(
            bg_packed, tint, hit_target,
        )));
        let current_match_bg = to_hsla(unpack_rgb(crate::ui::presets::wash(
            bg_packed,
            tint,
            current_target,
        )));
        Self {
            default_fg,
            default_bg,
            caret,
            selection_bg,
            match_bg,
            current_match_bg,
            fg_rgb,
            bg_rgb,
            legible_dim: cx
                .try_global::<Config>()
                .is_none_or(|c| c.theme_legible_palette),
        }
    }

    /// Blends every colour toward `under` for a dimmed pane. The alpha of each
    /// colour is left alone — the blend happens in RGB, so translucent tints
    /// (selection, matches) composite over the already-blended cell colours
    /// exactly as dimmed as opaque content does.
    ///
    /// `fg_rgb`/`bg_rgb` are deliberately left raw: they only feed `resolve`
    /// for the named foreground/background cells, and those cells get blended
    /// by `dim_cell` like every other cell — blending them here as well would
    /// dim the named colours twice.
    pub(super) fn dimmed(&self, dim: f32, under: Rgba) -> Self {
        Self {
            default_fg: blend_toward(self.default_fg, dim, under),
            default_bg: blend_toward(self.default_bg, dim, under),
            caret: blend_toward(self.caret, dim, under),
            selection_bg: blend_toward(self.selection_bg, dim, under),
            match_bg: blend_toward(self.match_bg, dim, under),
            current_match_bg: blend_toward(self.current_match_bg, dim, under),
            fg_rgb: self.fg_rgb,
            bg_rgb: self.bg_rgb,
            legible_dim: self.legible_dim,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::snapshot::CellPainter;
    use super::super::test_support::{caret_colors, painted_term, test_colors};
    use rio_vt::crosswords::pos::{Column, Line, Pos};

    #[test]
    fn to_hsla_normalizes_channels_and_alpha() {
        let black = to_hsla(ColorRgb { r: 0, g: 0, b: 0 });
        assert_eq!(black.a, 1.0);
        assert!(black.l.abs() < 1e-6, "black has zero lightness");

        let white = to_hsla(ColorRgb {
            r: 255,
            g: 255,
            b: 255,
        });
        assert!((white.l - 1.0).abs() < 1e-6, "white has full lightness");
        assert!(white.s.abs() < 1e-6, "white is desaturated");

        let back = Rgba::from(to_hsla(ColorRgb { r: 255, g: 0, b: 0 }));
        assert!((back.r - 1.0).abs() < 1e-3);
        assert!(back.g.abs() < 1e-3 && back.b.abs() < 1e-3);
    }

    #[test]
    fn resolve_covers_every_color_slot() {
        let mut palette = [ColorRgb { r: 0, g: 0, b: 0 }; 256];
        for (i, slot) in palette.iter_mut().enumerate() {
            slot.r = i as u8;
        }
        let fg = ColorRgb {
            r: 200,
            g: 201,
            b: 202,
        };
        let bg = ColorRgb {
            r: 10,
            g: 11,
            b: 12,
        };

        let spec = ColorRgb { r: 1, g: 2, b: 3 };
        assert_eq!(
            resolve(AnsiColor::Spec(spec), &palette, fg, bg),
            (spec, false)
        );

        let (rgb, is_def) = resolve(AnsiColor::Indexed(5), &palette, fg, bg);
        assert_eq!(rgb.r, 5);
        assert!(!is_def);

        assert_eq!(
            resolve(AnsiColor::Named(NamedColor::Foreground), &palette, fg, bg),
            (fg, true)
        );
        assert_eq!(
            resolve(AnsiColor::Named(NamedColor::Background), &palette, fg, bg),
            (bg, true)
        );

        let (rgb, is_def) = resolve(AnsiColor::Named(NamedColor::Red), &palette, fg, bg);
        assert_eq!(rgb.r, NamedColor::Red as u8);
        assert!(!is_def);
    }

    /// What a cell's foreground lands on screen as: its rgb composited over
    /// the cell background by its alpha, the way the glyph is actually drawn.
    fn on_screen(fg: Hsla, under: ColorRgb) -> u32 {
        let c = Rgba::from(fg);
        let ch = |v: f32, u: u8| ((v * c.a + (u as f32 / 255.) * (1. - c.a)) * 255.).round() as u32;
        ch(c.r, under.r) << 16 | ch(c.g, under.g) << 8 | ch(c.b, under.b)
    }

    /// The colours a pane on builtin `t` resolves cells with.
    fn builtin_colors(t: &crate::ui::presets::Theme) -> (PaintColors, [ColorRgb; 256]) {
        let (fg, bg) = (unpack_rgb(t.foreground), unpack_rgb(t.background_color()));
        let mut colors = test_colors();
        colors.default_fg = to_hsla(fg);
        colors.default_bg = to_hsla(bg);
        colors.fg_rgb = fg;
        colors.bg_rgb = bg;
        let mut palette = crate::terminal::palette::build();
        palette[..16].copy_from_slice(&t.active_palette(true).ansi16);
        (colors, palette)
    }

    /// Every colour faint text is commonly written in: the default foreground,
    /// the sixteen palette slots, the 256-colour greys apps reach for as
    /// "muted", and a truecolour grey.
    fn faint_samples() -> Vec<(String, AnsiColor)> {
        let mut v = vec![("fg".to_string(), AnsiColor::Named(NamedColor::Foreground))];
        for i in 0..16u8 {
            v.push((format!("ansi{i}"), AnsiColor::Indexed(i)));
        }
        for i in [240u8, 244, 248, 250] {
            v.push((format!("256:{i}"), AnsiColor::Indexed(i)));
        }
        v.push((
            "#999999".to_string(),
            AnsiColor::Spec(ColorRgb {
                r: 153,
                g: 153,
                b: 153,
            }),
        ));
        v
    }

    /// (plain, faint) on-screen colours of `color` as a pane on `t` paints them.
    fn plain_and_faint(
        colors: &PaintColors,
        palette: &[ColorRgb; 256],
        color: AnsiColor,
    ) -> (RenderCell, RenderCell) {
        let point = Pos::new(Line(0), Column(0));
        let sgr = match color {
            AnsiColor::Indexed(index) => format!("38;5;{index}"),
            AnsiColor::Spec(rgb) => format!("38;2;{};{};{}", rgb.r, rgb.g, rgb.b),
            AnsiColor::Named(NamedColor::Foreground) => "39".into(),
            other => panic!("unexpected faint sample: {other:?}"),
        };
        let mut cell = painted_term(&format!("\x1b[{sgr}mx"));
        let plain = CellPainter::new(&cell, palette, colors, None).at(point);
        rio_vt::performer::handler::Processor::default()
            .advance(&mut cell, format!("\r\x1b[2;{sgr}mx").as_bytes());
        let faint = CellPainter::new(&cell, palette, colors, None).at(point);
        (plain, faint)
    }

    #[test]
    fn faint_text_keeps_the_text_floor_on_every_light_builtin() {
        // #858: SGR 2 painted the ink at 66% over the cell, which on a light
        // background took Catppuccin Latte's foreground to 3.2:1 and every
        // bright-black the palette rescue had lifted to 4.5:1 back to ~2.5:1.
        // Faint text on a light cell must now clear 4.5:1 — or, for an ink
        // that never cleared it undimmed, stay at the ink's own ratio.
        use crate::ui::presets::{builtins, contrast};
        let mut failures = Vec::new();
        eprintln!("| theme | colour | plain | faint |");
        for t in builtins().into_iter().filter(|t| !t.dark) {
            let (colors, palette) = builtin_colors(&t);
            let bg = t.background_color();
            for (name, color) in faint_samples() {
                let (plain, faint) = plain_and_faint(&colors, &palette, color);
                let plain = contrast(on_screen(plain.fg, colors.bg_rgb), bg);
                let faint = contrast(on_screen(faint.fg, colors.bg_rgb), bg);
                eprintln!("| {} | {name} | {plain:.2} | {faint:.2} |", t.id);
                if faint < 4.5_f32.min(plain) - 0.05 {
                    failures.push(format!(
                        "{}/{name}: faint {faint:.2}:1 (plain {plain:.2}:1)",
                        t.id
                    ));
                }
            }
        }
        assert!(
            failures.is_empty(),
            "faint text under the floor:\n{}",
            failures.join("\n")
        );
    }

    #[test]
    fn faint_text_on_dark_builtins_is_the_plain_fade() {
        // The floor is a light-background rescue: every dark builtin must
        // keep painting SGR 2 exactly as it did, as the ink at DIM_OPACITY.
        for t in crate::ui::presets::builtins()
            .into_iter()
            .filter(|t| t.dark)
        {
            let (colors, palette) = builtin_colors(&t);
            for (name, color) in faint_samples() {
                let (plain, faint) = plain_and_faint(&colors, &palette, color);
                let mut fade = plain.fg;
                fade.a *= DIM_OPACITY;
                assert_eq!(
                    faint.fg, fade,
                    "{}/{name}: dark theme faint text changed",
                    t.id
                );
            }
        }
    }

    #[test]
    fn faint_text_stays_fainter_than_plain_text() {
        // The rescue buys legibility, not a restyle: faint text never gains
        // contrast over its own ink, and an ink with room to spare above the
        // floor still reads visibly fainter.
        use crate::ui::presets::{builtins, contrast};
        for t in builtins() {
            let (colors, palette) = builtin_colors(&t);
            let bg = t.background_color();
            for (name, color) in faint_samples() {
                let (plain, faint) = plain_and_faint(&colors, &palette, color);
                let plain = contrast(on_screen(plain.fg, colors.bg_rgb), bg);
                let faint = contrast(on_screen(faint.fg, colors.bg_rgb), bg);
                assert!(
                    faint <= plain + 0.01,
                    "{}/{name}: faint {faint:.2} > plain {plain:.2}",
                    t.id
                );
                if plain >= 6.0 {
                    assert!(
                        faint <= plain * 0.85,
                        "{}/{name}: faint {faint:.2} no longer reads fainter than {plain:.2}",
                        t.id
                    );
                }
            }
        }
    }

    #[test]
    fn faint_text_is_the_plain_fade_with_the_legibility_switch_off() {
        // Settings → Appearance's palette switch off renders colours as
        // authored; that includes faint text.
        for t in crate::ui::presets::builtins() {
            let (mut colors, palette) = builtin_colors(&t);
            colors.legible_dim = false;
            for (name, color) in faint_samples() {
                let (plain, faint) = plain_and_faint(&colors, &palette, color);
                let mut fade = plain.fg;
                fade.a *= DIM_OPACITY;
                assert_eq!(faint.fg, fade, "{}/{name}: switch off still rescued", t.id);
            }
        }
    }

    #[test]
    fn blend_toward_mixes_in_rgb_space_and_keeps_alpha() {
        let under = Rgba {
            r: 16. / 255.,
            g: 18. / 255.,
            b: 19. / 255.,
            a: 1.,
        };
        let red = to_hsla(ColorRgb {
            r: 218,
            g: 98,
            b: 125,
        });
        let half = Rgba::from(blend_toward(red, 0.5, under));
        assert!((half.r - 0.5 * (218. / 255.) - 0.5 * (16. / 255.)).abs() < 1e-3);
        assert!((half.g - 0.5 * (98. / 255.) - 0.5 * (18. / 255.)).abs() < 1e-3);
        assert!((half.b - 0.5 * (125. / 255.) - 0.5 * (19. / 255.)).abs() < 1e-3);
        assert_eq!(half.a, 1.0, "an opaque colour stays opaque");

        // A translucent tint keeps its own alpha: only the rgb is blended.
        let tint = Hsla { a: 0.24, ..red };
        let dimmed = Rgba::from(blend_toward(tint, 0.55, under));
        assert!((dimmed.a - 0.24).abs() < 1e-3, "alpha is untouched");
        assert!(
            (dimmed.r - 0.55 * (218. / 255.) - 0.45 * (16. / 255.)).abs() < 1e-3,
            "rgb still blends toward the under"
        );
    }

    #[test]
    fn dimming_a_stacked_fill_stays_continuous_with_its_segment() {
        // The reported bug: a powerline separator (fill) drawn over its
        // segment (bg) dimmed unevenly because the old pane element-opacity
        // alpha-multiplied each layer, so the separator kept the segment's own
        // dim visible through its (1 - dim) and landed with a seam against
        // the segment (brighter on this palette). Blending every colour
        // toward the under first keeps the composite exactly as dimmed as any
        // single layer.
        let under = Rgba {
            r: 16. / 255.,
            g: 18. / 255.,
            b: 19. / 255.,
            a: 1.,
        };
        let segment = to_hsla(ColorRgb {
            r: 218,
            g: 98,
            b: 125,
        });
        let fill = to_hsla(ColorRgb {
            r: 154,
            g: 52,
            b: 142,
        });
        let dim = 0.55;

        let over = |top: Rgba, bottom: Rgba| Rgba {
            r: top.r * top.a + bottom.r * (1. - top.a),
            g: top.g * top.a + bottom.g * (1. - top.a),
            b: top.b * top.a + bottom.b * (1. - top.a),
            a: top.a + bottom.a * (1. - top.a),
        };

        // New rendering: both layers are pre-blended and stay opaque, so the
        // fill over the segment composites to exactly the dimmed fill — the
        // same value the segment itself renders as, with no seam between them.
        let dimmed_segment = Rgba::from(blend_toward(segment, dim, under));
        let dimmed_fill = Rgba::from(blend_toward(fill, dim, under));
        let new_composite = over(dimmed_fill, dimmed_segment);
        assert!((new_composite.r - dimmed_fill.r).abs() < 1e-6);
        assert!((new_composite.g - dimmed_fill.g).abs() < 1e-6);
        assert!((new_composite.b - dimmed_fill.b).abs() < 1e-6);

        // Old rendering: the pane opacity turned both layers translucent, so
        // the fill showed the already-dimmed segment underneath and painted
        // with a seam against the segment next to it.
        let old_fill = Rgba {
            a: dim,
            ..Rgba::from(fill)
        };
        let old_segment = Rgba {
            a: dim,
            ..Rgba::from(segment)
        };
        let old_composite = over(old_fill, old_segment);
        assert!(
            old_composite.r > new_composite.r + 0.05,
            "on this palette the old alpha-multiplied separator was visibly brighter than the segment"
        );
    }

    #[test]
    fn dim_cell_blends_truecolor_cells_toward_the_under() {
        // Starship's prompt paints its segments with direct 38;2;/48;2; SGR
        // colours that a palette-level dim would never see; the pane dim must
        // reach those cells too, or the whole prompt would stay at full
        // brightness while indexed content around it dims.
        let under = Rgba {
            r: 16. / 255.,
            g: 18. / 255.,
            b: 19. / 255.,
            a: 1.,
        };
        let mut cell = RenderCell::default();
        cell.c = 'x';
        cell.fg = to_hsla(ColorRgb {
            r: 218,
            g: 98,
            b: 125,
        });
        cell.bg = to_hsla(ColorRgb {
            r: 154,
            g: 52,
            b: 142,
        });
        cell.draw_bg = true;

        let dimmed = dim_cell(cell, 0.55, under);
        assert_eq!(dimmed.c, 'x', "the character itself survives");
        assert!(dimmed.draw_bg, "the explicit background flag survives");
        let fg = Rgba::from(dimmed.fg);
        assert!((fg.r - (0.55 * 218. + 0.45 * 16.) / 255.).abs() < 1e-3);
        assert!((fg.g - (0.55 * 98. + 0.45 * 18.) / 255.).abs() < 1e-3);
        assert!((fg.b - (0.55 * 125. + 0.45 * 19.) / 255.).abs() < 1e-3);
        let bg = Rgba::from(dimmed.bg);
        assert!((bg.r - (0.55 * 154. + 0.45 * 16.) / 255.).abs() < 1e-3);
        assert_eq!(fg.a, 1.0, "opaque cell colours stay opaque");

        // A DIM-flagged cell keeps its reduced alpha through the blend.
        let mut dim_flag = RenderCell::default();
        dim_flag.fg = to_hsla(ColorRgb {
            r: 218,
            g: 98,
            b: 125,
        });
        dim_flag.fg.a = DIM_OPACITY;
        let dimmed_flag = dim_cell(dim_flag, 0.55, under);
        assert!(
            (dimmed_flag.fg.a - DIM_OPACITY).abs() < 1e-3,
            "the SGR dim alpha survives the pane dim"
        );
    }

    #[test]
    fn paint_colors_dimmed_keeps_translucent_tint_alphas() {
        let colors = caret_colors();
        let under = Rgba {
            r: 16. / 255.,
            g: 18. / 255.,
            b: 19. / 255.,
            a: 1.,
        };
        let dimmed = colors.dimmed(0.55, under);
        assert!((dimmed.caret.a - colors.caret.a).abs() < 1e-3);
        assert!(
            (dimmed.selection_bg.a - colors.selection_bg.a).abs() < 1e-3,
            "translucent overlays keep their own alpha so they still tint"
        );
        assert_ne!(dimmed.default_fg, colors.default_fg);
        // The named foreground/background rgb feed `resolve` for default cells,
        // which `dim_cell` blends — pre-blending them here would double-dim.
        assert_eq!(dimmed.fg_rgb, colors.fg_rgb);
        assert_eq!(dimmed.bg_rgb, colors.bg_rgb);
    }

    #[test]
    fn premultiplied_under_scales_rgb_by_the_window_opacity_and_stays_opaque() {
        // A dimmed pane blends toward the colour the workspace actually paints
        // behind it: the fill scaled by the window's own opacity (premultiplied
        // alpha), never toward a colour that includes the OS backdrop.
        let u = premultiplied(0xda_62_7d, 0.82);
        assert_eq!(u.a, 1.0, "the under must stay an opaque paint colour");
        assert!((u.r - 0.82 * (218. / 255.)).abs() < 1e-6);
        assert!((u.g - 0.82 * (98. / 255.)).abs() < 1e-6);
        assert!((u.b - 0.82 * (125. / 255.)).abs() < 1e-6);
        // A fully opaque window keeps the fill untouched.
        let opaque = premultiplied(0xda_62_7d, 1.0);
        assert!((opaque.r - 218. / 255.).abs() < 1e-6);
        assert!((opaque.g - 98. / 255.).abs() < 1e-6);
        assert!((opaque.b - 125. / 255.).abs() < 1e-6);
    }

    #[test]
    fn fill_under_maps_preset_fills_to_their_premultiplied_under() {
        use crate::ui::presets::Fill;

        // A solid fill is exactly the colour the workspace paints behind the
        // terminal, scaled by the window's own opacity.
        let solid = fill_under(&Fill::Solid(0xda_62_7d), Some(0.82));
        assert!((solid.r - 0.82 * (218. / 255.)).abs() < 1e-6);
        assert!((solid.g - 0.82 * (98. / 255.)).abs() < 1e-6);
        assert!((solid.b - 0.82 * (125. / 255.)).abs() < 1e-6);
        assert_eq!(solid.a, 1.0);

        // A gradient is approximated by its midpoint stop (`mix` rounds each
        // channel, so 0x00…ff lands on 128/255 rather than exactly 0.5).
        let vertical = fill_under(
            &Fill::Vertical {
                top: 0x00_00_00,
                bottom: 0xff_ff_ff,
            },
            None,
        );
        assert!((vertical.r - 128. / 255.).abs() < 1e-6);
        assert_eq!(vertical.r, vertical.g);
        assert_eq!(vertical.r, vertical.b);
        let horizontal = fill_under(
            &Fill::Horizontal {
                left: 0xff_00_00,
                right: 0x00_00_ff,
            },
            None,
        );
        assert!((horizontal.r - 128. / 255.).abs() < 1e-6);
        assert!((horizontal.b - 128. / 255.).abs() < 1e-6);

        // No explicit opacity means the fill is used at full strength.
        let full = fill_under(&Fill::Solid(0xda_62_7d), None);
        assert!((full.r - 218. / 255.).abs() < 1e-6);
    }

    #[test]
    fn theme_under_premultiplies_by_the_theme_background_alpha() {
        // The no-preset fallback mirrors `theme().background`, whose alpha is
        // the window's own opacity.
        let bg = to_hsla(ColorRgb {
            r: 218,
            g: 98,
            b: 125,
        });
        let mut bg = bg;
        bg.a = 0.82;
        let under = theme_under(bg);
        assert_eq!(under.a, 1.0);
        assert!((under.r - 0.82 * (218. / 255.)).abs() < 1e-6);
        assert!((under.g - 0.82 * (98. / 255.)).abs() < 1e-6);
        assert!((under.b - 0.82 * (125. / 255.)).abs() < 1e-6);
        // An opaque theme token is used untouched.
        let opaque = theme_under(to_hsla(ColorRgb {
            r: 218,
            g: 98,
            b: 125,
        }));
        assert!((opaque.r - 218. / 255.).abs() < 1e-6);
    }
}
