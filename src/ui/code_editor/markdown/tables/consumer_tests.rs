use super::{settle_live_geometry, start_in_source};
use crate::ui::app::test_window::harness_with_tabs;
use crate::ui::code_editor::markdown::{Command, Selection};
use gpui_kit::{Focusable as _, test::TestWindowExt as _};

#[gpui_kit::test]
fn table_authoring_insert_navigate_paste_undo_and_owning_host_reopen(
    cx: &mut gpui_kit::TestAppContext,
) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("authoring.md");
    let original = "Before  \r\n\r\nAfter  ";
    std::fs::write(&path, original).unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
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
            start_in_source(&view, &mut context);
            break view;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    settle_live_geometry(&view, &mut context);
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(10..10, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| window.click("markdown-insert-table", cx));
    settle_live_geometry(&view, &mut context);
    let inserted = "Before  \r\n\r\n|  |  |\r\n| --- | --- |\r\n|  |  |\r\n\r\nAfter  ";
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            inserted.as_bytes()
        );
        assert_eq!(view.selection(), Selection::caret(12));
    });
    context.update(|window, cx| window.input("Head", cx));
    context.update(|window, cx| window.press("tab", cx));
    context.update(|window, cx| window.input("Last", cx));
    // Enter visits the same column of the next row, not the separator or outside.
    context.update(|window, cx| window.press("enter", cx));
    context.run_until_parked();
    context.update(|window, cx| {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("x|y\r\nz\\|w\n".into()));
        window.press("secondary-v", cx);
    });
    settle_live_geometry(&view, &mut context);
    let expected =
        "Before  \r\n\r\n| Head | Last |\r\n| --- | --- |\r\n|  | x\\|y z\\|w |\r\n\r\nAfter  ";
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            expected.as_bytes()
        );
    });
    // Vertical movement stays in column two; Shift+Tab still reaches empty cell one.
    context.update(|window, cx| window.press("up", cx));
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        let source = view.document().read(cx).source().to_string();
        let last = source.find("Last").unwrap();
        assert!((last..=last + 4).contains(&view.selection().head()));
    });
    context.update(|window, cx| window.press("enter", cx));
    context.update(|window, cx| window.press("shift-tab", cx));
    context.update(|window, cx| window.input("First", cx));
    context.run_until_parked();
    let expected = expected.replace("|  | x", "| First | x");
    context.update(|window, cx| window.press("enter", cx));
    context.update(|window, cx| window.input("Below ", cx));
    context.run_until_parked();
    let expected = expected.replace("\r\n\r\nAfter", "\r\nBelow \r\nAfter");
    let identity = view.read_with(&context, |view, _| view.document().entity_id());
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().entity_id(), identity);
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            expected.replace("Below ", "").as_bytes()
        );
    });
    context.dispatch_action(gpui_kit::component::input::Redo);
    context.dispatch_action(crate::core::actions::EditorSave);
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
    assert_eq!(std::fs::read(&path).unwrap(), expected.as_bytes());
    app.update_in(&mut context, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.open_file_in_editor(&path, window, cx);
    });
    loop {
        context.run_until_parked();
        if app.read_with(&context, |app, _| app.active_buffer().is_some()) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    app.read_with(&context, |app, cx| {
        let reopened = app
            .active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .read(cx);
        assert_eq!(
            reopened.document().read(cx).encode().unwrap(),
            expected.as_bytes()
        );
    });
}

#[gpui_kit::test]
fn table_reverse_replacement_and_multi_caret_paste_preserve_structure(
    cx: &mut gpui_kit::TestAppContext,
) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("replacement.md");
    let original = "| A | B | C |\r\n| --- | --- | --- |\n| one | two | three |\r\n\nTail";
    std::fs::write(&path, original).unwrap();
    let source = original.replace("\r\n", "\n");
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
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
            start_in_source(&view, &mut context);
            break view;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    settle_live_geometry(&view, &mut context);
    let one = source.find("one").unwrap();
    let two = source.find("two").unwrap();
    view.update_in(&mut context, |view, window, cx| {
        view.focus_handle(cx).focus(window, cx);
        view.command(Command::Select(Selection::new(two + 3, one)), window, cx);
    });
    context.update(|window, cx| window.input("X|Y", cx));
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.replace("one | two", "X\\|Y | ").as_bytes()
        );
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        );
        assert_eq!(view.selection(), Selection::new(two + 3, one));
    });
    context.dispatch_action(gpui_kit::component::input::Redo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.replace("one | two", "X\\|Y | ").as_bytes()
        );
        assert_eq!(view.selection(), Selection::caret(one + "X\\|Y".len()));
    });
    // A caret command ends the typing burst, including a sparse replacement.
    context.update(|window, cx| window.press("left", cx));
    view.read_with(&context, |view, _| {
        assert_eq!(
            view.selection(),
            Selection::caret(one + "X\\|".len()),
            "left immediately after redo must stay inside the body cell"
        );
    });
    context.update(|window, cx| window.input("Z", cx));
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.replace("one | two", "X\\|ZY | ").as_bytes()
        );
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.replace("one | two", "X\\|Y | ").as_bytes()
        );
        assert_eq!(view.selection(), Selection::caret(one + "X\\|".len()));
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    view.update_in(&mut context, |view, _, cx| {
        view.select_range(one..one + 3, cx);
        view.add_selection(two..two + 3, cx);
    });
    context.update(|window, cx| {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("p|q\n".into()));
        window.press("secondary-v", cx);
    });
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.replace("one | two", "p\\|q | p\\|q").as_bytes()
        );
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        )
    });
}

#[gpui_kit::test]
fn table_structure_reverse_header_duplicate_and_minimum_save_reopen(
    cx: &mut gpui_kit::TestAppContext,
) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("structure-boundaries.md");
    let original =
        "Before  \r\n\n| **Head**  | `x\\|y` |\r\n| :----- | ----: |\n| body | tail |\r\n\nAfter  ";
    std::fs::write(&path, original).unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
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
            start_in_source(&view, &mut context);
            break view;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    settle_live_geometry(&view, &mut context);
    let source = original.replace("\r\n", "\n");
    let head = source.find("Head").unwrap();
    let end = source.find("x\\|y").unwrap() + 4;
    let reverse = Selection::new(end, head + 2);
    view.update_in(&mut context, |view, window, cx| {
        view.command(Command::Select(reverse), window, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    // Header deletion must not promote a body row or create a history entry.
    context.dispatch_action(crate::ui::code_editor::markdown::tables::DeleteRow);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        );
        assert_eq!(view.selection(), reverse);
    });
    context.dispatch_action(crate::ui::code_editor::markdown::tables::DuplicateRow);
    context.run_until_parked();
    settle_live_geometry(&view, &mut context);
    // Continue inside the duplicated formatted cell, not the header or delimiter.
    context.update(|window, cx| window.input("X", cx));
    context.run_until_parked();
    let expected = "Before  \r\n\n| **Head**  | `x\\|y` |\r\n| :----- | ----: |\n| **HeXad**  | `x\\|y` |\r\n| body | tail |\r\n\nAfter  ";
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            expected.as_bytes()
        );
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        );
        assert_eq!(view.selection(), reverse);
    });
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    context.dispatch_action(gpui_kit::component::input::Redo);
    context.dispatch_action(gpui_kit::component::input::Redo);
    context.dispatch_action(crate::core::actions::EditorSave);
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
    assert_eq!(std::fs::read(&path).unwrap(), expected.as_bytes());
    app.update_in(&mut context, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.open_file_in_editor(&path, window, cx);
    });
    loop {
        context.run_until_parked();
        if app.read_with(&context, |app, _| app.active_buffer().is_some()) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    app.read_with(&context, |app, cx| {
        let reopened = app
            .active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .read(cx);
        assert_eq!(
            reopened.document().read(cx).encode().unwrap(),
            expected.as_bytes()
        );
    });
}
