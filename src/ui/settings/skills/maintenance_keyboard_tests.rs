use super::*;
#[gpui_kit::test]
fn unknown_source_and_remove_review_remain_keyboard_reachable_in_both_locales(
    cx: &mut gpui_kit::TestAppContext,
) {
    let temp = tempfile::tempdir().unwrap();
    let root = lyrel_core::core::path_spelling::local_spelling_buf(
        std::fs::canonicalize(temp.path()).unwrap(),
    );
    let project = root.join("project");
    let installation = project.join(".agents/skills/unknown");
    std::fs::create_dir_all(&installation).unwrap();
    let content = "---\nname: unknown\ndescription: No inferred update URL\n---\n# Keep\n";
    std::fs::write(installation.join("SKILL.md"), content).unwrap();
    let (app, mut vcx) = crate::ui::settings::test_window::harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        super::super::super::tests::isolate_native_roots(&root.join("home"), cx);
        app.open_settings_section(SettingsSection::Skills, window, cx);
        app.active_settings()
            .unwrap()
            .skills
            .project_input
            .clone()
            .update(cx, |input, cx| {
                input.set_value(project.to_string_lossy().into_owned(), window, cx)
            });
        app.apply_skills_project(cx);
    });
    settle(&app, &mut vcx);
    vcx.simulate_resize(gpui_kit::size(px(1200.), px(1800.)));
    let selected = app.update_in(&mut vcx, |app, _, _| {
        app.active_settings()
            .unwrap()
            .skills
            .snapshot
            .as_ref()
            .unwrap()
            .filtered("unknown", Some(SourceKind::ProjectShared))
            .next()
            .unwrap()
            .path()
            .to_path_buf()
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        super::super::super::tests::reveal(
            window,
            cx,
            SharedString::from(row_id(&selected)),
            "skills-list-scroll",
        );
        window.click(SharedString::from(row_id(&selected)), cx);
    });
    for (locale, width, mode) in [
        ("en", 1100., gpui_kit::component::ThemeMode::Light),
        ("zh-CN", 680., gpui_kit::component::ThemeMode::Dark),
    ] {
        crate::ui::i18n::set_locale(locale);
        vcx.simulate_resize(gpui_kit::size(px(width), px(720.)));
        app.update_in(&mut vcx, |app, window, cx| {
            gpui_kit::component::Theme::change(mode, Some(window), cx);
            gpui_kit::component::Theme::update(cx, |theme| theme.font_size = px(20.));
            let s = app.active_settings_mut().unwrap();
            window.focus(&s.skills.focus, cx);
            cx.notify();
        });
        // Real Tab navigation must reach the command even when it needs scrolling.
        let mut found = false;
        for _ in 0..80 {
            if vcx.update(|window, cx| {
                window.render_frame(cx);
                window.find("skills-check-update").focused() == Some(true)
            }) {
                found = true;
                break;
            }
            vcx.update(|window, cx| window.press("tab", cx));
        }
        assert!(found, "{locale}: update check must be keyboard reachable");
        vcx.update(|window, cx| window.press("enter", cx));
        settle(&app, &mut vcx);
        app.update_in(&mut vcx, |app, _, _| {
            let state = &app.active_settings().unwrap().skills.maintenance;
            assert!(
                matches!(state.status, Some(UpdateStatus::UnknownSource(_))),
                "{locale}: status={:?}, error={:?}, busy={}",
                state.status,
                state.error,
                state.busy
            )
        });
        vcx.update(|window, cx| window.press("escape", cx));
        app.update_in(&mut vcx, |app, _, _| {
            assert!(app.active_settings().is_some_and(
                |s| s.section == SettingsSection::Skills && s.skills.selected.is_some()
            ))
        });
        vcx.update(|window, cx| {
            window.render_frame(cx);
            super::super::super::tests::reveal(
                window,
                cx,
                "skills-remove-review",
                "skills-detail-scroll",
            );
            let trigger = window.find("skills-remove-review");
            assert!(trigger.visible());
            assert!(trigger.bounds().size.width < px(width));
            window.click("skills-remove-review", cx);
        });
        settle(&app, &mut vcx);
        vcx.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.find("skills-remove-confirm").visible());
            window.press("secondary-f", cx);
            assert!(
                !app.read(cx)
                    .active_settings()
                    .unwrap()
                    .search
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
            );
            window.press("escape", cx);
        });
        assert_eq!(
            std::fs::read_to_string(installation.join("SKILL.md")).unwrap(),
            content
        );
        assert!(!project.join("skills-lock.json").exists());
    }
    crate::ui::i18n::set_locale("en");
}

#[gpui_kit::test]
fn skills_host_and_source_pickers_preserve_keyboard_selection_and_escape(
    cx: &mut gpui_kit::TestAppContext,
) {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    std::fs::create_dir_all(project.join(".agents/skills/project-only")).unwrap();
    std::fs::write(
        project.join(".agents/skills/project-only/SKILL.md"),
        "---\nname: project-only\n---\n# Project",
    )
    .unwrap();
    let (app, mut vcx) = crate::ui::settings::test_window::harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        super::super::super::tests::isolate_native_roots(&temp.path().join("home"), cx);
        app.open_settings_section(SettingsSection::Skills, window, cx);
        let s = app.active_settings_mut().unwrap();
        s.skills.project_input.clone().update(cx, |input, cx| {
            input.set_value(project.to_string_lossy().into_owned(), window, cx)
        });
        app.apply_skills_project(cx);
    });
    settle(&app, &mut vcx);
    vcx.simulate_resize(gpui_kit::size(px(1200.), px(1400.)));
    app.update_in(&mut vcx, |app, window, cx| {
        window.focus(&app.active_settings().unwrap().skills.focus, cx)
    });
    for (id, keys) in [
        ("skills-machine", "enter escape"),
        ("skills-source", "enter down down down enter"),
    ] {
        let mut reached = false;
        for _ in 0..100 {
            if vcx.update(|window, cx| {
                window.render_frame(cx);
                window.find(id).focused() == Some(true)
            }) {
                reached = true;
                break;
            }
            vcx.update(|window, cx| window.press("tab", cx));
        }
        assert!(reached, "{id} must be Tab-reachable");
        vcx.simulate_keystrokes(keys);
        vcx.run_until_parked();
    }
    assert_eq!(
        app.read_with(&vcx, |app, _| app.active_settings().unwrap().skills.filter),
        Some(SourceKind::ProjectShared)
    );
    assert_eq!(
        std::fs::read_to_string(project.join(".agents/skills/project-only/SKILL.md")).unwrap(),
        "---\nname: project-only\n---\n# Project"
    );
}
