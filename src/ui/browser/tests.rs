use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt as _;

use super::{BrowserCommand, BrowserEvent, layout_active_web};
use crate::ui::app::test_window::harness_with_tabs;

#[gpui_kit::test]
fn browser_tabs_select_independent_surfaces_and_drop_closed_popups(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    let (first, second) = app.update_in(&mut vcx, |app, _, cx| {
        app.open_web_page("https://example.test/first", None, cx);
        let first = layout_active_web(&app.tool_layout()).unwrap();
        app.attach_test_browser_surface(&first, cx);
        app.browser_reload(&first, cx);

        app.on_browser_event(
            &first,
            BrowserEvent::NewWindow {
                url: "https://example.test/second".into(),
            },
            cx,
        );
        let second = layout_active_web(&app.tool_layout()).unwrap();
        assert_ne!(first, second);
        app.attach_test_browser_surface(&second, cx);
        app.browser_forward(&second, cx);
        assert_eq!(
            app.browser_surface_commands(&first),
            [BrowserCommand::Reload]
        );
        assert_eq!(
            app.browser_surface_commands(&second),
            [BrowserCommand::Forward]
        );
        (first, second)
    });
    vcx.update(|window, _| {
        assert!(window.find(format!("web-tab-{first}")).visible());
        assert!(window.find(format!("web-tab-{second}")).visible());
    });

    app.update_in(&mut vcx, |app, window, cx| {
        app.close_right_panel_web_tab(second.clone(), window, cx);
        app.on_browser_event(
            &second,
            BrowserEvent::NewWindow {
                url: "https://example.test/stale".into(),
            },
            cx,
        );
        assert_eq!(
            layout_active_web(&app.tool_layout()).as_deref(),
            Some(first.as_str())
        );
        assert!(app.browser_surface_commands(&second).is_empty());
    });
}
