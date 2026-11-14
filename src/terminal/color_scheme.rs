//! DEC mode 2031, colour palette update notifications: a program that switches
//! it on (`CSI ? 2031 h`) is sent `CSI ? 997 ; 1 n` (dark) or `; 2 n` (light)
//! whenever the theme's background changes, and `CSI ? 996 n` asks for the
//! same report. Claude Code's `theme: auto` depends on it: it reads OSC 11 once
//! at startup and again only when a 997 arrives.
//! <https://contour-terminal.org/vt-extensions/color-palette-update-notifications/>

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::component::ActiveTheme;
use gpui_kit::{App, Context};
use lyrel_core::core::term_modes::{COLOR_SCHEME_UPDATES, TerminalModes};
use rio_vt::config::colors::ColorRgb;

use super::remote::PaneEvent;
use super::view::TerminalView;
use crate::ui::presets::is_dark;

/// The report for a background, dark or light by the same test the theme
/// presets use.
pub(super) fn report(bg: ColorRgb) -> String {
    let rgb = u32::from(bg.r) << 16 | u32::from(bg.g) << 8 | u32::from(bg.b);
    format!("\x1b[?997;{}n", if is_dark(rgb) { 1 } else { 2 })
}

/// Fold pane output into `modes` and publish whether 2031 is on.
pub(super) fn fold(modes: &mut TerminalModes, bytes: &[u8], on: &AtomicBool) {
    modes.feed(bytes);
    on.store(modes.is_on(COLOR_SCHEME_UPDATES), Ordering::Relaxed);
}

/// The answer to a `CSI ? 996 n`. Routed as a background colour request so
/// the view answers it from the theme it is drawing now, and a replay drops it
/// like every other reply.
pub(super) fn query_reply() -> PaneEvent {
    PaneEvent::ColorRequest(257, Arc::new(report))
}

fn background(cx: &App) -> ColorRgb {
    super::palette::hsla_to_rgb(cx.theme().background)
}

/// The background a program would see now: an explicit pane override, else the
/// live theme. Mode 2031's dark/light report follows this, not the workbench
/// theme in isolation.
pub(super) fn effective_background(
    cx: &App,
    overrides: &lyrel_core::core::pane_colors::PaneColors,
) -> ColorRgb {
    overrides
        .get(lyrel_core::core::pane_colors::DEFAULT_BG)
        .map(super::palette::rgb_color)
        .unwrap_or_else(|| background(cx))
}

/// Push a report to the pane each time its effective background changes while
/// its program has 2031 on. Every path to a new theme (a preset pick, a
/// follow-system flip, a config reload) ends in the `Theme` global; a colour
/// override or reset calls [`TerminalView::report_color_scheme_if_changed`]
/// directly.
pub(super) fn watch(cx: &mut Context<TerminalView>) {
    cx.observe_global::<gpui_kit::component::Theme>(|view, cx| {
        view.report_color_scheme_if_changed(cx);
    })
    .detach();
}

impl TerminalView {
    pub(super) fn report_color_scheme_if_changed(&mut self, cx: &mut Context<Self>) {
        let overrides = *self
            .terminal
            .color_overrides
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let now = effective_background(cx, &overrides);
        if std::mem::replace(&mut self.color_scheme_bg, now) != now
            && self.terminal.color_scheme_updates()
        {
            self.terminal.write(report(now).into_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dark_background_reports_dark_and_a_light_one_light() {
        assert_eq!(report(ColorRgb { r: 0, g: 43, b: 54 }), "\x1b[?997;1n");
        assert_eq!(
            report(ColorRgb {
                r: 253,
                g: 246,
                b: 227
            }),
            "\x1b[?997;2n"
        );
    }
}
