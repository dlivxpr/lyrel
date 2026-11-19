//! The desktop palette shared by the UI and the terminal grid: the active
//! theme's sixteen ANSI slots as a GPUI global, and the one HSL→RGB conversion
//! every surface agrees on. Defined here, below both the workbench and the
//! terminal, so neither side imports the other for a color.

use gpui_kit::Global;

/// The active theme's terminal colors. The workbench installs it when a
/// preset is applied; renderers below read it through `try_global`.
#[derive(Debug, Clone)]
pub struct ActivePalette {
    ansi16: [[u8; 3]; 16],
    selection_bg: [u8; 3],
}

impl Global for ActivePalette {}

impl ActivePalette {
    pub fn new(ansi16: [[u8; 3]; 16], selection_bg: [u8; 3]) -> Self {
        Self {
            ansi16,
            selection_bg,
        }
    }

    pub fn ansi16(&self) -> &[[u8; 3]; 16] {
        &self.ansi16
    }

    pub fn selection_bg(&self) -> [u8; 3] {
        self.selection_bg
    }
}

/// The shared HSL→RGB rounding rule: `round`, clamped into the u8 range.
pub fn hsla_to_rgb(c: gpui_kit::Hsla) -> [u8; 3] {
    let rgba = gpui_kit::Rgba::from(c);
    [rgba.r, rgba.g, rgba.b].map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsla_to_rgb_round_trips_a_known_color() {
        let rgb = hsla_to_rgb(gpui_kit::rgb(0x123456).into());
        assert_eq!(rgb, [0x12, 0x34, 0x56]);

        let black = hsla_to_rgb(gpui_kit::rgb(0x000000).into());
        assert_eq!(black, [0, 0, 0]);

        let white = hsla_to_rgb(gpui_kit::rgb(0xffffff).into());
        assert_eq!(white, [255, 255, 255]);
    }
}
