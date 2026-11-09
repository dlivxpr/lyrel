#[cfg(feature = "native-markdown-tests")]
use super::Selection;
use super::consumer_support::settle_live_geometry;
#[cfg(feature = "native-markdown-tests")]
use super::consumer_support::start_in_source;
use gpui_kit::{
    VisualContext as _,
    test::{TestEventExt as _, TestQueryExt as _, TestWindowExt as _},
};
#[cfg(feature = "native-markdown-tests")]
#[path = "tables/presentation_tests.rs"]
mod table_presentation;

#[cfg(feature = "native-markdown-tests")]
pub(super) fn native_cases() -> [crate::native_markdown::Case; 3] {
    [
        table_presentation::native_case(),
        crate::native_markdown::Case::new(
            "ui::code_editor::markdown::tests::native_navigation_home_end_and_words_use_visible_logical_content",
            native_navigation_home_end_and_words_use_visible_logical_content,
        ),
        crate::native_markdown::Case::new(
            "ui::code_editor::markdown::tests::native_text_color_picker_clear_geometry_and_shared_history",
            native_text_color_picker_clear_geometry_and_shared_history,
        ),
    ]
}
pub(super) fn native_text_color_picker_clear_geometry_and_shared_history(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use crate::ui::i18n::{L10nKey, t};
    use gpui_kit::{Focusable as _, MouseButton};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("foreground.md");
    let original = "\u{feff}A <span style=\"color:#123456\">**中🎉** tail</span> end\r\n";
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
            break view;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    settle_live_geometry(&view, &mut context);
    let source = original
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n");
    let start = source.find("**中🎉** tail").unwrap();
    let end = start + "**中🎉** tail".len();
    view.update_in(&mut context, |view, window, cx| {
        view.focus_handle(cx).focus(window, cx);
        view.select_range(start..end, cx);
    });
    settle_live_geometry(&view, &mut context);
    let identity = view.read_with(&context, |view, _| view.document().entity_id());
    let glyph = source.find('中').unwrap();
    let before = view.read_with(&context, |view, cx| {
        let bounds = view
            .range_to_bounds(&(glyph..glyph + "中🎉".len()), cx)
            .unwrap();
        assert_eq!(
            view.range_to_bounds(&(2..2), cx),
            view.range_to_bounds(&(start..start), cx)
        );
        bounds
    });
    context.update(|window, cx| window.click("markdown-format-menu", cx));
    context.run_until_parked();
    context.update(|window, cx| {
        let item = window.find_by_label(t(L10nKey::MarkdownFormatTextColor));
        assert_eq!(item.checked(), Some(true));
        window.pointer_down(item.bounds().center(), MouseButton::Left, cx);
        window.pointer_up(item.bounds().center(), MouseButton::Left, cx);
    });
    context.run_until_parked();
    context.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(
            view.read(cx).document().read(cx).encode().unwrap(),
            original.as_bytes()
        );
    });
    // The standard picker owns keyboard opening, hex input and confirmation.
    context.update(|window, cx| window.press("enter", cx));
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        );
    });
    context.update(|window, cx| {
        let input = window
            .find_all_by_role(gpui_kit::Role::TextInput)
            .into_iter()
            .find(|input| {
                input
                    .value()
                    .is_some_and(|value| value.eq_ignore_ascii_case("#123456"))
            })
            .expect("standard picker hex input");
        assert!(
            input.visible(),
            "hex entry must be a visible standard picker input"
        );
        window.pointer_down(input.bounds().center(), MouseButton::Left, cx);
        window.pointer_up(input.bounds().center(), MouseButton::Left, cx);
        window.press("secondary-a", cx);
        window.input("#7c3aed80", cx);
        assert_eq!(
            view.read(cx).document().read(cx).encode().unwrap(),
            original.as_bytes()
        );
        assert!(
            window
                .find_all_by_role(gpui_kit::Role::TextInput)
                .into_iter()
                .any(|input| {
                    input.visible()
                        && input
                            .value()
                            .is_some_and(|value| value.eq_ignore_ascii_case("#7c3aed80"))
                }),
            "standard picker accepts the public eight-digit RGBA hex value"
        );
        window.press("enter", cx);
    });
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        );
    });
    context.update(|window, cx| window.click("markdown-format-apply-color", cx));
    context.run_until_parked();
    let recolored = original.replace("#123456", "#7c3aed80");
    settle_live_geometry(&view, &mut context);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            recolored.as_bytes()
        );
        let source = view.document().read(cx).source().to_string();
        let glyph = source.find('中').unwrap();
        assert_eq!(
            view.range_to_bounds(&(glyph..glyph + "中🎉".len()), cx),
            Some(before)
        );
        assert_eq!(view.document().entity_id(), identity);
    });
    let normalized = recolored
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n");
    let glyph = normalized.find('中').unwrap();
    view.update_in(&mut context, |view, _, cx| {
        view.select_range(glyph..glyph + "中🎉".len(), cx)
    });
    context.dispatch_action(super::source::formats::CustomTextColor);
    context.run_until_parked();
    view.update_in(&mut context, |view, _, cx| {
        view.select_range(glyph..glyph, cx)
    });
    context.update(|window, cx| window.click("markdown-format-apply-color", cx));
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            recolored.as_bytes()
        )
    });
    context.update(|window, cx| {
        use gpui_kit::{ElementInputHandler, test::TestInput};
        let bounds = window.find("markdown-source").bounds();
        let mut input = TestInput::new(ElementInputHandler::new(bounds, view.clone()), window);
        input.compose_text(None, "ni", Some(2..2), cx);
        assert!(input.marked_text_range(cx).is_some());
        input.commit_text("你", cx);
        assert!(input.marked_text_range(cx).is_none());
    });
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            recolored.replacen("中🎉", "你中🎉", 1).as_bytes()
        );
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle_live_geometry(&view, &mut context);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            recolored.as_bytes()
        );
        assert_eq!(
            view.range_to_bounds(&(glyph..glyph + "中🎉".len()), cx),
            Some(before)
        );
    });
    let plain = normalized.find("end").unwrap();
    view.update_in(&mut context, |view, _, cx| {
        view.select_range(glyph..plain + 3, cx)
    });
    context.update(|window, cx| window.click("markdown-format-menu", cx));
    context.run_until_parked();
    context.update(|window, cx| {
        let mixed = t(L10nKey::MarkdownFormatMixed)
            .replace("{format}", t(L10nKey::MarkdownFormatTextColor));
        assert!(window.find_by_label(&mixed).visible());
        window.press("escape", cx);
    });
    context.run_until_parked();
    let tail = normalized.find("tail").unwrap();
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(tail..tail + 4, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.dispatch_action(super::source::formats::ClearTextColor);
    context.run_until_parked();
    let cleared = "\u{feff}A <span style=\"color:#7c3aed80\">**中🎉** </span>tail end\r\n";
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            cleared.as_bytes()
        );
        assert_eq!(
            view.selection().range(),
            tail + "</span>".len()..tail + "</span>".len() + 4
        );
    });
    for expected in [recolored.as_str(), original] {
        context.dispatch_action(gpui_kit::component::input::Undo);
        context.run_until_parked();
        view.read_with(&context, |view, cx| {
            assert_eq!(
                view.document().read(cx).encode().unwrap(),
                expected.as_bytes()
            )
        });
    }
    for expected in [recolored.as_str(), cleared] {
        context.dispatch_action(gpui_kit::component::input::Redo);
        context.run_until_parked();
        view.read_with(&context, |view, cx| {
            assert_eq!(
                view.document().read(cx).encode().unwrap(),
                expected.as_bytes()
            )
        });
    }
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    context.run_until_parked();
    context.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .find("markdown-source")
                .value()
                .unwrap()
                .contains("<span style=\"color:#7c3aed80\">")
        );
    });
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    settle_live_geometry(&view, &mut context);
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
    assert_eq!(std::fs::read(&path).unwrap(), cleared.as_bytes());
    view.read_with(&context, |view, _| {
        assert_eq!(view.document().entity_id(), identity)
    });
    // Both menu actions share the same focus handoff; Enter must open the
    // standard picker rather than replace the retained document selection.
    for key in [
        L10nKey::MarkdownFormatTextColor,
        L10nKey::MarkdownFormatHighlightColor,
    ] {
        let source = cleared.trim_start_matches('\u{feff}').replace("\r\n", "\n");
        let tail = source.find("tail").unwrap();
        view.update_in(&mut context, |view, window, cx| {
            view.select_range(tail..tail + 4, cx);
            view.focus_handle(cx).focus(window, cx);
        });
        context.update(|window, cx| window.click("markdown-format-menu", cx));
        context.run_until_parked();
        context.update(|window, cx| {
            let item = window.find_by_label(t(key));
            window.pointer_down(item.bounds().center(), MouseButton::Left, cx);
            window.pointer_up(item.bounds().center(), MouseButton::Left, cx);
        });
        context.run_until_parked();
        context.update(|window, cx| {
            window.render_frame(cx);
            window.press("enter", cx);
        });
        context.run_until_parked();
        view.read_with(&context, |view, cx| {
            assert_eq!(
                view.document().read(cx).encode().unwrap(),
                cleared.as_bytes()
            );
            assert_eq!(view.selection().range(), tail..tail + 4);
        });
        context.update(|window, cx| window.press("escape", cx));
        context.run_until_parked();
        context.update(|window, cx| window.click("markdown-format-cancel-color", cx));
        context.run_until_parked();
        context.update(|window, cx| {
            assert!(view.read(cx).focus_handle(cx).is_focused(window));
        });
    }
}
#[cfg(feature = "native-markdown-tests")]
fn native_navigation_home_end_and_words_use_visible_logical_content(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::{Focusable as _, test::TestWindowExt as _};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("navigation-words.md");
    let source = format!(
        "# Heading\n\n- **hello** world with enough text to wrap across several visual rows when narrow\n\n中文 English e\u{301} 👩‍💻\n\nlast\n\n{}e\u{301} tail",
        "x".repeat(4096)
    );
    std::fs::write(&path, &source).unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    cx.simulate_window_resize(
        context.window_handle(),
        gpui_kit::size(gpui_kit::px(620.), gpui_kit::px(800.)),
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
            start_in_source(&view, &mut context);
            break view;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    settle_live_geometry(&view, &mut context);
    let hello = source.find("hello").unwrap();
    let line_end = source.find("\n\n中文").unwrap();
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(hello..hello, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.dispatch_action(gpui_kit::component::input::SelectToNextWordEnd);
    view.read_with(&context, |view, _| {
        assert_eq!(view.selection(), Selection::new(hello, hello + 5))
    });
    context.update(|window, cx| {
        window.press("end", cx);
        assert_eq!(view.read(cx).selection(), Selection::caret(line_end));
        window.press("home", cx);
        assert_eq!(view.read(cx).selection(), Selection::caret(hello));
    });
    let unicode = source.find("中文").unwrap();
    view.update_in(&mut context, |view, _, cx| {
        view.select_range(unicode + "中文 ".len()..unicode + "中文 ".len(), cx)
    });
    context.update(|window, cx| {
        window.press("home", cx);
        assert_eq!(view.read(cx).selection(), Selection::caret(unicode));
        window.press("end", cx);
        assert_eq!(
            view.read(cx).selection(),
            Selection::caret(source.find("\n\nlast").unwrap())
        );
        assert_eq!(
            view.read(cx).document().read(cx).source().to_string(),
            source
        );
    });
    settle_live_geometry(&view, &mut context);
    view.update_in(&mut context, |view, _, cx| {
        view.select_range(unicode..unicode, cx)
    });
    context.update(|window, cx| {
        window.press("shift-right", cx);
        assert_eq!(
            view.read(cx).selection(),
            Selection::new(unicode, unicode + "中".len())
        );
        window.press("shift-left", cx);
        assert_eq!(view.read(cx).selection(), Selection::caret(unicode));
    });
    let token_start = source.rfind("\n\n").unwrap() + 2;
    let token_end = source.rfind(" tail").unwrap();
    let seam = view.read_with(&context, |view, cx| {
        let mut offset = 0;
        view.document()
            .read(cx)
            .source()
            .chunks()
            .find_map(|chunk| {
                offset += chunk.len();
                (token_start < offset && offset < token_end).then_some(offset)
            })
            .expect("long token spans Rope chunks")
    });
    view.update_in(&mut context, |view, _, cx| {
        view.select_range(seam..seam, cx)
    });
    context.run_until_parked();
    context.update(|window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        window.press("right", cx);
        assert_eq!(view.read(cx).selection(), Selection::caret(seam + 1));
        window.press("left", cx);
        assert_eq!(view.read(cx).selection(), Selection::caret(seam));
    });
    // Source word actions keep the complete Unicode word across real Rope
    // chunks, including a combining sequence after a long unbroken token.
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    context.run_until_parked();
    context.dispatch_action(gpui_kit::component::input::MoveToNextWord);
    view.read_with(&context, |view, _| {
        assert_eq!(view.selection(), Selection::caret(token_end))
    });
    context.dispatch_action(gpui_kit::component::input::MoveToPreviousWord);
    view.read_with(&context, |view, cx| {
        assert_eq!(view.selection(), Selection::caret(token_start));
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source.as_bytes()
        );
    });
}
