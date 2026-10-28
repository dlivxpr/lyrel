use super::*;
use crate::ui::settings::test_window::harness;
use gpui_kit::TestAppContext;

fn install_editable_theme(cx: &mut App, path: &std::path::Path) -> presets::Theme {
    let mut theme = presets::all(cx).remove(0);
    theme.id = "settings-test-theme".into();
    theme.path = Some(path.to_path_buf());
    theme.image = None;
    presets::write_theme_file(&theme).unwrap();
    let mut themes = presets::all(cx);
    themes.retain(|candidate| candidate.id != theme.id);
    themes.push(theme.clone());
    cx.set_global(presets::Themes(themes));
    cx.global_mut::<Config>().theme_follow_system = false;
    cx.global_mut::<Config>().theme_preset = theme.id.clone();
    theme
}

#[gpui_kit::test]
fn theme_edits_preview_without_writing_and_cancel_restores_original(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("draft.yaml");
    let (app, mut vcx) = harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        let theme = install_editable_theme(cx, &path);
        let original_file = std::fs::read(&path).unwrap();
        app.open_settings_section(SettingsSection::Appearance, window, cx);
        app.edit_active_theme(ThemeEdit::Accent, rgb(0x123456).into(), window, cx);
        assert!(app.theme_draft_dirty());
        assert_eq!(presets::by_id(cx, &theme.id).accent, 0x123456);
        assert_eq!(std::fs::read(&path).unwrap(), original_file);
        app.cancel_theme_draft(window, cx);
        assert!(!app.theme_draft_dirty());
        assert_eq!(presets::by_id(cx, &theme.id).accent, theme.accent);
        app.edit_active_theme(ThemeEdit::Accent, rgb(0x654321).into(), window, cx);
        assert!(app.save_theme_draft(window, cx));
        let saved_file = std::fs::read(&path).unwrap();
        assert_ne!(saved_file, original_file);
        assert!(!app.theme_draft_dirty());
        app.edit_active_theme(ThemeEdit::Accent, rgb(0x102030).into(), window, cx);
        // A real filesystem failure, without exposing the owner's draft.
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(!app.save_theme_draft(window, cx));
        assert!(app.theme_draft_dirty());
        assert!(app.active_settings().unwrap().theme.error().is_some());
        std::fs::remove_dir(&path).unwrap();
        std::fs::write(&path, saved_file).unwrap();
        assert!(app.save_theme_draft(window, cx));
        assert!(!app.theme_draft_dirty());
        assert!(app.active_settings().unwrap().theme.error().is_none());
        assert!(String::from_utf8(std::fs::read(&path).unwrap()).unwrap().contains("#102030"));
    });
}

#[gpui_kit::test]
fn reverting_theme_edits_clears_dirty_and_image_edits_keep_their_opacity(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let dir = tempfile::tempdir().unwrap();
    let (app, mut vcx) = harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        let path = dir.path().join("draft.yaml");
        let theme = install_editable_theme(cx, &path);
        app.open_settings_section(SettingsSection::Appearance, window, cx);
        app.edit_active_theme(ThemeEdit::Accent, rgb(0x123456).into(), window, cx);
        app.edit_active_theme(ThemeEdit::Accent, rgb(theme.accent).into(), window, cx);
        assert!(!app.theme_draft_dirty());
        let selection = app.active_settings_mut().unwrap().theme.begin_image_selection(&theme.id);
        app.apply_theme_image_selection(selection, dir.path().join("first.png"), window, cx);
        assert_eq!(presets::by_id(cx, &theme.id).image.unwrap().opacity, 0.3);
        app.set_theme_image_opacity(0.67, window, cx);
        let selection = app.active_settings_mut().unwrap().theme.begin_image_selection(&theme.id);
        app.apply_theme_image_selection(selection, dir.path().join("second.png"), window, cx);
        let image = presets::by_id(cx, &theme.id).image.unwrap();
        assert_eq!(image.path, dir.path().join("second.png"));
        assert_eq!(image.opacity, 0.67);
        app.set_theme_image_opacity(2.0, window, cx);
        assert_eq!(presets::by_id(cx, &theme.id).image.unwrap().opacity, 1.0);
        app.remove_theme_image(window, cx);
        assert!(!app.theme_draft_dirty());
        assert!(presets::by_id(cx, &theme.id).image.is_none());
        assert_eq!(std::fs::read_to_string(path).unwrap(), presets::to_yaml(&theme));
    });
}

#[gpui_kit::test]
fn late_theme_image_results_cannot_edit_an_abandoned_context(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let dir = tempfile::tempdir().unwrap();
    let (app, mut vcx) = harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        let path = dir.path().join("draft.yaml");
        let theme = install_editable_theme(cx, &path);
        let original_file = std::fs::read(&path).unwrap();
        for retirement in 0..4 {
            app.open_settings_section(SettingsSection::Appearance, window, cx);
            let selection = app.active_settings_mut().unwrap().theme.begin_image_selection(&theme.id);
            match retirement {
                0 => app.navigate_settings(SettingsSection::General, None, window, cx),
                1 => app.cancel_theme_draft(window, cx),
                2 => {
                    app.set_preset("dark", window, cx);
                    app.set_preset(&theme.id, window, cx);
                }
                _ => app.build_settings_state(window, cx),
            }
            app.apply_theme_image_selection(selection, dir.path().join("abandoned.png"), window, cx);
            assert!(!app.theme_draft_dirty(), "retirement {retirement}");
            assert!(presets::by_id(cx, &theme.id).image.is_none());
            assert_eq!(std::fs::read(&path).unwrap(), original_file);
        }
        let first = app.active_settings_mut().unwrap().theme.begin_image_selection(&theme.id);
        let current = app.active_settings_mut().unwrap().theme.begin_image_selection(&theme.id);
        app.apply_theme_image_selection(first, dir.path().join("old.png"), window, cx);
        assert!(!app.theme_draft_dirty());
        app.apply_theme_image_selection(current, dir.path().join("current.png"), window, cx);
        assert_eq!(presets::by_id(cx, &theme.id).image.unwrap().path, dir.path().join("current.png"));
        app.cancel_theme_draft(window, cx);
    });
}

#[gpui_kit::test]
fn closed_theme_session_rejects_picker_results_after_reopen(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let dir = tempfile::tempdir().unwrap();
    let (app, mut vcx) = harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        let path = dir.path().join("draft.yaml");
        let theme = install_editable_theme(cx, &path);
        app.open_settings_section(SettingsSection::Appearance, window, cx);
        let selection = app.active_settings_mut().unwrap().theme.begin_image_selection(&theme.id);
        app.close_settings(window, cx);
        app.edit_active_theme(ThemeEdit::Accent, rgb(0x123456).into(), window, cx);
        assert_eq!(presets::by_id(cx, &theme.id).accent, theme.accent);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), presets::to_yaml(&theme));
        app.open_settings_section(SettingsSection::Appearance, window, cx);
        app.apply_theme_image_selection(selection, dir.path().join("closed.png"), window, cx);
        assert!(!app.theme_draft_dirty());
        assert!(presets::by_id(cx, &theme.id).image.is_none());
    });
}

#[gpui_kit::test]
fn replaced_color_controls_cannot_edit_the_current_theme(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let dir = tempfile::tempdir().unwrap();
    let (app, mut vcx) = harness(cx);
    let original = app.update_in(&mut vcx, |app, window, cx| {
        let theme = install_editable_theme(cx, &dir.path().join("draft.yaml"));
        app.open_settings_section(SettingsSection::Appearance, window, cx);
        let picker = app.active_settings().unwrap().theme.seed().unwrap()[2].1.clone();
        app.rebuild_theme_editor(window, cx);
        picker.update(cx, |_, cx| {
            cx.emit(ColorPickerEvent::Change(Some(rgb(0x123456).into())));
        });
        theme
    });
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, _, cx| {
        assert!(!app.theme_draft_dirty());
        assert_eq!(presets::by_id(cx, &original.id).accent, original.accent);
        let picker = app.active_settings().unwrap().theme.seed().unwrap()[2].1.clone();
        picker.update(cx, |_, cx| {
            cx.emit(ColorPickerEvent::Change(Some(rgb(0x654321).into())));
        });
    });
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(app.theme_draft_dirty());
        assert_eq!(presets::by_id(cx, &original.id).accent, 0x654321);
        app.cancel_theme_draft(window, cx);
    });
}

#[gpui_kit::test]
fn theme_modes_remember_slots_without_changing_fonts_or_language(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let (app, mut vcx) = harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_settings_section(SettingsSection::Appearance, window, cx);
        {
            let config = cx.global_mut::<Config>();
            config.theme_follow_system = false;
            config.theme_preset = "dracula".into();
            config.theme_preset_light = "one_light".into();
            config.theme_preset_dark = "dark".into();
        }
        let before = cx.global::<Config>().clone();
        app.set_theme_follow_system(true, window, cx);
        assert!(cx.global::<Config>().theme_follow_system);
        assert_eq!(cx.global::<Config>().theme_preset_dark, "dracula");
        app.set_theme_mode(ThemeMode::Light, window, cx);
        assert!(!cx.global::<Config>().theme_follow_system);
        assert_eq!(cx.global::<Config>().theme_preset, "one_light");
        app.pick_slot_theme(false, "catppuccin_latte", window, cx);
        assert_eq!(cx.global::<Config>().theme_preset, "catppuccin_latte");
        assert_eq!(cx.global::<Config>().theme_preset_light, "catppuccin_latte");
        app.set_theme_mode(ThemeMode::System, window, cx);
        app.set_theme_mode(ThemeMode::Dark, window, cx);
        let config = cx.global::<Config>();
        assert!(!config.theme_follow_system);
        assert_eq!(config.theme_preset, "dracula");
        assert_eq!(config.theme_preset_light, "catppuccin_latte");
        assert_eq!(config.font_family, before.font_family);
        assert_eq!(config.ui_font_family, before.ui_font_family);
        assert_eq!(config.gui_language, before.gui_language);
    });
}
