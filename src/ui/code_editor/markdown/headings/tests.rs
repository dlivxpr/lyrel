use super::ToCodeBlock;
use gpui_kit::{Focusable as _, MouseButton, TestAppContext, VisualContext as _, test::TestWindowExt as _};

#[gpui_kit::test]
fn workbench_block_conversion_and_fold_keep_bytes_and_one_history(cx: &mut TestAppContext) {
    use crate::ui::app::test_window::harness_with_tabs;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("blocks.md");
    let original = b"# Section\r\n\r\n> alpha\n> > nested\r\n\r\n# Neighbor\nlast\r\n";
    std::fs::write(&path, original).unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut context, |app, window, cx| app.open_file_in_editor(&path, window, cx));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let view = loop {
        context.run_until_parked();
        if let Some(view) = app.read_with(&context, |app, _| app.active_editor_input().and_then(|input| input.markdown().cloned())) {
            break view;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    let settle = |context: &mut gpui_kit::VisualTestContext| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            context.run_until_parked();
            if view.read_with(context, |view, cx| view.projection.revision == view.document.read(cx).revision() && view.projection.block_index.ready) { break; }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    settle(&mut context);
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(14..14, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        window.click("markdown-block-commands", cx);
    });
    context.run_until_parked();
    context.update(|window, cx| {
        let label = crate::ui::i18n::t(crate::ui::i18n::L10nKey::MarkdownBlockNote);
        let at = window.find_by_label(label).bounds().center();
        window.pointer_down(at, MouseButton::Left, cx);
        window.pointer_up(at, MouseButton::Left, cx);
    });
    settle(&mut context);
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document.read(cx).source().to_string(), "# Section\n\n> [!NOTE]\n> alpha\n> > nested\n\n# Neighbor\nlast\n");
        assert_eq!(view.document.read(cx).encode().unwrap(), b"# Section\r\n\r\n> [!NOTE]\r\n> alpha\n> > nested\r\n\r\n# Neighbor\nlast\r\n");
    });
    context.update(|window, cx| {
        assert!(view.read(cx).focus_handle(cx).is_focused(window));
        window.press(if cfg!(target_os = "macos") { "cmd-alt-shift-f" } else { "ctrl-alt-shift-f" }, cx);
    });
    settle(&mut context);
    view.read_with(&context, |view, cx| assert!(view.document.read(cx).source().to_string().contains("> [!NOTE]-\n")));
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context);
    view.read_with(&context, |view, cx| assert!(!view.document.read(cx).source().to_string().contains("[!NOTE]-")));
    context.dispatch_action(gpui_kit::component::input::Redo);
    settle(&mut context);
    view.read_with(&context, |view, cx| assert!(view.document.read(cx).source().to_string().contains("[!NOTE]-")));
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context);
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context);
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(14..14, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| window.press(if cfg!(target_os = "macos") { "cmd-alt-4" } else { "ctrl-alt-4" }, cx));
    settle(&mut context);
    view.read_with(&context, |view, cx| assert_eq!(view.document.read(cx).encode().unwrap(), b"# Section\r\n\r\n#### alpha\n> nested\r\n\r\n# Neighbor\nlast\r\n"));
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context);
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(14..14, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.dispatch_action(ToCodeBlock);
    settle(&mut context);
    view.read_with(&context, |view, cx| assert_eq!(view.document.read(cx).encode().unwrap(), b"# Section\r\n\r\n```\r\nalpha\n> nested\r\n```\r\n\r\n# Neighbor\nlast\r\n"));
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context);
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(18..18, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| window.press("enter", cx));
    settle(&mut context);
    view.read_with(&context, |view, cx| assert!(view.document.read(cx).source().to_string().contains("> alpha\n> \n> > nested")));
    context.update(|window, cx| window.press("enter", cx));
    settle(&mut context);
    view.read_with(&context, |view, cx| assert!(view.document.read(cx).source().to_string().contains("> alpha\n\n> > nested")));
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context);
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(14..14, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    let revision_before_fold = view.read_with(&context, |view, cx| view.document.read(cx).revision());
    context.update(|window, cx| window.press(if cfg!(target_os = "macos") { "cmd-alt-f" } else { "ctrl-alt-f" }, cx));
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document.read(cx).revision(), revision_before_fold);
        assert!(!view.folds.hidden.is_empty());
        assert!(!view.folds.is_hidden(view.selection().head()));
    });
    context.update(|window, cx| window.input("X", cx));
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context);
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    settle(&mut context);
    context.dispatch_action(crate::core::actions::EditorSave);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        context.run_until_parked();
        if app.read_with(&context, |app, _| app.active_buffer().is_some_and(|file| !file.dirty && !file.saving)) { break; }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(std::fs::read(&path).unwrap(), original);
    app.update_in(&mut context, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.open_file_in_editor(&path, window, cx);
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        context.run_until_parked();
        if app.read_with(&context, |app, _| app.active_buffer().is_some()) { break; }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(std::fs::read(&path).unwrap(), original);
}
