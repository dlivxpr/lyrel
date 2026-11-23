//! The shared presentation seam: theme-global installation, the system
//! appearance record, and the small floating-surface/hairline helpers the
//! terminal client and the workbench shell both paint with.
//!
//! The shell's `apply_theme` wraps [`install_preset_theme`] with the platform
//! pieces only a window owner can do — native appearance sync, backdrop/DWM
//! materials, the machine-tree appearance note and menu refresh — while
//! terminal-side tests install a preset directly through this module.

use std::rc::{Rc, Weak};

use gpui_kit::component::scroll::ScrollbarMode;
use gpui_kit::component::{ActiveTheme, Theme, ThemeMode};
use gpui_kit::{App, Hsla, Pixels, Styled, Window, px, rgb};

use crate::config::Config;
use crate::presets;

#[derive(Clone, Copy)]
pub struct SystemAppearance {
    pub dark: bool,
}

impl gpui_kit::Global for SystemAppearance {}

fn is_dark(appearance: gpui_kit::WindowAppearance) -> bool {
    matches!(
        appearance,
        gpui_kit::WindowAppearance::Dark | gpui_kit::WindowAppearance::VibrantDark
    )
}

pub fn refresh_system_appearance(cx: &mut App) {
    let dark = is_dark(cx.window_appearance());
    cx.set_global(SystemAppearance { dark });
}

pub fn note_system_appearance(window: &Window, cx: &mut App) {
    let dark = is_dark(window.appearance());
    cx.set_global(SystemAppearance { dark });
}

pub fn system_dark(cx: &App) -> bool {
    cx.try_global::<SystemAppearance>()
        .is_some_and(|appearance| appearance.dark)
}

struct PresetPreview(Weak<String>);

impl gpui_kit::Global for PresetPreview {}

/// Override the effective preset only for the lifetime of the returned lease.
/// The preview never enters `Config`, even when another window saves settings.
pub fn preview_preset(id: String, cx: &mut App) -> Rc<String> {
    let lease = Rc::new(id);
    cx.set_global(PresetPreview(Rc::downgrade(&lease)));
    lease
}

pub fn effective_preset_id(cx: &App) -> String {
    if let Some(preset) = cx.try_global::<PresetPreview>().and_then(|p| p.0.upgrade()) {
        return preset.as_ref().clone();
    }
    let config = cx.global::<Config>();
    if !config.theme_follow_system {
        config.theme_preset.clone()
    } else if system_dark(cx) {
        config.theme_preset_dark.clone()
    } else {
        config.theme_preset_light.clone()
    }
}

struct StockUiFont(gpui_kit::SharedString);

impl gpui_kit::Global for StockUiFont {}

/// Install `theme` as the live GPUI theme and the presentation globals the
/// whole desktop reads (`ActiveBackground`, `Surfaces`, `Interactions`,
/// `ActiveAccent`, `ActiveLanes`, the active palette, fonts and notification
/// placement). `material_active` is the caller's answer to whether a Windows
/// DWM backdrop material shows through the window — the shell derives it from
/// the resolved backdrop; tests and non-Windows callers pass `false`.
pub fn install_preset_theme(
    window: Option<&mut Window>,
    cx: &mut App,
    theme: &presets::Theme,
    material_active: bool,
) {
    let config = cx.global::<Config>();
    let mode = if theme.dark {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    // A material needs some translucency to be visible; without an explicit
    // opacity override it defaults to `SYSTEM_MATERIAL_OPACITY` instead of
    // 1.0. Derived from the *resolved* appearance so old builds where
    // Blur/Acrylic fall back to plain transparency stay opaque by default.
    let material_opacity = 0.82f32;
    let fill_opacity = config
        .window_opacity
        .or(theme.opacity)
        .unwrap_or(if material_active {
            material_opacity
        } else {
            1.0
        });
    let opacity = (fill_opacity < 1.0).then_some(fill_opacity);
    let ui_font_family = config.ui_font_family.clone();
    let ui_font_size = px(config.ui_font_size);

    let m = theme.neutrals();
    let surfaces = theme.surfaces();
    let sem = theme.semantics();
    let interaction = theme.interactions();
    let active = theme.active_palette(config.theme_legible_palette);

    Theme::change(mode, window, cx);
    cx.set_global(active);
    cx.set_global(presets::ActiveBackground {
        fill: theme.background.clone(),
        opacity,
        image: theme.image.clone(),
    });
    cx.set_global(surfaces.clone());
    cx.set_global(interaction);
    cx.set_global(theme.parking());
    cx.set_global(presets::ActiveAccent(m.accent));
    cx.set_global(presets::ActiveAccentInk(m.accent_ink));
    // Same treatment as `Surfaces`: derived once here rather than recomputed
    // in `render`, because the graph reads it once per visible row per frame
    // and each entry costs a contrast bisection on three surfaces.
    cx.set_global(presets::ActiveLanes(theme.lanes()));

    if !cx.has_global::<StockUiFont>() {
        let stock = Theme::global(cx).font_family.clone();
        cx.set_global(StockUiFont(stock));
    }
    let stock = cx.global::<StockUiFont>().0.clone();

    let t = Theme::global_mut(cx);
    t.font_size = ui_font_size;
    // Toasts belong above the global status bar, in the bottom-right corner.
    // The margin follows the bar's `1.625rem` height at the same UI scale plus
    // the standard 8px gap, so zooming the interface keeps them clear of it.
    t.notification.placement = gpui_kit::Anchor::BottomRight;
    t.notification.margins.bottom = ui_font_size * 1.625 + px(8.);
    t.notification.margins.right = px(8.);
    // Assigned in both directions, never only when set: this is the one place
    // that decides the chrome's face, so it has to say "back to stock" as
    // plainly as it says "use Inter".
    t.font_family = match ui_font_family {
        Some(family) if !family.trim().is_empty() => family.into(),
        _ => stock,
    };
    let mut base: Hsla = rgb(m.background).into();
    if let Some(o) = opacity {
        base.a = o;
    }
    t.background = base;
    t.foreground = rgb(m.foreground).into();
    t.border = rgb(m.border).into();
    t.secondary = rgb(m.secondary).into();
    // Ink, not fill. gpui-component resolves both of these from its *stock*
    // foreground, so a button's label and the detail panel's section headings
    // were the only text in the window not written in the preset's own colour.
    t.button_foreground = rgb(m.foreground).into();
    t.secondary_foreground = rgb(m.foreground).into();
    t.muted = rgb(m.muted).into();
    t.muted_foreground = rgb(m.muted_foreground).into();
    // Components read both ThemeColor fields and layered tokens. Keep both
    // routes on our palette instead of leaking the component library defaults.
    t.tokens.foreground = Hsla::from(rgb(m.foreground)).into();
    t.tokens.border = Hsla::from(rgb(m.border)).into();
    t.tokens.muted = Hsla::from(rgb(m.muted)).into();
    t.tokens.muted_foreground = Hsla::from(rgb(m.muted_foreground)).into();
    t.tokens.secondary = Hsla::from(rgb(m.secondary)).into();
    t.tokens.secondary_foreground = Hsla::from(rgb(m.foreground)).into();
    t.button = rgb(m.popover).into();
    t.button_secondary = rgb(m.secondary).into();
    t.button_secondary_foreground = rgb(m.foreground).into();
    t.tokens.button = Hsla::from(rgb(m.popover)).into();
    t.tokens.button_foreground = Hsla::from(rgb(m.foreground)).into();
    t.tokens.button_secondary = Hsla::from(rgb(m.secondary)).into();
    t.tokens.button_secondary_foreground = Hsla::from(rgb(m.foreground)).into();
    t.popover = rgb(m.popover).into();
    t.tokens.popover = Hsla::from(rgb(m.popover)).into();
    t.tokens.popover_foreground = Hsla::from(rgb(m.foreground)).into();
    // The field beside the token, not a duplicate of it: gpui-component reads
    // this one for every tooltip, dropdown menu and date picker it draws. Left
    // unset it keeps the stock near-white, so menus and tooltips ignored the
    // preset and came out brighter than the window they float over.
    t.popover_foreground = rgb(m.foreground).into();

    let accent_fill = rgb(interaction.choice);
    let accent_text: Hsla = rgb(interaction.choice_ink).into();
    t.accent = accent_fill.into();
    t.accent_foreground = accent_text;
    t.tokens.accent = Hsla::from(accent_fill).into();
    t.tokens.accent_foreground = accent_text.into();

    let primary_base: Hsla = rgb(interaction.primary.fill).into();
    let primary_hover: Hsla = rgb(interaction.primary_hover).into();
    let primary_active: Hsla = rgb(interaction.primary_pressed).into();
    let primary_text: Hsla = rgb(interaction.primary.on_fill).into();
    t.primary = primary_base;
    t.primary_hover = primary_hover;
    t.primary_active = primary_active;
    t.primary_foreground = primary_text;
    t.button_primary_foreground = primary_text;
    t.tokens.primary = primary_base.into();
    t.tokens.primary_hover = primary_hover.into();
    t.tokens.primary_active = primary_active.into();
    t.tokens.primary_foreground = primary_text.into();
    t.tokens.button_primary = primary_base.into();
    t.tokens.button_primary_hover = primary_hover.into();
    t.tokens.button_primary_active = primary_active.into();
    t.tokens.button_primary_foreground = primary_text.into();

    let steps = |c: u32| {
        (
            Hsla::from(rgb(c)),
            Hsla::from(rgb(presets::mix(c, m.background, 0.15))),
            Hsla::from(rgb(presets::mix(c, m.foreground, 0.15))),
        )
    };

    let (ink, ink_hover, ink_active) = steps(sem.danger.ink);
    let (fill, fill_hover, fill_active) = steps(sem.danger.fill);
    let on_fill = Hsla::from(rgb(sem.danger.on_fill));
    t.danger = ink;
    t.danger_hover = ink_hover;
    t.danger_active = ink_active;
    t.danger_foreground = on_fill;
    t.tokens.danger = fill.into();
    t.tokens.danger_hover = fill_hover.into();
    t.tokens.danger_active = fill_active.into();
    t.tokens.danger_foreground = on_fill.into();
    t.tokens.button_danger = fill.into();
    t.tokens.button_danger_hover = fill_hover.into();
    t.tokens.button_danger_active = fill_active.into();
    t.tokens.button_danger_foreground = on_fill.into();

    let (ink, ink_hover, ink_active) = steps(sem.warning.ink);
    let (fill, fill_hover, fill_active) = steps(sem.warning.fill);
    let on_fill = Hsla::from(rgb(sem.warning.on_fill));
    t.warning = ink;
    t.warning_hover = ink_hover;
    t.warning_active = ink_active;
    t.warning_foreground = on_fill;
    t.tokens.warning = fill.into();
    t.tokens.warning_hover = fill_hover.into();
    t.tokens.warning_active = fill_active.into();
    t.tokens.warning_foreground = on_fill.into();
    t.tokens.button_warning = fill.into();
    t.tokens.button_warning_hover = fill_hover.into();
    t.tokens.button_warning_active = fill_active.into();
    t.tokens.button_warning_foreground = on_fill.into();

    let (ink, ink_hover, ink_active) = steps(sem.success.ink);
    let (fill, fill_hover, fill_active) = steps(sem.success.fill);
    let on_fill = Hsla::from(rgb(sem.success.on_fill));
    t.success = ink;
    t.success_hover = ink_hover;
    t.success_active = ink_active;
    t.success_foreground = on_fill;
    t.tokens.success = fill.into();
    t.tokens.success_hover = fill_hover.into();
    t.tokens.success_active = fill_active.into();
    t.tokens.success_foreground = on_fill.into();
    t.tokens.button_success = fill.into();
    t.tokens.button_success_hover = fill_hover.into();
    t.tokens.button_success_active = fill_active.into();
    t.tokens.button_success_foreground = on_fill.into();

    let (ink, ink_hover, ink_active) = steps(sem.info.ink);
    let (fill, fill_hover, fill_active) = steps(sem.info.fill);
    let on_fill = Hsla::from(rgb(sem.info.on_fill));
    t.info = ink;
    t.info_hover = ink_hover;
    t.info_active = ink_active;
    t.info_foreground = on_fill;
    t.tokens.info = fill.into();
    t.tokens.info_hover = fill_hover.into();
    t.tokens.info_active = fill_active.into();
    t.tokens.info_foreground = on_fill.into();
    t.tokens.button_info = fill.into();
    t.tokens.button_info_hover = fill_hover.into();
    t.tokens.button_info_active = fill_active.into();
    t.tokens.button_info_foreground = on_fill.into();

    // The command-line highlighter paints commands, flags, paths, strings and
    // operators with these, and the find bar flags a bad regex with `red`.
    // Unset they hold gpui-component's stock ramp, one ramp for every dark
    // preset and one for every light one — so the line you were typing kept a
    // palette the output right above it had already left behind.
    t.red = rgb(theme.ansi_ink(1)).into();
    t.green = rgb(theme.ansi_ink(2)).into();
    t.yellow = rgb(theme.ansi_ink(3)).into();
    t.blue = rgb(theme.ansi_ink(4)).into();
    t.magenta = rgb(theme.ansi_ink(5)).into();
    t.cyan = rgb(theme.ansi_ink(6)).into();

    t.link = rgb(sem.link.ink).into();
    t.link_hover = rgb(presets::mix(sem.link.ink, m.foreground, 0.25)).into();
    t.link_active = rgb(presets::mix(sem.link.ink, m.background, 0.20)).into();
    t.tokens.link = Hsla::from(rgb(sem.link.ink)).into();
    t.tokens.link_hover = Hsla::from(rgb(presets::mix(sem.link.ink, m.foreground, 0.25))).into();
    t.tokens.link_active = Hsla::from(rgb(presets::mix(sem.link.ink, m.background, 0.20))).into();

    let knob = if presets::is_lighter(m.background, m.foreground) {
        m.background
    } else {
        m.foreground
    };
    t.tokens.background = Hsla::from(rgb(m.background)).into();
    t.tokens.switch_thumb = Hsla::from(rgb(knob)).into();
    t.tokens.switch = Hsla::from(rgb(surfaces.window.selected)).into();

    // On-switches, sliders and primary actions share the accent role.
    t.tokens.slider_bar = Hsla::from(rgb(m.accent)).into();
    t.tokens.slider_thumb = Hsla::from(rgb(knob)).into();

    t.caret = rgb(m.caret).into();
    t.selection = rgb(m.selection).into();

    let scrollbar_thumb: Hsla = rgb(presets::mix(m.background, m.foreground, 0.18)).into();
    let scrollbar_thumb_hover: Hsla = rgb(presets::mix(m.background, m.foreground, 0.34)).into();
    t.scrollbar = gpui_kit::transparent_black();
    t.scrollbar_thumb = scrollbar_thumb;
    t.scrollbar_thumb_hover = scrollbar_thumb_hover;
    t.tokens.scrollbar = gpui_kit::transparent_black().into();
    t.tokens.scrollbar_thumb = scrollbar_thumb.into();
    t.tokens.scrollbar_thumb_hover = scrollbar_thumb_hover.into();

    // Not `should_auto_hide_scrollbars()`. That preference answers a question
    // about *legacy* scrollbars — the ones that take a gutter out of the
    // layout — and macOS says "don't hide them" for anyone with a mouse plugged
    // in. Ours are overlay bars painted on top of the content, so honouring it
    // parked an opaque bar over the switcher's tab column for the whole time
    // the panel was open, with nothing to fade it down. Every list in the app
    // gets the same bar, so it fades everywhere or nowhere.
    t.scrollbar_mode = ScrollbarMode::Scrolling;

    t.radius = px(8.);

    // Flat controls, floating panels. `Theme::shadow` gates exactly one thing —
    // the `shadow_xs` an inline control (button, input, select trigger,
    // checkbox, radio, slider knob) paints under itself — and never the drop
    // shadow on a menu, tooltip or popover, which every one of those draws
    // unconditionally. Left on, every field and button in the window carried a
    // faint lift that nothing else here has: this chrome separates surfaces
    // with low-contrast fills and hairlines, so a control sitting a millimetre
    // above the panel was the one place claiming depth, and it read as a
    // rendering artefact rather than as a material. Panels that really do
    // float keep their shadow.
    t.shadow = false;

    let sidebar_bg = Hsla::from(rgb(m.sidebar));
    // The current tab is a place, not an action: a neutral step of the rail's
    // own ladder plus a heavier title, never the accent. Blue is left to the
    // things that can be pressed.
    let sidebar_sel = rgb(surfaces.sidebar.selected);
    // `t.sidebar` stays the opaque theme token: the settings theme picker
    // paints with it on top of the (opaque) settings overlay, so diluting
    // it would wash out that panel. The workspace sidebar/right-panel
    // surfaces get their translucent variant at render time instead —
    // the shell derives them from `t.sidebar`.
    t.sidebar = sidebar_bg;
    t.tokens.sidebar = sidebar_bg.into();
    // The lighter tier: every `sidebar_border` site is a pane meeting another
    // pane, and both already carry their own fill. See `Neutrals`.
    t.sidebar_border = rgb(m.divider).into();
    t.sidebar_foreground = rgb(surfaces.sidebar.text_resting).into();
    t.sidebar_accent = sidebar_sel.into();
    t.tokens.sidebar_accent = Hsla::from(sidebar_sel).into();
    t.sidebar_accent_foreground = rgb(surfaces.sidebar.text_selected).into();

    t.list.active_highlight = true;
    t.list_active = rgb(interaction.choice).into();
    t.list_active_border = rgb(interaction.choice).into();
    t.list_hover = rgb(surfaces.popover.hover).into();
    // The matched substring in a searchable list is still text. Its fallback
    // would be gpui-component's stock `blue`, and the obvious accent fill is a
    // background role; both can sit legibly enough on the popover but not on
    // the keyboard row, which is painted in `accent`. `accent_foreground` is
    // the ink already paired with that fill, and it clears the popover behind
    // the unselected rows too. The row keeps the role paired with its own
    // surface, so the match never turns into the least readable text in the
    // list.
    let search_match_ink = t.accent_foreground;
    t.list_highlight_foreground = search_match_ink;

    // gpui-component's `input` is the border role, also used by default
    // buttons. Giving it the field background would erase their outlines.
    t.input = rgb(interaction.input_border).into();
    t.tokens.input = Hsla::from(rgb(interaction.input_border)).into();

    let button_hover: Hsla = rgb(surfaces.window.hover).into();
    let button_active: Hsla = rgb(surfaces.window.selected).into();
    t.tokens.button_hover = button_hover.into();
    t.tokens.button_active = button_active.into();
    t.tokens.secondary_hover = button_hover.into();
    t.tokens.secondary_active = button_active.into();
    t.tokens.button_secondary_hover = button_hover.into();
    t.tokens.button_secondary_active = button_active.into();

    t.ring = rgb(m.accent).into();
    t.tokens.ring = Hsla::from(rgb(m.accent)).into();
    // Every splitter and panel edge lights up with this while you drag it, and
    // the drop target for a dragged-in file tints with it. Unset it is a fixed
    // blue from gpui-component's stock theme — the same blue under all nine
    // presets. It is an interaction highlight, so it belongs with the focus
    // ring and the on-switch.
    t.drag_border = rgb(m.accent).into();

    // Keep the editor body transparent so it inherits gradients and wallpaper.
    // The published editor paints scrolling text before the fixed line-number
    // gutter, so that gutter must be opaque to cover text scrolling beneath it.
    // Match the existing document surface; only this narrow column stops
    // showing the background image. Active-line and invisible ink stay
    // translucent.
    let ink: Hsla = rgb(m.foreground).into();
    let mut highlight = (*t.highlight_theme).clone();
    highlight.style.editor_background = Some(gpui_kit::transparent_black());
    highlight.style.editor_gutter_background = Some(t.sidebar);
    highlight.style.editor_active_line = Some(ink.opacity(0.06));
    highlight.style.editor_line_number = Some(rgb(m.muted_foreground).into());
    highlight.style.editor_active_line_number = Some(ink);
    highlight.style.editor_invisible = Some(ink.opacity(0.25));
    t.highlight_theme = std::sync::Arc::new(highlight);
    Theme::sync_base(cx);
}

/// Shared opaque floating surface. In-window GPUI layers cannot blur the
/// terminal beneath them, so depth comes from the edge and shadow instead.
pub fn floating_surface<T: Styled>(element: T, cx: &App) -> T {
    let theme = cx.theme();
    element
        .bg(theme.popover)
        .text_color(theme.popover_foreground)
        .border_1()
        .border_color(theme.border.opacity(0.65))
        .rounded(crate::rounding::POPOVER_RADIUS)
        .shadow_xl()
}

/// One device pixel: the v4 hairline, 0.5px on a Retina panel and a whole
/// pixel on a 1x one. A literal `px(0.5)` would be antialiased down to half
/// its ink on a 1x display, under the contrast floor `divider` is held to.
pub fn hairline(window: &Window) -> Pixels {
    px(1. / window.scale_factor().max(1.))
}
