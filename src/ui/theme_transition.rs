//! Live theme crossfades, shared by the workbench chrome and every terminal.
//!
//! A theme switch re-derives the whole desktop palette — semantic neutrals,
//! interaction roles, surfaces, Git lanes, the terminal's frame colors — from
//! the one resolved preset. The fade therefore interpolates the *preset*, not
//! any rendered color list: each tick installs a preset mixed between the one
//! on screen and the one selected, and every consumer — sidebars, panels,
//! menus, terminal panes, in every window — reads the same installed globals
//! it always has. No surface keeps its own palette copy, so no two surfaces
//! can disagree mid-fade, and a pane's resolved cell colors stay frame-local
//! the way the terminal element has always treated them.
//!
//! The behavioral reference is Diri's `app_theme/live.rs`: the theme being
//! faded to owns the identity of every intermediate frame; a fade in flight
//! is abandoned where it stands and re-aimed at the newest selection, so the
//! last theme selected always wins; reduced motion (or the very first
//! install) lands the target palette in one step.
//!
//! Frames are timer-driven, not render-driven: the driver is one short app
//! task per fade, so a window closing mid-fade cannot strand the desktop on
//! a half-mixed palette, and a headless test can step a fade by advancing
//! the clock. Each intermediate install goes through
//! `install_preset_theme`, which refreshes every window, and the terminal
//! re-resolves its paint colors from the live theme on every paint — the
//! fade never sends terminal input, never resizes the PTY, and never drives
//! the TUI's own animations.

use std::time::{Duration, Instant};

use gpui_kit::{App, Global, Hsla};
use lyrel_ui_base::presets::{self, Fill};

/// How often the driver samples the fade while one is live.
const TICK: Duration = Duration::from_millis(8);
/// A theme change within one appearance (Diri `FADE`).
const FADE: Duration = Duration::from_millis(180);
/// A light/dark flip crossing most of the lightness range reads as a flash at
/// `FADE`'s length and gets the longer duration (Diri `FLIP_FADE`).
const FLIP_FADE: Duration = Duration::from_millis(240);
/// Oklab background-lightness travel that earns the flip duration
/// (Diri `FLIP_TRAVEL`).
const FLIP_TRAVEL: f32 = 0.6;

// --- Oklab color mixing ----------------------------------------------------
//
// Blending sRGB channels is what a dissolve between two screenshots does: its
// lightness runs ahead of the perceptual middle, and two accents of different
// hue meet in a tinted, dirtier color than either. Oklab moves lightness at a
// steady perceived rate and takes the straight line between the two colors,
// so a crossfade reads as the light changing rather than as one picture
// dissolving into another. Reimplemented from the behavior described in
// Diri's `crossfade.rs`; the matrices are the standard sRGB↔Oklab ones.

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// sRGB (gamma) → Oklab, as the Cartesian `[L, a, b]`. The line between two
/// colors runs through (a, b), never around the hue circle: a gray has no hue
/// of its own, and interpolating an angle would swing it through unrelated
/// colors on the way to a saturated one.
fn oklab(rgba: gpui_kit::Rgba) -> [f32; 3] {
    let r = srgb_to_linear(rgba.r);
    let g = srgb_to_linear(rgba.g);
    let b = srgb_to_linear(rgba.b);
    let l = 0.412_221_47 * r + 0.536_332_54 * g + 0.051_445_99 * b;
    let m = 0.211_903_5 * r + 0.680_699_55 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_84 * g + 0.629_978_7 * b;
    let (l, m, s) = (l.cbrt(), m.cbrt(), s.cbrt());
    [
        0.210_454_26 * l + 0.793_617_78 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_8 * m - 0.808_675_77 * s,
    ]
}

/// Oklab → in-gamut sRGB. The sRGB gamut is not convex in Oklab, so the
/// midpoint of two displayable colors can fall just outside it; the chroma
/// scale is bisected until the color re-enters the cube, shedding saturation
/// instead of clipping a channel.
fn oklab_to_srgb(lab: [f32; 3]) -> gpui_kit::Rgba {
    let to_linear = |lab: [f32; 3]| -> [f32; 3] {
        let l_ = lab[0] + 0.396_337_78 * lab[1] + 0.215_803_76 * lab[2];
        let m_ = lab[0] - 0.105_561_35 * lab[1] - 0.063_854_17 * lab[2];
        let s_ = lab[0] - 0.089_484_18 * lab[1] - 1.291_485_5 * lab[2];
        let (l, m, s) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
        [
            4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_93 * s,
            -1.268_438_1 * l + 2.609_757_4 * m - 0.341_319_4 * s,
            -0.004_196_086_5 * l - 0.703_418_6 * m + 1.707_614_7 * s,
        ]
    };
    let in_gamut = |lab: [f32; 3]| to_linear(lab).into_iter().all(|c| (0.0..=1.0).contains(&c));
    let mut lab = lab;
    if !in_gamut(lab) {
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..12 {
            let mid = (lo + hi) / 2.0;
            let candidate = [lab[0], lab[1] * mid, lab[2] * mid];
            if in_gamut(candidate) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        lab = [lab[0], lab[1] * lo, lab[2] * lo];
    }
    let [r, g, b] = to_linear(lab);
    gpui_kit::Rgba {
        r: linear_to_srgb(r.clamp(0.0, 1.0)),
        g: linear_to_srgb(g.clamp(0.0, 1.0)),
        b: linear_to_srgb(b.clamp(0.0, 1.0)),
        a: 1.0,
    }
}

/// `from` toward `to` along the straight Oklab line; alpha moves linearly.
/// The endpoints are exact — a finished fade leaves the catalog constant
/// behind, bit for bit — so `t <= 0.` is `from` and `t >= 1.` is `to`.
fn mix_color(from: Hsla, to: Hsla, t: f32) -> Hsla {
    if t <= 0.0 || from == to {
        return if t <= 0.0 { from } else { to };
    }
    if t >= 1.0 {
        return to;
    }
    let (a, b) = (gpui_kit::Rgba::from(from), gpui_kit::Rgba::from(to));
    let [from_l, from_a, from_b] = oklab(a);
    let [to_l, to_a, to_b] = oklab(b);
    let lerp = |x: f32, y: f32| x + (y - x) * t;
    let mut mixed = oklab_to_srgb([lerp(from_l, to_l), lerp(from_a, to_a), lerp(from_b, to_b)]);
    mixed.a = lerp(a.a, b.a);
    mixed.into()
}

fn unpack(n: u32) -> Hsla {
    Hsla::from(gpui_kit::rgb(n))
}

fn pack(h: Hsla) -> u32 {
    let c = gpui_kit::Rgba::from(h);
    let ch = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    (ch(c.r) << 16) | (ch(c.g) << 8) | ch(c.b)
}

/// The seed `neutrals()` floors the caret against: an authored caret, else
/// the accent.
fn effective_caret(theme: &presets::Theme) -> u32 {
    theme.caret.unwrap_or(theme.accent)
}

/// The seed `neutrals()` blends a default selection from.
fn effective_selection(theme: &presets::Theme) -> u32 {
    theme
        .selection
        .unwrap_or_else(|| presets::mix(theme.background_color(), theme.foreground, 0.20))
}

/// The preset on screen on the way from `from` to `to` at progress `t`.
/// Identity (id, name, dark) and the window attributes a preset carries
/// (opacity, blur, image, path) are the destination's from the first frame;
/// only the color inputs interpolate. The identity rule is what keeps a
/// settings preview or a follow-system resolution pointed at the theme being
/// faded to, never at an intermediate catalog entry.
fn mix_preset(from: &presets::Theme, to: &presets::Theme, t: f32) -> presets::Theme {
    if t <= 0.0 {
        return from.clone();
    }
    if t >= 1.0 {
        return to.clone();
    }
    let mix_u32 = |a: u32, b: u32| pack(mix_color(unpack(a), unpack(b), t));
    let background = match (&from.background, &to.background) {
        (Fill::Solid(a), Fill::Solid(b)) => Fill::Solid(mix_u32(*a, *b)),
        (
            Fill::Vertical { top, bottom },
            Fill::Vertical {
                top: to_top,
                bottom: to_bottom,
            },
        ) => Fill::Vertical {
            top: mix_u32(*top, *to_top),
            bottom: mix_u32(*bottom, *to_bottom),
        },
        (
            Fill::Horizontal { left, right },
            Fill::Horizontal {
                left: to_left,
                right: to_right,
            },
        ) => Fill::Horizontal {
            left: mix_u32(*left, *to_left),
            right: mix_u32(*right, *to_right),
        },
        // A structural mismatch keeps the destination's shape and blends the
        // source's base color into it: gradients stay gradients, solids stay
        // solid, and no frame collapses a designed fill to a flat slab.
        _ => match &to.background {
            Fill::Solid(c) => Fill::Solid(mix_u32(from.background.color(), *c)),
            Fill::Vertical { top, bottom } => Fill::Vertical {
                top: mix_u32(from.background.color(), *top),
                bottom: mix_u32(from.background.color(), *bottom),
            },
            Fill::Horizontal { left, right } => Fill::Horizontal {
                left: mix_u32(from.background.color(), *left),
                right: mix_u32(from.background.color(), *right),
            },
        },
    };
    let mut ansi16 = to.ansi16;
    for (i, slot) in ansi16.iter_mut().enumerate() {
        let a = from.ansi16[i];
        let b = to.ansi16[i];
        let mixed = mix_u32(
            ((a.0 as u32) << 16) | ((a.1 as u32) << 8) | a.2 as u32,
            ((b.0 as u32) << 16) | ((b.1 as u32) << 8) | b.2 as u32,
        );
        *slot = ((mixed >> 16) as u8, (mixed >> 8) as u8, mixed as u8);
    }
    presets::Theme {
        id: to.id.clone(),
        name: to.name.clone(),
        dark: to.dark,
        background,
        foreground: mix_u32(from.foreground, to.foreground),
        accent: mix_u32(from.accent, to.accent),
        caret: Some(mix_u32(effective_caret(from), effective_caret(to))),
        selection: Some(mix_u32(effective_selection(from), effective_selection(to))),
        opacity: to.opacity,
        blur: to.blur,
        image: to.image.clone(),
        ansi16,
        path: to.path.clone(),
    }
}

/// Whether installing `b` would paint the same colors as `a`. Opacity, blur,
/// backdrop, fonts and the background image are window or chrome attributes,
/// not colors: a change confined to them must stay an instant apply, never a
/// crossfade.
fn same_mix_inputs(a: &presets::Theme, b: &presets::Theme) -> bool {
    a.background == b.background
        && a.foreground == b.foreground
        && a.accent == b.accent
        && a.caret == b.caret
        && a.selection == b.selection
        && a.ansi16 == b.ansi16
        && a.dark == b.dark
}

/// How long the crossfade from `from` to `to` runs.
fn fade_duration(from: &presets::Theme, to: &presets::Theme) -> Duration {
    if from.dark != to.dark {
        let from_lightness = oklab(unpack(from.background.color()).into())[0];
        let to_lightness = oklab(unpack(to.background.color()).into())[0];
        let travel = (from_lightness - to_lightness).abs();
        if travel >= FLIP_TRAVEL {
            return FLIP_FADE;
        }
    }
    FADE
}

/// One crossfade: `from` (what is on screen) toward `to` (what was selected).
struct ThemeFade {
    from: presets::Theme,
    to: presets::Theme,
    started: Instant,
    duration: Duration,
    material_active: bool,
}

impl ThemeFade {
    fn progress(&self, now: Instant) -> f32 {
        (now.saturating_duration_since(self.started).as_secs_f32() / self.duration.as_secs_f32())
            .clamp(0.0, 1.0)
    }

    /// The preset to install at `now`. Identity is always the destination's;
    /// at `t >= 1` this is exactly `to`.
    fn sample(&self, now: Instant) -> presets::Theme {
        mix_preset(&self.from, &self.to, self.progress(now))
    }
}

#[derive(Default)]
struct ThemeTransition {
    fade: Option<ThemeFade>,
    /// The last preset handed to [`plan`]. While a fade runs this is the fade
    /// target; between fades it is what is on screen. It is what a new fade
    /// (or the same-inputs check) compares against, so a custom theme edited
    /// and re-applied under the same id still reads as a color change.
    installed: Option<presets::Theme>,
    /// Whether the driver task is live. Only ever one task runs: a retarget
    /// is picked up by the running loop's next tick.
    driving: bool,
}

impl Global for ThemeTransition {}

/// What `apply_theme` should do with the resolved preset.
pub(crate) enum ThemePlan {
    /// Install it before returning; no fade will run.
    InstallNow,
    /// A fade toward it is (or remains) in flight; the driver installs the
    /// intermediate presets, and `install_preset_theme`'s window refresh
    /// carries them to every open window each tick.
    Fading,
}

/// Decides how `apply_theme` lands `theme`, and keeps the fade bookkeeping.
/// Every apply funnels here — settings saves, system appearance changes,
/// theme-picker previews — so any sequence of rapid changes collapses onto
/// the last selection and the desktop never jumps backward to a catalog
/// constant mid-fade.
pub(crate) fn plan(theme: presets::Theme, material_active: bool, cx: &mut App) -> ThemePlan {
    if !cx.has_global::<ThemeTransition>() {
        cx.set_global(ThemeTransition::default());
    }
    if cx.reduce_motion() {
        // Reduced motion reaches the same final state in one step; a fade in
        // flight is dropped where it stands.
        let state = cx.global_mut::<ThemeTransition>();
        state.fade = None;
        state.installed = Some(theme);
        return ThemePlan::InstallNow;
    }
    let now = now();
    enum Action {
        InstallNow,
        /// A fade is live: re-aim it from the colors on screen. Any apply
        /// during a fade — a different theme, a preview step, an opacity
        /// nudge — continues smoothly toward the newest target instead of
        /// snapping to it.
        Retarget,
        StartFade,
    }
    let action = {
        let state = cx.global_mut::<ThemeTransition>();
        match &state.fade {
            Some(fade) => {
                let from = fade.sample(now);
                let duration = fade_duration(&from, &theme);
                state.fade = Some(ThemeFade {
                    from,
                    to: theme.clone(),
                    started: now,
                    duration,
                    material_active,
                });
                state.installed = Some(theme);
                Action::Retarget
            }
            None => match &state.installed {
                // The very first install (startup) has nothing to fade from.
                None => {
                    state.installed = Some(theme);
                    Action::InstallNow
                }
                Some(installed) if same_mix_inputs(installed, &theme) => Action::InstallNow,
                Some(installed) => {
                    let from = installed.clone();
                    let duration = fade_duration(&from, &theme);
                    state.fade = Some(ThemeFade {
                        from,
                        to: theme.clone(),
                        started: now,
                        duration,
                        material_active,
                    });
                    state.installed = Some(theme);
                    Action::StartFade
                }
            },
        }
    };
    if matches!(action, Action::StartFade) {
        start_driver(cx);
    }
    match action {
        Action::InstallNow => ThemePlan::InstallNow,
        Action::Retarget | Action::StartFade => ThemePlan::Fading,
    }
}

/// The one app task that owns fade frames while a fade is live. Timer-driven
/// on purpose: a render-driven chain (the Diri shape) strands the desktop on
/// a half-mixed palette if the window hosting the next frame closes
/// mid-fade, and a timer is what a headless test can step deterministically
/// by advancing the clock.
fn start_driver(cx: &mut App) {
    {
        let state = cx.global_mut::<ThemeTransition>();
        if state.driving {
            return;
        }
        state.driving = true;
    }
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(TICK).await;
            if !cx.update(tick) {
                return;
            }
        }
    })
    .detach();
}

/// Advances the fade one tick. Answers whether the driver owes another frame.
fn tick(cx: &mut App) -> bool {
    let now = now();
    if cx.reduce_motion() {
        // Motion was reduced mid-fade: land the target in one step and rest.
        let target = {
            let state = cx.global_mut::<ThemeTransition>();
            state.driving = false;
            state
                .fade
                .take()
                .map(|fade| (fade.to, fade.material_active))
        };
        if let Some((theme, material)) = target {
            lyrel_ui_base::presentation::install_preset_theme(None, cx, &theme, material);
        }
        return false;
    }
    enum Step {
        Rest,
        Mixed(presets::Theme, bool),
        Land(presets::Theme, bool),
    }
    enum FadeAction {
        Rest,
        Mixed(presets::Theme, bool),
        Land(presets::Theme, bool),
    }
    let step = {
        let state = cx.global_mut::<ThemeTransition>();
        // Sample the fade (cloning what the frame needs) before mutating the
        // state the sample borrows from.
        let action = match &state.fade {
            None => FadeAction::Rest,
            Some(fade) if fade.progress(now) >= 1.0 => {
                FadeAction::Land(fade.to.clone(), fade.material_active)
            }
            Some(fade) => FadeAction::Mixed(fade.sample(now), fade.material_active),
        };
        match action {
            FadeAction::Rest => {
                state.driving = false;
                Step::Rest
            }
            FadeAction::Mixed(theme, material) => Step::Mixed(theme, material),
            FadeAction::Land(theme, material) => {
                state.driving = false;
                state.fade = None;
                Step::Land(theme, material)
            }
        }
    };
    match step {
        Step::Rest => false,
        Step::Mixed(theme, material) => {
            lyrel_ui_base::presentation::install_preset_theme(None, cx, &theme, material);
            true
        }
        Step::Land(theme, material) => {
            lyrel_ui_base::presentation::install_preset_theme(None, cx, &theme, material);
            false
        }
    }
}

#[cfg(not(test))]
fn now() -> Instant {
    Instant::now()
}

#[cfg(test)]
thread_local! {
    /// The fade clock a test has pinned, if any. GPUI tests share a process
    /// with every other test, so the override lives on the test's own thread
    /// and only for the guard's lifetime.
    static TEST_NOW: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
fn now() -> Instant {
    TEST_NOW.with(|cell| cell.get().unwrap_or_else(Instant::now))
}

#[cfg(test)]
pub(crate) struct TestClockGuard {
    _private: (),
}

#[cfg(test)]
impl TestClockGuard {
    /// Moves the pinned clock forward. The virtual executor clock the driver
    /// task waits on is advanced separately, by the caller.
    pub(crate) fn advance(&self, by: Duration) {
        TEST_NOW.with(|cell| {
            let current = cell.get().expect("the test clock is installed");
            cell.set(Some(current + by));
        });
    }
}

#[cfg(test)]
impl Drop for TestClockGuard {
    fn drop(&mut self) {
        TEST_NOW.with(|cell| cell.set(None));
    }
}

/// Pins the fade clock to a manual instant. The real clock stays in place
/// for everything that does not read [`now`].
#[cfg(test)]
pub(crate) fn install_test_clock() -> TestClockGuard {
    TEST_NOW.with(|cell| cell.set(Some(Instant::now())));
    TestClockGuard { _private: () }
}

/// Whether a fade is currently in flight. The consumer tests assert this to
/// prove a transition settled — no pending frames — rather than guessing
/// from the colors alone.
#[cfg(test)]
pub(crate) fn fade_active(cx: &App) -> bool {
    cx.try_global::<ThemeTransition>()
        .is_some_and(|state| state.fade.is_some())
}

#[cfg(test)]
#[path = "theme_transition_tests.rs"]
mod tests;
