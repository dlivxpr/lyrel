use super::*;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext};
use crate::core::actions::{CloseActiveTab, EditorNewFile, EditorSaveAs};
use crate::ui::app::test_window::harness_with_tabs;
use std::time::{Duration, Instant};

#[path = "tab_tests.rs"]
mod tab;
#[path = "host_tests.rs"]
mod host;

fn settle(vcx: &mut VisualTestContext) {
    vcx.run_until_parked();
    vcx.update(|window, cx| window.render_frame(cx));
    vcx.run_until_parked();
}

fn wait_until(vcx: &mut VisualTestContext, mut ready: impl FnMut(&mut VisualTestContext) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        settle(vcx);
        if ready(vcx) { return; }
        assert!(Instant::now() < deadline, "save workflow did not settle");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn new_edited_file(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> BufferId {
    vcx.dispatch_action(EditorNewFile);
    settle(vcx);
    vcx.update(|window, cx| window.input("original edits", cx));
    settle(vcx);
    app.read_with(vcx, |app, _| app.tab_code().unwrap().active_id().unwrap())
}

#[gpui_kit::test]
fn save_before_close_only_removes_the_captured_tab_presentation(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("captured.txt");
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
    let id = new_edited_file(&app, &mut vcx);
    let original = app.read_with(&vcx, |app, _| app.tabs[0].id());
    vcx.dispatch_action(CloseActiveTab);
    settle(&mut vcx);
    assert!(vcx.has_pending_prompt());
    vcx.simulate_prompt_answer(t(L10nKey::Save));
    settle(&mut vcx);
    app.update_in(&mut vcx, |app, window, cx| app.activate(1, window, cx));
    let replacement = new_edited_file(&app, &mut vcx);
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    wait_until(&mut vcx, |vcx| app.read_with(vcx, |app, _| {
        app.tabs.iter().find(|tab| tab.id() == original).unwrap().code.as_deref().unwrap().files.is_empty()
    }));
    assert_eq!(std::fs::read_to_string(path).unwrap(), "original edits");
    app.read_with(&vcx, |app, _| {
        assert_ne!(id, replacement);
        assert_eq!(app.tab_code().unwrap().active_id(), Some(replacement));
        assert!(app.active_buffer().unwrap().dirty);
    });
}

#[gpui_kit::test]
fn cancelling_save_as_retires_close_even_after_a_later_successful_save(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("kept.txt");
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    let id = new_edited_file(&app, &mut vcx);
    vcx.dispatch_action(CloseActiveTab);
    settle(&mut vcx);
    vcx.simulate_prompt_answer(t(L10nKey::Save));
    settle(&mut vcx);
    cx.simulate_new_path_selection(|_| None);
    settle(&mut vcx);
    assert!(app.update_in(&mut vcx, |app, window, cx| app.editor_has_focus(window, cx)));
    vcx.dispatch_action(EditorSaveAs);
    settle(&mut vcx);
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    wait_until(&mut vcx, |vcx| app.read_with(vcx, |app, _| app.active_buffer().is_some_and(|file| !file.dirty && !file.saving)));
    assert_eq!(app.read_with(&vcx, |app, _| app.tab_code().unwrap().active_id()), Some(id));
    assert_eq!(std::fs::read_to_string(path).unwrap(), "original edits");
}

#[gpui_kit::test]
fn failed_save_before_close_keeps_edits_and_cannot_resume_on_a_later_save(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let occupied = dir.path().join("a-directory");
    std::fs::create_dir(&occupied).unwrap();
    let recovered = dir.path().join("recovered.txt");
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    let id = new_edited_file(&app, &mut vcx);
    vcx.dispatch_action(CloseActiveTab);
    settle(&mut vcx);
    vcx.simulate_prompt_answer(t(L10nKey::Save));
    settle(&mut vcx);
    cx.simulate_new_path_selection(|_| Some(occupied.clone()));
    wait_until(&mut vcx, |vcx| app.read_with(vcx, |app, _| app.active_buffer().is_some_and(|file| file.path == occupied && !file.saving)));
    app.read_with(&vcx, |app, cx| {
        let file = app.active_buffer().unwrap();
        assert_eq!(file.id(), id);
        assert!(file.dirty);
        assert_eq!(file.input.read(cx).text().to_string(), "original edits");
    });
    vcx.dispatch_action(EditorSaveAs);
    settle(&mut vcx);
    cx.simulate_new_path_selection(|_| Some(recovered.clone()));
    wait_until(&mut vcx, |vcx| app.read_with(vcx, |app, _| app.active_buffer().is_some_and(|file| !file.dirty && !file.saving)));
    assert_eq!(app.read_with(&vcx, |app, _| app.tab_code().unwrap().active_id()), Some(id));
    assert_eq!(std::fs::read_to_string(recovered).unwrap(), "original edits");
}

#[gpui_kit::test]
fn conflict_drops_original_close_even_when_overwrite_is_accepted(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("conflict.txt");
    std::fs::write(&path, "on disk").unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut vcx, |app, window, cx| app.open_file_in_editor(&path, window, cx));
    wait_until(&mut vcx, |vcx| app.read_with(vcx, |app, _| app.active_buffer().is_some()));
    vcx.update(|window, cx| { window.press("secondary-a", cx); window.input("mine", cx); });
    settle(&mut vcx);
    let id = app.read_with(&vcx, |app, _| app.tab_code().unwrap().active_id().unwrap());
    std::fs::write(&path, "external content").unwrap();
    std::fs::File::options().write(true).open(&path).unwrap().set_times(
        std::fs::FileTimes::new().set_modified(std::time::SystemTime::now() + Duration::from_secs(10))
    ).unwrap();
    vcx.dispatch_action(CloseActiveTab);
    settle(&mut vcx);
    vcx.simulate_prompt_answer(t(L10nKey::Save));
    wait_until(&mut vcx, |vcx| vcx.has_pending_prompt());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "external content");
    vcx.simulate_prompt_answer(t(L10nKey::EditorOverwrite));
    wait_until(&mut vcx, |vcx| app.read_with(vcx, |app, _| app.active_buffer().is_some_and(|file| !file.dirty && !file.saving)));
    assert_eq!(app.read_with(&vcx, |app, _| app.tab_code().unwrap().active_id()), Some(id));
    assert_eq!(std::fs::read_to_string(path).unwrap(), "mine");
}

#[gpui_kit::test]
fn stale_native_save_as_cannot_write_a_replacement_buffer(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stale.txt");
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    let original = new_edited_file(&app, &mut vcx);
    vcx.dispatch_action(EditorSaveAs);
    settle(&mut vcx);
    vcx.dispatch_action(CloseActiveTab);
    settle(&mut vcx);
    vcx.simulate_prompt_answer(t(L10nKey::EditorDiscard));
    settle(&mut vcx);
    let replacement = new_edited_file(&app, &mut vcx);
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    settle(&mut vcx);
    app.read_with(&vcx, |app, _| {
        assert_ne!(original, replacement);
        assert_eq!(app.tab_code().unwrap().active_id(), Some(replacement));
        assert!(app.active_buffer().unwrap().untitled.is_some());
    });
    assert!(!path.exists());
}
    #[gpui_kit::test]
    fn global_status_navigates_and_saves_the_displayed_document(cx: &mut TestAppContext) {
        use crate::core::actions::{EditorNewFile, EditorSaveAs, ToggleDocumentFill};
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        vcx.simulate_resize(gpui_kit::size(px(1600.), px(900.)));
        vcx.dispatch_action(EditorNewFile);
        settle(&mut vcx);
        vcx.update(|window, cx| window.input("one\ntwo\nthree", cx));
        settle(&mut vcx);
        vcx.update(|window, cx| window.click("status-cursor", cx));
        settle(&mut vcx);
        vcx.update(|window, cx| {
            window.press("secondary-a", cx);
            window.input("2:2", cx);
            window.press("enter", cx);
        });
        settle(&mut vcx);
        app.read_with(&vcx, |app, cx| {
            let pos = app
                .active_buffer()
                .unwrap()
                .input
                .read(cx)
                .cursor_position();
            assert_eq!((pos.line, pos.character), (1, 1));
        });
        vcx.update(|window, cx| window.click("status-cursor", cx));
        settle(&mut vcx);
        vcx.simulate_keystrokes("escape");
        settle(&mut vcx);
        assert!(app.update_in(&mut vcx, |app, window, cx| {
            app.code_panel_visible() && app.editor_has_focus(window, cx)
        }));
        for _ in 0..2 {
            vcx.dispatch_action(ToggleDocumentFill);
            settle(&mut vcx);
            vcx.update(|window, cx| {
                let cursor = window.find("status-cursor").bounds();
                let global = window.find("workbench-status").bounds();
                assert!(cursor.top() >= global.top() && cursor.bottom() <= global.bottom());
                assert_eq!(window.find_all("status-cursor").len(), 1);
                window.click("status-wrap", cx);
            });
            settle(&mut vcx);
            assert!(app.update_in(&mut vcx, |app, window, cx| app.editor_has_focus(window, cx)));
        }
        // The line-ending control changes the real save output, not just its label.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("status.txt");
        vcx.update(|window, cx| window.click("status-eol", cx));
        settle(&mut vcx);
        vcx.dispatch_action(EditorSaveAs);
        settle(&mut vcx);
        cx.simulate_new_path_selection(|_| Some(path.clone()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !path.exists() || app.read_with(&vcx, |app, _| app.active_buffer().unwrap().saving) {
            assert!(
                std::time::Instant::now() < deadline,
                "the document save did not finish"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
            settle(&mut vcx);
        }
        assert_eq!(std::fs::read(&path).unwrap(), b"one\r\ntwo\r\nthree");
    }

#[gpui_kit::test]
fn file_save_before_close_keeps_a_later_focused_editor_group(cx: &mut TestAppContext) {
    use crate::core::actions::EditorSplitRight;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("original-group.txt");
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    let original = new_edited_file(&app, &mut vcx);
    vcx.dispatch_action(CloseActiveTab);
    settle(&mut vcx);
    vcx.simulate_prompt_answer(t(L10nKey::Save));
    settle(&mut vcx);
    let replacement = new_edited_file(&app, &mut vcx);
    vcx.dispatch_action(EditorSplitRight);
    settle(&mut vcx);
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    wait_until(&mut vcx, |vcx| app.read_with(vcx, |app, _| !app.tab_code().unwrap().shows(original)));
    app.read_with(&vcx, |app, _| {
        assert_eq!(app.tab_code().unwrap().active_id(), Some(replacement));
        assert!(app.active_buffer().unwrap().dirty);
    });
    assert_eq!(std::fs::read_to_string(path).unwrap(), "original edits");
}
