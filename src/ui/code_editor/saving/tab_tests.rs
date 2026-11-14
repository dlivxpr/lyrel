use super::*;

#[gpui_kit::test]
fn tab_save_before_close_keeps_the_newly_focused_tab(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("closed-tab.txt");
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
    new_edited_file(&app, &mut vcx);
    let original = app.read_with(&vcx, |app, _| app.tabs[0].id());
    let survivor = app.read_with(&vcx, |app, _| app.tabs[1].id());
    app.update_in(&mut vcx, |app, window, cx| app.close_tab(0, window, cx));
    settle(&mut vcx);
    vcx.simulate_prompt_answer(t(L10nKey::Save));
    settle(&mut vcx);
    app.update_in(&mut vcx, |app, window, cx| app.activate(1, window, cx));
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            app.tabs.iter().all(|tab| tab.id() != original)
        })
    });
    app.read_with(&vcx, |app, _| {
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs[0].id(), survivor);
    });
    assert_eq!(std::fs::read_to_string(path).unwrap(), "original edits");
}

#[gpui_kit::test]
fn failed_save_does_not_close_the_tab_on_a_later_manual_save(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let occupied = dir.path().join("directory");
    let recovered = dir.path().join("recovered-tab.txt");
    std::fs::create_dir(&occupied).unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
    let id = new_edited_file(&app, &mut vcx);
    let original = app.read_with(&vcx, |app, _| app.tabs[0].id());
    app.update_in(&mut vcx, |app, window, cx| app.close_tab(0, window, cx));
    settle(&mut vcx);
    vcx.simulate_prompt_answer(t(L10nKey::Save));
    settle(&mut vcx);
    cx.simulate_new_path_selection(|_| Some(occupied.clone()));
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            app.active_buffer()
                .is_some_and(|file| file.untitled.is_some() && !file.saving)
                && app.editor.waiters.is_empty()
        })
    });
    app.read_with(&vcx, |app, cx| {
        let file = app.active_buffer().unwrap();
        assert_eq!(file.id(), id);
        assert!(file.path.as_os_str().is_empty());
        assert!(file.dirty);
        assert_eq!(file.input.text().unwrap().read(cx).text().to_string(), "original edits");
    });
    assert!(occupied.is_dir());
    assert_eq!(app.read_with(&vcx, |app, _| app.tabs.len()), 2);
    vcx.dispatch_action(EditorSaveAs);
    settle(&mut vcx);
    cx.simulate_new_path_selection(|_| Some(recovered.clone()));
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            app.active_buffer()
                .is_some_and(|file| !file.dirty && !file.saving)
        })
    });
    app.read_with(&vcx, |app, _| {
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.tabs[0].id(), original);
        let file = app.active_buffer().unwrap();
        assert_eq!(file.id(), id);
        assert_eq!(file.path, recovered);
        assert!(file.untitled.is_none());
    });
    assert_eq!(
        std::fs::read_to_string(recovered).unwrap(),
        "original edits"
    );
}
