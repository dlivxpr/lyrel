use super::*;
use gpui_kit::test::TestWindowExt as _;

#[test]
fn presentations_share_history_and_new_edits_replace_the_redo_branch() {
    let mut document = MarkdownDocument::new("中🎉 e\u{301}\n".into(), TextFormat::default());
    let mut first = PresentationState::new(Selection::caret(0));
    let mut second = PresentationState::new(Selection::caret(7));
    let transaction = document.command(
        &mut first,
        Command::Replace {
            range: 0..0,
            text: "# ".into(),
            kind: EditKind::Paste,
        },
    );
    second.rebase(&transaction);
    assert_eq!(document.source().to_string(), "# 中🎉 e\u{301}\n");
    assert_eq!(second.selection(), Selection::caret(9));
    document.command(&mut second, Command::Undo);
    assert_eq!(document.source().to_string(), "中🎉 e\u{301}\n");
    assert_eq!(second.selection(), Selection::caret(0));
    document.command(
        &mut second,
        Command::Replace {
            range: 0..0,
            text: "> ".into(),
            kind: EditKind::Paste,
        },
    );
    document.command(&mut first, Command::Redo);
    assert_eq!(document.source().to_string(), "> 中🎉 e\u{301}\n");
    assert_eq!(document.revision(), 3);
}

#[test]
fn typing_groups_stop_at_selection_changes_and_undo_restores_selection() {
    let mut document = MarkdownDocument::new("".into(), TextFormat::default());
    let mut selection = PresentationState::new(Selection::caret(0));
    document.command(&mut selection, Command::Insert("中".into()));
    document.command(&mut selection, Command::Insert("🎉".into()));
    document.command(&mut selection, Command::Undo);
    assert_eq!(document.source().to_string(), "");
    assert_eq!(selection.selection(), Selection::caret(0));
    document.command(&mut selection, Command::Redo);
    assert_eq!(selection.selection(), Selection::caret(7));
    document.command(&mut selection, Command::Select(Selection::caret(0)));
    document.command(&mut selection, Command::Insert("x".into()));
    document.command(&mut selection, Command::Undo);
    assert_eq!(document.source().to_string(), "中🎉");
    assert_eq!(selection.selection(), Selection::caret(0));
}

#[test]
fn native_ranges_and_delete_never_split_surrogates_or_graphemes() {
    let mut document = MarkdownDocument::new("A🎉e\u{301}中".into(), TextFormat::default());
    assert_eq!(document.utf16_range(2..2), 1..5);
    let mut selection = PresentationState::new(Selection::caret(8));
    document.command(&mut selection, Command::Backspace);
    assert_eq!(document.source().to_string(), "A🎉中");
    document.command(&mut selection, Command::Undo);
    assert_eq!(document.source().to_string(), "A🎉e\u{301}中");
    assert_eq!(selection.selection(), Selection::caret(8));
}

#[test]
fn composition_commits_once_and_cancel_restores_the_replaced_source() {
    let mut document = MarkdownDocument::new("a🎉z".into(), TextFormat::default());
    let mut selection = PresentationState::new(Selection::new(1, 5));
    document.command(
        &mut selection,
        Command::Compose {
            range: None,
            text: "ni".into(),
            selected: Some(2..2),
        },
    );
    document.command(
        &mut selection,
        Command::Compose {
            range: None,
            text: "你".into(),
            selected: Some(1..1),
        },
    );
    assert_eq!(document.source().to_string(), "a你z");
    document.command(&mut selection, Command::Commit("你好".into()));
    document.command(&mut selection, Command::Undo);
    assert_eq!(document.source().to_string(), "a🎉z");
    assert_eq!(selection.selection(), Selection::new(1, 5));
    document.command(
        &mut selection,
        Command::Compose {
            range: None,
            text: "x".into(),
            selected: None,
        },
    );
    document.command(&mut selection, Command::CancelComposition);
    assert_eq!(document.source().to_string(), "a🎉z");
    assert_eq!(selection.selection(), Selection::new(1, 5));
    document.command(&mut selection, Command::Redo);
    assert_eq!(document.source().to_string(), "a你好z");
}

#[test]
fn composing_a_combining_mark_never_removes_the_preceding_grapheme() {
    let mut document = MarkdownDocument::new("e".into(), TextFormat::default());
    let mut view = PresentationState::new(Selection::caret(1));
    document.command(
        &mut view,
        Command::Compose {
            range: None,
            text: "\u{301}".into(),
            selected: None,
        },
    );
    document.command(
        &mut view,
        Command::Compose {
            range: None,
            text: "\u{302}".into(),
            selected: None,
        },
    );
    document.command(&mut view, Command::Commit("\u{302}".into()));
    assert_eq!(document.source().to_string(), "e\u{302}");
    document.command(&mut view, Command::Undo);
    assert_eq!(document.source().to_string(), "e");
    assert_eq!(view.selection(), Selection::caret(1));
}

#[test]
fn typing_in_another_presentation_starts_a_new_undo_group() {
    let mut document = MarkdownDocument::new(String::new(), TextFormat::default());
    let mut first = PresentationState::default();
    let mut second = PresentationState::new(Selection::caret(1));
    document.command(&mut first, Command::Insert("a".into()));
    document.command(&mut second, Command::Insert("b".into()));
    document.command(&mut first, Command::Undo);
    assert_eq!(document.source().to_string(), "a");
}

#[test]
fn prose_split_hard_break_and_cross_paragraph_replacement_share_source_history() {
    let mut document = MarkdownDocument::new(
        "# 标题\n\nfirst second\n\nlast".into(),
        TextFormat::default(),
    );
    let mut state = PresentationState::new(Selection::caret(16));
    document.command(&mut state, Command::ParagraphBreak { hard: false });
    assert_eq!(
        document.source().to_string(),
        "# 标题\n\nfirst \n\nsecond\n\nlast"
    );
    document.command(&mut state, Command::ParagraphBreak { hard: true });
    assert_eq!(
        document.source().to_string(),
        "# 标题\n\nfirst \n\n  \nsecond\n\nlast"
    );
    document.command(&mut state, Command::Undo);
    document.command(&mut state, Command::Undo);
    assert_eq!(
        document.source().to_string(),
        "# 标题\n\nfirst second\n\nlast"
    );
    assert_eq!(state.selection(), Selection::caret(16));
    document.command(
        &mut state,
        Command::Replace {
            range: 13..28,
            text: "正文".into(),
            kind: EditKind::Command,
        },
    );
    assert_eq!(document.source().to_string(), "# 标题\n\nfir正文");
    document.command(&mut state, Command::Undo);
    assert_eq!(
        document.source().to_string(),
        "# 标题\n\nfirst second\n\nlast"
    );
}

#[gpui_kit::test]
fn native_live_split_join_and_source_switch_keep_one_document_and_history(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{Entity, Focusable as _};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("prose.md");
    std::fs::write(&path, "# Heading\n\nalpha beta\n\nlast\n").unwrap();
    let (app, mut view_context, _streams) = harness_with_tabs(cx, 1);
    view_context.update(|_, cx| cx.set_global(super::super::MarkdownWorkbench));
    app.update_in(&mut view_context, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let presentation: Entity<SourcePresentation> = loop {
        view_context.run_until_parked();
        if let Some(view) = app.read_with(&view_context, |app, _| {
            app.active_editor_input()
                .and_then(|input| input.markdown().cloned())
        }) {
            break view;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "opening Markdown did not settle"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    let identity = presentation.read_with(&view_context, |view, _| view.document().entity_id());
    view_context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    settle_live_geometry(&presentation, &mut view_context);
    presentation.update_in(&mut view_context, |view, window, cx| {
        assert!(view.is_live());
        assert_eq!(view.document().entity_id(), identity);
        assert_eq!(view.document().read(cx).revision(), 0);
        assert_eq!(view.selection(), Selection::caret(2));
        view.focus_handle(cx).focus(window, cx);
    });
    view_context.update(|window, cx| window.input("新", cx));
    view_context.run_until_parked();
    presentation.read_with(&view_context, |view, cx| {
        assert_eq!(
            view.document().read(cx).source().to_string(),
            "# 新Heading\n\nalpha beta\n\nlast\n"
        );
    });
    view_context.dispatch_action(gpui_kit::component::input::Undo);
    view_context.run_until_parked();
    presentation.update_in(&mut view_context, |view, window, cx| {
        view.select_range(17..17, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    view_context.update(|window, cx| {
        window.render_frame(cx);
        window.input("中🎉", cx);
        window.press("enter", cx);
    });
    view_context.run_until_parked();
    presentation.read_with(&view_context, |view, cx| {
        assert_eq!(
            view.document().read(cx).source().to_string(),
            "# Heading\n\nalpha 中🎉\n\nbeta\n\nlast\n"
        );
    });
    view_context.update(|window, cx| window.press("backspace", cx));
    view_context.run_until_parked();
    presentation.read_with(&view_context, |view, cx| {
        assert_eq!(
            view.document().read(cx).source().to_string(),
            "# Heading\n\nalpha 中🎉beta\n\nlast\n"
        );
    });
    view_context.dispatch_action(gpui_kit::component::input::Undo);
    view_context.run_until_parked();
    let selection = presentation.read_with(&view_context, |view, _| view.selection());
    view_context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    view_context.run_until_parked();
    presentation.read_with(&view_context, |view, cx| {
        assert!(!view.is_live());
        assert_eq!(view.document().entity_id(), identity);
        assert_eq!(view.selection(), selection);
        assert_eq!(
            view.document().read(cx).source().to_string(),
            "# Heading\n\nalpha 中🎉\n\nbeta\n\nlast\n"
        );
    });
    view_context.dispatch_action(gpui_kit::component::input::Undo);
    view_context.dispatch_action(gpui_kit::component::input::Undo);
    view_context.run_until_parked();
    presentation.read_with(&view_context, |view, cx| {
        assert_eq!(
            view.document().read(cx).source().to_string(),
            "# Heading\n\nalpha beta\n\nlast\n"
        );
    });
    app.read_with(&view_context, |app, _| {
        assert!(!app.active_buffer().unwrap().dirty)
    });
    view_context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    settle_live_geometry(&presentation, &mut view_context);
    presentation.update_in(&mut view_context, |view, window, cx| {
        view.select_range(17..17, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    view_context.update(|window, cx| window.input("写", cx));
    view_context.run_until_parked();
    view_context.dispatch_action(crate::core::actions::EditorSave);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        view_context.run_until_parked();
        if app.read_with(&view_context, |app, _| {
            app.active_buffer()
                .is_some_and(|file| !file.dirty && !file.saving)
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "saving live Markdown did not settle"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        std::fs::read(&path).unwrap(),
        "# Heading\n\nalpha 写beta\n\nlast\n".as_bytes()
    );
    app.update_in(&mut view_context, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.open_file_in_editor(&path, window, cx);
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        view_context.run_until_parked();
        if app.read_with(&view_context, |app, _| app.active_buffer().is_some()) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "reopening live Markdown did not settle"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    app.read_with(&view_context, |app, cx| {
        let reopened = app
            .active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .read(cx);
        assert_ne!(reopened.document().entity_id(), identity);
        assert_eq!(
            reopened.document().read(cx).source().to_string(),
            "# Heading\n\nalpha 写beta\n\nlast\n"
        );
    });
}

fn settle_live_geometry(
    view: &gpui_kit::Entity<SourcePresentation>,
    cx: &mut gpui_kit::VisualTestContext,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(100));
        cx.run_until_parked();
        cx.update(|window, cx| window.render_frame(cx));
        if view.read_with(cx, |view, cx| {
            let start = view.range_to_bounds(&(0..0), cx);
            start.is_some() && start == view.range_to_bounds(&(2..2), cx)
        }) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "live heading geometry did not settle"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[gpui_kit::test]
fn native_live_pointer_selection_cuts_raw_source_across_adjacent_prose(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::{TestEventExt as _, TestWindowExt as _};
    use gpui_kit::{Modifiers, MouseButton};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("selection.md");
    std::fs::write(&path, "# Heading\n\nalpha beta\n\nlast\n").unwrap();
    let (app, mut view_context, _streams) = harness_with_tabs(cx, 1);
    view_context.update(|_, cx| cx.set_global(super::super::MarkdownWorkbench));
    app.update_in(&mut view_context, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let presentation = loop {
        view_context.run_until_parked();
        if let Some(view) = app.read_with(&view_context, |app, _| {
            app.active_editor_input()
                .and_then(|input| input.markdown().cloned())
        }) {
            break view;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    view_context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    settle_live_geometry(&presentation, &mut view_context);
    view_context.update(|window, cx| {
        let start = presentation
            .read(cx)
            .range_to_bounds(&(11..11), cx)
            .unwrap()
            .center();
        let end = presentation
            .read(cx)
            .range_to_bounds(&(25..25), cx)
            .unwrap()
            .center();
        window.pointer_down(start, MouseButton::Left, cx);
        window.pointer_up(start, MouseButton::Left, cx);
        window.change_modifiers(Modifiers::shift(), cx);
        window.pointer_down(end, MouseButton::Left, cx);
        window.pointer_up(end, MouseButton::Left, cx);
        window.change_modifiers(Modifiers::default(), cx);
        assert_eq!(presentation.read(cx).selection(), Selection::new(11, 25));
        window.press("secondary-x", cx);
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("alpha beta\n\nla".into())
        );
    });
    view_context.run_until_parked();
    presentation.read_with(&view_context, |view, cx| {
        assert_eq!(
            view.document().read(cx).source().to_string(),
            "# Heading\n\nst\n"
        )
    });
    view_context.dispatch_action(gpui_kit::component::input::Undo);
    view_context.run_until_parked();
    presentation.read_with(&view_context, |view, cx| {
        assert_eq!(
            view.document().read(cx).source().to_string(),
            "# Heading\n\nalpha beta\n\nlast\n"
        );
        assert_eq!(view.selection(), Selection::new(11, 25));
    });
}

#[gpui_kit::test]
fn native_live_break_mapping_resize_and_composition_keep_source_ranges(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::{TestEventExt as _, TestInput, TestWindowExt as _};
    use gpui_kit::{
        ElementInputHandler, EntityInputHandler as _, Focusable as _, MouseButton,
        VisualContext as _, px, size,
    };
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mapping.md");
    let source = format!(
        "# Heading\n\nA &amp; e\u{301}🎉\nsoft  \nhard\n\n\\*literal\\*\n\n---\n\n{}",
        "wrapping words ".repeat(240)
    );
    std::fs::write(&path, &source).unwrap();
    let (app, mut view_context, _streams) = harness_with_tabs(cx, 1);
    view_context.update(|_, cx| cx.set_global(super::super::MarkdownWorkbench));
    app.update_in(&mut view_context, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let presentation = loop {
        view_context.run_until_parked();
        if let Some(view) = app.read_with(&view_context, |app, _| {
            app.active_editor_input()
                .and_then(|input| input.markdown().cloned())
        }) {
            break view;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    view_context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    settle_live_geometry(&presentation, &mut view_context);
    view_context.update(|window, cx| {
        let view = presentation.read(cx);
        let before_soft = view.range_to_bounds(&(26..26), cx).unwrap();
        let after_soft = view.range_to_bounds(&(27..27), cx).unwrap();
        let after_hard = view.range_to_bounds(&(34..34), cx).unwrap();
        assert_eq!(
            before_soft.top(),
            after_soft.top(),
            "soft LF flows on the same visual line"
        );
        assert!(
            after_hard.top() > after_soft.top(),
            "two-space hard break starts a visual line"
        );
        let after_entity = view.range_to_bounds(&(18..18), cx).unwrap().center();
        window.pointer_down(after_entity, MouseButton::Left, cx);
        window.pointer_up(after_entity, MouseButton::Left, cx);
        window.press("backspace", cx);
    });
    view_context.run_until_parked();
    presentation.read_with(&view_context, |view, cx| {
        assert_eq!(
            view.document().read(cx).source().to_string(),
            source.replacen("&amp;", "", 1)
        )
    });
    view_context.dispatch_action(gpui_kit::component::input::Undo);
    view_context.run_until_parked();
    view_context.update(|window, cx| {
        window.render_frame(cx);
        let after_escape = presentation
            .read(cx)
            .range_to_bounds(&(42..42), cx)
            .unwrap()
            .center();
        window.pointer_down(after_escape, MouseButton::Left, cx);
        window.pointer_up(after_escape, MouseButton::Left, cx);
        window.press("backspace", cx);
    });
    view_context.run_until_parked();
    presentation.read_with(&view_context, |view, cx| {
        assert_eq!(
            view.document().read(cx).source().to_string(),
            source.replacen("\\*literal", "literal", 1)
        )
    });
    view_context.dispatch_action(gpui_kit::component::input::Undo);
    view_context.run_until_parked();
    presentation.update_in(&mut view_context, |view, window, cx| {
        view.select_range(22..22, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    view_context.update(|window, cx| window.press("backspace", cx));
    view_context.run_until_parked();
    presentation.read_with(&view_context, |view, cx| {
        assert_eq!(
            view.document().read(cx).source().to_string(),
            source.replacen("e\u{301}", "", 1)
        )
    });
    view_context.dispatch_action(gpui_kit::component::input::Undo);
    view_context.run_until_parked();
    let identity = presentation.read_with(&view_context, |view, _| view.document().entity_id());
    let window_handle = view_context.window_handle();
    cx.simulate_window_resize(window_handle, size(px(720.), px(640.)));
    view_context.update(|window, cx| {
        gpui_kit::component::Theme::update(cx, |theme| theme.font_size = px(18.));
        window.refresh();
    });
    presentation.update_in(&mut view_context, |view, window, cx| {
        view.select_range(158..158, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    view_context.update(|window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        window.scroll(
            "markdown-source",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-64.))),
            cx,
        );
        assert!(presentation.read(cx).scroll_offset().y < px(0.));
        let caret = presentation
            .read(cx)
            .range_to_bounds(&(158..158), cx)
            .unwrap();
        let native = presentation
            .update(cx, |view, cx| {
                view.character_index_for_point(caret.center(), window, cx)
            })
            .unwrap();
        assert_eq!(native, source[..158].encode_utf16().count());
        window.pointer_down(caret.center(), MouseButton::Left, cx);
        window.pointer_up(caret.center(), MouseButton::Left, cx);
        window.press("left", cx);
        window.input("X", cx);
    });
    view_context.run_until_parked();
    presentation.read_with(&view_context, |view, cx| {
        assert_eq!(
            view.document().read(cx).source().to_string(),
            format!("{}X{}", &source[..157], &source[157..])
        );
    });
    view_context.dispatch_action(gpui_kit::component::input::Undo);
    view_context.run_until_parked();
    presentation.update_in(&mut view_context, |view, window, cx| {
        view.select_range(22..22, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    view_context.update(|window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        let caret = presentation
            .read(cx)
            .range_to_bounds(&(22..22), cx)
            .unwrap();
        let native_offset = presentation
            .update(cx, |view, cx| {
                view.character_index_for_point(caret.center(), window, cx)
            })
            .unwrap();
        assert_eq!(
            native_offset,
            "# Heading\n\nA &amp; e\u{301}".encode_utf16().count()
        );
        let bounds = window.find("markdown-source").bounds();
        let handler = ElementInputHandler::new(bounds, presentation.clone());
        let mut input = TestInput::new(handler, window);
        input.compose_text(None, "ni", Some(2..2), cx);
        assert!(input.marked_text_range(cx).is_some());
        input.compose_text(None, "你🎉", Some(3..3), cx);
        input.commit_text("你好🎉", cx);
        assert!(input.marked_text_range(cx).is_none());
    });
    view_context.run_until_parked();
    presentation.read_with(&view_context, |view, cx| {
        assert_eq!(view.document().entity_id(), identity);
        assert_eq!(
            view.document().read(cx).source().to_string(),
            source.replacen("e\u{301}🎉", "e\u{301}你好🎉🎉", 1)
        );
    });
    view_context.dispatch_action(gpui_kit::component::input::Undo);
    view_context.run_until_parked();
    presentation.read_with(&view_context, |view, cx| {
        assert_eq!(view.document().read(cx).source().to_string(), source)
    });
}

#[test]
fn deleting_and_composing_across_mixed_terminators_restores_exact_saved_bytes() {
    let original = b"a\r\nb\nc\r\ntail  ";
    let decoded = editor_text::decode_source(original).unwrap().unwrap();
    let mut document = MarkdownDocument::new(decoded.text, decoded.format);
    let mut view = PresentationState::new(Selection::new(1, 5));
    document.command(&mut view, Command::Delete);
    assert_eq!(document.source().to_string(), "a\ntail  ");
    assert_eq!(document.encode().unwrap(), b"a\r\ntail  ");
    document.command(&mut view, Command::Undo);
    assert_eq!(document.encode().unwrap(), original);
    document.command(&mut view, Command::Redo);
    assert_eq!(document.encode().unwrap(), b"a\r\ntail  ");
    document.command(&mut view, Command::Undo);
    document.command(
        &mut view,
        Command::Compose {
            range: None,
            text: "replacement\n".into(),
            selected: None,
        },
    );
    document.command(&mut view, Command::CancelComposition);
    assert_eq!(document.encode().unwrap(), original);
    document.command(
        &mut view,
        Command::Compose {
            range: None,
            text: "replacement\n".into(),
            selected: None,
        },
    );
    document.command(&mut view, Command::Commit("done\n".into()));
    assert_eq!(document.encode().unwrap(), b"adone\r\n\r\ntail  ");
    document.command(&mut view, Command::Undo);
    assert_eq!(document.encode().unwrap(), original);
}

#[test]
fn changing_only_a_terminator_is_an_undoable_edit_and_reload_replaces_its_representation() {
    let mut document = MarkdownDocument::new("a\nb\r\nc".into(), TextFormat::default());
    let mut view = PresentationState::default();
    document.command(
        &mut view,
        Command::Replace {
            range: 1..2,
            text: "\r\n".into(),
            kind: EditKind::Command,
        },
    );
    assert_eq!(document.source().to_string(), "a\nb\nc");
    assert_eq!(document.encode().unwrap(), b"a\r\nb\r\nc");
    document.command(&mut view, Command::Undo);
    assert_eq!(document.encode().unwrap(), b"a\nb\r\nc");
    document.command(
        &mut view,
        Command::Reload {
            text: "a\r\nb\nc".into(),
            format: TextFormat::default(),
        },
    );
    document.command(&mut view, Command::Undo);
    assert_eq!(document.encode().unwrap(), b"a\r\nb\nc");
}

#[test]
fn sparse_replacements_preserve_between_match_bytes_and_undo_as_one_command() {
    let original = b"cat\r\n* keep  \ncat\r\n[ref]: /target\ncat";
    let decoded = editor_text::decode_source(original).unwrap().unwrap();
    let mut document = MarkdownDocument::new(decoded.text, decoded.format);
    let before = Selection::new(0, 3);
    let mut view = PresentationState::new(before);
    document.command(
        &mut view,
        Command::BatchReplace {
            edits: vec![
                (0..3, "kitten".into()),
                (13..16, "kitten".into()),
                (32..35, "kitten".into()),
            ],
            kind: EditKind::Command,
        },
    );
    assert_eq!(
        document.encode().unwrap(),
        b"kitten\r\n* keep  \nkitten\r\n[ref]: /target\nkitten"
    );
    document.command(&mut view, Command::Undo);
    assert_eq!(document.encode().unwrap(), original);
    assert_eq!(view.selection(), before);
    document.command(&mut view, Command::Redo);
    assert_eq!(
        document.encode().unwrap(),
        b"kitten\r\n* keep  \nkitten\r\n[ref]: /target\nkitten"
    );
}

#[gpui_kit::test]
fn native_code_regions_keep_line_breaks_fences_and_shared_history(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::Focusable as _;
    use gpui_kit::test::TestWindowExt as _;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("code.md");
    let source = "# Heading\n\n```rust\nlet x = 1;\n```\n\n    one\n    \n    two\n\nlast";
    std::fs::write(&path, source).unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    context.update(|_, cx| cx.set_global(super::super::MarkdownWorkbench));
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
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    settle_live_geometry(&view, &mut context);
    let identity = view.read_with(&context, |view, _| view.document().entity_id());
    let code_at = source.find("let").unwrap();
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(code_at..code_at, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| {
        use gpui_kit::ElementInputHandler;
        use gpui_kit::test::TestInput;
        window.render_frame(cx);
        let code_line = view
            .read(cx)
            .range_to_bounds(&(code_at..code_at), cx)
            .unwrap();
        let fence_at = source.find("```\n\n").unwrap();
        let closing_fence = view
            .read(cx)
            .range_to_bounds(&(fence_at..fence_at), cx)
            .unwrap();
        assert!(
            closing_fence.top() > code_line.top(),
            "code newlines must retain distinct visual lines"
        );
        let bounds = window.find("markdown-source").bounds();
        let mut input = TestInput::new(ElementInputHandler::new(bounds, view.clone()), window);
        input.compose_text(None, "ni", Some(2..2), cx);
        assert!(input.marked_text_range(cx).is_some());
        input.compose_text(None, "", Some(0..0), cx);
        assert!(input.marked_text_range(cx).is_none());
    });
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).source().to_string(), source)
    });
    for (at, removed, insertion) in [
        (source.find("let").unwrap() + 4, 0, "\n"),
        (source.find("one").unwrap() + 3, 0, "\n    "),
        (source.find("    \n").unwrap() + 4, 4, "\n"),
        (source.find("```\n\n").unwrap() + 3, 0, "\n\n"),
    ] {
        view.update_in(&mut context, |view, window, cx| {
            view.select_range(at..at, cx);
            view.focus_handle(cx).focus(window, cx);
        });
        context.update(|window, cx| window.press("enter", cx));
        context.run_until_parked();
        view.read_with(&context, |view, cx| {
            assert_eq!(
                view.document().read(cx).source().to_string(),
                format!("{}{}{}", &source[..at - removed], insertion, &source[at..])
            );
        });
        context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
        context.dispatch_action(gpui_kit::component::input::Undo);
        context.run_until_parked();
        view.read_with(&context, |view, cx| {
            assert_eq!(view.document().entity_id(), identity);
            assert_eq!(view.document().read(cx).source().to_string(), source);
        });
        context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
        settle_live_geometry(&view, &mut context);
    }
    let boundary = source.find("last").unwrap();
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(boundary..boundary, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| window.press("backspace", cx));
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).source().to_string(),
            source.replacen("\n\nlast", "\nlast", 1)
        );
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    let from = source.find("```rust").unwrap();
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(from..source.len(), cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| {
        window.press("secondary-x", cx);
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().as_deref(),
            Some(&source[from..])
        );
    });
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).source().to_string(),
            "# Heading\n\n"
        )
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    let at = source.find("one").unwrap();
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(at..at + 3, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| window.input("中🎉", cx));
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).source().to_string(),
            source.replacen("one", "中🎉", 1)
        );
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).source().to_string(), source)
    });
    context.dispatch_action(gpui_kit::component::input::Redo);
    context.dispatch_action(crate::core::actions::EditorSave);
    let expected = source.replacen("one", "中🎉", 1);
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
        assert_eq!(reopened.document().read(cx).source().to_string(), expected);
    });
}

#[gpui_kit::test]
fn native_metadata_html_and_invalid_regions_edit_save_and_reopen_without_source_loss(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::{TestInput, TestWindowExt as _};
    use gpui_kit::{ElementInputHandler, Focusable as _};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source-regions.md");
    let source = "---\r\ntitle: demo\n---\r\n\n# Heading\n\n<script>neverRun()</script>\n\n<Unknown value={projectCode()} />\n\n[broken](\n";
    std::fs::write(&path, source).unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    context.update(|_, cx| cx.set_global(super::super::MarkdownWorkbench));
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
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    let normalized = source.replace("\r\n", "\n");
    let heading = normalized.find("# Heading").unwrap();
    loop {
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
        context.run_until_parked();
        context.update(|window, cx| window.render_frame(cx));
        if view.read_with(&context, |view, cx| {
            view.range_to_bounds(&(heading..heading), cx).is_some()
                && view.range_to_bounds(&(heading..heading), cx)
                    == view.range_to_bounds(&(heading + 2..heading + 2), cx)
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let at = normalized.find("demo").unwrap() + 4;
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(at..at, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| {
        window.press("enter", cx);
        window.input("author: writer", cx);
    });
    context.run_until_parked();
    let metadata_edit = normalized.replacen("title: demo", "title: demo\nauthor: writer", 1);
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).source().to_string(), metadata_edit)
    });
    let at = metadata_edit.find("neverRun").unwrap();
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(at..at + 8, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| {
        window.render_frame(cx);
        let bounds = window.find("markdown-source").bounds();
        let mut input = TestInput::new(ElementInputHandler::new(bounds, view.clone()), window);
        input.compose_text(None, "ni", Some(2..2), cx);
        assert!(input.marked_text_range(cx).is_some());
        input.commit_text("保留🎉", cx);
        assert!(input.marked_text_range(cx).is_none());
    });
    context.run_until_parked();
    let expected = metadata_edit.replacen("neverRun", "保留🎉", 1);
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).source().to_string(), expected)
    });
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).source().to_string(), metadata_edit)
    });
    context.dispatch_action(gpui_kit::component::input::Redo);
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
    assert_eq!(
        std::fs::read(&path).unwrap(),
        source
            .replacen("title: demo", "title: demo\nauthor: writer", 1)
            .replacen("neverRun", "保留🎉", 1)
            .as_bytes()
    );
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
        assert_eq!(reopened.document().read(cx).source().to_string(), expected);
    });
}

#[gpui_kit::test]
fn native_inline_reveal_is_local_and_typing_preserves_delimiters_and_history(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::{TestEventExt as _, TestWindowExt as _};
    use gpui_kit::{Focusable as _, MouseButton};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("inline.md");
    let original = "# Heading\n\nA **bold** and *soft* plus ~~gone~~ `code`.\n";
    std::fs::write(&path, original).unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    context.update(|_, cx| cx.set_global(super::super::MarkdownWorkbench));
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
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    settle_live_geometry(&view, &mut context);
    context.update(|window, cx| {
        let presentation = view.read(cx);
        assert_eq!(
            presentation.range_to_bounds(&(13..13), cx),
            presentation.range_to_bounds(&(15..15), cx)
        );
        let point = presentation
            .range_to_bounds(&(16..16), cx)
            .unwrap()
            .center();
        window.pointer_down(point, MouseButton::Left, cx);
        window.pointer_up(point, MouseButton::Left, cx);
    });
    context.update(|window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
    });
    view.read_with(&context, |view, cx| {
        assert_eq!(view.selection(), Selection::caret(16));
        assert!(
            view.range_to_bounds(&(15..15), cx).unwrap().left()
                > view.range_to_bounds(&(13..13), cx).unwrap().left()
        );
        assert_eq!(
            view.range_to_bounds(&(26..26), cx),
            view.range_to_bounds(&(27..27), cx)
        );
        assert_eq!(view.document().read(cx).source().to_string(), original);
    });
    app.read_with(&context, |app, _| {
        assert!(!app.active_buffer().unwrap().dirty)
    });
    context.update(|window, cx| window.input("中", cx));
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.replacen("bold", "b中old", 1).as_bytes()
        );
    });
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
    assert_eq!(
        std::fs::read(&path).unwrap(),
        original.replacen("bold", "b中old", 1).as_bytes()
    );
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    for (at, expected) in [
        (13, "A X**bold**"),
        (15, "A **Xbold**"),
        (19, "A **boldX**"),
        (21, "A **bold**X"),
    ] {
        view.update_in(&mut context, |view, _, cx| view.select_range(at..at, cx));
        context.update(|window, cx| window.input("X", cx));
        context.run_until_parked();
        view.read_with(&context, |view, cx| {
            assert_eq!(
                view.document().read(cx).encode().unwrap(),
                original.replacen("A **bold**", expected, 1).as_bytes()
            )
        });
        context.dispatch_action(gpui_kit::component::input::Undo);
        context.run_until_parked();
    }
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(11..11, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
    });
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        );
        assert_eq!(
            view.range_to_bounds(&(13..13), cx),
            view.range_to_bounds(&(15..15), cx)
        );
    });
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
    assert_eq!(std::fs::read(&path).unwrap(), original.as_bytes());
}

#[gpui_kit::test]
fn native_nested_inline_boundaries_entities_and_composition_preserve_source(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::{TestEventExt as _, TestInput, TestWindowExt as _};
    use gpui_kit::{ElementInputHandler, EntityInputHandler as _, Focusable as _, MouseButton};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("nested.md");
    let original = "# Heading\n\n**outer _e\u{301} &amp; \\*_ ~~gone~~ `a\nb`** tail\n";
    std::fs::write(&path, original).unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    context.update(|_, cx| cx.set_global(super::super::MarkdownWorkbench));
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
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    settle_live_geometry(&view, &mut context);
    // A click at the hidden closing edge inserts inside the formatted span.
    let closing = original.find("** tail").unwrap();
    context.update(|window, cx| {
        let point = view
            .read(cx)
            .range_to_bounds(&(closing..closing), cx)
            .unwrap()
            .center();
        window.pointer_down(point, MouseButton::Left, cx);
        window.pointer_up(point, MouseButton::Left, cx);
        assert_eq!(view.read(cx).selection(), Selection::caret(closing));
        window.input("X", cx);
    });
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.replacen("** tail", "X** tail", 1).as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    let entity = original.find("&amp;").unwrap();
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(entity + 5..entity + 5, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| window.press("backspace", cx));
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.replacen("&amp;", "", 1).as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    let escape = original.find("\\*").unwrap();
    view.update_in(&mut context, |view, _, cx| {
        view.select_range(escape + 2..escape + 2, cx)
    });
    context.update(|window, cx| window.press("backspace", cx));
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.replacen("\\*", "", 1).as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    let unicode = original.find("e\u{301}").unwrap();
    view.update_in(&mut context, |view, _, cx| {
        view.select_range(unicode..unicode + "e\u{301}".len(), cx)
    });
    context.update(|window, cx| {
        let handler =
            ElementInputHandler::new(window.find("markdown-source").bounds(), view.clone());
        let mut input = TestInput::new(handler, window);
        input.compose_text(None, "&amp;", Some(5..5), cx);
    });
    context
        .executor()
        .advance_clock(std::time::Duration::from_millis(100));
    context.run_until_parked();
    context.update(|window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        assert!(view.read(cx).is_composing());
        assert!(
            view.read(cx)
                .range_to_bounds(&(unicode + 1..unicode + 1), cx)
                .unwrap()
                .left()
                > view
                    .read(cx)
                    .range_to_bounds(&(unicode..unicode), cx)
                    .unwrap()
                    .left(),
            "preedit entity syntax stays literal after parse"
        );
        let marked = view
            .update(cx, |view, cx| view.marked_text_range(window, cx))
            .unwrap();
        let surface = window.find("markdown-source").bounds();
        let bounds = view.update(cx, |view, cx| {
            view.bounds_for_range(marked, surface, window, cx)
        });
        assert!(
            bounds.is_some(),
            "async parse retains native candidate geometry"
        );
        let handler =
            ElementInputHandler::new(window.find("markdown-source").bounds(), view.clone());
        let mut input = TestInput::new(handler, window);
        input.compose_text(None, "你🎉", Some(3..3), cx);
        input.commit_text("你好🎉", cx);
    });
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.replacen("e\u{301}", "你好🎉", 1).as_bytes()
        )
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

#[test]
fn list_commands_continue_split_exit_and_undo_without_reformatting_neighbors() {
    let original = "* root\n  7) **child** tail\n+ neighbor\n\n> > quote";
    let mut document = MarkdownDocument::new(original.into(), TextFormat::default());
    let mut state = PresentationState::new(Selection::caret(21));
    document.command(&mut state, Command::ListBreak { hard: false, at: None });
    assert_eq!(document.source().to_string(), "* root\n  7) **child**\n  8)  tail\n+ neighbor\n\n> > quote");
    assert_eq!(state.selection(), Selection::caret(27));
    document.command(&mut state, Command::Undo);
    assert_eq!(document.source().to_string(), original);
    assert_eq!(state.selection(), Selection::caret(21));
    document.command(&mut state, Command::Select(Selection::caret(original.len())));
    document.command(&mut state, Command::ListBreak { hard: false, at: None });
    assert_eq!(document.source().to_string(), "* root\n  7) **child** tail\n+ neighbor\n\n> > quote\n> > ");
    document.command(&mut state, Command::ListBreak { hard: false, at: None });
    assert_eq!(document.source().to_string(), "* root\n  7) **child** tail\n+ neighbor\n\n> > quote\n> ");
    document.command(&mut state, Command::ListBreak { hard: false, at: None });
    assert_eq!(document.source().to_string(), "* root\n  7) **child** tail\n+ neighbor\n\n> > quote\n");
}

#[gpui_kit::test]
fn native_nested_lists_tasks_quotes_share_input_history_and_host_save_reopen(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::{TestEventExt as _, TestQueryExt as _, TestInput, TestWindowExt as _};
    use gpui_kit::{ElementInputHandler, Focusable as _, MouseButton};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("lists.md");
    let original = "* root\r\n  7) [ ] **child**\n+ neighbor\r\n\n> > *quote*\n";
    std::fs::write(&path, original).unwrap();
    let normalized = original.replace("\r\n", "\n");
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    context.update(|_, cx| cx.set_global(super::super::MarkdownWorkbench));
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
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    settle_live_geometry(&view, &mut context);
    let at = normalized.find("**child**").unwrap() + 9;
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(at..at, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| window.press("enter", cx));
    context.run_until_parked();
    view.read_with(&context, |view, cx| assert_eq!(view.document().read(cx).source().to_string(), "* root\n  7) [ ] **child**\n  8) [ ] \n+ neighbor\n\n> > *quote*\n"));
    context.update(|window, cx| window.press("enter", cx));
    context.run_until_parked();
    view.read_with(&context, |view, cx| assert_eq!(view.document().read(cx).source().to_string(), "* root\n  7) [ ] **child**\n8) [ ] \n+ neighbor\n\n> > *quote*\n"));
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    let selection = view.read_with(&context, |view, _| view.selection());
    settle_live_geometry(&view, &mut context);
    context.update(|window, cx| {
        let checkbox = window.find_by_label("Toggle task: child").bounds().center();
        window.pointer_down(checkbox, MouseButton::Left, cx);
        window.pointer_up(checkbox, MouseButton::Left, cx);
    });
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).source().to_string(), "* root\n  7) [x] **child**\n+ neighbor\n\n> > *quote*\n");
        assert_eq!(view.selection(), selection);
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).source().to_string(), normalized);
        assert_eq!(view.selection(), selection);
    });
    view.update_in(&mut context, |view, window, cx| {
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| window.press("secondary-enter", cx));
    context.run_until_parked();
    view.read_with(&context, |view, cx| assert_eq!(view.document().read(cx).source().to_string(), "* root\n  7) [x] **child**\n+ neighbor\n\n> > *quote*\n"));
    let at = normalized.find("*quote*").unwrap() + 1;
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(at..at, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| {
        window.render_frame(cx);
        let bounds = window.find("markdown-source").bounds();
        let mut input = TestInput::new(ElementInputHandler::new(bounds, view.clone()), window);
        input.compose_text(None, "ni", Some(2..2), cx);
        assert!(input.marked_text_range(cx).is_some());
        input.commit_text("中文", cx);
    });
    context.run_until_parked();
    let expected = "* root\n  7) [x] **child**\n+ neighbor\n\n> > *中文quote*\n";
    view.read_with(&context, |view, cx| assert_eq!(view.document().read(cx).source().to_string(), expected));
    context.dispatch_action(crate::core::actions::EditorSave);
    loop {
        context.run_until_parked();
        if app.read_with(&context, |app, _| app.active_buffer().is_some_and(|file| !file.dirty && !file.saving)) { break; }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(std::fs::read(&path).unwrap(), "* root\r\n  7) [x] **child**\n+ neighbor\r\n\n> > *中文quote*\n".as_bytes());
    app.update_in(&mut context, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.open_file_in_editor(&path, window, cx);
    });
    loop {
        context.run_until_parked();
        if app.read_with(&context, |app, _| app.active_buffer().is_some()) { break; }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    app.read_with(&context, |app, cx| {
        let reopened = app.active_editor_input().unwrap().markdown().unwrap().read(cx);
        assert_eq!(reopened.document().read(cx).source().to_string(), expected);
    });
}

#[test]
fn list_indent_moves_only_the_target_subtree_and_restores_selection_on_undo() {
    let original = "- first\n+ target\n  4) **nested**\n* neighbor\n";
    let at = original.find("target").unwrap();
    let mut document = MarkdownDocument::new(original.into(), TextFormat::default());
    let mut state = PresentationState::new(Selection::caret(at));
    document.command(&mut state, Command::ListIndent { outdent: false, width: 4, at: None });
    assert_eq!(document.source().to_string(), "- first\n  + target\n    4) **nested**\n* neighbor\n");
    assert_eq!(state.selection(), Selection::caret(at + 2));
    document.command(&mut state, Command::Undo);
    assert_eq!(document.source().to_string(), original);
    assert_eq!(state.selection(), Selection::caret(at));
    document.command(&mut state, Command::Redo);
    document.command(&mut state, Command::ListIndent { outdent: true, width: 4, at: None });
    assert_eq!(document.source().to_string(), original);
    assert_eq!(state.selection(), Selection::caret(at));
}

#[test]
fn list_backspace_outdents_then_joins_and_task_toggle_is_one_history_entry() {
    let original = "* first\n  + [X] **task**\n- last";
    let at = original.find("**task**").unwrap();
    let mut document = MarkdownDocument::new(original.into(), TextFormat::default());
    let mut state = PresentationState::new(Selection::caret(at));
    document.command(&mut state, Command::ToggleTask { at });
    assert_eq!(document.source().to_string(), "* first\n  + [ ] **task**\n- last");
    assert_eq!(state.selection(), Selection::caret(at));
    document.command(&mut state, Command::Undo);
    assert_eq!(document.source().to_string(), original);
    assert_eq!(state.selection(), Selection::caret(at));
    document.command(&mut state, Command::ListBackspace);
    assert_eq!(document.source().to_string(), "* first\n+ [X] **task**\n- last");
    assert_eq!(state.selection(), Selection::caret(at - 2));
    document.command(&mut state, Command::ListBackspace);
    assert_eq!(document.source().to_string(), "* first**task**\n- last");
    document.command(&mut state, Command::Undo);
    assert_eq!(document.source().to_string(), "* first\n+ [X] **task**\n- last");
    assert_eq!(state.selection(), Selection::caret(at - 2));
    document.command(&mut state, Command::Undo);
    assert_eq!(document.source().to_string(), original);
    assert_eq!(state.selection(), Selection::caret(at));
}

#[test]
fn quote_inside_list_continues_without_creating_a_sibling_and_exits_one_layer() {
    let mut document = MarkdownDocument::new("- > **quote**\n+ untouched".into(), TextFormat::default());
    let mut state = PresentationState::new(Selection::caret(13));
    document.command(&mut state, Command::ListBreak { hard: false, at: None });
    assert_eq!(document.source().to_string(), "- > **quote**\n  > \n+ untouched");
    document.command(&mut state, Command::ListBreak { hard: false, at: None });
    assert_eq!(document.source().to_string(), "- > **quote**\n\n+ untouched");
    document.command(&mut state, Command::Undo);
    assert_eq!(document.source().to_string(), "- > **quote**\n  > \n+ untouched");
    assert_eq!(state.selection(), Selection::caret(18));
}

#[test]
fn list_backspace_keeps_quote_ancestry_and_does_not_join_into_a_fence() {
    let mut document = MarkdownDocument::new("> > - task".into(), TextFormat::default());
    let mut state = PresentationState::new(Selection::caret(6));
    document.command(&mut state, Command::ListBackspace);
    assert_eq!(document.source().to_string(), "> > task");
    assert_eq!(state.selection(), Selection::caret(4));
    document.command(&mut state, Command::ListBackspace);
    assert_eq!(document.source().to_string(), "> task");
    assert_eq!(state.selection(), Selection::caret(2));
    let original = "```rust\ncode\n```\n- task";
    let mut document = MarkdownDocument::new(original.into(), TextFormat::default());
    let mut state = PresentationState::new(Selection::caret(original.find("task").unwrap()));
    document.command(&mut state, Command::ListBackspace);
    assert_eq!(document.source().to_string(), "```rust\ncode\n```\ntask");
    document.command(&mut state, Command::Undo);
    assert_eq!(document.source().to_string(), original);
    assert_eq!(state.selection(), Selection::caret(original.find("task").unwrap()));
}

#[test]
fn editing_a_list_continuation_keeps_its_owning_item_and_neighbor_markers() {
    let original = "- first\n+ target\n  continued\n  * nested\n* neighbor";
    let at = original.find("continued").unwrap() + 9;
    let owner = original.find("+ target").unwrap();
    let mut document = MarkdownDocument::new(original.into(), TextFormat::default());
    let mut state = PresentationState::new(Selection::caret(at));
    document.command(&mut state, Command::ListBreak { hard: false, at: Some(owner) });
    assert_eq!(document.source().to_string(), "- first\n+ target\n  continued\n+ \n  * nested\n* neighbor");
    document.command(&mut state, Command::Undo);
    assert_eq!(document.source().to_string(), original);
    assert_eq!(state.selection(), Selection::caret(at));
    document.command(&mut state, Command::ListIndent { outdent: false, width: 4, at: Some(owner) });
    assert_eq!(document.source().to_string(), "- first\n  + target\n    continued\n    * nested\n* neighbor");
    assert_eq!(state.selection(), Selection::caret(at + 4));
    document.command(&mut state, Command::Undo);
    assert_eq!(document.source().to_string(), original);
    assert_eq!(state.selection(), Selection::caret(at));
}
