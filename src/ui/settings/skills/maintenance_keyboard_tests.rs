
#[gpui_kit::test]
fn unknown_source_and_remove_review_remain_keyboard_reachable_in_both_locales(cx: &mut gpui_kit::TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let installation = project.join(".agents/skills/unknown");
    std::fs::create_dir_all(&installation).unwrap();
    let content = "---\nname: unknown\ndescription: No inferred update URL\n---\n# Keep\n";
    std::fs::write(installation.join("SKILL.md"), content).unwrap();
    let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
    app.update_in(&mut vcx, |app, window, cx| {
        super::super::tests::isolate_native_roots(&temp.path().join("home"), cx);
        app.open_settings_section(SettingsSection::Skills, window, cx);
        app.active_settings().unwrap().skills.project_input.clone().update(cx, |input,cx|input.set_value(project.to_string_lossy().into_owned(),window,cx));
        app.apply_skills_project(cx);
    });
    settle(&app, &mut vcx);
    for (locale,width,mode) in [("en",1100.,gpui_kit::component::ThemeMode::Light),("zh-CN",680.,gpui_kit::component::ThemeMode::Dark)] {
        crate::ui::i18n::set_locale(locale);
        vcx.simulate_resize(gpui_kit::size(px(width),px(720.)));
        app.update_in(&mut vcx, |app,window,cx| {
            gpui_kit::component::Theme::change(mode,Some(window),cx);
            gpui_kit::component::Theme::update(cx,|theme|theme.font_size=px(20.));
            let s = app.active_settings_mut().unwrap();
            s.skills.selected = Some(std::fs::canonicalize(&installation).unwrap());
            window.focus(&s.skills.focus,cx);
            cx.notify();
        });
        // Real Tab navigation must reach the command even when it needs scrolling.
        let mut found = false;
        for _ in 0..80 {
            if vcx.update(|window,cx| { window.render_frame(cx); window.find("skills-check-update").focused()==Some(true) }) { found = true; break; }
            vcx.simulate_keystrokes("tab");
        }
        assert!(found,"{locale}: update check must be keyboard reachable");
        vcx.simulate_keystrokes("enter");
        settle(&app,&mut vcx);
        app.update_in(&mut vcx,|app,_,_|assert!(matches!(app.active_settings().unwrap().skills.maintenance.status,Some(UpdateStatus::UnknownSource(_)))));
        vcx.simulate_keystrokes("escape");
        app.update_in(&mut vcx,|app,_,_|assert!(app.active_settings().is_some_and(|s|s.section==SettingsSection::Skills && s.skills.selected.is_some())));
        vcx.update(|window,cx| {
            window.render_frame(cx);
            let top=window.find("skills-remove-review").bounds().top();
            let scroll=&app.read(cx).active_settings().unwrap().content_scroll;
            let offset=scroll.offset();
            scroll.set_offset(gpui_kit::point(offset.x,offset.y-top+px(200.)));
            window.render_frame(cx);
            let trigger=window.find("skills-remove-review");
            assert!(trigger.visible());
            assert!(trigger.bounds().size.width < px(width));
            window.click("skills-remove-review",cx);
        });
        settle(&app,&mut vcx);
        vcx.update(|window,cx| {
            window.render_frame(cx);
            let top=window.find("skills-remove-confirm").bounds().top();
            let scroll=&app.read(cx).active_settings().unwrap().content_scroll;
            let offset=scroll.offset();
            scroll.set_offset(gpui_kit::point(offset.x,offset.y-top+px(220.)));
            window.render_frame(cx);
            assert!(window.find("skills-remove-confirm").visible());
            window.press("escape",cx);
        });
        assert_eq!(std::fs::read_to_string(installation.join("SKILL.md")).unwrap(),content);
        assert!(!project.join("skills-lock.json").exists());
    }
    crate::ui::i18n::set_locale("en");
}
