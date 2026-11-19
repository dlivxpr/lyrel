// Retained whole-window tests for the `gutter` module, which now lives in
// `lyrel-workbench-editor`. Keeping this anchor at the original module path
// preserves every existing test selector.

pub(crate) use lyrel_workbench_editor::code_editor::gutter::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::app::LyrelApp;
    use gpui_kit::component::input::Position;
    use gpui_kit::test::TestWindowExt as _;
    use lyrel_workbench_editor::code_editor::TabCode;
    use std::ops::Range;

    fn h(old: Range<usize>, new: Range<usize>) -> Hunk {
        Hunk { old, new }
    }

    /// The whole loop in a real window: a base, an edited buffer, markers on
    /// the input, stepping between changes, and a revert that is one undo.
    #[gpui_kit::test]
    fn markers_steps_and_revert_in_an_open_buffer(cx: &mut gpui_kit::TestAppContext) {
        use crate::ui::app::test_window;

        let (app, mut vcx, _pane) = test_window::harness_with_tabs(cx, 1);
        let base = "one\ntwo\nthree\nfour\n";
        let (id, input) = app.update_in(&mut vcx, |app, window, cx| {
            app.editor_new_file(window, cx);
            let id = app
                .tab_code()
                .and_then(TabCode::active_id)
                .expect("a buffer");
            let input = app
                .editor
                .read(cx)
                .buffers()
                .get(id)
                .expect("the buffer")
                .input
                .code()
                .expect("ordinary code editor")
                .clone();
            input.update(cx, |state, cx| {
                state.set_value("one\nTWO\nthree\nfour\nfive\n", window, cx);
                state.focus(window, cx);
            });
            app.editor.update(cx, |editor, _| {
                editor
                    .buffers
                    .get_mut(id)
                    .unwrap()
                    .gutter
                    .set_base_for_test(base);
            });
            (id, input)
        });
        vcx.run_until_parked();

        app.update_in(&mut vcx, |app, window, cx| {
            let hunks = app
                .editor
                .update(cx, |editor, cx| editor.gutter_hunks_now(id, cx));
            assert_eq!(*hunks, vec![h(1..2, 1..2), h(4..4, 4..5)]);

            assert!(app.editor_gutter_step(true, window, cx));
            assert_eq!(input.read(cx).cursor_position().line, 1);
            assert!(app.editor_gutter_step(true, window, cx));
            assert_eq!(input.read(cx).cursor_position().line, 4);
            assert!(app.editor_gutter_step(true, window, cx), "wraps");
            assert_eq!(input.read(cx).cursor_position().line, 1);

            assert!(app.editor_gutter_can_revert(id, cx));
            assert!(app.editor_gutter_revert_at_cursor(window, cx));
            assert_eq!(
                input.read(cx).text().to_string(),
                "one\ntwo\nthree\nfour\nfive\n"
            );
            let hunks = app
                .editor
                .update(cx, |editor, cx| editor.gutter_hunks_now(id, cx));
            assert_eq!(*hunks, vec![h(4..4, 4..5)]);
        });

        // One undo takes the whole revert back.
        vcx.update(|window, cx| {
            window.dispatch_action(Box::new(gpui_kit::component::input::Undo), cx);
        });
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            assert_eq!(
                input.read(cx).text().to_string(),
                "one\nTWO\nthree\nfour\nfive\n"
            );
        });

        // The peek from the keyboard: from an unchanged line it goes to the
        // next change and takes the keys; Escape hands them back.
        let peek_open = |app: &gpui_kit::Entity<LyrelApp>,
                         vcx: &mut gpui_kit::VisualTestContext| {
            app.read_with(vcx, |app, cx| {
                app.editor
                    .read(cx)
                    .buffers()
                    .get(id)
                    .unwrap()
                    .gutter
                    .peek
                    .is_some()
            })
        };
        app.update_in(&mut vcx, |app, window, cx| {
            input.update(cx, |state, cx| {
                state.set_cursor_position(Position::new(0, 0), window, cx);
                state.focus(window, cx);
            });
            assert!(app.editor_gutter_peek_at_cursor(window, cx));
            assert_eq!(input.read(cx).cursor_position().line, 1);
        });
        vcx.run_until_parked();
        assert!(peek_open(&app, &mut vcx));
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        assert!(!peek_open(&app, &mut vcx));
        vcx.update(|window, cx| {
            use gpui_kit::Focusable as _;
            assert!(input.read(cx).focus_handle(cx).is_focused(window));
        });

        // Enter in the peek reverts the change it shows.
        app.update_in(&mut vcx, |app, window, cx| {
            assert!(app.editor_gutter_peek_at_cursor(window, cx));
        });
        vcx.run_until_parked();
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        assert!(!peek_open(&app, &mut vcx));
        vcx.update(|_, cx| {
            assert_eq!(
                input.read(cx).text().to_string(),
                "one\ntwo\nthree\nfour\nfive\n"
            );
        });
    }
}
