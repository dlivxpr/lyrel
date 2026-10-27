//! 窗口级标题栏与常驻六按钮；独立于标签导航和文档布局。

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, TitleBar, h_flex,
};
use gpui_kit::{
    AnyElement, App, Axis, ClickEvent, Pixels, Point, Window, img, point, prelude::*, px, rems,
};

use crate::core::actions::{
    SplitDown, SplitRight, ToggleLeftPanel, TogglePalette, ToggleRightPanel,
};
use crate::ui::i18n::{L10nKey, t};

pub(crate) const TITLE_BAR_HEIGHT: f32 = 48.;

/// The rule every title-bar row carries along its bottom edge.
///
/// `TitleBar` itself draws one (`gpui-component`'s own `.border_b_1()`), and
/// taffy sizes a node by its border box, so the bar's content box is this one
/// point short of its height and `items_center` puts a tile's centre half a
/// point above the bar's middle. Every row that stands in for the title bar
/// has to carry the same point — in a transparent ink where the row shows no
/// rule of its own — or its tiles land half a point below the traffic lights
/// and below the strip they are meant to read as one line with: one device
/// pixel on a Retina panel, and visible precisely because the two are side by
/// side in the same band.
pub(crate) const TITLE_BAR_BORDER: f32 = 1.;

/// The line the traffic lights and every [`title_strip_row`]'s contents sit
/// on, from the top of the window's content.
///
/// The lights hang from a point, not a centre, so macOS gives this back through
/// `traffic_light_position` less half the system button's height.
const TITLE_STRIP_CENTRE: f32 = (TITLE_BAR_HEIGHT - TITLE_BAR_BORDER) / 2.;

pub(crate) fn title_strip_centre() -> gpui_kit::Pixels {
    gpui_kit::px(TITLE_STRIP_CENTRE)
}

/// Split the title bar's bottom edge between the rule a row draws and the
/// padding that stands in for the part of it the rule does not reach.
fn title_strip_rule_and_pad(rule: gpui_kit::Pixels) -> (gpui_kit::Pixels, gpui_kit::Pixels) {
    let border = rule.min(gpui_kit::px(TITLE_BAR_BORDER));
    (border, gpui_kit::px(TITLE_BAR_BORDER - border.as_f32()))
}

/// The centre of the box a row built by [`title_strip_row`] gives its contents:
/// the bar's height, less whatever its bottom edge spends on rule and padding.
#[cfg(test)]
fn title_strip_content_centre(rule: gpui_kit::Pixels) -> gpui_kit::Pixels {
    let (border, pad) = title_strip_rule_and_pad(rule);
    gpui_kit::px((TITLE_BAR_HEIGHT - border.as_f32() - pad.as_f32()) / 2.)
}

/// Give a row the title bar's geometry: its height, its vertical centring, and
/// a bottom edge that leaves the content box the height
/// [`title_strip_centre`] assumes.
///
/// A method rather than a function, because a row's own chain — its insets, its
/// tiles, its drag band — is what the file is about, and this is one more link in
/// it. Calling it mid-chain keeps the row readable instead of burying it in an
/// argument.
///
/// `rule` is the border the row wants. A row standing in for the title bar with
/// no visible rule passes [`TITLE_BAR_BORDER`] in a transparent ink, so the point
/// is spent on the box and not on the paint. A row that draws its own thinner
/// hairline keeps that hairline — it stays flush with the box's bottom edge
/// either way — and makes up the difference as padding, which is what keeps its
/// tiles on the same line as the lights over them.
pub(crate) trait TitleStripRow: Sized {
    fn title_strip_row(self, rule: gpui_kit::Pixels, ink: gpui_kit::Hsla) -> Self;
}

impl<R: gpui_kit::Styled> TitleStripRow for R {
    fn title_strip_row(self, rule: gpui_kit::Pixels, ink: gpui_kit::Hsla) -> Self {
        let (border, pad) = title_strip_rule_and_pad(rule);
        self.h(gpui_kit::px(TITLE_BAR_HEIGHT))
            .items_center()
            .border_b(border)
            .border_color(ink)
            .pb(pad)
    }
}

pub(crate) const TILE_SIZE: f32 = 32.;
pub(crate) const TILE_GLYPH: f32 = 16.;
/// A tile that sits in a body row rather than in chrome: the box shrinks to
/// the minimum hit target, but the glyph keeps the chrome size. An 11px glyph
/// here read as a disabled ornament next to 14px text, and put a second,
/// smaller folder in the same column as the panel's folder tab.
pub(crate) const TILE_SIZE_SM: f32 = 24.;
pub(crate) const TILE_GLYPH_SM: f32 = TILE_GLYPH;

/// The tile that lives *inside* a list row rather than beside one, for the
/// buttons a row reveals on hover.
///
/// A box below [`TILE_SIZE_SM`] because of width: three `TILE_SIZE_SM` squares
/// would eat 72 of the 236px a file name has to live in, where three of these
/// eat 54.
pub(crate) const TILE_SIZE_XS: f32 = 18.;
pub(crate) const TILE_GLYPH_XS: f32 = 11.;

const TILE_PAD: f32 = (TILE_SIZE - TILE_GLYPH) / 2.;

pub(crate) const CONTENT_INSET: f32 = 12.;

const TILE_EDGE_GAP: f32 = 5.;

pub(crate) fn tile_trailing_inset() -> f32 {
    (CONTENT_INSET - TILE_PAD).max(TILE_EDGE_GAP)
}

const TITLE_BAR_LEAD: f32 = if cfg!(target_os = "macos") {
    TRAFFIC_LIGHTS_LEAD
} else {
    12.
};

/// Where macOS puts the window's traffic lights over the tab strip: centred on
/// the row of chrome tiles they share the bar with. `x` matches `y`, as AppKit's
/// unified toolbar does (19 and 19 in a 52-point bar).
///
/// `y` is the gap between the window's top edge and the *top* of the button
/// frame, and macOS draws those buttons [`TRAFFIC_LIGHT_BUTTON_H`] tall — so the
/// lights' centre falls half that below it. Deriving the number from
/// [`crate::ui::window_chrome::title_strip_centre`], the line every title-bar row puts its
/// contents on, is what keeps the lights on the tile line: the value the 40-point bar needed was
/// rewritten by hand when the bar grew to 48, without the border in it, and left
/// the lights half a point — one device pixel on a Retina panel — below
/// everything beside them.
pub(crate) fn traffic_light_position() -> Point<Pixels> {
    point(px(TRAFFIC_LIGHT_INSET), px(TRAFFIC_LIGHT_INSET))
}

/// The height of AppKit's own standard-window-button frame, which is what the
/// lights are drawn inside. It does not move with the bar's height.
const TRAFFIC_LIGHT_BUTTON_H: f32 = 14.;

// The same content centre drives both the native buttons and the rows beside
// them. Keep the inset const so the leading reserve can follow it at compile
// time instead of maintaining a second copy of the bar's border arithmetic.
const TRAFFIC_LIGHT_INSET: f32 = TITLE_STRIP_CENTRE - TRAFFIC_LIGHT_BUTTON_H / 2.;

/// Close to zoom: three 14-point buttons on a 20-point pitch.
const TRAFFIC_LIGHTS_W: f32 = 54.;

/// The room the lights take at the bar's leading edge on macOS: their span,
/// with the inset they sit at on either side.
const TRAFFIC_LIGHTS_LEAD: f32 =
    TRAFFIC_LIGHT_INSET + TRAFFIC_LIGHTS_W + TRAFFIC_LIGHT_INSET;


const WINDOW_MARK_SIZE: f32 = 20.;

fn title_bar_drag(
    row: gpui_kit::Stateful<gpui_kit::Div>,
    key: &'static str,
    window: &mut gpui_kit::Window,
    cx: &mut gpui_kit::App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    window_move_gesture(row, key, window, cx).on_double_click(|_, window, _| {
        window.titlebar_double_click();
    })
}

struct WindowMoveArm {
    should_move: bool,
}

pub(crate) fn window_move_gesture(
    row: gpui_kit::Stateful<gpui_kit::Div>,
    key: &'static str,
    window: &mut gpui_kit::Window,
    cx: &mut gpui_kit::App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    window_move_gesture_with(row, key, window, cx, gpui_kit::Window::start_window_move)
}

fn window_move_gesture_with(
    row: gpui_kit::Stateful<gpui_kit::Div>,
    key: &'static str,
    window: &mut gpui_kit::Window,
    cx: &mut gpui_kit::App,
    start_move: impl Fn(&gpui_kit::Window) + 'static,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let arm = window.use_keyed_state(key, cx, |_, _| WindowMoveArm { should_move: false });
    row.window_control_area(gpui_kit::WindowControlArea::Drag)
        .on_mouse_down(
            gpui_kit::MouseButton::Left,
            window.listener_for(&arm, |arm, _: &gpui_kit::MouseDownEvent, _, _| {
                arm.should_move = true;
            }),
        )
        .on_mouse_down_out(
            window.listener_for(&arm, |arm, _: &gpui_kit::MouseDownEvent, _, _| {
                arm.should_move = false;
            }),
        )
        .on_mouse_up(
            gpui_kit::MouseButton::Left,
            window.listener_for(&arm, |arm, _: &gpui_kit::MouseUpEvent, _, _| {
                arm.should_move = false;
            }),
        )
        .on_mouse_up_out(
            gpui_kit::MouseButton::Left,
            window.listener_for(&arm, |arm, _: &gpui_kit::MouseUpEvent, _, _| {
                arm.should_move = false;
            }),
        )
        .on_mouse_move(window.listener_for(
            &arm,
            move |arm, _: &gpui_kit::MouseMoveEvent, window, _| {
                if arm.should_move {
                    arm.should_move = false;
                    start_move(window);
                }
            },
        ))
}

fn window_mark() -> impl IntoElement {
    static LOGO: std::sync::OnceLock<Arc<gpui_kit::Image>> = std::sync::OnceLock::new();
    let logo = LOGO
        .get_or_init(|| {
            Arc::new(gpui_kit::Image::from_bytes(
                gpui_kit::ImageFormat::Png,
                include_bytes!("../../assets/logo@256.png").to_vec(),
            ))
        })
        .clone();
    img(logo)
        .size(gpui_kit::rems(WINDOW_MARK_SIZE / 16.))
        .flex_shrink_0()
}

/// Whether the title bar carries minimize, maximize and close right now.
///
/// Not in fullscreen. A fullscreen window has no caption: Windows clears
/// `WS_CAPTION` and answers `HTCLIENT` along the whole top edge, so the three
/// buttons would draw, light up under the pointer and do nothing when clicked.
/// The row they sit at the end of stays, because it is also the tab strip.
///
/// Never on macOS, which draws no buttons of its own: those are the system's
/// traffic lights, and the system hides them itself.
pub(crate) fn window_controls_drawn(fullscreen: bool) -> bool {
    !cfg!(target_os = "macos") && !fullscreen
}

pub(crate) fn chrome_tile_variant(cx: &gpui_kit::App) -> ButtonCustomVariant {
    chrome_tile_variant_for(false, cx)
}

fn chrome_tile_variant_for(selected: bool, cx: &gpui_kit::App) -> ButtonCustomVariant {
    ButtonCustomVariant::new(cx)
        .color(cx.theme().transparent)
        // Resting chrome is *not* body ink. `sidebar_foreground` is the rung a
        // tab title is written at, so a toolbar drawn in it made the two
        // controls at the top of the rail the darkest marks in the whole
        // sidebar — louder than the twenty rows they exist to act on, which is
        // the opposite of how a native sidebar ranks itself. One rung down puts
        // them level with the workspace chip beside them, and the hover fill
        // this variant already carries is what answers the pointer.
        .foreground(if selected {
            cx.theme().foreground
        } else {
            cx.theme().muted_foreground
        })
        // `sidebar_accent` is the surface's *selected* step, and it was handed
        // to hover as well — so a hovered tile wore the fill of a selected one
        // and, with the right panel open, two tiles read as current at once.
        // Hover takes the step the palette derives for it. (A selected button
        // never renders the hover style, so this only reaches the rest.)
        .hover(gpui_kit::rgb(cx.global::<crate::ui::presets::Surfaces>().sidebar.hover).into())
        .active(cx.theme().sidebar_accent)
}

pub(crate) const BUTTON_ICON_SCALE: f32 = 0.75;

pub(crate) fn chrome_tile(button: Button, selected: bool, cx: &gpui_kit::App) -> Button {
    chrome_tile_sized(button, TILE_SIZE, TILE_GLYPH, selected, cx)
}

pub(crate) fn chrome_tile_sized(
    button: Button,
    tile: f32,
    glyph: f32,
    selected: bool,
    cx: &gpui_kit::App,
) -> Button {
    button
        .custom(chrome_tile_variant_for(selected, cx))
        .selected(selected)
        .with_size(px(glyph / BUTTON_ICON_SCALE))
        .w(px(tile))
        .h(px(tile))
}


/// 工作台只接收语义意图，窗口 chrome 不持有工作台或标签页的可变访问权。
#[derive(Clone, Copy)]
pub(crate) enum ChromeCommand {
    ToggleLeftPanel,
    ToggleSearch,
    Split(Axis),
    ToggleRightPanel,
}

/// 常驻控件与原生窗口几何；拖动状态通过窗口内稳定 key 跨重绘保留。
pub(crate) struct WindowChrome {
    left: AnyElement,
    right: AnyElement,
}

impl WindowChrome {
    pub(crate) fn new<F>(
        left_open: bool,
        right_open: bool,
        can_split: bool,
        new_tab: impl IntoElement,
        cx: &App,
        on_command: impl Fn(ChromeCommand) -> F,
    ) -> Self
    where
        F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    {
        let tile = |button: Button| button.custom(chrome_tile_variant(cx)).size_8().rounded_lg();
        let group = || {
            h_flex()
                // 按钮区不能穿透到 TitleBar 的原生命中与按下拖动处理。
                .occlude()
                .flex_none()
                .p_0p5()
                .gap_0p5()
                .rounded_lg()
                .bg(cx.theme().muted.opacity(0.5))
        };
        let left = group()
            .child(
                tile(
                    Button::new("titlebar-left-panel").icon(
                        Icon::empty()
                            .path(if left_open {
                                "icons/panel-left.svg"
                            } else {
                                "icons/panel-left-collapsed.svg"
                            })
                            .size_4(),
                    ),
                )
                .accessibility_label(t(if left_open {
                    L10nKey::TabTooltipHideSidebar
                } else {
                    L10nKey::TabTooltipShowSidebar
                }))
                .tooltip_with_action(
                    t(if left_open {
                        L10nKey::TabTooltipHideSidebar
                    } else {
                        L10nKey::TabTooltipShowSidebar
                    }),
                    &ToggleLeftPanel,
                    None,
                )
                .on_click(on_command(ChromeCommand::ToggleLeftPanel)),
            )
            .child(
                tile(
                    Button::new("titlebar-search")
                        .icon(Icon::empty().path("icons/command.svg").size_4()),
                )
                .accessibility_label(t(L10nKey::AppMenuSearchEverywhere))
                .tooltip_with_action(t(L10nKey::AppMenuSearchEverywhere), &TogglePalette, None)
                .on_click(on_command(ChromeCommand::ToggleSearch)),
            )
            .child(new_tab);
        let right = group()
            .child(
                tile(
                    Button::new("titlebar-split-right")
                        .icon(Icon::empty().path("icons/split-right.svg").size_4()),
                )
                .disabled(!can_split)
                .accessibility_label(t(L10nKey::AppMenuSplitRight))
                .tooltip_with_action(t(L10nKey::AppMenuSplitRight), &SplitRight, None)
                .on_click(on_command(ChromeCommand::Split(Axis::Horizontal))),
            )
            .child(
                tile(
                    Button::new("titlebar-split-down")
                        .icon(Icon::empty().path("icons/split-down.svg").size_4()),
                )
                .disabled(!can_split)
                .accessibility_label(t(L10nKey::AppMenuSplitDown))
                .tooltip_with_action(t(L10nKey::AppMenuSplitDown), &SplitDown, None)
                .on_click(on_command(ChromeCommand::Split(Axis::Vertical))),
            )
            .child(
                tile(
                    Button::new("titlebar-right-panel").icon(
                        Icon::empty()
                            .path(if right_open {
                                "icons/panel-right.svg"
                            } else {
                                "icons/panel-right-collapsed.svg"
                            })
                            .size_4(),
                    ),
                )
                .accessibility_label(t(if right_open {
                    L10nKey::TabTooltipHideDetailPanel
                } else {
                    L10nKey::TabTooltipShowDetailPanel
                }))
                .tooltip_with_action(
                    t(if right_open {
                        L10nKey::TabTooltipHideDetailPanel
                    } else {
                        L10nKey::TabTooltipShowDetailPanel
                    }),
                    &ToggleRightPanel,
                    None,
                )
                .on_click(on_command(ChromeCommand::ToggleRightPanel)),
            );
        Self {
            left: left.into_any_element(),
            right: right.into_any_element(),
        }
    }

    pub(crate) fn render(self, window: &mut Window, cx: &mut App) -> AnyElement {
        let macos_fullscreen = cfg!(target_os = "macos") && window.is_fullscreen();
        let left_row = h_flex()
            .id("workbench-left-drag")
            .w_full()
            .min_w_0()
            .h(rems(3.))
            .flex_none()
            .border_b_1()
            .border_color(cx.theme().transparent)
            // 仅窗口模式保留原生交通灯几何；全屏恢复产品控件的边距。
            .pl(px(TITLE_BAR_LEAD))
            .when(macos_fullscreen, |row| row.pl_3())
            .pr_1()
            .gap_2()
            .child(window_mark())
            .child(self.left);
        let left = title_bar_drag(left_row, "workbench-left-drag", window, cx)
            .into_any_element();
        let trailing_inset = if macos_fullscreen { 0.75 } else { 0.25 };
        let strip = h_flex()
            .w_full()
            .min_w_0()
            .h_full()
            .justify_end()
            .pr(rems(trailing_inset))
            .child(self.right);
        let controls_drawn = window_controls_drawn(window.is_fullscreen());
        let native_width = if controls_drawn {
            let controls = window.window_controls();
            let count = 1 + u8::from(controls.minimize) + u8::from(controls.maximize);
            gpui_kit::component::TITLE_BAR_HEIGHT.as_f32() * f32::from(count)
        } else {
            0.
        };
        // 三按钮组与外边距共用宽度预算；原生控件不随界面缩放。
        let right_width = window.rem_size().as_f32() * (6.5 + trailing_inset) + native_width;
        let right = if !macos_fullscreen && (controls_drawn || cfg!(target_os = "macos")) {
            TitleBar::new()
                .h(rems(3.))
                .pl_0()
                .bg(cx.theme().transparent)
                .border_color(cx.theme().transparent)
                .child(strip)
                .into_any_element()
        } else {
            // 全屏无原生控件；避免 TitleBar 内部的全屏占位挤出按钮组。
            let row = h_flex()
                .id("workbench-right-drag")
                .h(rems(3.))
                .flex_none()
                .w_full()
                .child(strip);
            title_bar_drag(row, "workbench-right-drag", window, cx)
                .into_any_element()
        };
        h_flex()
            .flex_none()
            .h(rems(3.))
            .w_full()
            .child(gpui_kit::div().flex_1().min_w_0().child(left))
            .child(gpui_kit::div().flex_none().w(px(right_width)).child(right))
            .into_any_element()
    }
}

#[cfg(test)]
mod gpui_tests {
    use std::io::ErrorKind;

    use gpui_kit::test::{TestEventExt as _, TestWindowExt as _};
    use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};

    use crate::core::actions::{ActivateTab1, ActivateTab2};
    use crate::core::config::RightPanelTab;
    use crate::daemon::protocol::ClientMsg;
    use crate::daemon::transport::Stream;
    use crate::ui::app::{LyrelApp, test_window::harness_with_tabs};
    use super::{
        TITLE_BAR_BORDER, title_strip_centre, title_strip_content_centre, title_strip_rule_and_pad,
        traffic_light_position, TRAFFIC_LIGHT_BUTTON_H,
    };
    #[cfg(target_os = "macos")]
    use super::{TITLE_BAR_LEAD, TRAFFIC_LIGHT_INSET, TRAFFIC_LIGHTS_W};
    /// A title-bar row spends [`TITLE_BAR_BORDER`] of its height below its
    /// contents, and there are two ways it does that: it carries the whole point
    /// as a rule painted transparent (the strip's `TitleBar`, the rail's header,
    /// the code panel's, the settings band), or it draws its own thinner hairline
    /// and gives the rest back as padding (the diff overlay's 0.5pt rule). Both
    /// have to leave the contents on the line the traffic lights hang on — the
    /// rows sit side by side in the same band, and half a point of difference is
    /// the one device pixel that reads as a step.
    #[test]
    fn a_rows_own_hairline_and_the_bars_invisible_rule_reach_the_same_centre() {
        for rule in [px(0.), px(0.5), px(TITLE_BAR_BORDER), px(2.)] {
            let (border, pad) = title_strip_rule_and_pad(rule);
            assert!(
                border.as_f32() <= TITLE_BAR_BORDER,
                "a thicker rule than the bar's own point would push the contents \
                 onto a line nothing else is on"
            );
            assert!(
                pad.as_f32() >= 0.,
                "negative padding is not padding, and taffy would swallow it"
            );
            assert_eq!(
                title_strip_content_centre(rule),
                title_strip_centre(),
                "a rule of {} leaves the box short by {} of border and {} of padding",
                rule.as_f32(),
                border.as_f32(),
                pad.as_f32()
            );
        }
    }
    /// The lights share a line with the chrome tiles they sit beside. gpui
    /// hangs the top of the button frame `y` below the window's top edge, so
    /// their centre falls half a frame under that, and `title_strip_centre` is
    /// where every title-bar row puts its contents. Sixteen and a half points
    /// today, out of the 48pt bar.
    #[test]
    fn the_traffic_lights_sit_on_the_chrome_rows_centre() {
        let position = traffic_light_position();
        assert_eq!(position.x, position.y);
        assert_eq!(
            position.y + px(TRAFFIC_LIGHT_BUTTON_H / 2.),
            crate::ui::window_chrome::title_strip_centre()
        );
    }

    /// The strip, editor header and search band start at the lead, so the
    /// lights have to end, with their inset after them, before it does.
    #[test]
    #[cfg(target_os = "macos")]
    fn the_title_bar_lead_clears_the_traffic_lights() {
        let right = traffic_light_position().x + px(TRAFFIC_LIGHTS_W);
        assert!(right + px(TRAFFIC_LIGHT_INSET) <= px(crate::ui::window_chrome::TITLE_BAR_LEAD));
    }

    fn settle(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| window.render_frame(cx));
        vcx.run_until_parked();
    }

    fn wait_for_editor(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) {
        // HostOps 的真实线程池不随 GPUI executor 一起排空。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            settle(vcx);
            if app.read_with(vcx, |app, _| app.editor_active_local_file().is_some()) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the document did not finish opening"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    fn terminal_focused(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> bool {
        app.update_in(vcx, |app, window, cx| {
            app.tabs[app.active]
                .pane
                .leaves()
                .iter()
                .any(|leaf| leaf.contains_focused(window, cx))
        })
    }

    fn input(stream: &mut Stream) -> Option<Vec<u8>> {
        stream
            .set_read_timeout(Some(std::time::Duration::from_millis(100)))
            .unwrap();
        loop {
            match ClientMsg::read(stream) {
                Ok(ClientMsg::Input(bytes)) => return Some(bytes),
                Ok(_) => continue,
                Err(error)
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
                {
                    return None;
                }
                Err(error) => panic!("pane connection failed: {error}"),
            }
        }
    }
    #[cfg(target_os = "macos")]
    #[gpui_kit::test]
    fn macos_fullscreen_chrome_uses_compact_balanced_edge_insets(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        let edge_controls = ["titlebar-left-panel", "titlebar-right-panel"];
        for (width, font_size) in [(1440., 16.), (760., 16.), (1440., 20.), (760., 20.)] {
            app.update_in(&mut vcx, |_, _, cx| {
                cx.global_mut::<crate::core::config::Config>().ui_font_size = font_size;
                cx.notify();
            });
            vcx.simulate_resize(size(px(width), px(900.)));
            settle(&mut vcx);
            let windowed = vcx.update(|window, _| {
                assert!(!window.is_fullscreen());
                edge_controls.map(|id| window.find(id).bounds())
            });
            vcx.dispatch_action(crate::core::actions::ToggleFullscreen);
            settle(&mut vcx);
            let fullscreen = vcx.update(|window, _| {
                assert!(window.is_fullscreen());
                edge_controls.map(|id| window.find(id).bounds())
            });
            for (left_open, right_open) in
                [(true, true), (false, true), (false, false), (true, false)]
            {
                app.update_in(&mut vcx, |app, _, cx| {
                    app.sidebar_collapsed = !left_open;
                    app.right_panel_visible = right_open;
                    cx.notify();
                });
                settle(&mut vcx);
                vcx.update(|window, _| {
                    let [left, right] = edge_controls.map(|id| window.find(id).bounds());
                    let right_gap = window.viewport_size().width - right.right();
                    assert_eq!([left, right], fullscreen, "sidebars cannot move fullscreen controls");
                    assert!(
                        left.left() < window.rem_size() * 4.,
                        "fullscreen must not reserve the hidden traffic lights' leading space: {left:?}"
                    );
                    assert!(left.left() < windowed[0].left());
                    assert!(
                        right_gap >= window.rem_size() * 0.75 && right_gap <= window.rem_size(),
                        "fullscreen controls need a small trailing inset rather than hugging the window edge: {right_gap}"
                    );
                });
            }
            vcx.dispatch_action(crate::core::actions::ToggleFullscreen);
            settle(&mut vcx);
            vcx.update(|window, _| {
                assert!(!window.is_fullscreen());
                assert_eq!(edge_controls.map(|id| window.find(id).bounds()), windowed);
            });
        }
    }

    #[gpui_kit::test]
    fn zoom_and_fullscreen_preserve_chrome_hit_targets_and_terminal_top(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        let controls = [
            "titlebar-left-panel",
            "titlebar-search",
            "titlebar-add",
            "titlebar-split-right",
            "titlebar-split-down",
            "titlebar-right-panel",
        ];
        for (width, font_size) in [(760., 16.), (1440., 20.)] {
            app.update_in(&mut vcx, |_, _, cx| {
                cx.global_mut::<crate::core::config::Config>().ui_font_size = font_size;
                gpui_kit::component::Theme::update(cx, |theme| theme.font_size = px(font_size));
                cx.notify();
            });
            vcx.simulate_resize(size(px(width), px(900.)));
            settle(&mut vcx);
            let windowed = vcx.update(|window, _| controls.map(|id| window.find(id).bounds()));
            for fullscreen in [true, false] {
                vcx.dispatch_action(crate::core::actions::ToggleFullscreen);
                settle(&mut vcx);
                let anchored = vcx.update(|window, _| {
                    assert_eq!(window.is_fullscreen(), fullscreen);
                    controls.map(|id| window.find(id).bounds())
                });
                for button in [
                    "titlebar-left-panel",
                    "titlebar-right-panel",
                    "titlebar-left-panel",
                    "titlebar-right-panel",
                ] {
                    let before = app.read_with(&vcx, |app, cx| {
                        (app.left_panel_open(cx), app.right_panel_open(cx))
                    });
                    vcx.update(|window, cx| window.click(button, cx));
                    settle(&mut vcx);
                    app.read_with(&vcx, |app, cx| {
                        let expected = if button == "titlebar-left-panel" {
                            (!before.0, before.1)
                        } else {
                            (before.0, !before.1)
                        };
                        assert_eq!((app.left_panel_open(cx), app.right_panel_open(cx)), expected);
                        assert_eq!(app.pane_area.get().unwrap().top(), px(font_size * 3.));
                    });
                    vcx.update(|window, _| {
                        let current = controls.map(|id| window.find(id).bounds());
                        assert_eq!(current, anchored, "panels cannot move zoomed chrome");
                        for bounds in current {
                            assert_eq!(bounds.size, size(px(font_size * 2.), px(font_size * 2.)));
                            assert!(bounds.bottom() <= px(font_size * 3.));
                        }
                    });
                }
                if !fullscreen {
                    assert_eq!(anchored, windowed, "leaving fullscreen restores native reserves");
                }
            }
        }
    }

    #[gpui_kit::test]
    fn sidebar_toggles_preserve_titlebar_and_terminal_height(cx: &mut TestAppContext) {
        let (app, mut vcx, mut streams) = harness_with_tabs(cx, 1);
        vcx.simulate_resize(size(px(1440.), px(900.)));
        app.update_in(&mut vcx, |app, window, cx| {
            app.sidebar_collapsed = false;
            app.right_panel_visible = true;
            app.focus_active(window, cx);
            cx.notify();
        });
        settle(&mut vcx);
        let area = app.read_with(&vcx, |app, _| app.pane_area.get().unwrap());
        let title_height = vcx.update(|window, _| window.rem_size() * 3.);
        assert_eq!(
            area.top(),
            title_height,
            "the terminal starts below the persistent titlebar even with both sidebars open"
        );
        let rows = app.read_with(&vcx, |app, cx| {
            app.tabs[0].pane.terminals()[0]
                .read(cx)
                .terminal
                .size()
                .rows
        });
        let controls = [
            "titlebar-left-panel",
            "titlebar-search",
            "titlebar-add",
            "titlebar-split-right",
            "titlebar-split-down",
            "titlebar-right-panel",
        ];
        let control_bounds =
            vcx.update(|window, _| controls.map(|control| window.find(control).bounds()));
        for (button, left_open, right_open) in [
            ("titlebar-left-panel", false, true),
            ("titlebar-right-panel", false, false),
            ("titlebar-left-panel", true, false),
            ("titlebar-right-panel", true, true),
        ] {
            vcx.update(|window, cx| window.click(button, cx));
            settle(&mut vcx);
            app.update_in(&mut vcx, |app, _, cx| {
                assert_eq!(app.left_panel_open(cx), left_open);
                assert_eq!(app.right_panel_open(cx), right_open);
                let current = app.pane_area.get().unwrap();
                assert_eq!(
                    current.top(),
                    area.top(),
                    "sidebar toggles cannot move the terminal top"
                );
                assert_eq!(current.bottom(), area.bottom());
                assert_eq!(
                    app.tabs[0].pane.terminals()[0]
                        .read(cx)
                        .terminal
                        .size()
                        .rows,
                    rows,
                    "sidebar toggles change PTY columns, not its row budget"
                );
            });
            vcx.update(|window, cx| {
                for (control, expected) in controls.into_iter().zip(control_bounds) {
                    let bounds = window.find(control).bounds();
                    assert_eq!(bounds, expected, "{control} stays anchored to the window");
                    assert!(
                        bounds.bottom() <= area.top(),
                        "{control} cannot cover terminal rows"
                    );
                }
                let terminal = window.find("terminal-surface").bounds();
                assert_eq!(terminal.top(), area.top());
                // 首行中央保留窗格抓手；在抓手旁验证终端输入。
                window.click_at(
                    "terminal-surface",
                    gpui_kit::point(terminal.size.width / 4., px(12.)),
                    cx,
                );
            });
            settle(&mut vcx);
            assert!(terminal_focused(&app, &mut vcx));
            vcx.simulate_keystrokes("x");
            settle(&mut vcx);
            assert_eq!(input(&mut streams[0]).as_deref(), Some(&b"x"[..]));
        }
    }

    #[gpui_kit::test]
    fn titlebar_buttons_accept_clicks_with_pointer_drift(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        vcx.simulate_resize(size(px(1440.), px(900.)));
        settle(&mut vcx);

        for button in [
            "titlebar-left-panel",
            "titlebar-right-panel",
            "titlebar-search",
        ] {
            let before = app.read_with(&vcx, |app, cx| {
                (
                    app.left_panel_open(cx),
                    app.right_panel_open(cx),
                    app.search.is_open(),
                )
            });
            vcx.update(|window, cx| {
                let start = window.find(button).bounds().center();
                let end = start + gpui_kit::point(px(1.), px(1.));
                window.pointer_down(start, gpui_kit::MouseButton::Left, cx);
                window.pointer_move(end, Some(gpui_kit::MouseButton::Left), cx);
                window.pointer_up(end, gpui_kit::MouseButton::Left, cx);
            });
            settle(&mut vcx);
            let after = app.read_with(&vcx, |app, cx| {
                (
                    app.left_panel_open(cx),
                    app.right_panel_open(cx),
                    app.search.is_open(),
                )
            });
            let expected = match button {
                "titlebar-left-panel" => (!before.0, before.1, before.2),
                "titlebar-right-panel" => (before.0, !before.1, before.2),
                _ => (before.0, before.1, !before.2),
            };
            assert_eq!(
                after, expected,
                "{button} activates despite a 1px pointer drift"
            );
        }
    }

    #[gpui_kit::test]
    fn sidebar_chrome_preserves_document_and_terminal_input(cx: &mut TestAppContext) {
        let (app, mut vcx, mut streams) = harness_with_tabs(cx, 2);
        vcx.simulate_resize(size(px(1600.), px(900.)));
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("sidebar-focus.txt");
        std::fs::write(&path, "").unwrap();
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_file_in_editor(&path, window, cx)
        });
        wait_for_editor(&app, &mut vcx);
        let editor = app.read_with(&vcx, |app, _| app.editor_active_local_file().unwrap().input);
        assert!(app.update_in(&mut vcx, |app, window, cx| {
            app.document_dock_px(window, cx).is_some()
        }));

        for button in ["titlebar-left-panel", "titlebar-right-panel"] {
            for _ in 0..2 {
                let before = editor.read_with(&vcx, |editor, _| editor.value().to_string());
                vcx.update(|window, cx| window.click(button, cx));
                settle(&mut vcx);
                vcx.simulate_keystrokes("d");
                settle(&mut vcx);
                assert_eq!(
                    editor.read_with(&vcx, |editor, _| editor.value().to_string()),
                    format!("{before}d"),
                    "{button} preserves the docked document's input on hide and show"
                );
                assert_eq!(input(&mut streams[0]), None);
                assert_eq!(input(&mut streams[1]), None);
            }
        }

        app.update_in(&mut vcx, |app, window, cx| app.focus_active(window, cx));
        settle(&mut vcx);
        let before = editor.read_with(&vcx, |editor, _| editor.value().to_string());
        for button in ["titlebar-left-panel", "titlebar-right-panel"] {
            for _ in 0..2 {
                vcx.update(|window, cx| window.click(button, cx));
                settle(&mut vcx);
                assert!(terminal_focused(&app, &mut vcx));
                vcx.simulate_keystrokes("t");
                settle(&mut vcx);
                assert_eq!(input(&mut streams[0]).as_deref(), Some(&b"t"[..]));
                assert_eq!(input(&mut streams[0]), None, "one key is delivered once");
                assert_eq!(input(&mut streams[1]), None);
                assert_eq!(
                    editor.read_with(&vcx, |editor, _| editor.value().to_string()),
                    before
                );
            }
        }
    }

    #[gpui_kit::test]
    fn changes_tab_count_tracks_real_scm_status_on_one_surface(cx: &mut TestAppContext) {
        use crate::daemon::protocol::DaemonMsg;
        let dir = tempfile::TempDir::new().unwrap();
        let root = lyrel_core::core::path_spelling::local_spelling_buf(
            &std::fs::canonicalize(dir.path()).unwrap(),
        );
        let init = std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(init.status.success());
        let (app, mut vcx, mut streams) = harness_with_tabs(cx, 1);
        vcx.simulate_resize(size(px(1440.), px(900.)));
        DaemonMsg::Cwd(root.clone())
            .encode(&mut streams[0])
            .unwrap();
        app.update_in(&mut vcx, |app, _, cx| {
            app.right_panel_visible = true;
            app.right_panel_tab = RightPanelTab::Scm;
            cx.notify();
        });
        for expected in [0, 1, 3, 0] {
            for name in ["a.txt", "b.txt", "c.txt"] {
                let path = root.join(name);
                if path.exists() {
                    std::fs::remove_file(path).unwrap();
                }
            }
            for name in ["a.txt", "b.txt", "c.txt"].into_iter().take(expected) {
                std::fs::write(root.join(name), "changed\n").unwrap();
            }
            for at_prompt in [false, true] {
                DaemonMsg::Prompt {
                    active: true,
                    at_prompt,
                    last_exit: Some(0),
                }
                .encode(&mut streams[0])
                .unwrap();
            }
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            loop {
                vcx.executor()
                    .advance_clock(std::time::Duration::from_millis(300));
                settle(&mut vcx);
                let ready = app.read_with(&vcx, |app, cx| {
                    app.scm.repo.as_ref().is_some_and(|repo| {
                        crate::terminal::git_data::status_of(cx, repo.host, &repo.root)
                            .is_some_and(|status| status.total_entries == expected)
                    })
                });
                if ready {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "SCM count did not reach {expected}"
                );
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            vcx.update(|window, cx| {
                window.render_frame(cx);
                let counts = window.find_all("right-panel-change-count");
                if expected == 0 {
                    assert!(counts.is_empty(), "a clean working tree clears the count");
                } else {
                    assert_eq!(counts.len(), 1, "Changes owns the only count surface");
                    let count = window
                        .within("panel-title")
                        .find("right-panel-change-count");
                    assert!(count.visible());
                    assert_eq!(count.label(), Some(expected.to_string().as_str()));
                }
            });
        }
    }

    #[gpui_kit::test]
    fn closing_panel_chrome_restores_terminal_input_with_either_sidebar_state(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, mut streams) = harness_with_tabs(cx, 2);
        vcx.simulate_resize(size(px(1440.), px(900.)));
        let original = app.read_with(&vcx, |app, cx| {
            (
                app.workspace,
                app.tabs
                    .iter()
                    .map(|tab| tab.tree_id.get())
                    .collect::<Vec<_>>(),
                app.tabs
                    .iter()
                    .map(|tab| {
                        let terminal = tab.pane.terminals().pop().unwrap();
                        (terminal.entity_id(), terminal.read(cx).pane_id)
                    })
                    .collect::<Vec<_>>(),
            )
        });

        for collapsed in [false, true] {
            app.update_in(&mut vcx, |app, window, cx| {
                app.sidebar_collapsed = collapsed;
                app.right_panel_visible = false;
                app.right_panel_tab = RightPanelTab::Files;
                app.focus_active(window, cx);
                cx.notify();
            });
            settle(&mut vcx);
            vcx.update(|window, cx| {
                assert_eq!(window.find_all("titlebar-right-panel").len(), 1);
                window.click("titlebar-right-panel", cx);
            });
            settle(&mut vcx);
            app.update_in(&mut vcx, |app, window, cx| {
                assert!(app.right_panel_open(cx));
                app.file_search
                    .update(cx, |search, cx| search.focus(window, cx));
            });
            settle(&mut vcx);
            vcx.update(|window, cx| {
                assert_eq!(window.find_all("titlebar-right-panel").len(), 1);
                window.click("titlebar-right-panel", cx);
            });
            settle(&mut vcx);
            assert!(!app.read_with(&vcx, |app, cx| app.right_panel_open(cx)));
            assert!(terminal_focused(&app, &mut vcx));

            vcx.simulate_keystrokes("x");
            settle(&mut vcx);
            assert_eq!(input(&mut streams[0]).as_deref(), Some(&b"x"[..]));
            assert_eq!(input(&mut streams[0]), None, "one key is delivered once");
            assert_eq!(
                input(&mut streams[1]),
                None,
                "the hidden tab receives no key"
            );
            vcx.dispatch_action(ActivateTab2);
            settle(&mut vcx);
            assert_eq!(app.read_with(&vcx, |app, _| app.active), 1);
            assert!(terminal_focused(&app, &mut vcx));
            vcx.dispatch_action(ActivateTab1);
            settle(&mut vcx);
        }
        app.read_with(&vcx, |app, cx| {
            assert_eq!(app.workspace, original.0);
            assert_eq!(
                app.tabs
                    .iter()
                    .map(|tab| tab.tree_id.get())
                    .collect::<Vec<_>>(),
                original.1
            );
            assert_eq!(
                app.tabs
                    .iter()
                    .map(|tab| {
                        let terminal = tab.pane.terminals().pop().unwrap();
                        (terminal.entity_id(), terminal.read(cx).pane_id)
                    })
                    .collect::<Vec<_>>(),
                original.2
            );
        });
    }

    #[gpui_kit::test]
    fn navigation_clicks_and_modal_keys_reach_only_the_selected_session(cx: &mut TestAppContext) {
        let (app, mut vcx, mut streams) = harness_with_tabs(cx, 2);
        vcx.simulate_resize(size(px(1440.), px(900.)));
        let first_tab_key =
            vcx.update(|_, cx| crate::ui::keymap::effective_key("ActivateTab1", cx).unwrap());
        for collapsed in [false, true] {
            app.update_in(&mut vcx, |app, window, cx| {
                app.sidebar_collapsed = collapsed;
                app.right_panel_visible = false;
                app.focus_active(window, cx);
                cx.notify();
            });
            settle(&mut vcx);
            if collapsed {
                vcx.update(|window, cx| window.click("titlebar-left-panel", cx));
                settle(&mut vcx);
            }
            let second_tab = app.read_with(&vcx, |app, _| app.sidebar_slots.borrow()[1]);
            assert!(second_tab.size.width > px(0.));
            vcx.simulate_click(second_tab.center(), gpui_kit::Modifiers::none());
            settle(&mut vcx);
            assert_eq!(app.read_with(&vcx, |app, _| app.active), 1);
            assert!(terminal_focused(&app, &mut vcx));

            vcx.update(|window, cx| window.click("titlebar-search", cx));
            settle(&mut vcx);
            assert!(app.read_with(&vcx, |app, _| app.search.is_open()));
            vcx.simulate_keystrokes(&first_tab_key);
            vcx.simulate_keystrokes("q");
            settle(&mut vcx);
            assert_eq!(app.read_with(&vcx, |app, _| app.active), 1);
            assert_eq!(input(&mut streams[0]), None);
            assert_eq!(input(&mut streams[1]), None);
            vcx.simulate_keystrokes("escape");
            settle(&mut vcx);
            assert!(app.read_with(&vcx, |app, _| !app.search.is_open()));
            assert!(terminal_focused(&app, &mut vcx));
            vcx.simulate_keystrokes("y");
            settle(&mut vcx);
            assert_eq!(input(&mut streams[1]).as_deref(), Some(&b"y"[..]));
            assert_eq!(input(&mut streams[0]), None);
            vcx.update(|window, _| {
                assert_eq!(window.viewport_size(), size(px(1440.), px(900.)));
            });
            vcx.dispatch_action(ActivateTab1);
            settle(&mut vcx);
        }
    }
    #[gpui_kit::test]
    fn file_fill_keeps_search_available_and_returns_input_to_the_document(cx: &mut TestAppContext) {
        use crate::core::actions::ToggleDocumentFill;
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("navigation.txt");
        std::fs::write(&path, "").unwrap();
        for collapsed in [false, true] {
            let (app, mut vcx, mut streams) = harness_with_tabs(cx, 1);
            vcx.simulate_resize(size(px(1000.), px(800.)));
            app.update_in(&mut vcx, |app, window, cx| {
                app.sidebar_collapsed = collapsed;
                app.right_panel_visible = false;
                app.focus_active(window, cx);
                cx.notify();
            });
            app.update_in(&mut vcx, |app, window, cx| {
                app.open_file_in_editor(&path, window, cx)
            });
            wait_for_editor(&app, &mut vcx);
            vcx.dispatch_action(ToggleDocumentFill);
            settle(&mut vcx);
            let editor =
                app.read_with(&vcx, |app, _| app.editor_active_local_file().unwrap().input);
            vcx.simulate_keystrokes("a");
            settle(&mut vcx);
            let before = editor.read_with(&vcx, |editor, _| editor.value().to_string());
            vcx.update(|window, cx| window.click("titlebar-search", cx));
            settle(&mut vcx);
            assert!(app.read_with(&vcx, |app, _| app.search.is_open()));
            vcx.simulate_keystrokes("q");
            vcx.simulate_keystrokes("escape");
            settle(&mut vcx);
            assert!(app.read_with(&vcx, |app, _| !app.search.is_open()));
            assert_eq!(
                editor.read_with(&vcx, |editor, _| editor.value().to_string()),
                before
            );
            assert!(app.update_in(&mut vcx, |app, window, cx| {
                app.document_dock_px(window, cx).is_none()
            }));
            vcx.simulate_keystrokes("b");
            settle(&mut vcx);
            assert_eq!(
                editor.read_with(&vcx, |editor, _| editor.value().to_string()),
                format!("{before}b")
            );
            assert_eq!(
                input(&mut streams[0]),
                None,
                "search and editor input never leak into the terminal"
            );
        }
    }

    #[gpui_kit::test]
    fn persistent_controls_keep_navigation_and_real_sidebar_resizing_available(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, mut streams) = harness_with_tabs(cx, 1);
        vcx.simulate_resize(size(px(1440.), px(900.)));
        app.update_in(&mut vcx, |app, window, cx| {
            app.sidebar_collapsed = false;
            app.right_panel_visible = true;
            app.focus_active(window, cx);
            cx.notify();
        });
        settle(&mut vcx);
        let initial = app.read_with(&vcx, |app, _| {
            (app.sidebar_width.get(), app.right_panel_width.get())
        });
        vcx.update(|window, cx| {
            let left = window.find("sidebar-resize").bounds().center();
            window.drag(left, left + gpui_kit::point(px(70.), px(0.)), cx);
        });
        settle(&mut vcx);
        vcx.update(|window, cx| {
            let right = window.find("right-panel-resize").bounds().center();
            window.drag(right, right - gpui_kit::point(px(60.), px(0.)), cx);
        });
        settle(&mut vcx);
        app.read_with(&vcx, |app, cx| {
            assert!(app.sidebar_width.get() > initial.0 + 40.);
            assert!(app.right_panel_width.get() > initial.1 + 40.);
            assert_eq!(
                cx.global::<crate::core::config::Config>().sidebar_width,
                app.sidebar_width.get()
            );
            assert_eq!(
                cx.global::<crate::core::config::Config>().right_panel_width,
                app.right_panel_width.get()
            );
        });
        vcx.update(|window, cx| window.click("rail-workspace-head", cx));
        settle(&mut vcx);
        assert!(app.read_with(&vcx, |app, _| app.switcher.is_some()));
        vcx.simulate_keystrokes("escape");
        settle(&mut vcx);
        for width in [760., 1440.] {
            vcx.simulate_resize(size(px(width), px(900.)));
            vcx.update(|window, cx| {
                window.click("titlebar-left-panel", cx);
                window.click("titlebar-right-panel", cx);
            });
            settle(&mut vcx);
            vcx.update(|window, cx| window.click("titlebar-add", cx));
            settle(&mut vcx);
            vcx.simulate_keystrokes("q");
            settle(&mut vcx);
            assert_eq!(
                input(&mut streams[0]),
                None,
                "new-tab choices own keyboard input until dismissed"
            );
            vcx.simulate_keystrokes("escape");
            settle(&mut vcx);
            assert!(terminal_focused(&app, &mut vcx));
            vcx.simulate_keystrokes("n");
            settle(&mut vcx);
            assert_eq!(input(&mut streams[0]).as_deref(), Some(&b"n"[..]));
        }
    }
}
#[cfg(test)]
mod window_drag_tests {
    use gpui_kit::{
        Context, InteractiveElement as _, IntoElement, Modifiers, MouseButton, MouseDownEvent,
        MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, PlatformInput, Point, Render,
        Styled as _, TestAppContext, VisualTestContext, Window, div, point, px,
    };
    use std::cell::Cell;
    use std::rc::Rc;

    fn row(
        key: &'static str,
        moves: &Rc<Cell<usize>>,
        window: &mut Window,
        cx: &mut gpui_kit::App,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let moves = moves.clone();
        super::window_move_gesture_with(
            div().id(key).w_full().h(px(40.)),
            key,
            window,
            cx,
            move |_| moves.set(moves.get() + 1),
        )
    }

    struct Host {
        moves: Rc<Cell<usize>>,
    }
    impl Render for Host {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(row("stand-in-title-bar", &self.moves, window, cx))
        }
    }

    struct HandleOverRow {
        occluded: bool,
        moves: Rc<Cell<usize>>,
    }
    impl Render for HandleOverRow {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let handle = div()
                .absolute()
                .top_0()
                .left(px(296.))
                .w(px(8.))
                .h_full()
                .cursor_col_resize()
                .on_mouse_down(MouseButton::Left, |_, window, _| window.refresh());
            let handle = if self.occluded {
                handle.occlude()
            } else {
                handle
            };
            div()
                .relative()
                .size_full()
                .child(row("stand-in-title-bar", &self.moves, window, cx))
                .child(handle)
        }
    }

    fn down(at: Point<Pixels>) -> PlatformInput {
        PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::none(),
            click_count: 1,
            first_mouse: false,
        })
    }

    fn up(at: Point<Pixels>) -> PlatformInput {
        PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::none(),
            click_count: 1,
        })
    }

    fn moved(at: Point<Pixels>, held: bool) -> PlatformInput {
        PlatformInput::MouseMove(MouseMoveEvent {
            position: at,
            pressed_button: held.then_some(MouseButton::Left),
            modifiers: Modifiers::none(),
        })
    }

    const ON_ROW: Point<Pixels> = Point {
        x: px(300.),
        y: px(20.),
    };

    fn drifted(at: Point<Pixels>) -> Point<Pixels> {
        point(at.x + px(12.), at.y + px(3.))
    }

    fn press_repaint_move(vcx: &mut VisualTestContext, at: Point<Pixels>) {
        vcx.update(|window, cx| {
            window.dispatch_event(moved(at, false), cx);
            window.dispatch_event(down(at), cx);
        });
        vcx.update(|window, _| window.refresh());
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            window.dispatch_event(moved(drifted(at), true), cx);
        });
        vcx.run_until_parked();
    }

    #[gpui_kit::test]
    fn a_drag_survives_repaint_and_requests_one_native_move(cx: &mut TestAppContext) {
        let moves = Rc::new(Cell::new(0));
        let window = cx.add_window(|_, _| Host {
            moves: moves.clone(),
        });
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        press_repaint_move(&mut vcx, ON_ROW);
        assert_eq!(moves.get(), 1);
        vcx.update(|window, cx| {
            window.dispatch_event(moved(drifted(drifted(ON_ROW)), true), cx);
        });
        vcx.run_until_parked();
        assert_eq!(
            moves.get(),
            1,
            "one press must request only one native move"
        );
    }

    #[gpui_kit::test]
    fn a_press_alone_does_not_move_the_window(cx: &mut TestAppContext) {
        let moves = Rc::new(Cell::new(0));
        let window = cx.add_window(|_, _| Host {
            moves: moves.clone(),
        });
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        vcx.update(|window, cx| {
            window.dispatch_event(moved(ON_ROW, false), cx);
            window.dispatch_event(down(ON_ROW), cx);
            window.dispatch_event(up(ON_ROW), cx);
        });
        vcx.run_until_parked();
        assert_eq!(moves.get(), 0);
    }

    #[gpui_kit::test]
    fn a_release_disarms_so_a_later_hover_does_not_drag(cx: &mut TestAppContext) {
        let moves = Rc::new(Cell::new(0));
        let window = cx.add_window(|_, _| Host {
            moves: moves.clone(),
        });
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        vcx.update(|window, cx| {
            window.dispatch_event(moved(ON_ROW, false), cx);
            window.dispatch_event(down(ON_ROW), cx);
            window.dispatch_event(up(ON_ROW), cx);
        });
        vcx.update(|window, _| window.refresh());
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            window.dispatch_event(moved(drifted(ON_ROW), false), cx);
        });
        vcx.run_until_parked();
        assert_eq!(moves.get(), 0);
    }

    #[gpui_kit::test]
    fn a_press_on_a_resize_handle_does_not_move_the_window(cx: &mut TestAppContext) {
        let moves = Rc::new(Cell::new(0));
        let window = cx.add_window(|_, _| HandleOverRow {
            occluded: true,
            moves: moves.clone(),
        });
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        press_repaint_move(&mut vcx, ON_ROW);
        assert_eq!(moves.get(), 0);
    }

    #[gpui_kit::test]
    fn a_handle_without_a_blocking_hitbox_hands_the_press_to_the_row(cx: &mut TestAppContext) {
        let moves = Rc::new(Cell::new(0));
        let window = cx.add_window(|_, _| HandleOverRow {
            occluded: false,
            moves: moves.clone(),
        });
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        press_repaint_move(&mut vcx, ON_ROW);
        assert_eq!(moves.get(), 1);
    }

    #[gpui_kit::test]
    fn two_rows_on_screen_keep_separate_arms(cx: &mut TestAppContext) {
        struct TwoRows {
            first: Rc<Cell<usize>>,
            second: Rc<Cell<usize>>,
        }
        impl Render for TwoRows {
            fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                div()
                    .size_full()
                    .child(row("row-a", &self.first, window, cx))
                    .child(row("row-b", &self.second, window, cx))
            }
        }

        let first = Rc::new(Cell::new(0));
        let second = Rc::new(Cell::new(0));
        let window = cx.add_window(|_, _| TwoRows {
            first: first.clone(),
            second: second.clone(),
        });
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        vcx.update(|window, cx| {
            window.dispatch_event(moved(point(px(300.), px(20.)), false), cx);
            window.dispatch_event(down(point(px(300.), px(20.))), cx);
        });
        vcx.update(|window, _| window.refresh());
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            window.dispatch_event(moved(point(px(312.), px(60.)), false), cx);
        });
        vcx.run_until_parked();
        assert_eq!((first.get(), second.get()), (0, 0));
        press_repaint_move(&mut vcx, point(px(300.), px(60.)));
        assert_eq!((first.get(), second.get()), (0, 1));
    }
}
