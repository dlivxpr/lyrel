// Retained whole-window tests for the `commands` module, which now lives in
// `lyrel-workbench-editor`. Keeping this anchor at the original module path
// preserves every existing test selector.

pub(crate) use lyrel_workbench_editor::code_editor::commands::*;

#[cfg(test)]
mod gpui_tests {
    use super::*;
    use crate::ui::app::test_window::harness_with_tabs;
    use crate::ui::code_editor::nav::gpui_tests::open_rs;
    use gpui_kit::{TestAppContext, VisualTestContext};

    fn select_two(
        input: &gpui_kit::Entity<gpui_kit::component::input::EditorState>,
        vcx: &mut VisualTestContext,
    ) {
        input.update_in(vcx, |state, window, cx| {
            state.set_value("foo foo", window, cx);
            state.set_selected_range(0..3, cx);
            state.add_selection(4..7, cx);
            state.focus(window, cx);
        });
        vcx.run_until_parked();
    }

    #[gpui_kit::test]
    fn ide_commands_change_only_the_active_selection_but_typing_keeps_all_cursors(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        open_rs(&app, &mut vcx, "/commands/active.rs");
        vcx.run_until_parked();
        let input = app.read_with(&vcx, |app, cx| {
            app.active_buffer(cx)
                .unwrap()
                .input
                .code()
                .expect("ordinary code editor")
                .clone()
        });
        select_two(&input, &mut vcx);
        vcx.dispatch_action(TransformToUppercase);
        vcx.run_until_parked();
        assert_eq!(
            input.read_with(&vcx, |state, _| state.text().to_string()),
            "FOO foo"
        );
        vcx.dispatch_action(gpui_kit::component::input::Undo);
        vcx.run_until_parked();
        assert_eq!(
            input.read_with(&vcx, |state, _| state.text().to_string()),
            "foo foo"
        );

        select_two(&input, &mut vcx);
        vcx.simulate_keystrokes("x");
        vcx.run_until_parked();
        assert_eq!(
            input.read_with(&vcx, |state, _| state.text().to_string()),
            "x x"
        );
    }

    #[gpui_kit::test]
    fn editor_shortcuts_edit_the_focused_document_instead_of_running_workspace_commands(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        open_rs(&app, &mut vcx, "/commands/keyboard.rs");
        vcx.run_until_parked();
        let input = app.read_with(&vcx, |app, cx| {
            app.active_buffer(cx)
                .unwrap()
                .input
                .code()
                .expect("ordinary code editor")
                .clone()
        });
        input.update_in(&mut vcx, |state, window, cx| {
            state.set_value("one\ntwo\nthree", window, cx);
            state.set_selected_range(5..5, cx);
            state.focus(window, cx);
        });
        vcx.run_until_parked();
        vcx.simulate_keystrokes("secondary-/");
        vcx.run_until_parked();
        assert_eq!(
            input.read_with(&vcx, |state, _| state.text().to_string()),
            "one\n// two\nthree"
        );
        vcx.simulate_keystrokes("secondary-z");
        vcx.run_until_parked();
        assert_eq!(
            input.read_with(&vcx, |state, _| state.text().to_string()),
            "one\ntwo\nthree"
        );
        vcx.simulate_keystrokes("alt-up");
        vcx.run_until_parked();
        assert_eq!(
            input.read_with(&vcx, |state, _| state.text().to_string()),
            "two\none\nthree"
        );
    }
    #[gpui_kit::test]
    fn markdown_workbench_shortcuts_keep_commands_selection_and_shared_undo(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("commands.md");
        std::fs::write(&path, "one\ntwo\nthree").unwrap();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_file_in_editor(&path, window, cx)
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !app.read_with(&vcx, |app, cx| app.active_buffer(cx).is_some()) {
            vcx.run_until_parked();
            assert!(
                std::time::Instant::now() < deadline,
                "Markdown open did not settle"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        app.update_in(&mut vcx, |app, window, cx| {
            app.toggle_document_preview(window, cx)
        });
        app.update_in(&mut vcx, |app, window, cx| {
            let input = app.active_buffer(cx).unwrap().input.clone();
            input.text().unwrap().set_selected_range(5..5, cx);
            input.focus(window, cx);
        });
        vcx.simulate_keystrokes("secondary-/");
        vcx.run_until_parked();
        app.read_with(&vcx, |app, cx| {
            assert_eq!(
                app.active_buffer(cx)
                    .unwrap()
                    .input
                    .text()
                    .unwrap()
                    .read(cx)
                    .text()
                    .to_string(),
                "one\n<!-- two -->\nthree"
            );
            assert!(app.active_buffer(cx).unwrap().dirty);
        });
        vcx.simulate_keystrokes("secondary-z");
        vcx.run_until_parked();
        vcx.simulate_keystrokes("alt-up");
        vcx.run_until_parked();
        app.read_with(&vcx, |app, cx| {
            assert_eq!(
                app.active_buffer(cx)
                    .unwrap()
                    .input
                    .text()
                    .unwrap()
                    .read(cx)
                    .text()
                    .to_string(),
                "two\none\nthree"
            );
        });
        vcx.simulate_keystrokes("secondary-a");
        vcx.simulate_keystrokes("f o o space f o o");
        vcx.run_until_parked();
        app.update_in(&mut vcx, |app, window, cx| {
            let input = app.active_editor_input(cx).unwrap();
            input.text().unwrap().set_selected_range(0..3, cx);
            input.focus(window, cx);
        });
        vcx.simulate_keystrokes("secondary-shift-l");
        vcx.simulate_keystrokes("x");
        vcx.run_until_parked();
        app.read_with(&vcx, |app, cx| {
            assert_eq!(
                app.active_buffer(cx)
                    .unwrap()
                    .input
                    .text()
                    .unwrap()
                    .read(cx)
                    .text()
                    .to_string(),
                "x x"
            );
        });
        vcx.simulate_keystrokes("secondary-z");
        vcx.run_until_parked();
        app.read_with(&vcx, |app, cx| {
            assert_eq!(
                app.active_buffer(cx)
                    .unwrap()
                    .input
                    .text()
                    .unwrap()
                    .read(cx)
                    .text()
                    .to_string(),
                "foo foo"
            );
        });
    }
    #[gpui_kit::test]
    fn markdown_line_commands_preserve_mixed_endings_and_undo(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mixed-commands.md");
        std::fs::write(&path, b"one\r\ntwo\nthree\r\n").unwrap();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_file_in_editor(&path, window, cx)
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !app.read_with(&vcx, |app, cx| app.active_buffer(cx).is_some()) {
            vcx.run_until_parked();
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        app.update_in(&mut vcx, |app, window, cx| {
            app.toggle_document_preview(window, cx);
            let input = app.active_editor_input(cx).unwrap();
            input.text().unwrap().set_selected_range(4..7, cx);
            input.focus(window, cx);
        });
        vcx.dispatch_action(MoveLineUp);
        app.read_with(&vcx, |app, cx| {
            let input = app.active_editor_input(cx).unwrap();
            assert_eq!(
                input
                    .markdown()
                    .unwrap()
                    .read(cx)
                    .document()
                    .read(cx)
                    .encode()
                    .unwrap(),
                b"two\none\r\nthree\r\n"
            );
        });
        vcx.dispatch_action(gpui_kit::component::input::Undo);
        vcx.dispatch_action(CopyLineDown);
        app.read_with(&vcx, |app, cx| {
            let input = app.active_editor_input(cx).unwrap();
            assert_eq!(
                input
                    .markdown()
                    .unwrap()
                    .read(cx)
                    .document()
                    .read(cx)
                    .encode()
                    .unwrap(),
                b"one\r\ntwo\ntwo\nthree\r\n"
            );
        });
        vcx.dispatch_action(gpui_kit::component::input::Undo);
        app.read_with(&vcx, |app, cx| {
            let input = app.active_editor_input(cx).unwrap();
            assert_eq!(
                input
                    .markdown()
                    .unwrap()
                    .read(cx)
                    .document()
                    .read(cx)
                    .encode()
                    .unwrap(),
                b"one\r\ntwo\nthree\r\n"
            );
            assert_eq!(input.text().unwrap().read(cx).selected_range(), 4..7);
        });
    }

    #[gpui_kit::test]
    fn markdown_repeated_multiline_indent_preserves_selection_and_undo(cx: &mut TestAppContext) {
        use gpui_kit::test::TestWindowExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("indent.md");
        std::fs::write(&path, "one\ntwo\nthree").unwrap();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_file_in_editor(&path, window, cx)
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !app.read_with(&vcx, |app, cx| app.active_buffer(cx).is_some()) {
            vcx.run_until_parked();
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        app.update_in(&mut vcx, |app, window, cx| {
            app.toggle_document_preview(window, cx);
            let input = app.active_editor_input(cx).unwrap();
            input.text().unwrap().set_selected_range(0..7, cx);
            input.focus(window, cx);
        });
        vcx.update(|window, cx| {
            window.render_frame(cx);
            window.press("tab", cx);
            window.press("tab", cx);
        });
        app.read_with(&vcx, |app, cx| {
            let input = app.active_editor_input(cx).unwrap();
            assert_eq!(
                input.text().unwrap().read(cx).text().to_string(),
                "        one\n        two\nthree"
            );
            assert_eq!(input.text().unwrap().read(cx).selected_range(), 8..23);
        });
        vcx.dispatch_action(gpui_kit::component::input::Undo);
        vcx.dispatch_action(gpui_kit::component::input::Undo);
        app.read_with(&vcx, |app, cx| {
            let input = app.active_editor_input(cx).unwrap();
            assert_eq!(
                input.text().unwrap().read(cx).text().to_string(),
                "one\ntwo\nthree"
            );
            assert_eq!(input.text().unwrap().read(cx).selected_range(), 0..7);
        });
    }

    #[gpui_kit::test]
    fn markdown_explicit_commands_and_repeated_occurrences_keep_their_selection_scope(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("primary-command.md");
        std::fs::write(&path, "foo foo foo").unwrap();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_file_in_editor(&path, window, cx)
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !app.read_with(&vcx, |app, cx| app.active_buffer(cx).is_some()) {
            vcx.run_until_parked();
            assert!(
                std::time::Instant::now() < deadline,
                "Markdown open did not settle"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        app.update_in(&mut vcx, |app, window, cx| {
            app.toggle_document_preview(window, cx)
        });
        app.update_in(&mut vcx, |app, window, cx| {
            let input = app.active_editor_input(cx).unwrap();
            input.text().unwrap().set_selected_range(0..3, cx);
            input.focus(window, cx);
            assert!(app.editor_apply_command(Command::All, window, cx));
            assert!(app.editor_apply_command(Command::Upper, window, cx));
        });
        vcx.run_until_parked();
        app.read_with(&vcx, |app, cx| {
            let input = app.active_editor_input(cx).unwrap();
            assert_eq!(
                input.text().unwrap().read(cx).text().to_string(),
                "FOO foo foo"
            );
            assert_eq!(input.text().unwrap().read(cx).selected_range(), 0..3);
        });
        vcx.dispatch_action(gpui_kit::component::input::Undo);
        vcx.run_until_parked();
        app.read_with(&vcx, |app, cx| {
            assert_eq!(
                app.active_editor_input(cx)
                    .unwrap()
                    .text()
                    .unwrap()
                    .read(cx)
                    .text()
                    .to_string(),
                "foo foo foo"
            )
        });
        app.update_in(&mut vcx, |app, window, cx| {
            let input = app.active_editor_input(cx).unwrap();
            input.text().unwrap().set_selected_range(0..3, cx);
            input.focus(window, cx);
            assert!(app.editor_apply_command(Command::Next, window, cx));
            assert!(app.editor_apply_command(Command::Next, window, cx));
        });
        vcx.simulate_keystrokes("x");
        vcx.run_until_parked();
        app.read_with(&vcx, |app, cx| {
            assert_eq!(
                app.active_editor_input(cx)
                    .unwrap()
                    .text()
                    .unwrap()
                    .read(cx)
                    .text()
                    .to_string(),
                "x x x"
            )
        });
        vcx.dispatch_action(gpui_kit::component::input::Undo);
        vcx.run_until_parked();
        app.read_with(&vcx, |app, cx| {
            assert_eq!(
                app.active_editor_input(cx)
                    .unwrap()
                    .text()
                    .unwrap()
                    .read(cx)
                    .text()
                    .to_string(),
                "foo foo foo"
            )
        });
    }
    #[gpui_kit::test]
    fn markdown_source_commands_apply_and_undo_through_the_workbench(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("inventory.md");
        std::fs::write(&path, "initial").unwrap();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_file_in_editor(&path, window, cx)
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !app.read_with(&vcx, |app, cx| app.active_buffer(cx).is_some()) {
            vcx.run_until_parked();
            assert!(
                std::time::Instant::now() < deadline,
                "Markdown open did not settle"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        app.update_in(&mut vcx, |app, window, cx| {
            app.toggle_document_preview(window, cx)
        });
        for (command, before, selection, expected) in [
            (Command::Comment, "中", 0..3, "<!-- 中 -->"),
            (Command::BlockComment, "中", 0..3, "<!-- 中 -->"),
            (Command::MoveUp, "a\n中", 2..5, "中\na"),
            (Command::MoveDown, "a\n中", 0..1, "中\na"),
            (Command::CopyUp, "中", 0..3, "中\n中"),
            (Command::CopyDown, "中", 0..3, "中\n中"),
            (Command::Delete, "a\n中", 2..5, "a"),
            (Command::InsertAbove, "a\nb", 2..2, "a\n\nb"),
            (Command::InsertBelow, "a\nb", 2..2, "a\nb\n"),
            (Command::Upper, "aBc", 0..3, "ABC"),
            (Command::Lower, "aBc", 0..3, "abc"),
            (Command::Title, "hello world", 0..11, "Hello World"),
            (Command::Trim, "a  \nb\t", 0..0, "a\nb"),
            (Command::Join, "a\n b", 0..0, "a b"),
            (Command::Unwrap, "(中)", 1..4, "中"),
        ] {
            app.update_in(&mut vcx, |app, window, cx| {
                let input = app.active_editor_input(cx).unwrap();
                let text = input.text().unwrap();
                let len = text.read(cx).text().len();
                text.replace(0..len, before, window, cx);
                text.set_selected_range(selection, cx);
                input.focus(window, cx);
                assert!(app.editor_apply_command(command, window, cx));
            });
            vcx.run_until_parked();
            app.read_with(&vcx, |app, cx| {
                assert_eq!(
                    app.active_buffer(cx)
                        .unwrap()
                        .input
                        .text()
                        .unwrap()
                        .read(cx)
                        .text()
                        .to_string(),
                    expected
                )
            });
            vcx.dispatch_action(gpui_kit::component::input::Undo);
            vcx.run_until_parked();
            app.read_with(&vcx, |app, cx| {
                assert_eq!(
                    app.active_buffer(cx)
                        .unwrap()
                        .input
                        .text()
                        .unwrap()
                        .read(cx)
                        .text()
                        .to_string(),
                    before
                )
            });
        }
        app.update_in(&mut vcx, |app, window, cx| {
            let input = app.active_editor_input(cx).unwrap();
            let text = input.text().unwrap();
            let len = text.read(cx).text().len();
            text.replace(0..len, "(中)\nfoo foo foo\n", window, cx);
            text.set_selected_range(0..0, cx);
            assert!(app.editor_apply_command(Command::Bracket, window, cx));
            assert_eq!(text.read(cx).selected_range(), 4..4);
            text.set_selected_range(7..7, cx);
            assert!(app.editor_apply_command(Command::SelectLine, window, cx));
            assert_eq!(text.read(cx).selected_range(), 6..18);
            text.set_selected_range(7..7, cx);
            assert!(app.editor_apply_command(Command::Next, window, cx));
            assert_eq!(text.read(cx).selected_range(), 6..9);
            assert!(app.editor_apply_command(Command::Skip, window, cx));
            assert_eq!(text.read(cx).selected_range(), 10..13);
        });
    }
}
