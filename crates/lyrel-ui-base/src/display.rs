//! Small read-only display formatting shared by desktop surfaces.

use crate::i18n::{L10nKey, t};

/// The word tabs at the top of a side panel, a half step under body text.
pub const TAB_TEXT: f32 = 12.5 * (1. / 16.);

/// Horizontal inset between window-chrome content and the frame.
pub const CONTENT_INSET: f32 = 12.;

/// Tabular figures, so a column of counts, sizes and positions keeps its
/// digits on one grid while the numbers change under it.
pub fn tabular_figures() -> gpui_kit::FontFeatures {
    gpui_kit::FontFeatures(std::sync::Arc::new(vec![("tnum".to_string(), 1)]))
}

/// A byte count the way the remote install sheet and the SQLite browser both
/// spell it: whole bytes below one KiB, then one decimal in binary units.
pub fn human_bytes(n: u64) -> String {
    const KIB: f64 = 1024.0;
    let n = n as f64;
    if n < KIB {
        return format!("{} {}", n as u64, t(L10nKey::RemoteInstallBytes));
    }
    let units = ["KiB", "MiB", "GiB"];
    let mut value = n / KIB;
    for (i, unit) in units.iter().enumerate() {
        if value < KIB || i == units.len() - 1 {
            return format!("{value:.1} {unit}");
        }
        value /= KIB;
    }
    unreachable!("the loop returns on its last iteration")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_in_binary_units() {
        crate::i18n::set_locale("en");
        assert_eq!(human_bytes(512), "512 bytes");
        assert_eq!(human_bytes(2048), "2.0 KiB");
        assert_eq!(human_bytes(5 * 1024 * 1024), "5.0 MiB");
    }
}

/// Inline-code highlighting for the Markdown surfaces the workbench editor and
/// the shell's detail views share.
pub fn markdown_style(cx: &gpui_kit::App) -> gpui_kit::component::text::TextViewStyle {
    use gpui_kit::component::ActiveTheme as _;

    gpui_kit::component::text::TextViewStyle::default().inline_code(gpui_kit::HighlightStyle {
        background_color: Some(cx.theme().foreground.opacity(0.08)),
        ..Default::default()
    })
}
