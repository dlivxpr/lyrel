use super::super::super::test_host::FileHost;
use super::*;
use std::sync::atomic::Ordering;

#[gpui_kit::test]
fn in_flight_save_closes_only_its_original_tab_after_focus_changes(cx: &mut TestAppContext) {
    in_flight_save_scenario(cx, false);
}

#[gpui_kit::test]
fn markdown_in_flight_save_closes_only_original_tab(cx: &mut TestAppContext) {
    in_flight_save_scenario(cx, true);
}

fn in_flight_save_scenario(cx: &mut TestAppContext, markdown: bool) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(if markdown {
        "in-flight.md"
    } else {
        "in-flight.txt"
    });
    std::fs::write(&path, "before").unwrap();
    let host = FileHost::new(HostId(700));
    let release = host.pause_next_write();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
    if markdown {
        vcx.update(|_, cx| cx.set_global(MarkdownWorkbench));
    }
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_on_host(host.clone(), &path, window, cx)
    });
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| app.active_buffer().is_some())
    });
    vcx.update(|window, cx| {
        window.press("secondary-a", cx);
        window.input("saved original", cx);
    });
    settle(&mut vcx);
    let original = app.read_with(&vcx, |app, _| app.tabs[0].id());
    let survivor = app.read_with(&vcx, |app, _| app.tabs[1].id());
    app.update_in(&mut vcx, |app, window, cx| app.close_tab(0, window, cx));
    settle(&mut vcx);
    vcx.simulate_prompt_answer(t(L10nKey::Save));
    wait_until(&mut vcx, |_| host.entered.load(Ordering::SeqCst));
    app.update_in(&mut vcx, |app, window, cx| app.activate(1, window, cx));
    let replacement = new_edited_file(&app, &mut vcx);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "before");
    release.send(()).unwrap();
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            app.tabs.iter().all(|tab| tab.id() != original)
        })
    });
    app.read_with(&vcx, |app, _| {
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs[0].id(), survivor);
        assert_eq!(app.tab_code().unwrap().active_id(), Some(replacement));
        assert!(app.active_buffer().unwrap().dirty);
    });
    assert_eq!(std::fs::read_to_string(path).unwrap(), "saved original");
}

#[gpui_kit::test]
fn remote_save_as_bar_keeps_its_host_and_local_shared_path_independent(cx: &mut TestAppContext) {
    remote_save_as_scenario(cx, false);
}

#[gpui_kit::test]
fn markdown_remote_save_as_and_later_saves_remain_on_original_host(cx: &mut TestAppContext) {
    remote_save_as_scenario(cx, true);
}

fn remote_save_as_scenario(cx: &mut TestAppContext, markdown: bool) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(if markdown {
        "same-name.md"
    } else {
        "same-name.txt"
    });
    let target = dir.path().join(if markdown {
        "remote-save-as.md"
    } else {
        "remote-save-as.txt"
    });
    std::fs::write(&path, "unchanged local").unwrap();
    let host = FileHost::new(HostId(701));
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    if markdown {
        vcx.update(|_, cx| cx.set_global(MarkdownWorkbench));
    }
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| app.active_buffer().is_some())
    });
    let local = app.read_with(&vcx, |app, _| app.tab_code().unwrap().active_id().unwrap());
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_on_host(host.clone(), &path, window, cx)
    });
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            app.editor_active_location()
                .is_some_and(|(host, _)| host == HostId(701))
        })
    });
    vcx.update(|window, cx| {
        window.press("secondary-a", cx);
        window.input("remote edits", cx);
    });
    settle(&mut vcx);
    vcx.dispatch_action(EditorSaveAs);
    settle(&mut vcx);
    vcx.update(|window, cx| {
        window.press("secondary-a", cx);
        window.input(&target.display().to_string(), cx);
        window.press("enter", cx);
    });
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            app.active_buffer()
                .is_some_and(|file| file.path == target && !file.dirty && !file.saving)
        })
    });
    app.read_with(&vcx, |app, cx| {
        assert_eq!(app.active_buffer().unwrap().host.id(), HostId(701));
        assert_eq!(
            app.editor
                .buffers
                .get(local)
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string(),
            "unchanged local"
        );
    });
    assert_eq!(std::fs::read_to_string(path).unwrap(), "unchanged local");
    assert_eq!(std::fs::read_to_string(target).unwrap(), "remote edits");
    assert_eq!(host.writes.load(Ordering::SeqCst), 1);
    vcx.update(|window, cx| window.input("!", cx));
    settle(&mut vcx);
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_save_active(window, cx)
    });
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            !app.active_buffer().unwrap().dirty && !app.active_buffer().unwrap().saving
        })
    });
    assert_eq!(host.writes.load(Ordering::SeqCst), 2);
    app.read_with(&vcx, |app, _| {
        assert!(
            app.editor
                .local_file(app.active_buffer().unwrap().id())
                .is_none()
        )
    });
}

#[gpui_kit::test]
fn a_delayed_save_cannot_close_a_replacement_editor_group(cx: &mut TestAppContext) {
    replacement_group_scenario(cx, false);
}

#[gpui_kit::test]
fn markdown_delayed_save_cannot_close_replacement_editor_group(cx: &mut TestAppContext) {
    replacement_group_scenario(cx, true);
}

fn replacement_group_scenario(cx: &mut TestAppContext, markdown: bool) {
    let dir = tempfile::tempdir().unwrap();
    let root = crate::ui::code_editor::tests::test_real_dir(dir.path());
    let left_path = root.join(if markdown { "left.md" } else { "left.txt" });
    let path = root.join(if markdown { "saved.md" } else { "saved.txt" });
    std::fs::write(&left_path, "left").unwrap();
    std::fs::write(&path, "before").unwrap();
    let host = FileHost::new(HostId(702));
    let release = host.pause_next_write();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    if markdown {
        vcx.update(|_, cx| cx.set_global(MarkdownWorkbench));
    }
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&left_path, window, cx);
    });
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| app.active_buffer().is_some())
    });
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_split(window, cx);
        app.editor_open_on_host(host.clone(), &path, window, cx);
    });
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            app.active_buffer().is_some_and(|file| file.path == path)
        })
    });
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_close_file(0, window, cx);
    });
    vcx.update(|window, cx| {
        window.press("secondary-a", cx);
        window.input("saved original", cx);
    });
    settle(&mut vcx);
    let id = app.read_with(&vcx, |app, _| app.tab_code().unwrap().active_id().unwrap());
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_close_file(0, window, cx);
    });
    settle(&mut vcx);
    vcx.simulate_prompt_answer(t(L10nKey::Save));
    wait_until(&mut vcx, |_| host.entered.load(Ordering::SeqCst));
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_focus_group(true, window, cx);
        app.editor_open_on_host(host.clone(), &path, window, cx);
        app.editor_focus_group(false, window, cx);
        app.editor_close_file(0, window, cx);
        assert!(!app.tab_code().unwrap().is_split());
        app.editor_split(window, cx);
        assert_eq!(app.tab_code().unwrap().views_of(id), 2);
    });
    release.send(()).unwrap();
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            app.editor.buffers.get(id).is_some_and(|file| !file.saving)
        })
    });
    app.read_with(&vcx, |app, cx| {
        assert_eq!(app.tab_code().unwrap().views_of(id), 2);
        assert_eq!(
            app.editor
                .buffers
                .get(id)
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string(),
            "saved original"
        );
    });
    assert_eq!(std::fs::read_to_string(path).unwrap(), "saved original");
}

#[gpui_kit::test]
fn markdown_remote_save_as_rejects_identity_changed_while_bar_was_open(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("original.md");
    let moved = dir.path().join("moved.md");
    let target = dir.path().join("stale-target.md");
    std::fs::write(&path, "original").unwrap();
    let host = FileHost::new(HostId(703));
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| cx.set_global(MarkdownWorkbench));
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_on_host(host.clone(), &path, window, cx)
    });
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| app.active_buffer().is_some())
    });
    vcx.dispatch_action(EditorSaveAs);
    settle(&mut vcx);
    std::fs::rename(&path, &moved).unwrap();
    app.update_in(&mut vcx, |app, _, cx| {
        app.editor_path_moved(HostId(703), &path, &moved, cx)
    });
    vcx.update(|window, cx| {
        window.press("secondary-a", cx);
        window.input(&target.display().to_string(), cx);
        window.press("enter", cx);
    });
    settle(&mut vcx);
    assert!(!target.exists());
    assert_eq!(host.writes.load(Ordering::SeqCst), 0);
    app.read_with(&vcx, |app, cx| {
        let file = app.active_buffer().unwrap();
        assert_eq!(file.path, moved);
        assert_eq!(file.host.id(), HostId(703));
        assert_eq!(file.input.read(cx).text().to_string(), "original");
    });
}
