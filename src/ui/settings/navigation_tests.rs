use super::*;
use crate::ui::settings::test_window::harness;
use gpui_kit::{TestAppContext, size};

#[gpui_kit::test]
fn enter_on_a_shortcut_search_result_opens_its_local_filter(cx: &mut TestAppContext) {
    let (app, mut vcx) = harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_settings_section(SettingsSection::General, window, cx);
        let input = app.active_settings().unwrap().navigation.search().clone();
        input.update(cx, |input, cx| input.set_value("SplitRight", window, cx));
        app.autoselect_settings_search(cx);
    });
    vcx.run_until_parked();
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, _, cx| {
        let state = app.active_settings().unwrap();
        assert!(state.navigation.section() == SettingsSection::Keybindings);
        assert!(!state.navigation.is_searching());
        assert_eq!(
            state.shortcut_search.read(cx).value().as_str(),
            "SplitRight"
        );
    });
}

#[gpui_kit::test]
fn search_reuses_rows_and_reset_changes_only_the_selected_setting(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let (app, mut vcx) = harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_settings_section(SettingsSection::General, window, cx);
        app.set_notify_threshold(71, cx);
        app.set_copy_on_select(!Config::default().copy_on_select, cx);
        let search = app.active_settings().unwrap().navigation.search().clone();
        search.update(cx, |s, cx| s.set_value("notify_threshold_secs", window, cx));
        app.autoselect_settings_search(cx);
    });
    vcx.simulate_resize(size(px(720.), px(560.)));
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(app.active_settings().unwrap().navigation.is_searching());
        app.reset_settings_value(
            crate::ui::i18n::L10nKey::SettingsNotifyThreshold,
            window,
            cx,
        );
        assert_eq!(
            cx.global::<Config>().notify_threshold_secs,
            Config::default().notify_threshold_secs
        );
        assert_eq!(
            cx.global::<Config>().copy_on_select,
            !Config::default().copy_on_select
        );
    });
    vcx.run_until_parked();
}

#[gpui_kit::test]
fn every_category_and_full_search_can_layout_at_minimum_width(cx: &mut TestAppContext) {
    let (app, mut vcx) = harness(cx);
    for section in SettingsSection::ALL {
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_settings_section(section, window, cx)
        });
        vcx.simulate_resize(size(px(720.), px(560.)));
        vcx.run_until_parked();
    }
    app.update_in(&mut vcx, |app, _, cx| {
        app.active_settings_mut().unwrap().navigation.set_modified_only(true);
        app.autoselect_settings_search(cx);
    });
    vcx.run_until_parked();
}

#[gpui_kit::test]
fn clearing_search_preserves_the_category_and_target_navigation_clears_search(
    cx: &mut TestAppContext,
) {
    let (app, mut vcx) = harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_settings_section(SettingsSection::Appearance, window, cx);
        let search = app.active_settings().unwrap().navigation.search().clone();
        search.update(cx, |s, cx| s.set_value("mouse", window, cx));
        app.autoselect_settings_search(cx);
        assert!(app.active_settings().unwrap().navigation.section() == SettingsSection::Appearance);
        search.update(cx, |s, cx| s.set_value("", window, cx));
        app.autoselect_settings_search(cx);
        assert!(!app.active_settings().unwrap().navigation.is_searching());
        app.navigate_settings(
            SettingsSection::KeyboardMouse,
            Some(crate::ui::i18n::L10nKey::SettingsMouseZoom),
            window,
            cx,
        );
        assert!(app.active_settings().unwrap().navigation.section() == SettingsSection::KeyboardMouse);
    });
    vcx.run_until_parked();
}

#[gpui_kit::test]
fn search_and_modified_filter_restore_the_page_location_on_clear_and_reopen(cx: &mut TestAppContext) {
    let (app, mut vcx) = harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        app.navigate_settings(SettingsSection::Appearance, None, window, cx);
        let offset = gpui_kit::point(px(0.), px(-180.));
        let search = app.active_settings().unwrap().navigation.search().clone();
        app.active_settings().unwrap().navigation.scroll().set_offset(offset);
        search.update(cx, |input, cx| input.set_value("font", window, cx));
        app.autoselect_settings_search(cx);
        app.active_settings_mut().unwrap().navigation.set_modified_only(true);
        app.autoselect_settings_search(cx);
        search.update(cx, |input, cx| input.set_value("", window, cx));
        app.autoselect_settings_search(cx);
        assert!(app.active_settings().unwrap().navigation.is_searching());
        app.active_settings_mut().unwrap().navigation.set_modified_only(false);
        app.autoselect_settings_search(cx);
        assert_eq!(app.active_settings().unwrap().navigation.scroll().offset(), offset);
        search.update(cx, |input, cx| input.set_value("font", window, cx));
        app.autoselect_settings_search(cx);
        app.close_settings(window, cx);
        app.build_settings_state(window, cx);
        let navigation = &app.active_settings().unwrap().navigation;
        assert!(navigation.section() == SettingsSection::Appearance);
        assert!(!navigation.is_searching());
        assert_eq!(navigation.scroll().offset(), offset);
        let replacement_search = navigation.search().clone();
        search.update(cx, |input, cx| input.set_value("old session", window, cx));
        assert!(replacement_search.read(cx).value().is_empty());
        assert!(!app.active_settings().unwrap().navigation.is_searching());
    });
}

#[gpui_kit::test]
fn an_unsaved_navigation_answer_cannot_navigate_a_replacement_session(cx: &mut TestAppContext) {
    let (app, mut vcx) = harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        app.active_settings_mut().unwrap().save_error = Some("old session write failed".into());
        app.navigate_settings(SettingsSection::About, None, window, cx);
    });
    vcx.run_until_parked();
    assert!(vcx.has_pending_prompt());
    app.update_in(&mut vcx, |app, window, cx| {
        app.close_settings(window, cx);
        app.build_settings_state(window, cx);
        app.active_settings_mut().unwrap().save_error = Some("replacement write failed".into());
    });
    vcx.simulate_prompt_answer(t(L10nKey::EditorDiscard));
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, _, _| {
        let state = app.active_settings().unwrap();
        assert!(state.navigation.section() == SettingsSection::General);
        assert_eq!(state.save_error.as_deref(), Some("replacement write failed"));
    });
}

#[gpui_kit::test]
fn deferred_workbench_navigation_cannot_target_a_replacement_session(cx: &mut TestAppContext) {
    let (workspace, mut vcx) = crate::ui::app::test_window::harness(cx);
    let app = crate::ui::settings::test_window::open(&workspace, &mut vcx);
    workspace.update_in(&mut vcx, |workspace, window, cx| {
        workspace.open_settings_section(SettingsSection::About, window, cx);
        app.update(cx, |app, cx| {
            app.close_settings(window, cx);
            app.build_settings_state(window, cx);
        });
    });
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, _, _| {
        assert!(app.active_settings().unwrap().navigation.section() == SettingsSection::General);
    });
}
