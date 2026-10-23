//! 窗口级标题栏与持久控件；不持有标签页、终端连接或焦点状态。

use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, TitleBar, h_flex};
use gpui_kit::{AnyElement, Context, MouseButton, Window, div, prelude::*, px};

use crate::core::config::Config;
use crate::ui::app::{
    LyrelApp, TITLE_BAR_BORDER, TITLE_BAR_HEIGHT, TITLE_BAR_LEAD, TitleStripRow as _,
    tile_trailing_inset, window_controls_drawn,
};
use crate::ui::i18n::{L10nKey, t};
use crate::ui::search::SearchTab;
use crate::ui::tab_sidebar::RAIL_INSET;
use crate::ui::tab_strip::{CHIP_GAP, chrome_tile};

const GRAB_HANDLE_W: f32 = 80.;
const SIDEBAR_GRAB_HANDLE_W: f32 = 48.;

/// The panel toggle and its trailing inset. The open panel uses this same
/// reservation so the title bar never budgets for a second, absent control.
pub(crate) fn trailing_chrome_tiles_w() -> f32 {
    let trailing_pad = if cfg!(target_os = "macos") {
        tile_trailing_inset()
    } else {
        4.
    };
    trailing_pad + crate::ui::app::TILE_SIZE
}

/// The whole trailing cluster: those tiles and the OS window buttons beyond
/// them.
///
/// Anything else drawn into that end of the title bar has to stop short of it —
/// which for the hoisted document header means the case where the detail panel
/// is closed and the document column runs to the window's right edge.
pub(crate) fn trailing_chrome_w(fullscreen: bool) -> f32 {
    trailing_chrome_tiles_w() + crate::ui::app::window_controls_w(fullscreen)
}
/// The Search Everywhere button in the middle of the title bar: the width of
/// a field that reads as one, and the 28px of the rail's own search.
const TITLEBAR_SEARCH_W: f32 = 360.;
const TITLEBAR_SEARCH_H: f32 = 28.;

/// Air between the search box and the nearest chrome tile when the bar is too
/// narrow for the box to sit at its full width.
const TITLEBAR_SEARCH_CLEAR_GAP: f32 = 8.;

/// Where the title-bar search box is centred, as offsets from the tab strip it
/// is drawn inside.
///
/// The strip does not start at the terminal column's left edge. The title bar
/// leaves a lead before it — `TRAFFIC_LIGHTS_LEAD` on macOS, for the traffic lights, whether or
/// not the rail is standing in front of them, plus the bar's own inset in
/// fullscreen; 12 elsewhere — and a box centred on the strip was centred that
/// much too far right by half: 40pt on macOS then, which is plainly visible with
/// the rail collapsed and the box standing alone in an empty bar (#1033).
/// So the band reaches back over the lead, and its left edge is the column's.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SearchBand {
    /// `left` for the band, against the strip's left edge: the lead, negated.
    left: f32,
    /// `right` for the band, against the strip's right edge.
    right: f32,
    /// How much of *each* end of the band the box keeps clear of, so that in
    /// a narrow bar it shrinks rather than slides under the chrome tiles. The
    /// same at both ends, because a box clear of only one is no longer
    /// centred.
    clear: f32,
}

/// [`SearchBand`] for a strip that starts `lead` in from the terminal column's
/// left edge.
///
/// `docked_right` is what is docked to the right of the terminal — the detail
/// panel and a document column. Off macOS the bar spans those too, so the band
/// stops at the terminal column: the strip already stops short of the window
/// controls, so they come off the columns' share. On macOS the bar is inside
/// the terminal column already and the strip runs to its end.
///
/// `leading_w` is the collapsed rail's group at the strip's left end, measured
/// from the strip's left edge; `trailing_w` the chrome tiles at the band's
/// right end, when they are there rather than off over a docked column.
fn search_band(
    macos: bool,
    lead: f32,
    docked_right: f32,
    controls_w: f32,
    leading_w: Option<f32>,
    trailing_w: Option<f32>,
) -> SearchBand {
    let right = match !macos && docked_right > 0. {
        true => (docked_right - controls_w).max(0.),
        false => 0.,
    };
    let leading = leading_w.map_or(0., |w| lead + w + TITLEBAR_SEARCH_CLEAR_GAP);
    let trailing = trailing_w.map_or(0., |w| w + TITLEBAR_SEARCH_CLEAR_GAP);
    SearchBand {
        left: -lead,
        right,
        clear: leading.max(trailing),
    }
}

/// How far the title bar's lead reaches before the tab strip starts: the room
/// we have `TitleBar` leave for the traffic lights on macOS, and the bar's own inset
/// that gpui-component adds to it in fullscreen, where the lights are gone but
/// the room is not; `TITLE_BAR_LEAD` elsewhere, in both of the rows the strip
/// can sit in.
fn title_bar_strip_lead(fullscreen: bool, rem: f32) -> f32 {
    match cfg!(target_os = "macos") && fullscreen {
        // `pl_3` on the bar inside `TitleBar`.
        true => crate::ui::app::TITLE_BAR_LEAD + 0.75 * rem,
        false => crate::ui::app::TITLE_BAR_LEAD,
    }
}

/// How wide the collapsed rail's group at the start of the strip is: the
/// window mark where there is one, then New Tab and Show Sidebar. Mirrors the
/// `left_group` built in `render_window_strip`, `ml` included.
fn collapsed_rail_group_w() -> f32 {
    use crate::ui::app::{CONTENT_INSET, TILE_SIZE, WINDOW_MARK_SIZE, tile_trailing_inset};
    let mark = match cfg!(target_os = "macos") {
        true => 0.,
        false => (CONTENT_INSET - tile_trailing_inset()) + WINDOW_MARK_SIZE + 4. + 2.,
    };
    crate::ui::app::title_bar_hug_offset() + mark + TILE_SIZE + 2. + TILE_SIZE
}

impl LyrelApp {
    pub(crate) fn render_window_title_bar(
        &self,
        show_chips: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let strip = self.render_window_strip(show_chips, window, cx);
        // No window buttons in fullscreen, where they cannot work: the window
        // has no caption for the platform to hit-test, so they would draw,
        // light up under the pointer and do nothing when clicked. `TitleBar`
        // always draws them, so the strip goes into a plain row of the same
        // geometry instead — the row itself stays, since it holds the tabs,
        // the chrome tiles and the docked document's header.
        let title_bar =
            if window_controls_drawn(window.is_fullscreen()) || cfg!(target_os = "macos") {
                // `TitleBar` pads its own fixed 80pt; the lead is ours, since
                // it follows where the traffic lights are put.
                TitleBar::new()
                    .h(px(TITLE_BAR_HEIGHT))
                    .pl(px(TITLE_BAR_LEAD))
                    .bg(cx.theme().transparent)
                    .border_color(cx.theme().transparent)
                    .child(strip)
                    .into_any_element()
            } else {
                div()
                    .flex_shrink_0()
                    .flex()
                    .flex_row()
                    .pl(px(TITLE_BAR_LEAD))
                    .title_strip_row(px(TITLE_BAR_BORDER), cx.theme().transparent)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .h_full()
                            .flex_1()
                            .child(strip),
                    )
                    .into_any_element()
            };
        title_bar
    }

    pub(crate) fn render_sidebar_chrome(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // On screen at rest unless the Appearance switch says otherwise; then
        // these two tiles wait for the pointer to reach the rail. The mark
        // stays either way — it names the window rather than doing anything.
        let auto_hide_chrome = cx.global::<Config>().auto_hide_titlebar_buttons;
        let chrome_shown = crate::ui::app::titlebar_chrome_shown(
            auto_hide_chrome,
            self.sidebar_chrome_hover.get(),
        );
        let controls = h_flex()
            .flex_shrink_0()
            .title_strip_row(px(TITLE_BAR_BORDER), cx.theme().transparent)
            .justify_end()
            .gap(px(2.))
            .pr(px(RAIL_INSET))
            .when_some(crate::ui::app::window_mark(), |row, mark| {
                row.child(
                    div()
                        .flex_shrink_0()
                        .pl(px(crate::ui::app::CONTENT_INSET))
                        .child(mark),
                )
                .child(div().flex_1().min_w(px(SIDEBAR_GRAB_HANDLE_W)))
            })
            .child(
                crate::ui::app::resting_chrome(div().occlude().flex_shrink_0(), chrome_shown)
                    // These are the same controls as the collapsed title bar's
                    // pair. Keep their hit targets and glyphs unchanged when
                    // the sidebar opens, rather than switching to row-sized tiles.
                    .child(self.new_tab_button("sidebar-add", cx)),
            )
            .child(
                crate::ui::app::resting_chrome(div().occlude().flex_shrink_0(), chrome_shown)
                    .child(
                        crate::ui::tab_strip::chrome_tile(
                            Button::new("sidebar-collapse")
                                .icon(Icon::empty().path("icons/panel-left.svg")),
                            false,
                            cx,
                        )
                        .rounded_lg()
                        .accessibility_label(t(L10nKey::TabTooltipHideSidebar))
                        .tooltip_with_action(
                            t(L10nKey::TabTooltipHideSidebar),
                            &crate::core::actions::ToggleLeftPanel,
                            None,
                        )
                        .on_click(cx.listener(|this, _, _window, cx| this.toggle_left_panel(cx))),
                    ),
            );
        crate::ui::app::title_bar_drag(
            controls.id("sidebar-titlebar-drag"),
            "sidebar-titlebar-drag",
            window,
            cx,
        ).into_any_element()
    }

    fn render_window_strip(
        &self,
        show_chips: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        // On macOS an open detail panel draws its own chrome in the title bar,
        // so the strip stops at the panel's edge rather than running the width
        // of the window. Sizing it to the whole viewport made it overrun that
        // edge, and what got pushed out past it was the New Tab button.
        let panel_w = match cfg!(target_os = "macos") && self.right_panel_open(cx) {
            true => self.right_panel_px(window, cx),
            false => 0.,
        };
        // A docked document column has the same claim on this strip the detail
        // panel does, and it is answered in the same two ways. On macOS the
        // strip lives *inside* the terminal column, so the column's width comes
        // off `strip_w` the way the panel's does — sizing the strip to more
        // than it gets is what pushed the New Tab button out before.
        // Everywhere else the strip spans the workspace and the column's header
        // is drawn over its trailing end, so the width is reserved as a corner
        // instead: that header carries no fill of its own, and a chip left
        // under it showed through the file name while staying clickable.
        let document_w = self.document_dock_px(window, cx).unwrap_or(0.);
        let controls_w = crate::ui::app::window_controls_w(window.is_fullscreen());
        let strip_w = if cfg!(target_os = "macos") {
            (window.viewport_size().width
                - px(crate::ui::app::TITLE_BAR_LEAD + panel_w + document_w))
            .max(px(160.))
        } else {
            (window.viewport_size().width - px(crate::ui::app::TITLE_BAR_LEAD + controls_w))
                .max(px(140.))
        };
        let chrome_band_w = (!cfg!(target_os = "macos") && self.right_panel_open(cx))
            .then(|| (self.right_panel_px(window, cx) - controls_w - 1.).max(0.));
        // `corner_w` reserves the trailing window chrome. With the panel open on
        // macOS that chrome belongs to the panel's own header, which the strip
        // now stops short of, so reserving for it here would charge the chips
        // for it twice.
        // On macOS the trailing tiles sit at the end of the terminal column's
        // strip only while nothing stands to the right of it. A docked document
        // takes the window's right edge the way the panel does, so the tiles
        // give way to it instead of hanging in the middle of the window beside
        // the document's own header. ⌘J and the palette still reach both.
        let strip_chrome = !cfg!(target_os = "macos") || (panel_w <= 0. && document_w <= 0.);
        let corner_w = if !strip_chrome || panel_w > 0. {
            0.
        } else {
            chrome_band_w.unwrap_or_else(trailing_chrome_tiles_w)
        } + if cfg!(target_os = "macos") {
            // Already taken out of `strip_w` above; charging it here too would
            // narrow the chips by a column's width twice over.
            0.
        } else {
            document_w
        };
        let fixed_w = 3. * CHIP_GAP + crate::ui::app::TILE_SIZE + corner_w;
        let chips_avail = (strip_w - px(fixed_w + GRAB_HANDLE_W)).max(px(80.));
        let chips = self.tab_strip(show_chips, chips_avail, window, cx);
        let add_button = div()
            .occlude()
            .flex_shrink_0()
            .child(self.new_tab_button("tab-add", cx));

        // Same bargain the rail's own tiles keep when the Appearance switch is
        // on: present in the layout, painted only while the pointer is on the
        // bar. The New Tab button that trails the chips is left alone — it is
        // part of the row of tabs, not of the window's chrome.
        let auto_hide_chrome = cx.global::<Config>().auto_hide_titlebar_buttons;
        let strip_chrome_shown =
            crate::ui::app::titlebar_chrome_shown(auto_hide_chrome, self.strip_chrome_hover.get());
        let rail_collapsed = !show_chips && !self.left_panel_open(cx);
        let left_group = rail_collapsed.then(|| {
            h_flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(2.))
                .ml(px(crate::ui::app::title_bar_hug_offset()))
                .when_some(crate::ui::app::window_mark(), |group, mark| {
                    group.child(
                        div()
                            .flex_shrink_0()
                            .pl(px(crate::ui::app::CONTENT_INSET
                                - crate::ui::app::tile_trailing_inset()))
                            .pr(px(4.))
                            .child(mark),
                    )
                })
                // The logo above stays put: it is the window's mark, not a
                // control, and a window that loses its identity when nobody is
                // pointing at it reads as a different window.
                .child(
                    crate::ui::app::resting_chrome(
                        div().occlude().flex_shrink_0(),
                        strip_chrome_shown,
                    )
                    .child(self.new_tab_button("titlebar-add-collapsed", cx)),
                )
                .child(
                    crate::ui::app::resting_chrome(
                        div().occlude().flex_shrink_0(),
                        strip_chrome_shown,
                    )
                    .child(
                        chrome_tile(
                            Button::new("titlebar-expand-sidebar")
                                .icon(Icon::empty().path("icons/panel-left-collapsed.svg")),
                            false,
                            cx,
                        )
                        .rounded_lg()
                        .accessibility_label(t(L10nKey::TabTooltipShowSidebar))
                        .tooltip_with_action(
                            t(L10nKey::TabTooltipShowSidebar),
                            &crate::core::actions::ToggleLeftPanel,
                            None,
                        )
                        .on_click(cx.listener(|this, _, _window, cx| this.toggle_left_panel(cx))),
                    ),
                )
        });

        // macOS places these controls in the open panel's own title bar, and
        // drops them while a docked document holds the right edge.
        //
        // Elsewhere, with the panel open, they stand in the band above it,
        // beside the panel's own tab row — which is painted whenever the panel
        // is, so a band that grew a button on hover read as a glitch next to
        // it. Once the panel is open they belong to its chrome, and stay.
        let right_chrome_shown = strip_chrome_shown || self.right_panel_open(cx);
        let right_chrome = strip_chrome.then(|| {
            crate::ui::app::resting_chrome(div().flex_shrink_0(), right_chrome_shown)
                .child(self.window_chrome(cx))
        });

        // With the tabs in the rail, the middle of the bar over the terminal
        // is the way into Search Everywhere: a field-shaped button, centred,
        // that opens the search where it stands. The rail beside it already
        // names every tab, so the bar has no title of its own to repeat. The
        // box alone takes the pointer; the rest of the bar still drags.
        //
        // Centred over the terminal column, not the bar: see `search_band`.
        // Off macOS the bar spans the workspace while a document or the detail
        // panel is docked, and centred on all of it the box landed under the
        // document's hoisted header, which has no fill to hide it.
        let docked_right = document_w
            + match self.right_panel_open(cx) {
                true => self.right_panel_px(window, cx),
                false => 0.,
            };
        let band = search_band(
            cfg!(target_os = "macos"),
            title_bar_strip_lead(window.is_fullscreen(), window.rem_size().as_f32()),
            docked_right,
            controls_w,
            rail_collapsed.then(collapsed_rail_group_w),
            (strip_chrome && docked_right <= 0.).then(trailing_chrome_tiles_w),
        );
        let centre_search = (!show_chips).then(|| {
            // The chord as text, not caps: a cap's fill is this box's own
            // grey, so on it a cap is only a gap between two letters.
            let chord = crate::ui::keymap::effective_key("TogglePalette", cx)
                .map(|spec| crate::ui::keymap::key_tokens(&spec).join(""));
            // Two equal springs either side keep the box centred; each stops
            // at the band's `clear`, and past that it is the box that gives.
            let spring = || div().flex_1().min_w(px(band.clear));
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(band.left))
                .right(px(band.right))
                .flex()
                .items_center()
                .child(spring())
                .child(
                    h_flex()
                        .id("titlebar-search")
                        .occlude()
                        .w(px(TITLEBAR_SEARCH_W))
                        .min_w_0()
                        // Enough of the bar that the label and its chord still
                        // fit with a document docked beside the terminal; at a
                        // half it read `Search Everywhe…` in exactly the
                        // layout people search from most.
                        .max_w(gpui_kit::relative(0.6))
                        .h(px(TITLEBAR_SEARCH_H))
                        .items_center()
                        .gap(px(7.))
                        .pl(px(10.))
                        .pr(px(10.))
                        .rounded(px(7.))
                        // The left rail's fill (`Surfaces::rail`, #f5f5f3 /
                        // #1e1e20 on the default pair) — not `sidebar`, which
                        // is the window's own fill and vanished into the bar.
                        .bg(gpui_kit::rgb(
                            cx.global::<crate::ui::presets::Surfaces>().rail.base,
                        ))
                        .cursor_pointer()
                        .text_size(window.rem_size() * 0.8125)
                        .text_color(cx.theme().muted_foreground)
                        .hover(|s| s.text_color(cx.theme().foreground))
                        .child(Icon::new(IconName::Search).size(px(12.)).flex_shrink_0())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(t(L10nKey::AppMenuSearchEverywhere)),
                        )
                        .when_some(chord, |row, chord| {
                            row.child(div().flex_shrink_0().child(chord))
                        })
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(
                            cx.listener(|this, _, window, cx| match this.search.is_some() {
                                true => this.close_search(window, cx),
                                false => this.open_search(SearchTab::All, "", window, cx),
                            }),
                        ),
                )
                .child(spring())
        });

        h_flex()
            .id("tab-strip")
            .relative()
            .items_center()
            .gap_1p5()
            .when(show_chips, |this| this.w(strip_w))
            .when(!show_chips, |this| this.w_full())
            .pl_0()
            .min_w_0()
            .when_some(centre_search, |this, search| this.child(search))
            .when_some(left_group, |this, g| this.child(g))
            .child(chips)
            .when(show_chips, move |this| this.child(add_button))
            .child(div().flex_1().min_w(px(GRAB_HANDLE_W)))
            .when_some(right_chrome, |this, chrome| match chrome_band_w {
                Some(w) => this.child(
                    h_flex()
                        .flex_none()
                        .w(px(w))
                        .items_center()
                        .pl(px(tile_trailing_inset()))
                        .child(chrome),
                ),
                None => this.child(chrome),
            })
            .when(auto_hide_chrome, |this| {
                this.child(crate::ui::app::hover_sheet(
                    "strip-chrome-hover",
                    &self.strip_chrome_hover,
                ))
            })
    }

    /// 右侧面板开关可挂载在窗口 strip 或 macOS 面板头，只选择一个位置。
    /// Persistent window controls, including when either sidebar is collapsed.
    pub(crate) fn window_chrome(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let trailing = match cfg!(target_os = "macos") {
            true => tile_trailing_inset(),
            false => 4.,
        };
        // Opening the panel moves this control to the panel's header on
        // macOS. Reuse the same tile and inset there: a different hit target
        // moves the centred glyph even when the icon itself keeps its size.
        let panel_open = self.right_panel_open(cx);
        h_flex()
            .flex_shrink_0()
            .items_center()
            .pr(px(trailing))
            .child(
                div().occlude().flex_shrink_0().child(
                    chrome_tile(
                        Button::new("titlebar-right-panel")
                            // Keep the outline fixed; only the panel region's
                            // fill changes to show whether the panel is visible.
                            .icon(Icon::empty().path(match panel_open {
                                true => "icons/panel-right.svg",
                                false => "icons/panel-right-collapsed.svg",
                            })),
                        false,
                        cx,
                    )
                    .rounded_lg()
                    .accessibility_label(match panel_open {
                        true => t(L10nKey::TabTooltipHideDetailPanel),
                        false => t(L10nKey::TabTooltipShowDetailPanel),
                    })
                    .tooltip_with_action(
                        match panel_open {
                            true => t(L10nKey::TabTooltipHideDetailPanel),
                            false => t(L10nKey::TabTooltipShowDetailPanel),
                        },
                        &crate::core::actions::ToggleRightPanel,
                        None,
                    )
                    // On macOS this tile is drawn inside the panel's own
                    // titlebar while the panel is open, so closing from it
                    // destroys the element holding the focus — and a keymap
                    // scoped to a focused thing goes quiet with it, leaving the
                    // ⌘J that would undo this doing nothing. Hand the terminal
                    // back what it lost.
                    .on_click(cx.listener(|this, _, window, cx| {
                        let closing = this.right_panel_open(cx);
                        this.toggle_right_panel(cx);
                        if closing {
                            this.focus_active(window, cx);
                        }
                    })),
                ),
            )
    }
}

#[cfg(test)]
mod gpui_tests {
    use std::io::ErrorKind;

    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{Entity, Focusable as _, TestAppContext, VisualTestContext, px, size};

    use crate::core::actions::{ActivateTab1, ActivateTab2};
    use crate::core::config::{Config, RightPanelTab, TabBarPosition};
    use crate::daemon::protocol::ClientMsg;
    use crate::daemon::transport::Stream;
    use crate::ui::app::{LyrelApp, test_window::harness_with_tabs};

    fn settle(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| window.render_frame(cx));
        vcx.run_until_parked();
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

    #[gpui_kit::test]
    fn closing_panel_chrome_restores_terminal_input_in_both_tab_modes(cx: &mut TestAppContext) {
        let (app, mut vcx, mut streams) = harness_with_tabs(cx, 2);
        vcx.simulate_resize(size(px(1440.), px(900.)));
        let original = app.read_with(&vcx, |app, cx| {
            (
                app.workspace,
                app.tabs.iter().map(|tab| tab.tree_id.get()).collect::<Vec<_>>(),
                app.tabs.iter().map(|tab| {
                    let terminal = tab.pane.terminals().pop().unwrap();
                    (terminal.entity_id(), terminal.read(cx).pane_id)
                }).collect::<Vec<_>>(),
            )
        });

        for position in [TabBarPosition::Top, TabBarPosition::Left] {
            for collapsed in [false, true] {
                app.update_in(&mut vcx, |app, window, cx| {
                    cx.global_mut::<Config>().tab_bar_position = position;
                    cx.global_mut::<Config>().auto_hide_titlebar_buttons = false;
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
                    app.file_search.update(cx, |search, cx| search.focus(window, cx));
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
                assert_eq!(input(&mut streams[1]), None, "the hidden tab receives no key");
                vcx.dispatch_action(ActivateTab2);
                settle(&mut vcx);
                assert_eq!(app.read_with(&vcx, |app, _| app.active), 1);
                assert!(terminal_focused(&app, &mut vcx));
                vcx.dispatch_action(ActivateTab1);
                settle(&mut vcx);
            }
        }
        app.read_with(&vcx, |app, cx| {
            assert_eq!(app.workspace, original.0);
            assert_eq!(app.tabs.iter().map(|tab| tab.tree_id.get()).collect::<Vec<_>>(), original.1);
            assert_eq!(app.tabs.iter().map(|tab| {
                let terminal = tab.pane.terminals().pop().unwrap();
                (terminal.entity_id(), terminal.read(cx).pane_id)
            }).collect::<Vec<_>>(), original.2);
        });
    }


    #[gpui_kit::test]
    fn navigation_clicks_and_modal_keys_reach_only_the_selected_session(cx: &mut TestAppContext) {
        let (app, mut vcx, mut streams) = harness_with_tabs(cx, 2);
        vcx.simulate_resize(size(px(1440.), px(900.)));
        let palette_key = vcx.update(|_, cx| {
            crate::ui::keymap::effective_key("TogglePalette", cx).unwrap()
        });
        let first_tab_key = vcx.update(|_, cx| {
            crate::ui::keymap::effective_key("ActivateTab1", cx).unwrap()
        });
        for position in [TabBarPosition::Top, TabBarPosition::Left] {
            app.update_in(&mut vcx, |app, window, cx| {
                cx.global_mut::<Config>().tab_bar_position = position;
                app.sidebar_collapsed = false;
                app.right_panel_visible = false;
                app.focus_active(window, cx);
                cx.notify();
            });
            settle(&mut vcx);
            let second_tab = app.read_with(&vcx, |app, _| match position {
                TabBarPosition::Top => app.strip_slots.borrow()[1],
                TabBarPosition::Left => app.sidebar_slots.borrow()[1],
            });
            assert!(second_tab.size.width > px(0.));
            vcx.simulate_click(second_tab.center(), gpui_kit::Modifiers::none());
            settle(&mut vcx);
            assert_eq!(app.read_with(&vcx, |app, _| app.active), 1);
            assert!(terminal_focused(&app, &mut vcx));

            vcx.simulate_keystrokes(&palette_key);
            settle(&mut vcx);
            assert!(app.read_with(&vcx, |app, _| app.search.is_some()));
            vcx.simulate_keystrokes(&first_tab_key);
            vcx.simulate_keystrokes("q");
            settle(&mut vcx);
            assert_eq!(app.read_with(&vcx, |app, _| app.active), 1);
            assert_eq!(input(&mut streams[0]), None);
            assert_eq!(input(&mut streams[1]), None);
            vcx.simulate_keystrokes("escape");
            settle(&mut vcx);
            assert!(app.read_with(&vcx, |app, _| app.search.is_none()));
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::TRAFFIC_LIGHTS_LEAD;
    /// Where a band lands in the terminal column, for a strip that starts
    /// `lead` in and ends `strip_end` from the column's left edge.
    fn band_in_column(band: SearchBand, lead: f32, strip_end: f32) -> (f32, f32) {
        (lead + band.left, strip_end - band.right)
    }

    /// #1033, with the reporter's window: 1946pt wide, the rail collapsed.
    /// The strip started 80pt in, after the traffic lights, and centred on it
    /// the box sat at 1013 — 40pt right of the window's middle at 973.
    #[test]
    fn with_the_rail_collapsed_on_macos_the_search_box_centres_on_the_window() {
        let (w, lead) = (1946., TRAFFIC_LIGHTS_LEAD);
        let band = search_band(true, lead, 0., 0., Some(58.), Some(49.));
        let (l, r) = band_in_column(band, lead, w);
        assert_eq!((l, r), (0., w));
        assert_eq!((l + r) / 2., 973.);
        // Clear of the traffic lights and the two tiles after them, at both
        // ends so that clearing them does not pull the box off centre.
        assert_eq!(band.clear, lead + 58. + TITLEBAR_SEARCH_CLEAR_GAP);
    }

    /// With the rail open, the title bar sits in the terminal column but
    /// still leaves the traffic lights' lead before the strip, over nothing
    /// — the lights are over the rail. The band reaches back over that too,
    /// and centres on the column.
    #[test]
    fn with_the_rail_open_on_macos_the_search_box_centres_on_the_terminal_column() {
        let (column, lead) = (1200., TRAFFIC_LIGHTS_LEAD);
        let band = search_band(true, lead, 0., 0., None, Some(49.));
        assert_eq!(band_in_column(band, lead, column), (0., column));
        assert_eq!(band.clear, 49. + TITLEBAR_SEARCH_CLEAR_GAP);

        // A docked panel on macOS is beside the column, not under its bar:
        // nothing to subtract from the strip's end.
        let docked = search_band(true, lead, 360., 0., None, None);
        assert_eq!(band_in_column(docked, lead, column), (0., column));
        assert_eq!(docked.clear, 0.);
    }

    /// Fullscreen keeps the traffic lights' lead and adds the bar's own inset.
    #[test]
    fn fullscreen_on_macos_reaches_back_over_the_bars_extra_inset_as_well() {
        let lead = TRAFFIC_LIGHTS_LEAD + 12.;
        let band = search_band(true, lead, 0., 0., Some(58.), Some(49.));
        assert_eq!(band_in_column(band, lead, 1440.), (0., 1440.));
    }

    /// Off macOS the bar spans the docked columns and stops short of the
    /// window buttons; the band ends where the terminal column does, as it
    /// always has, and now starts where it does too.
    #[test]
    fn off_macos_the_band_is_the_terminal_column_beside_a_docked_panel() {
        let (w, lead, controls, panel) = (1600., 12., 102., 360.);
        let strip_end = w - controls;
        let band = search_band(false, lead, panel, controls, None, None);
        assert_eq!(band.right, panel - controls);
        assert_eq!(band_in_column(band, lead, strip_end), (0., w - panel));

        // Nothing docked: the band runs to the end of the strip, short of
        // the window buttons, and keeps clear of the tiles standing there.
        let bare = search_band(false, lead, 0., controls, None, Some(38.));
        assert_eq!(band_in_column(bare, lead, strip_end), (0., strip_end));
        assert_eq!(bare.clear, 38. + TITLEBAR_SEARCH_CLEAR_GAP);

        // A panel narrower than the buttons leaves no room to take off.
        let thin = search_band(false, lead, 80., controls, None, None);
        assert_eq!(thin.right, 0.);
    }
}
