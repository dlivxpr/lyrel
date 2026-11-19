// Retained whole-window tests for the `split` module, which now lives in
// `lyrel-workbench-editor`. Keeping this anchor at the original module path
// preserves every existing test selector.

pub(crate) use lyrel_workbench_editor::code_editor::split::*;

use lyrel_workbench_editor::code_editor::*;

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: u64) -> Vec<BufferId> {
        (0..n).map(|_| BufferId::new()).collect()
    }

    fn code(files: &[BufferId], active: usize) -> TabCode {
        let mut code = TabCode::new();
        code.files = files.to_vec();
        code.active = active;
        code
    }

    #[test]
    fn splitting_opens_the_file_in_front_in_a_focused_right_group() {
        let ids = ids(3);
        let mut code = code(&ids, 1);
        code.split_with(ids[1]);
        assert_eq!(code.files, [ids[1]]);
        assert_eq!(code.active_id(), Some(ids[1]));
        assert!(!code.focus_is_left());
        let sides = code.sides();
        assert_eq!(sides.len(), 2);
        assert_eq!(
            (sides[0].files.clone(), sides[0].focused),
            (ids.clone(), false)
        );
        assert!(sides[1].focused);
        assert_eq!(code.views_of(ids[1]), 2);
        assert_eq!(code.views_of(ids[0]), 1);
        assert_eq!(code.all_files(), [ids[1], ids[0], ids[2]]);
    }

    #[test]
    fn the_focus_swaps_the_groups_but_not_their_places() {
        let ids = ids(3);
        let mut code = code(&ids[..2], 0);
        code.split_with(ids[0]);
        code.show(ids[2]);
        code.swap_focus();
        assert!(code.focus_is_left());
        assert_eq!(code.files, ids[..2]);
        assert_eq!(code.other_active_id(), Some(ids[2]));
        let sides = code.sides();
        assert!(sides[0].focused && !sides[1].focused);
        assert_eq!(sides[1].files, [ids[0], ids[2]]);
    }

    #[test]
    fn a_group_that_loses_its_last_file_closes() {
        let ids = ids(2);
        let mut code = code(&ids, 0);
        code.split_with(ids[1]);
        // Closing the right group's only file leaves the left one, focused.
        assert!(code.close_in_focused(ids[1]));
        assert!(!code.is_split());
        assert_eq!(code.files, ids);
        assert_eq!(code.active_id(), Some(ids[0]), "it keeps what was in front");

        // Closing the other group's last file from elsewhere (the buffer went
        // away) closes that group instead.
        code.split_with(ids[0]);
        code.swap_focus();
        assert!(code.forget_everywhere(ids[0]));
        assert!(!code.is_split());
        assert_eq!(code.files, [ids[1]]);
        assert!(!code.shows(ids[0]));
    }

    #[test]
    fn a_restore_lays_out_the_recorded_groups() {
        let ids = ids(3);
        // As the restore finds it: every file arrived at the end of the strip.
        let mut code = code(&ids, 0);
        let untouched = code.restore_groups(
            &ids,
            (vec![ids[0], ids[1]], 1),
            Some((vec![ids[2]], 0, true)),
        );
        assert!(untouched);
        assert!(!code.focus_is_left());
        assert_eq!(code.active_id(), Some(ids[2]));
        assert_eq!(code.other_active_id(), Some(ids[1]));
    }

    #[test]
    fn a_file_opened_while_the_restore_loaded_stays_in_front() {
        let ids = ids(4);
        // ids[3] was opened by hand; the restore's three arrived behind it.
        let mut code = code(&[ids[3], ids[0], ids[1], ids[2]], 0);
        let untouched = code.restore_groups(
            &ids[..3],
            (vec![ids[0], ids[1]], 0),
            Some((vec![ids[2]], 0, false)),
        );
        assert!(!untouched);
        assert!(code.is_split(), "the recorded split is still put back");
        assert!(code.focus_is_left());
        assert_eq!(code.files, [ids[0], ids[1], ids[3]]);
        assert_eq!(
            code.active_id(),
            Some(ids[3]),
            "what was opened stays in front"
        );
        assert_eq!(code.other_active_id(), Some(ids[2]));
    }

    #[test]
    fn a_split_made_while_the_restore_loaded_is_kept() {
        let ids = ids(4);
        let mut code = code(&[ids[3]], 0);
        code.split_with(ids[3]);
        // The restore's files arrive in the focused group.
        code.files.extend([ids[0], ids[1]]);
        let before = (code.files.clone(), code.active, code.split.clone());
        let untouched =
            code.restore_groups(&ids[..2], (vec![ids[0]], 0), Some((vec![ids[1]], 0, true)));
        assert!(!untouched);
        assert_eq!(
            (code.files.clone(), code.active, code.split.clone()),
            before
        );
    }

    #[test]
    fn closing_in_one_group_keeps_the_file_in_the_other() {
        let ids = ids(2);
        let mut code = code(&ids, 0);
        code.split_with(ids[0]);
        code.show(ids[1]);
        code.close_in_focused(ids[0]);
        assert!(code.is_split());
        assert!(code.shows(ids[0]));
        assert_eq!(code.views_of(ids[0]), 1);
    }
}

#[cfg(test)]
mod gpui_tests {
    use std::path::PathBuf;

    use gpui_kit::{Entity, TestAppContext, VisualTestContext};

    use super::super::nav::gpui_tests::{caret, open_rs, put_caret};
    use super::*;
    use crate::ui::app::LyrelApp;
    use crate::ui::app::test_window::harness_with_tabs;
    use crate::ui::editor_session::{self, SplitEditor, TabEditor};

    fn front(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> PathBuf {
        app.read_with(vcx, |app, cx| app.active_buffer(cx).unwrap().path.clone())
    }

    fn split(app: &Entity<LyrelApp>, vcx: &mut VisualTestContext) -> bool {
        app.read_with(vcx, |app, _| app.tab_code().is_some_and(TabCode::is_split))
    }

    #[gpui_kit::test]
    fn two_groups_keep_their_own_file_and_caret(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        open_rs(&app, &mut vcx, "/split/a.rs");
        put_caret(&app, &mut vcx, 5);
        assert!(app.update_in(&mut vcx, |app, window, cx| app.editor_split(window, cx)));
        assert!(split(&app, &mut vcx));
        // The new group is on the right, focused, showing the same file.
        app.read_with(&vcx, |app, _| {
            let code = app.tab_code().unwrap();
            assert!(!code.focus_is_left());
            assert_eq!(code.active_id(), code.other_active_id());
        });
        // A file opened now opens in the focused (right) group only.
        open_rs(&app, &mut vcx, "/split/b.rs");
        put_caret(&app, &mut vcx, 40);
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/split/b.rs"));

        // To the left: a.rs, with its own caret.
        assert!(app.update_in(&mut vcx, |app, window, cx| {
            app.editor_focus_group(true, window, cx)
        }));
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/split/a.rs"));
        assert_eq!(caret(&app, &mut vcx).line, 5);
        assert!(
            !app.update_in(&mut vcx, |app, window, cx| app
                .editor_focus_group(true, window, cx)),
            "no group further left: the chord carries on to the panes"
        );
        // And back.
        assert!(app.update_in(&mut vcx, |app, window, cx| {
            app.editor_focus_group(false, window, cx)
        }));
        assert_eq!(caret(&app, &mut vcx).line, 40);

        // Closing both files of the right group closes the group.
        app.update_in(&mut vcx, |app, window, cx| {
            let n = app.tab_code().unwrap().files.len();
            for _ in 0..n {
                app.editor_close_file(0, window, cx);
            }
        });
        assert!(!split(&app, &mut vcx));
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/split/a.rs"));
        assert!(
            app.read_with(&vcx, |app, cx| app
                .editor
                .read(cx)
                .buffers()
                .iter()
                .all(|b| b.path != std::path::Path::new("/split/b.rs"))),
            "a file no group shows any more is closed"
        );
    }

    #[gpui_kit::test]
    fn a_split_is_recorded_and_comes_back(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        // Canonical, as a loaded file's path is: /var is /private/var here.
        let root = crate::ui::code_editor::tests::test_real_dir(dir.path());
        let (a, b, c) = (root.join("a.rs"), root.join("b.rs"), root.join("c.rs"));
        for p in [&a, &b, &c] {
            std::fs::write(p, "fn main() {}\n").unwrap();
        }
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
        crate::ui::i18n::set_locale("en");
        let state = TabEditor {
            files: vec![a.clone(), b.clone()],
            active: 1,
            visible: true,
            split: Some(SplitEditor {
                files: vec![c.clone()],
                active: 0,
                focused: true,
            }),
        };
        // The second tab was split when the window last closed.
        let tab = app.read_with(&vcx, |app, _| app.tabs[1].id());
        vcx.update(|_, cx| editor_session::put(cx, tab, state.clone()));
        app.update_in(&mut vcx, |app, window, cx| {
            app.tabs.select(1);
            // As on a relaunch: nothing has been put back for it yet.
            app.editor.update(cx, |editor, _| {
                editor.restored.remove(&tab);
            });
            app.editor_restore_active(window, cx);
        });
        // The files are read on a real thread; let them land.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !split(&app, &mut vcx) {
            assert!(
                std::time::Instant::now() < deadline,
                "the files never landed"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
            vcx.run_until_parked();
        }
        app.read_with(&vcx, |app, cx| {
            let code = app.tab_code().unwrap();
            assert!(code.is_split());
            assert!(!code.focus_is_left(), "the right group had the focus");
            let path = |id| app.editor.read(cx).buffers().get(id).unwrap().path.clone();
            assert_eq!(path(code.active_id().unwrap()), c);
            assert_eq!(path(code.other_active_id().unwrap()), b);
        });
        // And recording it writes the same thing down.
        app.update(&mut vcx, |app, cx| app.editor_record_sessions(cx));
        let recorded = vcx.update(|_, cx| editor_session::get(cx, tab)).unwrap();
        assert_eq!(recorded, state);
    }

    #[gpui_kit::test]
    fn a_split_with_nothing_on_the_left_still_comes_back(cx: &mut TestAppContext) {
        // The left group held only untitled or remote buffers, which are
        // not recorded: only the right group's files were written down.
        let dir = tempfile::tempdir().unwrap();
        let root = crate::ui::code_editor::tests::test_real_dir(dir.path());
        let c = root.join("c.rs");
        std::fs::write(&c, "fn main() {}\n").unwrap();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        let state = TabEditor {
            files: vec![],
            active: 0,
            visible: true,
            split: Some(SplitEditor {
                files: vec![c.clone()],
                active: 0,
                focused: true,
            }),
        };
        let tab = app.read_with(&vcx, |app, _| app.tabs[0].id());
        vcx.update(|_, cx| editor_session::put(cx, tab, state.clone()));
        app.update_in(&mut vcx, |app, window, cx| {
            app.editor.update(cx, |editor, _| {
                editor.restored.remove(&tab);
            });
            app.editor_restore_active(window, cx);
        });
        let has_files = |app: &Entity<LyrelApp>, vcx: &mut VisualTestContext| {
            app.read_with(vcx, |app, _| {
                app.tab_code().is_some_and(|c| c.active_id().is_some())
            })
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !has_files(&app, &mut vcx) {
            assert!(
                std::time::Instant::now() < deadline,
                "the right group's file never came back"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
            vcx.run_until_parked();
        }
        app.read_with(&vcx, |app, cx| {
            let code = app.tab_code().unwrap();
            assert_eq!(
                app.editor
                    .read(cx)
                    .buffers()
                    .get(code.active_id().unwrap())
                    .unwrap()
                    .path,
                c
            );
        });
    }

    #[gpui_kit::test]
    fn a_key_pressed_right_after_a_focus_move_acts_on_the_new_group(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        open_rs(&app, &mut vcx, "/split/a.rs");
        app.update_in(&mut vcx, |app, window, cx| app.editor_split(window, cx));
        open_rs(&app, &mut vcx, "/split/b.rs");
        vcx.run_until_parked();
        assert_eq!(front(&app, &mut vcx), PathBuf::from("/split/b.rs"));
        // The keyboard moves to the left group's text, and a key follows
        // before anything is drawn.
        vcx.update(|window, cx| {
            let left = app.read(cx).tab_code().unwrap().other_active_id().unwrap();
            let input = app
                .read(cx)
                .editor
                .read(cx)
                .buffers()
                .get(left)
                .unwrap()
                .input
                .code()
                .expect("ordinary code editor")
                .clone();
            input.update(cx, |state, cx| state.focus(window, cx));
            let before = app.read(cx).active_buffer(cx).unwrap().path.clone();
            assert_eq!(
                before,
                PathBuf::from("/split/b.rs"),
                "nothing has drawn yet"
            );
            // Go to Line asks whether the editor has the keyboard, which it
            // only does if the group focus has followed it.
            window.dispatch_keystroke(gpui_kit::Keystroke::parse("ctrl-g").unwrap(), cx);
            assert!(
                app.read(cx).editor.read(cx).has_bar(),
                "the command saw the group the keyboard is in"
            );
        });
    }

    /// A command from a menu is dispatched without a draw first, so nothing
    /// but the capture listener can have settled the group.
    #[gpui_kit::test]
    fn a_menu_command_right_after_a_focus_move_acts_on_the_new_group(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        open_rs(&app, &mut vcx, "/split/a.rs");
        app.update_in(&mut vcx, |app, window, cx| app.editor_split(window, cx));
        open_rs(&app, &mut vcx, "/split/b.rs");
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            let left = app.read(cx).tab_code().unwrap().other_active_id().unwrap();
            let input = app
                .read(cx)
                .editor
                .read(cx)
                .buffers()
                .get(left)
                .unwrap()
                .input
                .code()
                .expect("ordinary code editor")
                .clone();
            input.update(cx, |state, cx| state.focus(window, cx));
            let node = window.focused(cx).unwrap();
            node.dispatch_action(&crate::core::actions::EditorGoToLine, window, cx);
            assert!(
                app.read(cx).editor.read(cx).has_bar(),
                "the command saw the group the keyboard is in"
            );
            let front = app.read(cx).active_buffer(cx).unwrap().path.clone();
            assert_eq!(front, PathBuf::from("/split/a.rs"));
        });
    }

    #[gpui_kit::test]
    fn what_is_opened_while_a_restore_loads_survives_it(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::ui::code_editor::tests::test_real_dir(dir.path());
        let (a, b, c) = (root.join("a.rs"), root.join("b.rs"), root.join("c.rs"));
        for p in [&a, &b, &c] {
            std::fs::write(p, "fn main() {}\n").unwrap();
        }
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        let state = TabEditor {
            files: vec![a.clone(), b.clone()],
            active: 0,
            visible: true,
            split: Some(SplitEditor {
                files: vec![c.clone()],
                active: 0,
                focused: false,
            }),
        };
        let tab = app.read_with(&vcx, |app, _| app.tabs[0].id());
        vcx.update(|_, cx| editor_session::put(cx, tab, state));
        app.update_in(&mut vcx, |app, window, cx| {
            app.editor.update(cx, |editor, _| {
                editor.restored.remove(&tab);
            });
            app.editor_restore_active(window, cx);
        });
        // Opened by hand while the three are still being read.
        open_rs(&app, &mut vcx, "/split/opened.rs");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !split(&app, &mut vcx) {
            assert!(
                std::time::Instant::now() < deadline,
                "the files never landed"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
            vcx.run_until_parked();
        }
        app.read_with(&vcx, |app, cx| {
            let code = app.tab_code().unwrap();
            let path = |id| app.editor.read(cx).buffers().get(id).unwrap().path.clone();
            assert_eq!(
                path(code.active_id().unwrap()),
                PathBuf::from("/split/opened.rs")
            );
            let shown: Vec<PathBuf> = code.all_files().into_iter().map(path).collect();
            for p in [&a, &b, &c] {
                assert!(shown.contains(p), "{} came back", p.display());
            }
            assert_eq!(path(code.other_active_id().unwrap()), c);
            assert!(code.visible);
        });
    }
}
