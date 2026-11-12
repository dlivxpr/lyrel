//! 生产工作台表格与代码消费者；native registry 使用同一完整场景和真实字体。
use super::super::*;
use super::foundation::{save, settle, until};
use crate::ui::app::test_window::harness_with_tabs;
#[cfg(feature = "native-markdown-tests")]
use gpui_kit::test::TestEventExt as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext};
use lyrel_markdown_editor::EditorState as MarkdownEditor;

fn active(app: &Entity<LyrelApp>, context: &VisualTestContext) -> Entity<MarkdownEditor> {
    app.read_with(context, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    })
}
fn select(view: &Entity<MarkdownEditor>, at: usize, context: &mut VisualTestContext) {
    view.update_in(context, |view, window, cx| {
        view.select_range(at..at, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    settle(context);
}
fn table_command(view: &Entity<MarkdownEditor>, label: L10nKey, context: &mut VisualTestContext) {
    view.update_in(context, |view, window, cx| match label {
        L10nKey::EditorTableInsertRowAfter => view.insert_table_row(true, cx),
        L10nKey::EditorTableInsertColumnAfter => view.insert_table_column(true, cx),
        L10nKey::EditorTableAlignCenter => {
            view.set_caret_table_align(lyrel_markdown_editor::CellAlign::Center, cx)
        }
        L10nKey::EditorTableStyleStriped => view.set_table_style(Some("striped"), cx),
        L10nKey::EditorTableDeleteColumn => view.delete_table_column(cx),
        L10nKey::EditorTableDeleteRow => view.delete_table_row(cx),
        L10nKey::EditorTableFitColumn => view.fit_caret_table_column(window, cx),
        _ => unreachable!("not a table command under test"),
    });
    settle(context);
}
fn bytes(view: &Entity<MarkdownEditor>, context: &VisualTestContext) -> Vec<u8> {
    view.read_with(context, |view, cx| {
        view.document().read(cx).encode().unwrap()
    })
}

pub(super) fn workflow(cx: &mut TestAppContext) {
    use super::super::test_host::{FileHost, canonical_tempdir};
    let directory = canonical_tempdir();
    let path = directory.path().join("table-code.md");
    let original = b"\xef\xbb\xbfbefore  \r\n\n| name | value |\r\n| :--- | ---: |\n| alpha | \xe4\xb8\xad\xe6\x96\x87 |\r\n| z | e\xcc\x81\xf0\x9f\x8e\x89 |\n\r\n```rust keep-this-metadata\nlet answer = 42;\r\n```\n\r\nafter$$x$$literal  ";
    std::fs::write(&path, original).unwrap();
    let host = FileHost::mapped(HostId(239), directory.path());
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    context.simulate_resize(gpui_kit::size(px(1400.), px(1000.)));
    app.update_in(&mut context, |app, window, cx| {
        app.editor_open_on_host(host.clone(), Path::new("/table-code.md"), window, cx)
    });
    until(&mut context, |context| {
        app.read_with(context, |app, _| app.active_buffer().is_some())
    });
    let first = active(&app, &context);
    context.update(|window, _| {
        assert!(
            window.try_find("markdown-table-insert").is_none(),
            "the insertion picker stays available through slash commands without a duplicate toolbar button"
        );
        assert!(
            window.try_find("markdown-table-commands").is_none(),
            "table commands stay in the native context menu without a duplicate toolbar button"
        );
    });
    let start = first.read_with(&context, |view, _| view.text().find("alpha").unwrap());
    select(&first, start, &mut context);
    context.update(|window, cx| window.press("tab", cx));
    assert_eq!(
        first.read_with(&context, |view, _| view.text()
            [view.cursor()..view.cursor() + "中文".len()]
            .to_string()),
        "中文"
    );
    context.update(|window, cx| window.press("down", cx));
    first.read_with(&context, |view, _| {
        assert!(view.text().is_char_boundary(view.cursor()))
    });
    context.update(|window, cx| window.press("up", cx));
    context.update(|window, cx| window.input("界", cx));
    settle(&mut context);
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context);
    assert_eq!(bytes(&first, &context), original);
    for command in [
        L10nKey::EditorTableInsertRowAfter,
        L10nKey::EditorTableInsertColumnAfter,
        L10nKey::EditorTableAlignCenter,
        L10nKey::EditorTableStyleStriped,
        L10nKey::EditorTableDeleteColumn,
        L10nKey::EditorTableDeleteRow,
    ] {
        select(&first, start, &mut context);
        table_command(&first, command, &mut context);
        assert_ne!(bytes(&first, &context), original);
        context.dispatch_action(gpui_kit::component::input::Undo);
        settle(&mut context);
        assert_eq!(
            bytes(&first, &context),
            original,
            "Undo restores exact mixed line endings and neighbors after {command:?}"
        );
    }
    // The language picker is a real focus-owning Kit menu: keyboard selection and dismissal.
    let code = first.read_with(&context, |view, _| view.text().find("let answer").unwrap());
    select(&first, code, &mut context);
    context.update(|window, cx| window.click("markdown-code-language", cx));
    context.update(|window, cx| {
        window.press("home", cx);
        window.press("down", cx);
        window.press("enter", cx);
    });
    settle(&mut context);
    first.read_with(&context, |view, _| {
        assert!(
            view.text()
                .contains(" keep-this-metadata\nlet answer = 42;")
        )
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context);
    assert_eq!(bytes(&first, &context), original);
    select(&first, code, &mut context);
    context.update(|window, cx| window.click("markdown-code-copy", cx));
    context.update(|_, cx| {
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some("let answer = 42;")
        )
    });
    // Cross-structure selection uses the same source/history, not a hosted text widget.
    first.update_in(&mut context, |view, window, cx| {
        let end = view.text().find("after$$").unwrap();
        view.select_range(start..end, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|_, cx| {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
            "replacement\n\n".into(),
        ));
    });
    context.dispatch_action(gpui_kit::component::input::Paste);
    settle(&mut context);
    assert!(first.read_with(&context, |view, _| {
        view.text().ends_with("after$$x$$literal  ")
    }));
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context);
    assert_eq!(bytes(&first, &context), original);
    let end = first.read_with(&context, |view, _| view.text().len());
    select(&first, end, &mut context);
    context.update(|window, cx| super::table_code::open_table_picker(&first, window, cx));
    settle(&mut context);
    context.update(|window, _| {
        assert!(
            window.try_find("markdown-table-picker").is_some(),
            "the slash command opens the native table picker without a toolbar duplicate"
        );
        assert_eq!(
            window.find("markdown-table-picker").focused(),
            Some(true),
            "the native picker owns keyboard focus"
        )
    });
    context.update(|window, cx| {
        window.press("right", cx);
        window.press("down", cx);
        window.press("enter", cx);
    });
    settle(&mut context);
    assert_ne!(bytes(&first, &context), original);
    first.update_in(&mut context, |view, window, cx| {
        view.focus_handle(cx).focus(window, cx)
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context);
    assert_eq!(bytes(&first, &context), original);
    // Both real presentations own their controls and edit one shared document.
    app.update_in(&mut context, |app, window, cx| {
        assert!(app.editor_split(window, cx))
    });
    settle(&mut context);
    let second = active(&app, &context);
    assert_ne!(first.entity_id(), second.entity_id());
    select(&second, start, &mut context);
    second.update_in(&mut context, |view, _, cx| view.insert_table_row(true, cx));
    settle(&mut context);
    assert_eq!(bytes(&first, &context), bytes(&second, &context));
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context);
    assert_eq!(bytes(&second, &context), original);
    // Insert a custom-sized table through the visible picker, cancel first, then commit.
    let end = second.read_with(&context, |view, _| view.text().len());
    select(&second, end, &mut context);
    context.update(|window, cx| super::table_code::open_table_picker(&second, window, cx));
    settle(&mut context);
    context.update(|window, cx| window.press("escape", cx));
    settle(&mut context);
    assert_eq!(bytes(&second, &context), original);
    context.update(|window, cx| super::table_code::open_table_picker(&second, window, cx));
    settle(&mut context);
    context.update(|window, cx| {
        window.click("markdown-table-rows", cx);
        window.press("secondary-a", cx);
        window.input("3", cx);
        window.click("markdown-table-columns", cx);
        window.press("secondary-a", cx);
        window.input("4", cx);
        window.click("markdown-table-insert-custom", cx);
    });
    settle(&mut context);
    let expected = bytes(&second, &context);
    assert!(expected.starts_with(original));
    save(&app, &mut context);
    assert_eq!(std::fs::read(&path).unwrap(), expected);
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context);
    assert_eq!(bytes(&second, &context), original);
    context.dispatch_action(gpui_kit::component::input::Redo);
    settle(&mut context);
    save(&app, &mut context);
    app.update_in(&mut context, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.editor_close_file(0, window, cx);
        app.editor_open_on_host(host, Path::new("/table-code.md"), window, cx);
    });
    until(&mut context, |context| {
        app.read_with(context, |app, _| app.active_buffer().is_some())
    });
    assert_eq!(bytes(&active(&app, &context), &context), expected);
}

#[cfg(test)]
#[gpui_kit::test]
fn table_code_workbench_controls_history_host_reopen(cx: &mut TestAppContext) {
    workflow(cx);
}

#[cfg(feature = "native-markdown-tests")]
pub(super) fn aligned_geometry(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("aligned.md");
    let original = "before  \r\n\n<!-- table:grid cols=160,220 -->\r\n| Left | Right |\n| :--- | ---: |\r\n| alpha | 中文 e\u{301} 🎉 |\n| wrapped Unicode 中文 👩‍💻 words words words words | z |\r\n\nafter$$x$$literal  ";
    std::fs::write(&path, original).unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    context.simulate_resize(gpui_kit::size(px(1400.), px(1000.)));
    app.update_in(&mut context, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    until(&mut context, |context| {
        app.read_with(context, |app, _| app.active_buffer().is_some())
    });
    let view = active(&app, &context);
    for token in ["alpha", "中文", "e\u{301}", "🎉", "👩‍💻", "words words"] {
        let at = view.read_with(&context, |view, _| view.text().find(token).unwrap());
        select(&view, at, &mut context);
        let position = context.update(|window, cx| {
            view.read(cx)
                .source_bounds(&(at..at), window)
                .unwrap()
                .center()
        });
        context.update(|window, cx| {
            window.pointer_move(position, None, cx);
            window.pointer_down(position, MouseButton::Left, cx);
            window.pointer_up(position, MouseButton::Left, cx);
        });
        settle(&mut context);
        view.read_with(&context, |view, _| {
            assert_eq!(view.cursor(), at, "native cell hit for {token}")
        });
        context.update(|window, cx| window.input("界", cx));
        settle(&mut context);
        context.dispatch_action(gpui_kit::component::input::Undo);
        settle(&mut context);
        assert_eq!(bytes(&view, &context), original.as_bytes());
    }
    let emoji = view.read_with(&context, |view, _| view.text().find("👩‍💻").unwrap());
    select(&view, emoji, &mut context);
    context.update(|window, cx| window.press("shift-right", cx));
    view.read_with(&context, |view, _| {
        assert_eq!(&view.text()[view.selected_range()], "👩‍💻")
    });
    context.dispatch_action(gpui_kit::component::input::Copy);
    context.update(|_, cx| {
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some("👩‍💻")
        )
    });
    let at = view.read_with(&context, |view, _| view.text().find("alpha").unwrap());
    select(&view, at, &mut context);
    // Resize the painted border itself, not a test-only width setter.
    let border = context.update(|window, cx| {
        let caret = view.read(cx).source_bounds(&(at..at), window).unwrap();
        gpui_kit::point(caret.left() - px(10.) + px(160.), caret.center().y)
    });
    context.update(|window, cx| {
        window.pointer_move(border, None, cx);
        window.pointer_down(border, MouseButton::Left, cx);
        window.pointer_move(
            border - gpui_kit::point(px(56.), px(0.)),
            Some(MouseButton::Left),
            cx,
        );
        window.pointer_up(
            border - gpui_kit::point(px(56.), px(0.)),
            MouseButton::Left,
            cx,
        );
    });
    settle(&mut context);
    let resized = bytes(&view, &context);
    assert_ne!(resized, original.as_bytes());
    assert!(resized.ends_with(b"\r\n\nafter$$x$$literal  "));
    table_command(&view, L10nKey::EditorTableFitColumn, &mut context);
    assert_ne!(bytes(&view, &context), resized);
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context);
    assert_eq!(bytes(&view, &context), resized);
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context);
    assert_eq!(bytes(&view, &context), original.as_bytes());
    save(&app, &mut context);
    assert_eq!(std::fs::read(&path).unwrap(), original.as_bytes());
}

#[cfg(feature = "native-markdown-tests")]
pub(super) fn wide_table(cx: &mut TestAppContext) {
    // The complete fixed 48-column, 64-body-row fixture; no truncated viewport sample.
    let mut original = String::from("# Complete wide table\n\n|");
    for column in 0..48 {
        original.push_str(&format!(" Column {column:02} |"));
    }
    original.push_str("\n|");
    for _ in 0..48 {
        original.push_str(" --- |");
    }
    original.push('\n');
    for row in 0..64 {
        original.push('|');
        for column in 0..48 {
            original.push_str(&format!(" R{row} C{column} 中文 👩‍💻 |"));
        }
        original.push('\n');
    }
    assert_eq!(
        original.len(),
        88_987,
        "retain the full pinned wide-table fixture"
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("wide-table.md");
    std::fs::write(&path, &original).unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    context.simulate_resize(gpui_kit::size(px(1200.), px(900.)));
    app.update_in(&mut context, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    until(&mut context, |context| {
        app.read_with(context, |app, _| app.active_buffer().is_some())
    });
    let view = active(&app, &context);
    let first = original.find("R0 C0").unwrap();
    select(&view, first, &mut context);
    let position = context.update(|window, cx| {
        view.read(cx)
            .source_bounds(&(first..first), window)
            .unwrap()
            .center()
    });
    context.update(|window, cx| {
        window.pointer_move(position, None, cx);
        window.pointer_down(position, MouseButton::Left, cx);
        window.pointer_up(position, MouseButton::Left, cx);
        window.input("界", cx);
    });
    settle(&mut context);
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context);
    assert_eq!(bytes(&view, &context), original.as_bytes());
    let page_offset = view.read_with(&context, |view, _| view.scroll_offset());
    let first_x = context.update(|window, cx| {
        view.read(cx)
            .source_bounds(&(first..first), window)
            .unwrap()
            .left()
    });
    // A horizontal wheel belongs to the wide native grid, never to a source-mode escape.
    context.update(|window, cx| {
        window.scroll(
            "markdown-document-scroll",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(-800.), px(0.))),
            cx,
        );
    });
    settle(&mut context);
    context.update(|window, cx| {
        assert!(
            view.read(cx)
                .source_bounds(&(first..first), window)
                .unwrap()
                .left()
                < first_x
        );
    });
    assert_eq!(
        view.read_with(&context, |view, _| view.scroll_offset()),
        page_offset,
        "horizontal wheel scrolls the table, not the containing document"
    );
    // Reach the actual far edge through the same wheel handler, then edit both
    // the first and last body rows while retaining every cell in the document.
    context.update(|window, cx| {
        window.scroll(
            "markdown-document-scroll",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(-100_000.), px(0.))),
            cx,
        );
    });
    settle(&mut context);
    for width in [1200., 1800.] {
        context.simulate_resize(gpui_kit::size(px(width), px(900.)));
        for token in ["R0 C47", "R63 C47"] {
            let at = original.find(token).unwrap();
            select(&view, at, &mut context);
            until(&mut context, |context| {
                context.update(|window, cx| {
                    view.read(cx)
                        .source_bounds(&(at..at), window)
                        .is_some_and(|bounds| {
                            bounds.left() > px(0.)
                                && bounds.right() < window.viewport_size().width
                                && bounds.top() > px(0.)
                                && bounds.bottom() < window.viewport_size().height
                        })
                })
            });
            let target = context.update(|window, cx| {
                view.read(cx)
                    .source_bounds(&(at..at), window)
                    .unwrap()
                    .center()
            });
            context.update(|window, cx| {
                window.pointer_move(target, None, cx);
                window.pointer_down(target, MouseButton::Left, cx);
                window.pointer_up(target, MouseButton::Left, cx);
            });
            assert_eq!(
                view.read_with(&context, |view, _| view.cursor()),
                at,
                "the visible far-edge cell maps to its exact source at {token}"
            );
            context.update(|window, cx| window.input("界", cx));
            settle(&mut context);
            let mut expected = original.clone();
            expected.insert_str(at, "界");
            assert_eq!(bytes(&view, &context), expected.as_bytes());
            save(&app, &mut context);
            assert_eq!(std::fs::read(&path).unwrap(), expected.as_bytes());
            context.dispatch_action(gpui_kit::component::input::Undo);
            settle(&mut context);
            assert_eq!(bytes(&view, &context), original.as_bytes());
        }
    }
    assert!(view.read_with(&context, |view, _| view.is_live()));
    assert_eq!(bytes(&view, &context), original.as_bytes());
    context.simulate_resize(gpui_kit::size(px(1800.), px(1000.)));
    settle(&mut context);
    save(&app, &mut context);
    assert_eq!(std::fs::read(&path).unwrap(), original.as_bytes());
    app.update_in(&mut context, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.open_file_in_editor(&path, window, cx);
    });
    until(&mut context, |context| {
        app.read_with(context, |app, _| app.active_buffer().is_some())
    });
    assert_eq!(
        bytes(&active(&app, &context), &context),
        original.as_bytes()
    );
}

#[cfg(feature = "native-markdown-tests")]
pub(super) fn native_cases() -> impl Iterator<Item = crate::native_markdown::Case> {
    [
        crate::native_markdown::Case::new("ui::code_editor::markdown::table_code::workbench_controls_history_host_reopen", workflow),
        crate::native_markdown::Case::new("ui::code_editor::markdown::source::review_tests::native_aligned_table_pointer_and_selection_follow_content", aligned_geometry),
        crate::native_markdown::Case::new("ui::code_editor::markdown::tests::table_presentation::table_presentation_drag_fit_scroll_edit_history_and_host_reopen", wide_table),
    ].into_iter()
}
