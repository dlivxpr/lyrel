use super::*;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext};
use crate::ui::app::test_window::harness_with_tabs;
use super::super::test_host::FileHost;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};


fn wait_until(vcx: &mut VisualTestContext, mut ready: impl FnMut(&mut VisualTestContext) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        vcx.run_until_parked();
        if ready(vcx) { return; }
        assert!(Instant::now() < deadline, "editor workflow did not settle");
        vcx.executor().advance_clock(Duration::from_millis(250));
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn active_id(app: &Entity<LyrelApp>, vcx: &VisualTestContext) -> Option<BufferId> {
    app.read_with(vcx, |app, _| app.tab_code().and_then(TabCode::active_id))
}

#[gpui_kit::test]
fn opening_across_tabs_shares_edits_and_closing_one_presentation_keeps_them(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shared.txt");
    std::fs::write(&path, "first\nsecond\nthird\n").unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
    app.update_in(&mut vcx, |app, window, cx| app.open_file_in_editor_at(&path, Some(2), Some(3), window, cx));
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    let first = active_id(&app, &vcx).unwrap();
    app.read_with(&vcx, |app, cx| {
        assert_eq!(app.active_buffer().unwrap().input.read(cx).cursor_position(), Position::new(1, 2));
    });
    vcx.update(|window, cx| { window.render_frame(cx); window.input("edited", cx); });
    vcx.run_until_parked();
    let edited = app.read_with(&vcx, |app, cx| app.active_buffer().unwrap().input.read(cx).text().to_string());
    assert!(edited.contains("edited"));
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
        assert_eq!(app.active_buffer().unwrap().input.read(cx).text().to_string(), edited);
        assert_eq!(app.editor_unsaved(), vec![first]);
    });
}

#[gpui_kit::test]
fn equal_paths_on_distinct_hosts_do_not_share_buffers_or_local_lsp_documents(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("identity.txt");
    std::fs::write(&path, "same path\n").unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut vcx, |app, window, cx| app.open_file_in_editor(&path, window, cx));
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    let local = active_id(&app, &vcx).unwrap();
    let remote: SharedHost = FileHost::new(HostId(69));
    app.update_in(&mut vcx, |app, window, cx| app.editor_open_on_host(remote, &path, window, cx));
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some_and(|id| id != local));
    app.read_with(&vcx, |app, _| {
        assert_eq!(app.editor.local_files().iter().map(|file| file.id).collect::<Vec<_>>(), vec![local]);
        let remote = app.active_buffer().unwrap();
        assert_eq!(app.editor_active_location().unwrap().0, HostId(69));
        assert_eq!(remote.path, app.editor.local_file(local).unwrap().path);
    });
}

#[gpui_kit::test]
fn external_writes_reload_clean_text_but_preserve_unsaved_edits(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("watched.txt");
    std::fs::write(&path, "initial\n").unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut vcx, |app, window, cx| app.open_file_in_editor(&path, window, cx));
    wait_until(&mut vcx, |vcx| app.read_with(vcx, |app, _| app.editor.buffers.watch.is_some()));
    std::fs::write(&path, "external version\n").unwrap();
    wait_until(&mut vcx, |vcx| app.read_with(vcx, |app, cx| app.active_buffer().is_some_and(|file| file.input.read(cx).text().to_string() == "external version\n")));
    vcx.update(|window, cx| { window.render_frame(cx); window.input("unsaved", cx); });
    vcx.run_until_parked();
    let text = app.read_with(&vcx, |app, cx| app.active_buffer().unwrap().input.read(cx).text().to_string());
    assert!(text.contains("unsaved"));
    std::fs::write(&path, "another external version\n").unwrap();
    wait_until(&mut vcx, |vcx| app.read_with(vcx, |app, _| app.active_buffer().unwrap().conflict.is_some()));
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
    app.update_in(&mut vcx, |app, window, cx| app.editor_open_on_host(host.clone(), &path, window, cx));
    wait_until(&mut vcx, |_| host.entered.load(Ordering::SeqCst));
    app.update_in(&mut vcx, |app, window, cx| app.close_tab(0, window, cx));
    release.send(()).unwrap();
    wait_until(&mut vcx, |vcx| app.read_with(vcx, |app, _| app.editor.buffers.pending.is_empty()));
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
    assert_eq!(classify_external_change(false, false, ours, seen(ours)), ExternalChange::Ignore);
    assert_eq!(classify_external_change(false, false, ours, seen(mt(101, 0))), ExternalChange::Reload);
    assert_eq!(classify_external_change(false, true, ours, seen(mt(101, 0))), ExternalChange::Conflict);
    assert_eq!(classify_external_change(false, false, mt(100, 0), seen(mt(100, 1))), ExternalChange::Reload);
    assert_eq!(classify_external_change(true, false, ours, seen(mt(101, 0))), ExternalChange::Ignore);
    assert_eq!(classify_external_change(false, false, None, seen(None)), ExternalChange::Reload);
}

#[test]
fn a_file_that_vanished_is_reported_deleted_whether_or_not_it_was_edited() {
    assert_eq!(classify_external_change(false, false, mt(1, 0), Observed::Missing), ExternalChange::Deleted);
    assert_eq!(classify_external_change(false, true, mt(1, 0), Observed::Missing), ExternalChange::Deleted);
    // Our save replaces by rename; the watcher can catch the gap between.
    assert_eq!(classify_external_change(true, false, mt(1, 0), Observed::Missing), ExternalChange::Ignore);
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
    wait_until(&mut vcx, |vcx| app.read_with(vcx, |app, _| app.editor_active_location().is_some_and(|(host, _)| host == HostId(71))));
    release.send(()).unwrap();
    wait_until(&mut vcx, |vcx| app.read_with(vcx, |app, _| app.editor.buffers.pending.is_empty()));
    app.read_with(&vcx, |app, cx| {
        assert_eq!(app.editor_active_location().unwrap().0, HostId(71));
        assert_eq!(app.active_buffer().unwrap().input.read(cx).cursor_position(), Position::new(1, 3));
        assert_eq!(app.tab_code().unwrap().files.len(), 2, "the older request may finish only in the background");
    });
}
