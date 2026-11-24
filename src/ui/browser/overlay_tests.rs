//! Combined overlay coverage for the Side Panel browser (#403): the native
//! page gives way to the workbench's covering surfaces, paints a bounded still
//! frame from the selected tab, and comes back without a reload.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, TestAppContext, VisualTestContext};

use super::*;
use crate::ui::app::test_window::harness_with_tabs;
use crate::ui::notification_history::{Notification, NotificationWindowExt as _};
use crate::ui::search::SearchTab;
use crate::ui::surface_coordinator::CoverSource;

fn visibility(app: &LyrelApp, tab: &str) -> Option<bool> {
    app.right_panel
        .web_pages
        .get(tab)
        .and_then(|page| page.surface.as_ref())
        .and_then(|surface| surface.test_visibility())
}

fn register_window(
    app: &mut LyrelApp,
    window: &mut gpui_kit::Window,
    cx: &mut gpui_kit::Context<LyrelApp>,
) {
    let app_entity = cx.entity().downgrade();
    crate::ui::windows::WindowRegistry::register(
        cx,
        app.workspace,
        window.window_handle(),
        app_entity,
    );
}

fn frame(vcx: &mut VisualTestContext) {
    vcx.update(|window, cx| window.draw(cx).clear(cx));
}

fn finish_tab_switch(vcx: &mut VisualTestContext) {
    vcx.run_until_parked();
    vcx.executor()
        .advance_clock(std::time::Duration::from_millis(250));
    vcx.run_until_parked();
    frame(vcx);
}

fn open_page(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext, address: &str) -> String {
    let id = app.update_in(vcx, |app, window, cx| {
        register_window(app, window, cx);
        app.open_web_page(address, None, cx);
        let id = layout_active_web(&app.tool_layout()).expect("the new page is selected");
        app.attach_test_browser_surface(&id, cx);
        id
    });
    finish_tab_switch(vcx);
    assert_eq!(
        app.read_with(vcx, |app, _| visibility(app, &id)),
        Some(true)
    );
    id
}

fn select_page(app: &mut LyrelApp, id: &str, window: &mut Window, cx: &mut Context<LyrelApp>) {
    let entry = app
        .tool_layout()
        .order
        .into_iter()
        .find(|entry| matches!(entry, PanelTab::Web(tab) if tab.id == id))
        .expect("page is open");
    app.select_panel_tab(&entry, window, cx);
}

fn page_surface<'a>(app: &'a LyrelApp, tab: &str) -> &'a BrowserSurfaceHost {
    app.right_panel.web_pages[tab].surface.as_ref().unwrap()
}

#[gpui_kit::test]
fn closing_the_last_terminal_tab_hides_the_native_page(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    let tab = open_page(&app, &mut vcx, "https://example.test/preview");

    app.update_in(&mut vcx, |app, window, cx| app.close_tab(0, window, cx));
    frame(&mut vcx);

    vcx.update(|window, _| {
        assert!(window.try_find("browser-surface-anchor").is_none());
    });
    assert_eq!(
        app.read_with(&vcx, |app, _| visibility(app, &tab)),
        Some(false),
        "the native page must leave the empty workspace when the Side Panel unmounts"
    );
}

#[gpui_kit::test]
fn search_everywhere_conceals_the_page_and_restores_it_without_a_reload(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    let tab = open_page(&app, &mut vcx, "https://example.test/form");
    app.update_in(&mut vcx, |app, window, cx| {
        page_surface(app, &tab).test_take_focus();
        app.open_search(SearchTab::All, "", window, cx);
    });
    vcx.run_until_parked();
    frame(&mut vcx);
    app.read_with(&vcx, |app, _| {
        assert_eq!(visibility(app, &tab), Some(false));
    });
    vcx.update(|window, _| {
        assert!(
            window
                .try_find(format!("browser-still-frame-{tab}"))
                .is_some()
        );
        assert!(window.try_find("browser-still-frame-image").is_some());
    });

    app.update_in(&mut vcx, |app, window, cx| app.close_search(window, cx));
    frame(&mut vcx);
    app.read_with(&vcx, |app, _| {
        assert_eq!(visibility(app, &tab), Some(true));
        assert_eq!(page_surface(app, &tab).test_focus_requests(), 1);
        assert!(matches!(
            app.browser_surface_commands(&tab).as_slice(),
            [BrowserCommand::Capture { .. }]
        ));
    });
}

#[gpui_kit::test]
fn overlapping_covers_lift_one_at_a_time_and_a_tab_switch_uses_the_new_page(
    cx: &mut TestAppContext,
) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    let first = open_page(&app, &mut vcx, "https://example.test/first");
    let second = open_page(&app, &mut vcx, "https://example.test/second");
    app.update_in(&mut vcx, |app, window, cx| {
        select_page(app, &first, window, cx)
    });
    finish_tab_switch(&mut vcx);

    app.update_in(&mut vcx, |app, _, cx| {
        app.set_surface_cover(CoverSource::SearchEverywhere, true, cx);
        app.set_surface_cover(CoverSource::ToolAddMenu, true, cx);
    });
    assert_eq!(
        app.read_with(&vcx, |app, _| visibility(app, &first)),
        Some(false)
    );

    app.update_in(&mut vcx, |app, _, cx| {
        app.set_surface_cover(CoverSource::SearchEverywhere, false, cx);
    });
    assert_eq!(
        app.read_with(&vcx, |app, _| visibility(app, &first)),
        Some(false)
    );

    app.update_in(&mut vcx, |app, window, cx| {
        select_page(app, &second, window, cx)
    });
    finish_tab_switch(&mut vcx);
    app.read_with(&vcx, |app, _| {
        assert_eq!(visibility(app, &first), Some(false));
        assert_eq!(visibility(app, &second), Some(false));
    });
    vcx.update(|window, _| {
        assert!(
            window
                .try_find(format!("browser-still-frame-{second}"))
                .is_some()
        );
        assert!(
            window
                .try_find(format!("browser-still-frame-{first}"))
                .is_none()
        );
    });

    app.update_in(&mut vcx, |app, _, cx| {
        app.set_surface_cover(CoverSource::ToolAddMenu, false, cx);
    });
    frame(&mut vcx);
    app.read_with(&vcx, |app, _| {
        assert_eq!(visibility(app, &second), Some(true));
        assert_eq!(visibility(app, &first), Some(false));
    });
}

#[gpui_kit::test]
fn ordinary_toasts_clear_the_page_without_hiding_it(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.update(|window, cx| crate::ui::theme::apply_theme(Some(window), cx));
    let tab = open_page(&app, &mut vcx, "https://example.test/table");
    let panel = app.update_in(&mut vcx, |app, window, cx| app.right_panel_px(window, cx));
    vcx.update(|window, cx| {
        assert_eq!(
            gpui_kit::component::Theme::global(cx)
                .notification
                .margins
                .right,
            px(panel + 8.),
            "the toast stack clears the WebView at the content edge"
        );
        assert_eq!(
            gpui_kit::component::Theme::global(cx)
                .notification
                .margins
                .bottom,
            px(16. * lyrel_ui_base::display::GLOBAL_STATUS_BAR_HEIGHT_REM + 8.),
            "the toast stack still clears the global status bar"
        );

        window.notify_app(Notification::new().message("Saved"), cx);
    });
    frame(&mut vcx);
    assert_eq!(
        app.read_with(&vcx, |app, _| visibility(app, &tab)),
        Some(true)
    );
    vcx.update(|_, cx| {
        assert_eq!(
            gpui_kit::component::Theme::global(cx)
                .notification
                .margins
                .right,
            px(panel + 8.),
            "the clearance survives the toast"
        );
    });
}

#[gpui_kit::test]
fn a_dialog_cover_conceals_and_releases_the_page(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    let tab = open_page(&app, &mut vcx, "https://example.test/dialog");
    let cover = vcx.update(|window, cx| lyrel_ui_base::native_surface::cover(window, cx));
    frame(&mut vcx);
    assert_eq!(
        app.read_with(&vcx, |app, _| visibility(app, &tab)),
        Some(false)
    );
    vcx.update(|window, _| {
        assert!(
            window
                .try_find(format!("browser-still-frame-{tab}"))
                .is_some()
        );
    });

    vcx.update(|_, cx| cover.release(cx));
    frame(&mut vcx);
    assert_eq!(
        app.read_with(&vcx, |app, _| visibility(app, &tab)),
        Some(true)
    );
}

#[gpui_kit::test]
fn focused_popup_menu_stays_above_the_native_page(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, MouseButton};

    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    let tab = open_page(&app, &mut vcx, "https://example.test/popup");
    let trigger = vcx.update(|window, cx| {
        crate::ui::surface_coordinator::install_popup_menu_presence(cx);
        window.find(("tab-row", 0usize)).bounds().center()
    });
    vcx.simulate_mouse_down(trigger, MouseButton::Right, Modifiers::none());
    vcx.simulate_mouse_up(trigger, MouseButton::Right, Modifiers::none());
    frame(&mut vcx);
    assert_eq!(
        app.read_with(&vcx, |app, _| visibility(app, &tab)),
        Some(false)
    );

    vcx.simulate_keystrokes("escape");
    frame(&mut vcx);
    assert_eq!(
        app.read_with(&vcx, |app, _| visibility(app, &tab)),
        Some(true)
    );
}
