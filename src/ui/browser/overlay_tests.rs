//! Combined overlay coverage for the Side Panel browser (#403): the native
//! page gives way to the workbench's covering surfaces, paints a bounded still
//! frame from the selected tab, and comes back without a reload.

use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt as _;

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

fn open_page(
    app: &mut LyrelApp,
    address: &str,
    window: &mut gpui_kit::Window,
    cx: &mut gpui_kit::Context<LyrelApp>,
) -> String {
    app.open_web_page(address, None, cx);
    let id = layout_active_web(&app.tool_layout()).expect("the new page is selected");
    app.attach_test_browser_surface(&id, cx);
    window.draw(cx).clear(cx);
    assert_eq!(
        visibility(app, &id),
        Some(true),
        "the freshly painted page is visible"
    );
    id
}

#[gpui_kit::test]
fn search_everywhere_conceals_the_page_and_restores_it_without_a_reload(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut vcx, |app, window, cx| {
        register_window(app, window, cx);
        let tab = open_page(app, "https://example.test/form", window, cx);
        fn page_surface<'a>(app: &'a LyrelApp, tab: &str) -> &'a BrowserSurfaceHost {
            app.right_panel.web_pages[tab].surface.as_ref().unwrap()
        }
        page_surface(app, &tab).test_take_focus();

        app.open_search(SearchTab::All, "", window, cx);
        window.draw(cx).clear(cx);
        assert_eq!(
            visibility(app, &tab),
            Some(false),
            "Search Everywhere hides the live page"
        );
        assert!(
            window
                .try_find(format!("browser-still-frame-{tab}"))
                .is_some(),
            "the bounded still frame names the concealed tab"
        );
        assert!(
            app.right_panel.web_pages[&tab].still_frame(),
            "the page paints the still frame while its surface is hidden"
        );

        app.close_search(window, cx);
        window.draw(cx).clear(cx);
        assert_eq!(
            visibility(app, &tab),
            Some(true),
            "closing the overlay restores the live page"
        );
        assert_eq!(page_surface(app, &tab).test_focus_requests(), 1);
        assert!(
            app.browser_surface_commands(&tab).is_empty(),
            "concealment never navigates or reloads the page"
        );
    });
}

#[gpui_kit::test]
fn overlapping_covers_lift_one_at_a_time_and_a_tab_switch_uses_the_new_page(
    cx: &mut TestAppContext,
) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut vcx, |app, window, cx| {
        register_window(app, window, cx);
        let first = open_page(app, "https://example.test/first", window, cx);
        app.open_web_page("https://example.test/second", None, cx);
        let second = layout_active_web(&app.tool_layout()).expect("second selected");
        app.attach_test_browser_surface(&second, cx);

        let first_entry = app
            .tool_layout()
            .order
            .into_iter()
            .find(|entry| matches!(entry, crate::ui::right_panel::PanelTab::Web(tab) if tab.id == first))
            .expect("first page is open");
        app.select_panel_tab(&first_entry, window, cx);
        window.draw(cx).clear(cx);
        assert_eq!(visibility(app, &first), Some(true));

        app.set_surface_cover(CoverSource::SearchEverywhere, true, cx);
        app.set_surface_cover(CoverSource::ToolAddMenu, true, cx);
        window.draw(cx).clear(cx);
        assert_eq!(visibility(app, &first), Some(false));

        app.set_surface_cover(CoverSource::SearchEverywhere, false, cx);
        window.draw(cx).clear(cx);
        assert_eq!(
            visibility(app, &first),
            Some(false),
            "the second cover keeps the page concealed"
        );

        let second_entry = app
            .tool_layout()
            .order
            .into_iter()
            .find(|entry| matches!(entry, crate::ui::right_panel::PanelTab::Web(tab) if tab.id == second))
            .expect("second page is open");
        app.select_panel_tab(&second_entry, window, cx);
        window.draw(cx).clear(cx);
        assert_eq!(
            visibility(app, &first),
            Some(false),
            "switching tabs while covered keeps the old surface hidden"
        );
        assert_eq!(visibility(app, &second), Some(false));
        assert!(
            window
                .try_find(format!("browser-still-frame-{second}"))
                .is_some(),
            "the still frame belongs to the newly selected surface"
        );
        assert!(
            window
                .try_find(format!("browser-still-frame-{first}"))
                .is_none(),
            "no stale frame from the previous tab remains"
        );

        app.set_surface_cover(CoverSource::ToolAddMenu, false, cx);
        window.draw(cx).clear(cx);
        assert_eq!(visibility(app, &second), Some(true));
        assert_eq!(visibility(app, &first), Some(false));
    });
}

#[gpui_kit::test]
fn ordinary_toasts_clear_the_page_without_hiding_it(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut vcx, |app, window, cx| {
        register_window(app, window, cx);
        crate::ui::theme::apply_theme(Some(window), cx);
        let tab = open_page(app, "https://example.test/table", window, cx);
        let panel = app.right_panel_px(window, cx);
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
        window.draw(cx).clear(cx);
        assert_eq!(
            visibility(app, &tab),
            Some(true),
            "a toast must not conceal the page"
        );
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
    app.update_in(&mut vcx, |app, window, cx| {
        register_window(app, window, cx);
        let tab = open_page(app, "https://example.test/dialog", window, cx);
        let cover = lyrel_ui_base::native_surface::cover(window, cx);
        window.draw(cx).clear(cx);
        assert_eq!(
            visibility(app, &tab),
            Some(false),
            "a dialog cover hides the native page"
        );
        assert!(
            window
                .try_find(format!("browser-still-frame-{tab}"))
                .is_some()
        );

        cover.release(cx);
        window.draw(cx).clear(cx);
        assert_eq!(
            visibility(app, &tab),
            Some(true),
            "releasing the dialog restores the page"
        );
    });
}
