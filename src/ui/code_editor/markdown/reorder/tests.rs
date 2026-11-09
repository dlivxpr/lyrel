use gpui_kit::test::{TestEventExt as _, TestWindowExt as _};
use gpui_kit::{Focusable as _, MouseButton, TestAppContext, VisualContext as _};

#[gpui_kit::test]
fn workbench_pointer_reorder_keeps_fragments_history_and_host_bytes(cx: &mut TestAppContext) {
    use crate::ui::app::test_window::harness_with_tabs;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("reorder.md");
    let original = "\u{feff}# Start\r\n\n* root\r\n  + child ![x](missing.png) $x$\n\n> [!NOTE]\r\n> quote\n\n# End\r\ntail\n";
    let moved = "\u{feff}# Start\r\n\n\n> [!NOTE]\r\n> quote\n\n* root\r\n  + child ![x](missing.png) $x$\n# End\r\ntail\n";
    std::fs::write(&path, original).unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    cx.simulate_window_resize(
        context.window_handle(),
        gpui_kit::size(gpui_kit::px(1100.), gpui_kit::px(1500.)),
    );
    app.update_in(&mut context, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let view = loop {
        context.run_until_parked();
        if let Some(view) = app.read_with(&context, |app, _| {
            app.active_editor_input()
                .and_then(|input| input.markdown().cloned())
        }) {
            break view;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    let settle = |context: &mut gpui_kit::VisualTestContext| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            context
                .executor()
                .advance_clock(std::time::Duration::from_millis(100));
            context.run_until_parked();
            if view.read_with(context, |view, cx| {
                view.projection.revision == view.document.read(cx).revision()
                    && view.projection.block_index.ready
            }) {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        context.update(|window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
        });
    };
    settle(&mut context);
    view.update_in(&mut context, |view, window, cx| {
        view.focus_handle(cx).focus(window, cx)
    });
    let source = original
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n");
    let list = source.find("* root").unwrap();
    let heading = source.find("# End").unwrap();
    let grip = view.read_with(&context, |view, _| {
        view.projection
            .leaves
            .iter()
            .find(|leaf| leaf.range.contains(&list))
            .unwrap()
            .id
    });
    context.update(|window, cx| {
        let from = window
            .find(("markdown-reorder-grip", grip))
            .bounds()
            .center();
        let to = view
            .read(cx)
            .range_to_bounds(&(heading..heading), cx)
            .unwrap()
            .origin;
        window.pointer_down(from, MouseButton::Left, cx);
        window.pointer_move(to, Some(MouseButton::Left), cx);
        assert!(window.find("markdown-reorder-indicator").visible());
        window.pointer_up(to, MouseButton::Left, cx);
    });
    settle(&mut context);
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document.read(cx).encode().unwrap(), moved.as_bytes())
    });
    // A second pointer gesture cancelled by Escape must neither select text nor
    // create an undo entry. The following typing remains a separate edit.
    let grip = view.read_with(&context, |view, _| {
        view.projection
            .leaves
            .iter()
            .find(|leaf| matches!(leaf.kind, super::super::Kind::List(_)))
            .unwrap()
            .id
    });
    let selection = view.read_with(&context, |view, _| view.selection());
    context.update(|window, cx| {
        let from = window
            .find(("markdown-reorder-grip", grip))
            .bounds()
            .center();
        let to = window.find("markdown-source").bounds().origin;
        window.pointer_down(from, MouseButton::Left, cx);
        window.pointer_move(to, Some(MouseButton::Left), cx);
        window.press("escape", cx);
        window.pointer_up(to, MouseButton::Left, cx);
        assert_eq!(view.read(cx).selection(), selection);
        assert_eq!(
            view.read(cx).document.read(cx).encode().unwrap(),
            moved.as_bytes()
        );
        window.press("end", cx);
        window.input("!", cx);
    });
    settle(&mut context);
    let edited = moved.replacen("* root", "* root!", 1);
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document.read(cx).encode().unwrap(), edited.as_bytes())
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context);
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document.read(cx).encode().unwrap(), moved.as_bytes())
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document.read(cx).encode().unwrap(),
            original.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Redo);
    context.dispatch_action(gpui_kit::component::input::Redo);
    settle(&mut context);
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    settle(&mut context);
    context.dispatch_action(crate::core::actions::EditorSave);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        context.run_until_parked();
        if app.read_with(&context, |app, _| {
            app.active_buffer()
                .is_some_and(|file| !file.dirty && !file.saving)
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(std::fs::read(&path).unwrap(), edited.as_bytes());
    app.update_in(&mut context, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.open_file_in_editor(&path, window, cx);
    });
    loop {
        context.run_until_parked();
        if let Some(input) = app.read_with(&context, |app, _| app.active_editor_input()) {
            input.markdown().unwrap().read_with(&context, |view, cx| {
                assert_eq!(view.document.read(cx).encode().unwrap(), edited.as_bytes())
            });
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
