//! The desktop palette shared by the UI and the terminal grid: the active
//! theme's sixteen ANSI slots as a GPUI global, and the one HSL→RGB conversion
//! every surface agrees on. Defined here, below both the workbench and the
//! terminal, so neither side imports the other for a color.

use gpui_kit::Global;
use rio_vt::config::colors::ColorRgb;

/// The active theme's terminal colors. The workbench installs it when a
/// preset is applied; renderers below read it through `try_global`.
#[derive(Debug, Clone)]
pub struct ActivePalette {
    pub ansi16: [ColorRgb; 16],
    pub sel_bg: ColorRgb,
}

impl Global for ActivePalette {}

/// The shared HSL→RGB rounding rule: `round`, clamped into the u8 range.
pub fn hsla_to_rgb(c: gpui_kit::Hsla) -> ColorRgb {
    let rgba = gpui_kit::Rgba::from(c);
    ColorRgb {
        r: (rgba.r * 255.0).round().clamp(0.0, 255.0) as u8,
        g: (rgba.g * 255.0).round().clamp(0.0, 255.0) as u8,
        b: (rgba.b * 255.0).round().clamp(0.0, 255.0) as u8,
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
        assert_eq!((white.r, white.g, white.b), (0xff, 0xff, 0xff));
    }
}
