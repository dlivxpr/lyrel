use super::super::test_host::FileHost;
use super::*;
use crate::ui::app::test_window::harness_with_tabs;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

fn wait_until(vcx: &mut VisualTestContext, mut ready: impl FnMut(&mut VisualTestContext) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        vcx.run_until_parked();
        if ready(vcx) {
            return;
        }
        assert!(Instant::now() < deadline, "editor workflow did not settle");
        vcx.executor().advance_clock(Duration::from_millis(250));
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn active_id(app: &Entity<LyrelApp>, vcx: &VisualTestContext) -> Option<BufferId> {
    app.read_with(vcx, |app, _| app.tab_code().and_then(TabCode::active_id))
}

#[gpui_kit::test]
fn opening_across_tabs_shares_edits_and_closing_one_presentation_keeps_them(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shared.txt");
    std::fs::write(&path, "first\nsecond\nthird\n").unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor_at(&path, Some(2), Some(3), window, cx)
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    let first = active_id(&app, &vcx).unwrap();
    app.read_with(&vcx, |app, cx| {
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .cursor_position(),
            Position::new(1, 2)
        );
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.input("edited", cx);
    });
    vcx.run_until_parked();
    let edited = app.read_with(&vcx, |app, cx| {
        app.active_buffer()
            .unwrap()
            .input
            .read(cx)
            .text()
            .to_string()
    });
    assert_eq!(edited, "first\nseeditedcond\nthird\n");
    app.update_in(&mut vcx, |app, window, cx| {
        app.activate(1, window, cx);
        app.open_file_in_editor(&path, window, cx);
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    assert_eq!(active_id(&app, &vcx), Some(first));
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.activate(0, window, cx);
    });
    app.read_with(&vcx, |app, cx| {
        assert_eq!(app.tab_code().unwrap().active_id(), Some(first));
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string(),
            edited
        );
        assert_eq!(app.editor_unsaved(), vec![first]);
    });
    app.update_in(&mut vcx, |app, window, cx| app.focus_editor(window, cx));
    vcx.update(|window, cx| window.render_frame(cx));
    vcx.dispatch_action(crate::core::actions::EditorSave);
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            app.active_buffer()
                .is_some_and(|file| !file.dirty && !file.saving)
        })
    });
    assert_eq!(std::fs::read(&path).unwrap(), edited.as_bytes());
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.open_file_in_editor(&path, window, cx);
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    app.read_with(&vcx, |app, cx| {
        let file = app.active_buffer().unwrap();
        assert_ne!(file.id(), first);
        assert_eq!(file.host.id(), HostId::LOCAL);
        assert_eq!(file.input.read(cx).text().to_string(), edited);
        assert!(app.editor_unsaved().is_empty());
    });
}

#[gpui_kit::test]
fn markdown_workbench_shares_native_source_history_and_host_save_across_groups(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shared.mdx");
    std::fs::write(&path, "alpha\nbeta\n").unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| cx.set_global(MarkdownWorkbench));
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    let id = active_id(&app, &vcx).unwrap();
    let first = app.read_with(&vcx, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.input("中🎉", cx);
    });
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(app.editor_split(window, cx));
    });
    vcx.update(|window, cx| window.render_frame(cx));
    let second = app.read_with(&vcx, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    vcx.update(|_, cx| {
        assert_ne!(first.entity_id(), second.entity_id());
        assert_eq!(
            first.read(cx).document().entity_id(),
            second.read(cx).document().entity_id()
        );
        assert_eq!(first.read(cx).selection().head(), 7);
        assert_eq!(second.read(cx).selection().head(), 0);
    });
    vcx.dispatch_action(gpui_kit::component::input::SelectAll);
    vcx.dispatch_action(gpui_kit::component::input::Copy);
    vcx.update(|_, cx| {
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("中🎉alpha\nbeta\n".into())
        )
    });
    vcx.dispatch_action(gpui_kit::component::input::Cut);
    vcx.run_until_parked();
    app.read_with(&vcx, |app, cx| {
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string(),
            ""
        )
    });
    vcx.dispatch_action(gpui_kit::component::input::Undo);
    vcx.dispatch_action(gpui_kit::component::input::MoveToStart);
    vcx.update(|_, cx| cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("\r\n".into())));
    vcx.dispatch_action(gpui_kit::component::input::Paste);
    vcx.run_until_parked();
    app.read_with(&vcx, |app, cx| {
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string(),
            "\n中🎉alpha\nbeta\n"
        )
    });
    vcx.dispatch_action(gpui_kit::component::input::Backspace);
    vcx.run_until_parked();
    app.read_with(&vcx, |app, cx| {
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string(),
            "中🎉alpha\nbeta\n"
        )
    });
    vcx.dispatch_action(gpui_kit::component::input::Undo);
    vcx.dispatch_action(gpui_kit::component::input::Undo);
    vcx.dispatch_action(gpui_kit::component::input::Undo);
    vcx.run_until_parked();
    app.read_with(&vcx, |app, cx| {
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string(),
            "alpha\nbeta\n"
        )
    });
    vcx.dispatch_action(gpui_kit::component::input::Redo);
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        assert_eq!(app.tab_code().unwrap().active_id(), Some(id));
        app.focus_editor(window, cx);
    });
    vcx.update(|window, cx| window.render_frame(cx));
    vcx.dispatch_action(crate::core::actions::EditorSave);
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            app.active_buffer()
                .is_some_and(|file| !file.dirty && !file.saving)
        })
    });
    assert_eq!(
        std::fs::read(&path).unwrap(),
        "中🎉alpha\nbeta\n".as_bytes()
    );
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.open_file_in_editor(&path, window, cx);
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    app.read_with(&vcx, |app, cx| {
        let file = app.active_buffer().unwrap();
        assert_ne!(file.id(), id);
        assert!(file.input.code().is_none());
        assert_eq!(file.input.read(cx).text().to_string(), "中🎉alpha\nbeta\n");
        assert!(app.editor_unsaved().is_empty());
    });
}

#[gpui_kit::test]
fn markdown_native_handler_converts_utf16_and_composition_undo(cx: &mut TestAppContext) {
    use gpui_kit::EntityInputHandler as _;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ime.md");
    std::fs::write(&path, "A🎉Z").unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| cx.set_global(MarkdownWorkbench));
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    let view = app.read_with(&vcx, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        view.update(cx, |view, cx| {
            view.replace_and_mark_text_in_range(Some(1..3), "に🎉", Some(1..3), window, cx);
            assert_eq!(view.marked_text_range(window, cx), Some(1..4));
            assert_eq!(
                view.selected_text_range(false, window, cx).unwrap().range,
                2..4
            );
            view.replace_and_mark_text_in_range(None, "你好", Some(2..2), window, cx);
            view.replace_text_in_range(None, "你好", window, cx);
        });
        window.render_frame(cx);
        view.update(cx, |view, cx| {
            let start = view
                .bounds_for_range(0..0, gpui_kit::Bounds::default(), window, cx)
                .unwrap();
            let committed = view
                .bounds_for_range(3..3, gpui_kit::Bounds::default(), window, cx)
                .unwrap();
            assert!(committed.left() > start.left());
            assert_eq!(committed.top(), start.top());
            assert!(committed.size.height > gpui_kit::px(0.));
        });
    });
    vcx.dispatch_action(gpui_kit::component::input::Undo);
    vcx.run_until_parked();
    app.read_with(&vcx, |app, cx| {
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string(),
            "A🎉Z"
        )
    });
}

#[gpui_kit::test]
fn equal_paths_on_distinct_hosts_do_not_share_buffers_or_local_lsp_documents(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("identity.txt");
    std::fs::write(&path, "same path\n").unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    let local = active_id(&app, &vcx).unwrap();
    let remote: SharedHost = FileHost::new(HostId(69));
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_on_host(remote, &path, window, cx)
    });
    wait_until(&mut vcx, |vcx| {
        active_id(&app, vcx).is_some_and(|id| id != local)
    });
    app.read_with(&vcx, |app, _| {
        assert_eq!(
            app.editor
                .local_files()
                .iter()
                .map(|file| file.id)
                .collect::<Vec<_>>(),
            vec![local]
        );
        let remote = app.active_buffer().unwrap();
        assert_eq!(app.editor_active_location().unwrap().0, HostId(69));
        assert_eq!(remote.path, app.editor.local_file(local).unwrap().path);
    });
}

#[gpui_kit::test]
fn external_writes_reload_clean_text_but_preserve_unsaved_edits(cx: &mut TestAppContext) {
    external_writes_scenario(cx, false);
}

#[gpui_kit::test]
fn markdown_external_writes_reload_clean_text_but_preserve_unsaved_edits(cx: &mut TestAppContext) {
    external_writes_scenario(cx, true);
}

fn external_writes_scenario(cx: &mut TestAppContext, markdown: bool) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(if markdown { "watched.md" } else { "watched.txt" });
    std::fs::write(&path, "initial\n").unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    if markdown { vcx.update(|_, cx| cx.set_global(MarkdownWorkbench)); }
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| app.editor.buffers.watch.is_some())
    });
    std::fs::write(&path, "external version\n").unwrap();
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, cx| {
            app.active_buffer()
                .is_some_and(|file| file.input.read(cx).text().to_string() == "external version\n")
        })
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.input("unsaved", cx);
    });
    vcx.run_until_parked();
    let text = app.read_with(&vcx, |app, cx| {
        app.active_buffer()
            .unwrap()
            .input
            .read(cx)
            .text()
            .to_string()
    });
    assert!(text.contains("unsaved"));
    std::fs::write(&path, "another external version\n").unwrap();
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            app.active_buffer().unwrap().conflict.is_some()
        })
    });
    app.read_with(&vcx, |app, cx| {
        let file = app.active_buffer().unwrap();
        assert!(matches!(file.conflict, Some(DiskConflict::Changed(_))));
        assert_eq!(file.input.read(cx).text().to_string(), text);
        assert!(app.editor.is_dirty(HostId::LOCAL, &file.path));
    });
}

#[gpui_kit::test]
fn an_open_finishing_after_its_tab_closed_does_not_land_in_the_next_tab(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("late.txt");
    std::fs::write(&path, "late contents\n").unwrap();
    let host = FileHost::new(HostId(70));
    let release = host.pause_next_read();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_on_host(host.clone(), &path, window, cx)
    });
    wait_until(&mut vcx, |_| host.entered.load(Ordering::SeqCst));
    app.update_in(&mut vcx, |app, window, cx| app.close_tab(0, window, cx));
    release.send(()).unwrap();
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| app.editor.buffers.pending.is_empty())
    });
    app.read_with(&vcx, |app, _| {
        assert_eq!(app.tabs.len(), 1);
        assert!(app.editor_active_location().is_none());
        assert!(app.tab_code().is_none_or(|code| code.files.is_empty()));
    });
}

fn mt(secs: i64, nanos: u32) -> Option<MTime> {
    Some(MTime { secs, nanos })
}

#[test]
fn external_changes_are_told_apart_from_our_own_saves() {
    let ours = mt(100, 0);
    let seen = |m| Observed::Present(m);
    assert_eq!(
        classify_external_change(false, false, ours, seen(ours)),
        ExternalChange::Ignore
    );
    assert_eq!(
        classify_external_change(false, false, ours, seen(mt(101, 0))),
        ExternalChange::Reload
    );
    assert_eq!(
        classify_external_change(false, true, ours, seen(mt(101, 0))),
        ExternalChange::Conflict
    );
    assert_eq!(
        classify_external_change(false, false, mt(100, 0), seen(mt(100, 1))),
        ExternalChange::Reload
    );
    assert_eq!(
        classify_external_change(true, false, ours, seen(mt(101, 0))),
        ExternalChange::Ignore
    );
    assert_eq!(
        classify_external_change(false, false, None, seen(None)),
        ExternalChange::Reload
    );
}

#[test]
fn a_file_that_vanished_is_reported_deleted_whether_or_not_it_was_edited() {
    assert_eq!(
        classify_external_change(false, false, mt(1, 0), Observed::Missing),
        ExternalChange::Deleted
    );
    assert_eq!(
        classify_external_change(false, true, mt(1, 0), Observed::Missing),
        ExternalChange::Deleted
    );
    // Our save replaces by rename; the watcher can catch the gap between.
    assert_eq!(
        classify_external_change(true, false, mt(1, 0), Observed::Missing),
        ExternalChange::Ignore
    );
}

#[gpui_kit::test]
fn a_late_open_cannot_replace_a_newer_hosts_document_or_cursor(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("navigation.txt");
    std::fs::write(&path, "first\nsecond line\nthird line\n").unwrap();
    let older = FileHost::new(HostId(70));
    let newer: SharedHost = FileHost::new(HostId(71));
    let release = older.pause_next_read();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_on_host_at(older.clone(), &path, Some(Position::new(0, 0)), window, cx);
    });
    wait_until(&mut vcx, |_| older.entered.load(Ordering::SeqCst));
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_on_host_at(newer, &path, Some(Position::new(1, 3)), window, cx);
    });
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            app.editor_active_location()
                .is_some_and(|(host, _)| host == HostId(71))
        })
    });
    release.send(()).unwrap();
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| app.editor.buffers.pending.is_empty())
    });
    app.read_with(&vcx, |app, cx| {
        assert_eq!(app.editor_active_location().unwrap().0, HostId(71));
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .cursor_position(),
            Position::new(1, 3)
        );
        assert_eq!(
            app.tab_code().unwrap().files.len(),
            2,
            "the older request may finish only in the background"
        );
    });
}

#[gpui_kit::test]
fn markdown_session_restore_surfaces_failed_reads_without_losing_valid_files(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let valid = dir.path().join("restored.md");
    let missing = dir.path().join("unavailable.md");
    std::fs::write(&valid, "# restored\n").unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| cx.set_global(MarkdownWorkbench));
    app.update_in(&mut vcx, |app, window, cx| {
        let tab = app.tabs[0].id();
        app.tab_code_mut_or_init();
        app.editor.restored.remove(&tab);
        editor_session::put(cx, tab, TabEditor { files: vec![missing.clone(), valid.clone()], active: 1, visible: true, ..Default::default() });
        app.editor_restore_active(window, cx);
    });
    wait_until(&mut vcx, |vcx| app.read_with(vcx, |app, _| app.active_buffer().is_some()));
    app.read_with(&vcx, |app, cx| {
        let file = app.active_buffer().unwrap();
        assert_eq!(file.path.file_name(), valid.file_name());
        assert_eq!(file.input.read(cx).text().to_string(), "# restored\n");
        assert!(file.input.code().is_none());
        assert_eq!(app.tab_code().unwrap().files.len(), 1);
    });
    wait_until(&mut vcx, |vcx| app.read_with(vcx, |_, cx| {
        crate::ui::notification_history::history(cx).read(cx).records()
            .any(|record| record.message().contains("unavailable.md"))
    }));
}
