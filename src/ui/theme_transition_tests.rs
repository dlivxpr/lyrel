//! The production-workbench consumer tests for theme crossfades, plus the
//! pure mixing tests behind them.
//!
//! The seam is the real app path: `apply_theme`, which every theme consumer
//! — settings saves, system appearance changes, theme-picker previews —
//! funnels through. Colors are read back through the same public resolution
//! the surfaces use: `cx.theme()` for the chrome and
//! `PaintColors::resolve_with` for a terminal frame. Frames are stepped by
//! advancing the pinned fade clock together with the virtual executor clock,
//! so a test observes the transition itself, not only its endpoints.

use std::time::{Duration, Instant};

use gpui_kit::component::ActiveTheme as _;
use lyrel_core::core::config::MotionPreference;
use lyrel_core::core::pane_colors::PaneColors;
use lyrel_terminal::element::PaintColors;
use lyrel_ui_base::config::Config;
use lyrel_ui_base::presets;

use super::{fade_duration, mix_color, mix_preset};
use crate::ui::app::test_window;

/// Where the crossfade currently sits: the chrome colors every workbench
/// surface reads from `cx.theme()`, next to the colors a terminal frame
/// resolves through `PaintColors::resolve_with` — the entry the terminal
/// element paints with.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
struct Frame {
    background: u32,
    foreground: u32,
    caret: u32,
    terminal_bg: u32,
    terminal_fg: u32,
    terminal_caret: u32,
}

fn frame(cx: &gpui_kit::App) -> Frame {
    let theme = cx.theme();
    let terminal = PaintColors::resolve_with(theme, PaneColors::default(), cx);
    let (terminal_fg, terminal_bg, terminal_caret) = terminal.defaults();
    Frame {
        background: super::pack(theme.background),
        foreground: super::pack(theme.foreground),
        caret: super::pack(theme.caret),
        terminal_bg: super::pack(terminal_bg),
        terminal_fg: super::pack(terminal_fg),
        terminal_caret: super::pack(terminal_caret),
    }
}

fn set_motion(vcx: &mut gpui_kit::VisualTestContext, preference: MotionPreference) {
    vcx.update(|_, cx| {
        cx.global_mut::<Config>().motion_preference = preference;
        crate::ui::motion::apply(cx);
    });
}

fn switch_theme(vcx: &mut gpui_kit::VisualTestContext, id: &str) {
    vcx.update(|window, cx| {
        cx.global_mut::<Config>().theme_preset = id.to_string();
        crate::ui::theme::apply_theme(Some(window), cx);
    });
}

/// Advances the pinned fade clock and the virtual executor clock together,
/// letting the driver task run every frame it owes.
fn pump(vcx: &mut gpui_kit::VisualTestContext, clock: &super::TestClockGuard, ms: u64) {
    clock.advance(Duration::from_millis(ms));
    vcx.executor().advance_clock(Duration::from_millis(ms));
    vcx.run_until_parked();
}

#[gpui_kit::test]
fn theme_switch_crossfades_interface_and_terminal_colors_in_every_window(
    cx: &mut gpui_kit::TestAppContext,
) {
    let clock = super::install_test_clock();
    let (_first_app, mut first) = test_window::harness(cx);
    let (_second_app, mut second) = test_window::harness(cx);
    set_motion(&mut first, MotionPreference::Allow);
    set_motion(&mut second, MotionPreference::Allow);

    // Baselines the fade is judged against, captured as instant installs
    // through the same reader: what a snap switch to each theme looks like.
    set_motion(&mut first, MotionPreference::Reduce);
    switch_theme(&mut first, "dark");
    let dark = first.update(|_, cx| frame(cx));
    switch_theme(&mut first, "light");
    let light = first.update(|_, cx| frame(cx));
    set_motion(&mut first, MotionPreference::Allow);

    // The switch lands in a frame that still shows the old colors; the first
    // mixed frame arrives with the next driver tick.
    switch_theme(&mut first, "dark");
    assert_eq!(
        first.update(|_, cx| frame(cx)),
        light,
        "the frame the change lands in still shows the old colors"
    );
    assert!(
        first.update(|_, cx| super::fade_active(cx)),
        "an allowed theme change starts a crossfade"
    );

    // Mid-fade: both windows travel the same palette, and the terminal's own
    // resolution moves with the chrome's instead of keeping a snapshot.
    pump(&mut first, &clock, 16);
    let mid = first.update(|_, cx| frame(cx));
    assert_ne!(
        mid, light,
        "the transition is observable, not a before/after snap"
    );
    assert_ne!(mid, dark);
    second.update(|_, cx| {
        assert_eq!(frame(cx), mid, "every open window travels the same palette");
    });
    assert_eq!(
        mid.terminal_bg, mid.background,
        "the terminal frame background tracks the chrome background"
    );
    assert_eq!(
        mid.terminal_fg, mid.foreground,
        "the terminal frame ink tracks the chrome foreground"
    );
    assert_eq!(
        mid.terminal_caret, mid.caret,
        "the terminal caret tracks the theme caret"
    );

    // To completion: exactly the instant baseline, then the driver rests.
    pump(&mut first, &clock, 300);
    assert_eq!(first.update(|_, cx| frame(cx)), dark);
    second.update(|_, cx| assert_eq!(frame(cx), dark));
    assert!(!first.update(|_, cx| super::fade_active(cx)));

    pump(&mut first, &clock, 100);
    assert_eq!(
        first.update(|_, cx| frame(cx)),
        dark,
        "a landed fade leaves no further frames behind"
    );
}

#[gpui_kit::test]
fn reduced_motion_lands_the_target_theme_immediately_and_snaps_a_running_fade(
    cx: &mut gpui_kit::TestAppContext,
) {
    let clock = super::install_test_clock();
    let (_app, mut vcx) = test_window::harness(cx);
    set_motion(&mut vcx, MotionPreference::Reduce);

    switch_theme(&mut vcx, "dark");
    let dark = vcx.update(|_, cx| frame(cx));
    pump(&mut vcx, &clock, 100);
    assert_eq!(
        vcx.update(|_, cx| frame(cx)),
        dark,
        "reduced motion installs the target colors in one step"
    );
    assert!(!vcx.update(|_, cx| super::fade_active(cx)));

    switch_theme(&mut vcx, "light");
    let light = vcx.update(|_, cx| frame(cx));
    assert_ne!(light, dark);

    // A running fade snaps to the target on the first tick after motion is
    // reduced, then settles without owing more frames.
    set_motion(&mut vcx, MotionPreference::Allow);
    switch_theme(&mut vcx, "dark");
    pump(&mut vcx, &clock, 30);
    let mid = vcx.update(|_, cx| frame(cx));
    assert_ne!(mid, dark, "the fade was traveling before the reduction");
    assert!(vcx.update(|_, cx| super::fade_active(cx)));

    set_motion(&mut vcx, MotionPreference::Reduce);
    pump(&mut vcx, &clock, 16);
    assert_eq!(
        vcx.update(|_, cx| frame(cx)),
        dark,
        "reducing motion mid-fade lands the selected theme in one step"
    );
    assert!(!vcx.update(|_, cx| super::fade_active(cx)));
    pump(&mut vcx, &clock, 100);
    assert_eq!(vcx.update(|_, cx| frame(cx)), dark);
}

#[gpui_kit::test]
fn rapid_retargeting_lands_on_the_last_selected_theme(cx: &mut gpui_kit::TestAppContext) {
    let clock = super::install_test_clock();
    let (_app, mut vcx) = test_window::harness(cx);

    // Instant baselines for comparison.
    set_motion(&mut vcx, MotionPreference::Reduce);
    switch_theme(&mut vcx, "dark");
    let dark = vcx.update(|_, cx| frame(cx));
    switch_theme(&mut vcx, "nord");
    let nord = vcx.update(|_, cx| frame(cx));
    switch_theme(&mut vcx, "light");
    let light = vcx.update(|_, cx| frame(cx));
    set_motion(&mut vcx, MotionPreference::Allow);

    // The first selection starts a fade; a quick second selection re-aims it
    // from wherever the palette had traveled to — nothing snaps back to a
    // catalog constant.
    switch_theme(&mut vcx, "dark");
    pump(&mut vcx, &clock, 90);
    let mid = vcx.update(|_, cx| frame(cx));
    assert_ne!(mid, light);
    assert_ne!(mid, dark);
    switch_theme(&mut vcx, "nord");
    assert_eq!(
        vcx.update(|_, cx| frame(cx)),
        mid,
        "re-aiming leaves the colors on screen untouched until the next tick"
    );
    pump(&mut vcx, &clock, 60);
    let retargeted = vcx.update(|_, cx| frame(cx));
    assert_ne!(retargeted, nord, "the re-aimed fade is still traveling");
    assert_ne!(
        retargeted, light,
        "it did not restart from the previous catalog theme"
    );
    assert_ne!(retargeted, dark);

    pump(&mut vcx, &clock, 300);
    assert_eq!(
        vcx.update(|_, cx| frame(cx)),
        nord,
        "the last selected theme wins"
    );
    assert!(!vcx.update(|_, cx| super::fade_active(cx)));
}

#[gpui_kit::test]
fn theme_crossfade_sends_no_terminal_input_and_no_resize(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::daemon::protocol::ClientMsg;
    use std::io::ErrorKind;

    let clock = super::install_test_clock();
    let (_app, mut vcx, mut stream) = test_window::harness_with_pane(cx);
    set_motion(&mut vcx, MotionPreference::Allow);

    // Drain whatever the pane setup legitimately wrote (the spawn
    // handshake): the assertion below is that the fade itself stays silent.
    stream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let quiet_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match ClientMsg::read(&mut stream) {
            Ok(_) => {}
            Err(_) => break,
        }
        if Instant::now() > quiet_deadline {
            panic!("the pane setup never went quiet");
        }
    }

    switch_theme(&mut vcx, "dark");
    for _ in 0..20 {
        pump(&mut vcx, &clock, 16);
    }
    assert!(!vcx.update(|_, cx| super::fade_active(cx)));

    let deadline = Instant::now() + Duration::from_millis(300);
    loop {
        match ClientMsg::read(&mut stream) {
            Ok(ClientMsg::Input(bytes)) => {
                panic!("theme crossfade sent terminal input: {bytes:?}")
            }
            Ok(ClientMsg::Resize(size)) => {
                panic!("theme crossfade changed the pane grid geometry: {size:?}")
            }
            Ok(_) => {}
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(error) => panic!("pane socket failed: {error}"),
        }
        if Instant::now() > deadline {
            break;
        }
    }
}

#[test]
fn mix_preset_keeps_destination_identity_and_exact_endpoints() {
    let themes = presets::builtins();
    let light = themes.iter().find(|t| t.id == "light").unwrap();
    let dark = themes.iter().find(|t| t.id == "dark").unwrap();

    let untouched = mix_preset(light, dark, 0.0);
    assert_eq!(untouched.id, light.id);
    assert_eq!(untouched.background, light.background);
    assert_eq!(untouched.ansi16, light.ansi16);

    let landed = mix_preset(light, dark, 1.0);
    assert_eq!(landed.id, dark.id);
    assert_eq!(landed.background, dark.background);
    assert_eq!(landed.ansi16, dark.ansi16);
    assert_eq!(landed.caret, dark.caret);

    let midway = mix_preset(light, dark, 0.5);
    assert!(
        midway.dark,
        "identity is the destination's from the first traveling frame"
    );
    assert_eq!(midway.id, dark.id);
    assert_eq!(midway.opacity, dark.opacity);
    assert_eq!(midway.blur, dark.blur);
    assert_ne!(midway.background, light.background);
    assert_ne!(midway.background, dark.background);
    assert_ne!(midway.ansi16, light.ansi16);
    assert_ne!(midway.ansi16, dark.ansi16);
}

#[test]
fn fade_duration_flip_tracks_lightness_travel() {
    let themes = presets::builtins();
    let get = |id: &str| themes.iter().find(|t| t.id == id).unwrap().clone();
    let light = get("light");
    let dark = get("dark");
    let nord = get("nord");
    let one_light = get("one_light");

    // Light↔dark crosses most of the lightness range and earns the longer
    // flip; two themes of one appearance stay on the short fade.
    assert_eq!(fade_duration(&light, &dark), super::FLIP_FADE);
    assert_eq!(fade_duration(&dark, &light), super::FLIP_FADE);
    assert_eq!(fade_duration(&dark, &nord), super::FADE);
    assert_eq!(fade_duration(&light, &one_light), super::FADE);
}

#[test]
fn oklab_mix_stays_perceptually_straight() {
    let white: gpui_kit::Hsla = gpui_kit::rgb(0xffffff).into();
    let black: gpui_kit::Hsla = gpui_kit::rgb(0x000000).into();
    let mid = gpui_kit::Rgba::from(mix_color(white, black, 0.5));
    assert!(
        (mid.r - mid.g).abs() < 1e-3 && (mid.g - mid.b).abs() < 1e-3,
        "the gray midpoint stays gray, not a tinted dissolve"
    );
    assert!(
        (0.15..=0.85).contains(&mid.r),
        "the perceptual midpoint is not an sRGB-channel dissolve"
    );

    // Two saturated colors of unrelated hue meet in gamut.
    let blue: gpui_kit::Hsla = gpui_kit::rgb(0x1f6bf0).into();
    let amber: gpui_kit::Hsla = gpui_kit::rgb(0xe0a345).into();
    let cross = gpui_kit::Rgba::from(mix_color(blue, amber, 0.5));
    for channel in [cross.r, cross.g, cross.b] {
        assert!(
            (0.0..=1.0).contains(&channel),
            "the out-of-gamut midpoint is pulled back into range"
        );
    }

    // Endpoints are exact, so a landed fade matches an instant install.
    let a: gpui_kit::Hsla = gpui_kit::rgb(0x78a8f5).into();
    let b: gpui_kit::Hsla = gpui_kit::rgb(0x1f6bf0).into();
    assert_eq!(mix_color(a, b, 0.0), a);
    assert_eq!(mix_color(a, b, 1.0), b);
}
