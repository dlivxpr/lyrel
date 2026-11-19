// Retained whole-window tests for the `problems` module, which now lives in
// `lyrel-workbench-editor`. Keeping this anchor at the original module path
// preserves every existing test selector.

#[cfg(test)]
mod tests {
    use gpui_kit::test::TestWindowExt as _;

    /// The pane opens from its command and draws with nothing to show.
    #[gpui_kit::test]
    fn the_pane_toggles_and_draws_empty(cx: &mut gpui_kit::TestAppContext) {
        use crate::ui::app::test_window;

        let (app, mut vcx, _pane) = test_window::harness_with_tabs(cx, 1);
        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_new_file(window, cx);
            app.toggle_editor_problems(cx);
        });
        vcx.run_until_parked();
        vcx.update(|window, cx| window.render_frame(cx));
        app.update_in(&mut vcx, |app, window, cx| {
            assert!(app.editor.read(cx).problems_open());
            assert!(app.render_editor_problems(window, cx).is_some());
            app.toggle_editor_problems(cx);
            assert!(app.render_editor_problems(window, cx).is_none());
        });
    }
}
