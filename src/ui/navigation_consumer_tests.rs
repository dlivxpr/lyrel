//! Production Root consumers for manual-activation navigation, not private focus helpers.
use crate::core::actions::{FocusRightPanelNavigation, FocusSettingsNavigation};
use crate::core::config::RightPanelTab;
use crate::ui::settings::SettingsSection;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext, px, size};

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..4 {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.render_frame(cx);
        });
    }
    cx.run_until_parked();
}

#[gpui_kit::test]
fn panel_navigation_enters_selected_roves_activates_and_returns_to_content(
    cx: &mut TestAppContext,
) {
    for locale in ["en", "zh-CN"] {
        crate::ui::i18n::set_locale(locale);
        let (app, mut vcx, _stream) = crate::ui::app::test_window::harness_with_pane(cx);
        vcx.simulate_resize(size(px(1000.), px(720.)));
        let content = app.update_in(&mut vcx, |app, window, cx| {
            app.focus_active(window, cx);
            window.focused(cx).expect("terminal content owns focus")
        });
        vcx.dispatch_action(FocusRightPanelNavigation);
        settle(&mut vcx);
        vcx.update(|window, cx| {
            assert_eq!(window.find("right-panel-tab-info").focused(), Some(true));
            window.press("right", cx);
            assert_eq!(window.find("right-panel-tab-files").focused(), Some(true));
            assert_eq!(window.find("right-panel-tab-info").selected(), Some(true));
            window.press("enter", cx);
        });
        settle(&mut vcx);
        assert_eq!(
            app.read_with(&vcx, |app, _| app.right_panel_tab),
            RightPanelTab::Files
        );
        vcx.update(|window, cx| {
            assert_eq!(window.find("right-panel-tab-files").focused(), Some(true));
            window.press("right", cx);
            assert_eq!(window.find("right-panel-tab-changes").focused(), Some(true));
            window.press("space", cx);
        });
        settle(&mut vcx);
        assert_eq!(
            app.read_with(&vcx, |app, _| app.right_panel_tab),
            RightPanelTab::Scm
        );
        vcx.update(|window, cx| {
            window.press("end", cx);
            assert_eq!(window.find("right-panel-tab-github").focused(), Some(true));
            window.press("right", cx);
            assert_eq!(window.find("right-panel-tab-info").focused(), Some(true));
            window.press("home", cx);
            window.press("tab", cx);
            assert_ne!(window.find("right-panel-tab-info").focused(), Some(true));
            window.press("shift-tab", cx);
            assert_eq!(window.find("right-panel-tab-changes").focused(), Some(true));
            window.press("escape", cx);
            assert!(
                content.is_focused(window),
                "Escape returns to the owning terminal"
            );
            window.click("right-panel-tab-files", cx);
            assert!(
                content.is_focused(window),
                "pointer activation keeps content focus"
            );
        });
    }
    crate::ui::i18n::set_locale("en");
}

#[gpui_kit::test]
fn settings_navigation_is_vertical_manual_and_restores_page_focus(cx: &mut TestAppContext) {
    for locale in ["en", "zh-CN"] {
        crate::ui::i18n::set_locale(locale);
        let (settings, mut vcx) = crate::ui::settings::test_window::harness(cx);
        vcx.simulate_resize(size(px(960.), px(700.)));
        vcx.dispatch_action(FocusSettingsNavigation);
        settle(&mut vcx);
        vcx.update(|window, cx| {
            assert_eq!(
                window.find("settings-nav-settings:general").focused(),
                Some(true)
            );
            window.press("down", cx);
            assert_eq!(
                window.find("settings-nav-settings:appearance").focused(),
                Some(true)
            );
            assert_eq!(
                window.find("settings-nav-settings:general").selected(),
                Some(true)
            );
            window.press("enter", cx);
        });
        settle(&mut vcx);
        assert!(settings.read_with(&vcx, |settings, _| {
            settings.active_settings().unwrap().navigation.section() == SettingsSection::Appearance
        }));
        vcx.update(|window, cx| {
            assert_eq!(
                window.find("settings-nav-settings:appearance").focused(),
                Some(true)
            );
            window.press("down", cx);
            window.press("space", cx);
        });
        settle(&mut vcx);
        assert!(settings.read_with(&vcx, |settings, _| {
            settings.active_settings().unwrap().navigation.section() == SettingsSection::Terminal
        }));
        vcx.update(|window, cx| {
            window.press("end", cx);
            assert_eq!(
                window.find("settings-nav-settings:about").focused(),
                Some(true)
            );
            window.press("down", cx);
            assert_eq!(
                window.find("settings-nav-settings:general").focused(),
                Some(true)
            );
            window.press("home", cx);
            window.press("tab", cx);
            assert_ne!(
                window.find("settings-nav-settings:general").focused(),
                Some(true)
            );
            window.press("shift-tab", cx);
            assert_eq!(
                window.find("settings-nav-settings:terminal").focused(),
                Some(true)
            );
            window.press("escape", cx);
        });
        settings.read_with(&vcx, |settings, _| {
            assert!(
                settings.has_settings(),
                "leaving navigation must not close settings"
            );
        });
        settings.update_in(&mut vcx, |settings, window, _| {
            assert!(
                settings
                    .active_settings()
                    .unwrap()
                    .focus_handle
                    .is_focused(window)
            );
        });
    }
    crate::ui::i18n::set_locale("en");
}
