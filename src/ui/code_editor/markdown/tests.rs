use super::*;
use gpui_kit::{VisualContext as _, test::TestWindowExt as _};

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
    document.command(
        &mut state,
        Command::ListBreak {
            hard: false,
            at: None,
        },
    );
    assert_eq!(
        document.source().to_string(),
        "* root\n  7) **child**\n  8)  tail\n+ neighbor\n\n> > quote"
    );
    assert_eq!(state.selection(), Selection::caret(27));
    document.command(&mut state, Command::Undo);
    assert_eq!(document.source().to_string(), original);
    assert_eq!(state.selection(), Selection::caret(21));
    document.command(
        &mut state,
        Command::Select(Selection::caret(original.len())),
    );
    document.command(
        &mut state,
        Command::ListBreak {
            hard: false,
            at: None,
        },
    );
    assert_eq!(
        document.source().to_string(),
        "* root\n  7) **child** tail\n+ neighbor\n\n> > quote\n> > "
    );
    document.command(
        &mut state,
        Command::ListBreak {
            hard: false,
            at: None,
        },
    );
    assert_eq!(
        document.source().to_string(),
        "* root\n  7) **child** tail\n+ neighbor\n\n> > quote\n> "
    );
    document.command(
        &mut state,
        Command::ListBreak {
            hard: false,
            at: None,
        },
    );
    assert_eq!(
        document.source().to_string(),
        "* root\n  7) **child** tail\n+ neighbor\n\n> > quote\n"
    );
}

#[gpui_kit::test]
fn native_nested_lists_tasks_quotes_share_input_history_and_host_save_reopen(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::{TestEventExt as _, TestInput, TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{ElementInputHandler, Focusable as _, MouseButton};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("lists.md");
    let original = "* root\r\n  7) [ ] **child**\n+ neighbor\r\n\n> > *quote*\n";
    std::fs::write(&path, original).unwrap();
    let normalized = original.replace("\r\n", "\n");
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
    let at = normalized.find("**child**").unwrap() + 9;
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(at..at, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| window.press("enter", cx));
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).source().to_string(),
            "* root\n  7) [ ] **child**\n  8) [ ] \n+ neighbor\n\n> > *quote*\n"
        )
    });
    context.update(|window, cx| window.press("enter", cx));
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).source().to_string(),
            "* root\n  7) [ ] **child**\n8) [ ] \n+ neighbor\n\n> > *quote*\n"
        )
    });
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
        assert_eq!(
            view.document().read(cx).source().to_string(),
            "* root\n  7) [x] **child**\n+ neighbor\n\n> > *quote*\n"
        );
        assert_eq!(view.selection(), selection);
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).source().to_string(), normalized);
        assert_eq!(view.selection(), selection);
    });
    context.update(|_, cx| crate::ui::keymap::init(cx));
    view.update_in(&mut context, |view, window, cx| {
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| {
        window.press(
            if cfg!(target_os = "macos") {
                "cmd-enter"
            } else {
                "ctrl-enter"
            },
            cx,
        )
    });
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).source().to_string(),
            "* root\n  7) [x] **child**\n+ neighbor\n\n> > *quote*\n"
        )
    });
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
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).source().to_string(), expected)
    });
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
    assert_eq!(
        std::fs::read(&path).unwrap(),
        "* root\r\n  7) [x] **child**\n+ neighbor\r\n\n> > *中文quote*\n".as_bytes()
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

#[test]
fn list_indent_moves_only_the_target_subtree_and_restores_selection_on_undo() {
    let original = "- first\n+ target\n  4) **nested**\n* neighbor\n";
    let at = original.find("target").unwrap();
    let mut document = MarkdownDocument::new(original.into(), TextFormat::default());
    let mut state = PresentationState::new(Selection::caret(at));
    document.command(
        &mut state,
        Command::ListIndent {
            outdent: false,
            width: 4,
            at: None,
        },
    );
    assert_eq!(
        document.source().to_string(),
        "- first\n  + target\n    4) **nested**\n* neighbor\n"
    );
    assert_eq!(state.selection(), Selection::caret(at + 2));
    document.command(&mut state, Command::Undo);
    assert_eq!(document.source().to_string(), original);
    assert_eq!(state.selection(), Selection::caret(at));
    document.command(&mut state, Command::Redo);
    document.command(
        &mut state,
        Command::ListIndent {
            outdent: true,
            width: 4,
            at: None,
        },
    );
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
    assert_eq!(
        document.source().to_string(),
        "* first\n  + [ ] **task**\n- last"
    );
    assert_eq!(state.selection(), Selection::caret(at));
    document.command(&mut state, Command::Undo);
    assert_eq!(document.source().to_string(), original);
    assert_eq!(state.selection(), Selection::caret(at));
    document.command(&mut state, Command::ListBackspace);
    assert_eq!(
        document.source().to_string(),
        "* first\n+ [X] **task**\n- last"
    );
    assert_eq!(state.selection(), Selection::caret(at - 2));
    document.command(&mut state, Command::ListBackspace);
    assert_eq!(document.source().to_string(), "* first**task**\n- last");
    document.command(&mut state, Command::Undo);
    assert_eq!(
        document.source().to_string(),
        "* first\n+ [X] **task**\n- last"
    );
    assert_eq!(state.selection(), Selection::caret(at - 2));
    document.command(&mut state, Command::Undo);
    assert_eq!(document.source().to_string(), original);
    assert_eq!(state.selection(), Selection::caret(at));
}

#[test]
fn quote_inside_list_continues_without_creating_a_sibling_and_exits_one_layer() {
    let mut document =
        MarkdownDocument::new("- > **quote**\n+ untouched".into(), TextFormat::default());
    let mut state = PresentationState::new(Selection::caret(13));
    document.command(
        &mut state,
        Command::ListBreak {
            hard: false,
            at: None,
        },
    );
    assert_eq!(
        document.source().to_string(),
        "- > **quote**\n  > \n+ untouched"
    );
    document.command(
        &mut state,
        Command::ListBreak {
            hard: false,
            at: None,
        },
    );
    assert_eq!(
        document.source().to_string(),
        "- > **quote**\n\n+ untouched"
    );
    document.command(&mut state, Command::Undo);
    assert_eq!(
        document.source().to_string(),
        "- > **quote**\n  > \n+ untouched"
    );
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
    assert_eq!(
        state.selection(),
        Selection::caret(original.find("task").unwrap())
    );
}

#[test]
fn editing_a_list_continuation_keeps_its_owning_item_and_neighbor_markers() {
    let original = "- first\n+ target\n  continued\n  * nested\n* neighbor";
    let at = original.find("continued").unwrap() + 9;
    let owner = original.find("+ target").unwrap();
    let mut document = MarkdownDocument::new(original.into(), TextFormat::default());
    let mut state = PresentationState::new(Selection::caret(at));
    document.command(
        &mut state,
        Command::ListBreak {
            hard: false,
            at: Some(owner),
        },
    );
    assert_eq!(
        document.source().to_string(),
        "- first\n+ target\n  continued\n+ \n  * nested\n* neighbor"
    );
    document.command(&mut state, Command::Undo);
    assert_eq!(document.source().to_string(), original);
    assert_eq!(state.selection(), Selection::caret(at));
    document.command(
        &mut state,
        Command::ListIndent {
            outdent: false,
            width: 4,
            at: Some(owner),
        },
    );
    assert_eq!(
        document.source().to_string(),
        "- first\n  + target\n    continued\n    * nested\n* neighbor"
    );
    assert_eq!(state.selection(), Selection::caret(at + 4));
    document.command(&mut state, Command::Undo);
    assert_eq!(document.source().to_string(), original);
    assert_eq!(state.selection(), Selection::caret(at));
}

#[gpui_kit::test]
fn native_table_cells_navigate_exit_edit_and_save_without_rewriting_neighbors(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::TestEventExt as _;
    use gpui_kit::{Focusable as _, MouseButton};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cells.md");
    let original = "# Heading\r\n\nBefore  \r\n\n| **Name** | Value |\r\n| :--- | ---: |\n| one | two |\r\n| three |  |\n\nAfter  ";
    let source = original.replace("\r\n", "\n");
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
    let name = source.find("Name").unwrap();
    let value = source.find("Value").unwrap();
    let one = source.find("one").unwrap();
    let two = source.find("two").unwrap();
    let three = source.find("three").unwrap();
    let empty = source.find("  |\n\nAfter").unwrap() + 1;
    context.update(|window, cx| {
        let first = view.read(cx).range_to_bounds(&(name..name), cx).unwrap();
        let second = view.read(cx).range_to_bounds(&(value..value), cx).unwrap();
        let body = view.read(cx).range_to_bounds(&(one..one), cx).unwrap();
        assert_eq!(
            first.top(),
            second.top(),
            "cells share a rendered header row"
        );
        assert!(second.left() > first.right(), "cells have distinct columns");
        assert!(
            body.top() > first.top(),
            "delimiter row is not an editable row"
        );
        window.pointer_down(second.center(), MouseButton::Left, cx);
        window.pointer_up(second.center(), MouseButton::Left, cx);
        assert_eq!(view.read(cx).selection(), Selection::caret(value));
    });
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(name..name, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    for at in [
        value,
        one,
        two,
        three,
        empty,
        source.find("After").unwrap() - 1,
    ] {
        context.update(|window, cx| window.press("tab", cx));
        context.run_until_parked();
        view.read_with(&context, |view, cx| {
            assert_eq!(view.selection(), Selection::caret(at));
            assert_eq!(
                view.document().read(cx).encode().unwrap(),
                original.as_bytes()
            );
        });
    }
    view.update_in(&mut context, |view, _, cx| {
        view.select_range(empty..empty, cx)
    });
    for at in [
        three,
        two,
        one,
        value,
        name,
        source.find("| **Name").unwrap() - 1,
    ] {
        context.update(|window, cx| window.press("shift-tab", cx));
        context.run_until_parked();
        view.read_with(&context, |view, _| {
            assert_eq!(view.selection(), Selection::caret(at))
        });
    }
    view.update_in(&mut context, |view, _, cx| {
        view.select_range(empty..empty, cx)
    });
    context.update(|window, cx| window.input("Z", cx));
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original
                .replacen("| three |  |", "| three | Z |", 1)
                .as_bytes()
        );
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    for (at, key) in [
        (one, "backspace"),
        (one + 3, "delete"),
        (two, "escape"),
        (name, "enter"),
    ] {
        view.update_in(&mut context, |view, _, cx| view.select_range(at..at, cx));
        context.update(|window, cx| window.press(key, cx));
        context.run_until_parked();
        view.read_with(&context, |view, cx| {
            assert_eq!(
                view.document().read(cx).encode().unwrap(),
                original.as_bytes()
            );
            if matches!(key, "escape" | "enter") {
                assert_eq!(
                    view.selection(),
                    Selection::caret(source.find("After").unwrap() - 1)
                );
            }
        });
    }
    view.update_in(&mut context, |view, _, cx| {
        view.select_range(one..one + 3, cx)
    });
    context.update(|window, cx| window.input("中🎉", cx));
    context.run_until_parked();
    let expected = original.replacen("one", "中🎉", 1);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            expected.as_bytes()
        );
    });
    let identity = view.read_with(&context, |view, _| view.document().entity_id());
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().entity_id(), identity);
        assert_eq!(view.selection(), Selection::new(one, one + 3));
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
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

#[test]
fn table_cell_transactions_preserve_mixed_endings_and_restore_the_cell_selection() {
    let original =
        "before  \r\n\n| a | b |\r\n| --- | --- |\n| e\u{301}🎉 | &amp; \\| |\r\n\n[ref]: target";
    let source = original.replace("\r\n", "\n");
    let at = source.find("e\u{301}").unwrap();
    let mut document = MarkdownDocument::new(original.into(), TextFormat::default());
    let mut state = PresentationState::new(Selection::new(at, at + "e\u{301}🎉".len()));
    let range = state.selection().range();
    document.command(
        &mut state,
        Command::Replace {
            range,
            text: "中".into(),
            kind: EditKind::Typing,
        },
    );
    assert_eq!(
        document.encode().unwrap(),
        original.replacen("e\u{301}🎉", "中", 1).as_bytes()
    );
    document.command(&mut state, Command::Undo);
    assert_eq!(
        state.selection(),
        Selection::new(at, at + "e\u{301}🎉".len())
    );
    assert_eq!(document.encode().unwrap(), original.as_bytes());
    document.command(&mut state, Command::Redo);
    assert_eq!(state.selection(), Selection::caret(at + "中".len()));
}

#[gpui_kit::test]
fn native_table_cell_escapes_wrapping_unicode_and_preedit_share_the_document_map(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::{TestEventExt as _, TestInput};
    use gpui_kit::{ElementInputHandler, EntityInputHandler as _, Focusable as _, MouseButton};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("table-mapping.md");
    let original = format!(
        "# Heading\n\n| A | B |\n| --- | --- |\n| **e\u{301}🎉** \\| &amp; `x\\|y` | {} |\n\nTail",
        "wrapping words ".repeat(40),
    );
    std::fs::write(&path, &original).unwrap();
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
    view.update_in(&mut context, |view, _, cx| view.set_soft_wrap(true, cx));
    settle_live_geometry(&view, &mut context);
    for (token, removed) in [("\\| &amp;", "\\|"), ("&amp;", "&amp;"), ("x\\|y", "\\|")] {
        let at = original.find(token).unwrap() + usize::from(token == "x\\|y") + removed.len();
        context.update(|window, cx| {
            window.render_frame(cx);
            let point = view
                .read(cx)
                .range_to_bounds(&(at..at), cx)
                .unwrap()
                .center();
            window.pointer_down(point, MouseButton::Left, cx);
            window.pointer_up(point, MouseButton::Left, cx);
            window.press("backspace", cx);
        });
        context.run_until_parked();
        let start = at - removed.len();
        view.read_with(&context, |view, cx| {
            assert_eq!(
                view.document().read(cx).encode().unwrap(),
                format!("{}{}", &original[..start], &original[at..]).as_bytes()
            );
        });
        context.dispatch_action(gpui_kit::component::input::Undo);
        settle_live_geometry(&view, &mut context);
    }
    let insertion = original.find("wrapping").unwrap();
    view.update_in(&mut context, |view, _, cx| {
        view.select_range(insertion..insertion, cx)
    });
    context.update(|window, cx| window.input("|", cx));
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.replacen("wrapping", "\\|wrapping", 1).as_bytes()
        );
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle_live_geometry(&view, &mut context);
    let unicode = original.find("e\u{301}").unwrap();
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(unicode + "e\u{301}".len()..unicode + "e\u{301}".len(), cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| window.press("backspace", cx));
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.replacen("e\u{301}", "", 1).as_bytes()
        );
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle_live_geometry(&view, &mut context);
    let wrapped = original.find("wrapping").unwrap();
    let wrapped_end = wrapped + "wrapping words ".len() * 8;
    context.update(|window, cx| {
        let first = view
            .read(cx)
            .range_to_bounds(&(wrapped..wrapped), cx)
            .unwrap();
        let later = view
            .read(cx)
            .range_to_bounds(&(wrapped_end..wrapped_end), cx)
            .unwrap();
        assert!(
            later.top() > first.top(),
            "long cell content wraps inside its own column"
        );
        let short_cell = view
            .read(cx)
            .range_to_bounds(&(unicode..unicode), cx)
            .unwrap();
        let lower_left = gpui_kit::point(short_cell.left(), later.center().y);
        window.pointer_down(lower_left, MouseButton::Left, cx);
        window.pointer_up(lower_left, MouseButton::Left, cx);
        assert!(
            view.read(cx).selection().head() < wrapped,
            "a tall neighboring cell must not steal a click in this column"
        );
        window.render_frame(cx);
        window.render_frame(cx);
        let later = view
            .read(cx)
            .range_to_bounds(&(wrapped_end..wrapped_end), cx)
            .unwrap();
        let native = view
            .update(cx, |view, cx| {
                view.character_index_for_point(later.center(), window, cx)
            })
            .unwrap();
        assert_eq!(native, original[..wrapped_end].encode_utf16().count());
        window.pointer_down(later.center(), MouseButton::Left, cx);
        window.pointer_up(later.center(), MouseButton::Left, cx);
        assert_eq!(view.read(cx).selection(), Selection::caret(wrapped_end));
        let handler =
            ElementInputHandler::new(window.find("markdown-source").bounds(), view.clone());
        let mut input = TestInput::new(handler, window);
        input.compose_text(None, "ni", Some(2..2), cx);
    });
    context
        .executor()
        .advance_clock(std::time::Duration::from_millis(100));
    context.run_until_parked();
    context.update(|window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        let marked = view
            .update(cx, |view, cx| view.marked_text_range(window, cx))
            .unwrap();
        let surface = window.find("markdown-source").bounds();
        let candidate = view
            .update(cx, |view, cx| {
                view.bounds_for_range(marked, surface, window, cx)
            })
            .unwrap();
        let caret = view
            .read(cx)
            .range_to_bounds(&(wrapped_end + 2..wrapped_end + 2), cx)
            .unwrap();
        assert_eq!(candidate.top(), caret.top());
        let handler = ElementInputHandler::new(surface, view.clone());
        let mut input = TestInput::new(handler, window);
        input.compose_text(None, "你🎉", Some(3..3), cx);
        input.commit_text("你好🎉", cx);
    });
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            format!(
                "{}你好🎉{}",
                &original[..wrapped_end],
                &original[wrapped_end..]
            )
            .as_bytes()
        );
    });
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        )
    });
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    settle_live_geometry(&view, &mut context);
    view.update_in(&mut context, |view, _, cx| {
        view.select_range(unicode..unicode, cx)
    });
    context.update(|window, cx| {
        let handler =
            ElementInputHandler::new(window.find("markdown-source").bounds(), view.clone());
        TestInput::new(handler, window).compose_text(None, "ni", Some(2..2), cx);
        window.press("escape", cx);
    });
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert!(!view.is_composing());
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        );
        assert_eq!(view.selection(), Selection::caret(unicode));
    });
}

#[gpui_kit::test]
fn native_table_at_document_edges_transfers_focus_without_inserting_rows(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::Focusable as _;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("table-only.md");
    let original = "| A | B |\n| --- | --- |\n| a | b |";
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
    loop {
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
        context.run_until_parked();
        context.update(|window, cx| window.render_frame(cx));
        if view.read_with(&context, |view, cx| {
            let a = view.range_to_bounds(&(2..2), cx).unwrap();
            let b = view.range_to_bounds(&(6..6), cx).unwrap();
            let delimiter = original.find("---").unwrap();
            let hidden = view.range_to_bounds(&(delimiter..delimiter), cx).unwrap();
            a.top() == b.top() && b.left() > a.left() && hidden.top() == a.top()
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
    }
    for (at, key) in [(2, "shift-tab"), (original.find("b |").unwrap(), "tab")] {
        view.update_in(&mut context, |view, window, cx| {
            view.select_range(at..at, cx);
            view.focus_handle(cx).focus(window, cx);
        });
        context.update(|window, cx| window.press(key, cx));
        context.run_until_parked();
        context.update(|window, cx| {
            assert!(!view.read(cx).focus_handle(cx).is_focused(window));
            assert_eq!(
                view.read(cx).document().read(cx).encode().unwrap(),
                original.as_bytes()
            );
        });
    }
}

fn edit_link_destination_when_ready(
    view: &gpui_kit::Entity<SourcePresentation>,
    context: &mut gpui_kit::VisualTestContext,
    expected: Range<usize>,
) {
    use gpui_kit::test::TestWindowExt as _;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
        context.run_until_parked();
        context.dispatch_action(super::source::EditLinkDestination);
        if view.read_with(context, |view, _| view.selection().range() == expected) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "link destination command did not become available"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[gpui_kit::test]
fn native_links_edit_labels_and_reference_destinations_in_shared_history(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::{TestEventExt as _, TestWindowExt as _};
    use gpui_kit::{Focusable as _, MouseButton};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("links.md");
    let original = "# Heading\n\nstart [**文**](<next.md> 'title') [one][Ref] [two][Ref] <https://example.com> [Ref][] [Ref] <mail@example.com> www.example.net\n\n[Ref]: <old.md>  \"keep\"\n[Other]: untouched.md\n";
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
    let one = original.find("one").unwrap();
    let auto = original.find("<https").unwrap();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.range_to_bounds(&(one - 1..one - 1), cx),
            view.range_to_bounds(&(one..one), cx)
        );
        assert_eq!(
            view.range_to_bounds(&(auto..auto), cx),
            view.range_to_bounds(&(auto + 1..auto + 1), cx)
        );
        for syntax in ["[Ref][]", "[Ref] <mail"] {
            let at = original.find(syntax).unwrap();
            assert_eq!(
                view.range_to_bounds(&(at..at), cx),
                view.range_to_bounds(&(at + 1..at + 1), cx)
            );
        }
    });
    context.update(|window, cx| {
        let point = view
            .read(cx)
            .range_to_bounds(&(one + 1..one + 1), cx)
            .unwrap()
            .center();
        window.pointer_down(point, MouseButton::Left, cx);
        window.pointer_up(point, MouseButton::Left, cx);
    });
    view.read_with(&context, |view, cx| {
        assert_eq!(view.selection(), Selection::caret(one + 1));
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        );
    });
    context.update(|window, cx| window.input("中", cx));
    context.run_until_parked();
    let label_edited = original.replacen("one", "o中ne", 1);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            label_edited.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle_live_geometry(&view, &mut context);
    let destination = original.find("old.md").unwrap();
    edit_link_destination_when_ready(&view, &mut context, destination..destination + 6);
    view.read_with(&context, |view, _| {
        assert_eq!(
            view.selection(),
            Selection::new(destination, destination + 6)
        )
    });
    context.update(|window, cx| window.input("new.md", cx));
    context.run_until_parked();
    let edited = original.replacen("old.md", "new.md", 1);
    view.read_with(&context, |view, cx| {
        assert!(view.is_live());
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            edited.as_bytes()
        );
    });
    context.update(|window, cx| assert!(view.read(cx).focus_handle(cx).is_focused(window)));
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.selection(),
            Selection::new(destination, destination + 6)
        );
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        );
    });
    view.update_in(&mut context, |view, _, cx| {
        view.select_range(one + 1..one + 1, cx)
    });
    edit_link_destination_when_ready(&view, &mut context, destination..destination + 6);
    view.update_in(&mut context, |view, _, cx| {
        view.select_range(one + 1..one + 1, cx)
    });
    context.update(|window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        window.click("markdown-link-destination", cx);
        assert_eq!(
            view.read(cx).selection().range(),
            destination..destination + 6
        );
        assert!(view.read(cx).focus_handle(cx).is_focused(window));
    });
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    context.dispatch_action(gpui_kit::component::input::Redo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            edited.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    settle_live_geometry(&view, &mut context);
    for (label_at, target, replacement) in [
        (original.find("文").unwrap(), "next.md", "other.md"),
        (auto + 3, "https://example.com", "https://example.org"),
        (
            original.find("mail@").unwrap() + 2,
            "mail@example.com",
            "other@example.com",
        ),
        (
            original.find("www.").unwrap() + 2,
            "www.example.net",
            "www.example.org",
        ),
    ] {
        view.update_in(&mut context, |view, window, cx| {
            view.select_range(label_at..label_at, cx);
            view.focus_handle(cx).focus(window, cx);
        });
        let start = original.find(target).unwrap();
        edit_link_destination_when_ready(&view, &mut context, start..start + target.len());
        view.read_with(&context, |view, _| {
            assert_eq!(view.selection().range(), start..start + target.len())
        });
        context.update(|window, cx| window.input(replacement, cx));
        context.run_until_parked();
        view.read_with(&context, |view, cx| {
            assert_eq!(
                view.document().read(cx).encode().unwrap(),
                original.replacen(target, replacement, 1).as_bytes()
            )
        });
        context.dispatch_action(gpui_kit::component::input::Undo);
        view.update_in(&mut context, |view, _, cx| view.select_range(0..0, cx));
        settle_live_geometry(&view, &mut context);
    }
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(one..one, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    edit_link_destination_when_ready(&view, &mut context, destination..destination + 6);
    context.update(|window, cx| window.input("new.md", cx));
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
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        context.run_until_parked();
        if app.read_with(&context, |app, cx| {
            app.active_editor_input()
                .is_some_and(|input| input.read(cx).text().to_string() == edited)
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let reopened = app.read_with(&context, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    settle_live_geometry(&reopened, &mut context);
    reopened.read_with(&context, |view, cx| {
        assert_eq!(
            view.range_to_bounds(&(one - 1..one - 1), cx),
            view.range_to_bounds(&(one..one), cx)
        );
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            edited.as_bytes()
        );
    });
}

#[gpui_kit::test]
fn native_image_atoms_cross_one_boundary_and_source_edits_share_undo(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::Focusable as _;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("images.md");
    let source = "before ![图](missing.png) after\n\n![引用][picture]\n\n[picture]: other.png\n";
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
    let start = source.find("![图]").unwrap();
    let end = source.find(" after").unwrap();
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(start..start, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| window.press("right", cx));
    context.run_until_parked();
    view.read_with(&context, |view, _| {
        assert_eq!(view.selection().range(), end..end)
    });
    view.update_in(&mut context, |view, _, cx| {
        view.select_range(start + "![图](".len()..end - 1, cx)
    });
    context.update(|window, cx| window.input("new.png", cx));
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).source().to_string(),
            source.replacen("missing.png", "new.png", 1)
        );
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).source().to_string(), source)
    });
    let reference = source.find("![引用]").unwrap();
    let reference_end = source.find("\n\n[picture]").unwrap();
    view.update_in(&mut context, |view, _, cx| {
        view.select_range(reference..reference, cx)
    });
    settle_live_geometry(&view, &mut context);
    context.update(|window, cx| window.press("right", cx));
    context.run_until_parked();
    view.read_with(&context, |view, _| {
        assert_eq!(view.selection().range(), reference_end..reference_end)
    });
}

#[gpui_kit::test]
fn opening_native_images_never_contacts_http_until_loading_intent(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use std::io::{Read as _, Write as _};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let counted = requests.clone();
    let (stop, stopping) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        while matches!(
            stopping.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ) {
            if let Ok((mut stream, _)) = listener.accept() {
                counted.fetch_add(1, Ordering::SeqCst);
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                    .unwrap();
                let mut request = [0; 4096];
                let _ = stream.read(&mut request);
                stream
                    .write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .unwrap();
            } else {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
    });
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("privacy.md");
    let url = format!("http://{address}/private.png");
    std::fs::write(directory.path().join("vector.svg"), format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><image href="{url}" width="16" height="16"/></svg>"#)).unwrap();
    let source = format!("before ![private]({url}) ![nested](vector.svg) after");
    std::fs::write(&path, &source).unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    context.update(|_, cx| {
        cx.set_global(super::super::MarkdownWorkbench);
        cx.set_http_client(Arc::new(reqwest_client::ReqwestClient::new()));
    });
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
    let status_id = format!("markdown-image:{url}");
    context.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window
                .find(gpui_kit::SharedString::from(status_id.clone()))
                .label(),
            Some(crate::ui::i18n::t(crate::ui::i18n::L10nKey::EditorImageUnloaded).as_ref())
        );
    });
    loop {
        context.run_until_parked();
        if context.update(|window, cx| {
            window.render_frame(cx);
            window.find("markdown-image:vector.svg").label()
                == Some(crate::ui::i18n::t(crate::ui::i18n::L10nKey::EditorImageFailed).as_ref())
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        requests.load(Ordering::SeqCst),
        0,
        "render, prefetch and size discovery must not request images"
    );
    let before = view.read_with(&context, |view, cx| {
        view.range_to_bounds(&(source.len()..source.len()), cx)
            .expect("caret geometry before loading")
    });
    context.update(|window, cx| window.click("markdown-load-images", cx));
    loop {
        context.run_until_parked();
        let failed = context.update(|window, cx| {
            window.render_frame(cx);
            window
                .find(gpui_kit::SharedString::from(status_id.clone()))
                .label()
                .is_some_and(|label| {
                    label.starts_with(crate::ui::i18n::t(
                        crate::ui::i18n::L10nKey::EditorImageFailed,
                    ))
                })
        });
        if failed {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).source().to_string(), source);
        assert_eq!(
            view.range_to_bounds(&(source.len()..source.len()), cx)
                .expect("caret geometry after loading"),
            before
        );
    });
    stop.send(()).unwrap();
    server.join().unwrap();
}

#[gpui_kit::test]
fn native_document_images_read_owning_host_and_reject_old_link_results(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use crate::ui::code_editor::test_host::FileHost;
    use gpui_kit::Focusable as _;
    use std::sync::atomic::Ordering;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("owned-images.md");
    let old = directory.path().join("old.png");
    let new = directory.path().join("new.png");
    image::RgbaImage::from_pixel(3, 2, image::Rgba([255, 0, 0, 255]))
        .save(&old)
        .unwrap();
    image::RgbaImage::from_pixel(2, 3, image::Rgba([0, 255, 0, 255]))
        .save(&new)
        .unwrap();
    let source = "before ![owned](old.png) after";
    std::fs::write(&path, source).unwrap();
    let host = FileHost::new(lyrel_core::host::HostId(114));
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    context.update(|_, cx| cx.set_global(super::super::MarkdownWorkbench));
    app.update_in(&mut context, |app, window, cx| {
        app.editor_open_on_host(host.clone(), &path, window, cx)
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
    let release = host.pause_next_read();
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    loop {
        context.run_until_parked();
        if host.entered.load(Ordering::SeqCst) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    context.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("markdown-image:old.png").label(),
            Some(crate::ui::i18n::t(crate::ui::i18n::L10nKey::EditorImageLoading).as_ref())
        );
    });
    let from = source.find("old.png").unwrap();
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(from..from + 7, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| window.input("new.png", cx));
    loop {
        context.run_until_parked();
        let loaded = context.update(|window, cx| {
            window.render_frame(cx);
            window
                .try_find("markdown-image:new.png")
                .is_some_and(|status| {
                    status.label()
                        == Some(
                            crate::ui::i18n::t(crate::ui::i18n::L10nKey::EditorImageLoaded)
                                .as_ref(),
                        )
                })
        });
        if loaded {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let moved_directory = directory.path().join("renamed");
    std::fs::create_dir(&moved_directory).unwrap();
    let moved = moved_directory.join("owned-images.md");
    std::fs::rename(&path, &moved).unwrap();
    app.update_in(&mut context, |app, _, cx| {
        app.editor_path_moved(lyrel_core::host::HostId(114), &path, &moved, cx);
    });
    loop {
        context.run_until_parked();
        if context.update(|window, cx| {
            window.render_frame(cx);
            window.find("markdown-image:new.png").label()
                == Some(crate::ui::i18n::t(crate::ui::i18n::L10nKey::EditorImageFailed).as_ref())
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    release.send(()).unwrap();
    settle_live_geometry(&view, &mut context);
    context.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("markdown-image:old.png").is_none());
        assert_eq!(
            window.find("markdown-image:new.png").label(),
            Some(crate::ui::i18n::t(crate::ui::i18n::L10nKey::EditorImageFailed).as_ref())
        );
    });
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).source().to_string(),
            source.replace("old.png", "new.png")
        )
    });
    assert_eq!(std::fs::read(&old).unwrap()[..8], *b"\x89PNG\r\n\x1a\n");
    assert!(new.exists());
}

#[gpui_kit::test]
fn local_image_references_follow_document_rename_without_moving_assets(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("local-images.md");
    let asset = directory.path().join("existing.png");
    image::RgbaImage::from_pixel(2, 2, image::Rgba([0, 0, 255, 255]))
        .save(&asset)
        .unwrap();
    let source = "| Picture |\n| --- |\n| ![existing][image] |\n\n[image]: existing.png\n";
    std::fs::write(&path, source).unwrap();
    let destination = directory.path().join("moved");
    std::fs::create_dir(&destination).unwrap();
    let moved = destination.join("renamed.md");
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
    loop {
        context.run_until_parked();
        if context.update(|window, cx| {
            window.render_frame(cx);
            window
                .try_find("markdown-image:existing.png")
                .is_some_and(|status| {
                    status.label()
                        == Some(
                            crate::ui::i18n::t(crate::ui::i18n::L10nKey::EditorImageLoaded)
                                .as_ref(),
                        )
                })
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    std::fs::rename(&path, &moved).unwrap();
    app.update_in(&mut context, |app, _, cx| {
        let host = app.active_buffer().unwrap().host.id();
        app.editor_path_moved(host, &path, &moved, cx);
    });
    loop {
        context.run_until_parked();
        if context.update(|window, cx| {
            window.render_frame(cx);
            window.find("markdown-image:existing.png").label()
                == Some(crate::ui::i18n::t(crate::ui::i18n::L10nKey::EditorImageFailed).as_ref())
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).source().to_string(), source);
        assert_eq!(view.document().read(cx).revision(), 0);
    });
    assert!(
        asset.exists(),
        "existing linked images are never moved with the document"
    );
    assert!(!destination.join("existing.png").exists());
    assert_eq!(std::fs::read_to_string(moved).unwrap(), source);
}

#[gpui_kit::test]
fn native_table_structure_actions_share_undo_and_save_reopen(cx: &mut gpui_kit::TestAppContext) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::{
        Focusable as _, MouseButton,
        test::{TestEventExt as _, TestQueryExt as _, TestWindowExt as _},
    };
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("structure.md");
    let original =
        "before  \r\n\n| **A** | B |\r\n| --- | ---: |\n| x\\|y | `z` |\r\n\n[ref]: target  ";
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
    context.dispatch_action(super::tables::DeleteRow);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        )
    });
    let at = original.replace("\r\n", "\n").find("x\\|y").unwrap();
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(at..at + 4, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    // The binding dispatches the same Action used by the native editor menu.
    context.update(|window, cx| window.press("secondary-alt-shift-down", cx));
    context.run_until_parked();
    settle_live_geometry(&view, &mut context);
    let after_row = "before  \r\n\n| **A** | B |\n| --- | ---: |\n| x\\|y | `z` |\n|  |  |\r\n\n[ref]: target  ";
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            after_row.as_bytes()
        );
        assert!(
            view.range_to_bounds(&view.selection().range(), cx)
                .is_some()
        );
    });
    context.update(|window, cx| assert!(view.read(cx).focus_handle(cx).is_focused(window)));
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        );
        assert_eq!(view.selection(), Selection::new(at, at + 4));
    });
    context.dispatch_action(gpui_kit::component::input::Redo);
    context.run_until_parked();
    context.dispatch_action(super::tables::InsertColumnAfter);
    context.run_until_parked();
    context.update(|window, cx| window.click("markdown-table-commands", cx));
    context.run_until_parked();
    context.update(|window, cx| {
        let item = window.find_by_label(
            crate::ui::i18n::t(crate::ui::i18n::L10nKey::EditorTableAlignCenter).as_ref(),
        );
        window.pointer_down(item.bounds().center(), MouseButton::Left, cx);
        window.pointer_up(item.bounds().center(), MouseButton::Left, cx);
    });
    context.run_until_parked();
    let expected = "before  \r\n\n| **A** |  | B |\n| --- | :---: | ---: |\n| x\\|y |  | `z` |\n|  |  |  |\r\n\n[ref]: target  ";
    settle_live_geometry(&view, &mut context);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            expected.as_bytes()
        );
        assert!(
            view.range_to_bounds(&view.selection().range(), cx)
                .is_some()
        );
    });
    context.update(|window, cx| assert!(view.read(cx).focus_handle(cx).is_focused(window)));
    let before_delete = view.read_with(&context, |view, _| view.selection());
    context.dispatch_action(super::tables::DeleteColumn);
    context.run_until_parked();
    settle_live_geometry(&view, &mut context);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            after_row.as_bytes()
        );
        assert!(
            view.range_to_bounds(&view.selection().range(), cx)
                .is_some()
        );
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            expected.as_bytes()
        );
        assert_eq!(view.selection(), before_delete);
    });
    context.dispatch_action(super::tables::DeleteRow);
    context.run_until_parked();
    settle_live_geometry(&view, &mut context);
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).encode().unwrap(), b"before  \r\n\n| **A** |  | B |\n| --- | :---: | ---: |\n| x\\|y |  | `z` |\r\n\n[ref]: target  ");
        assert!(view.range_to_bounds(&view.selection().range(), cx).is_some());
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            expected.as_bytes()
        );
        assert_eq!(view.selection(), before_delete);
    });
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
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

#[test]
fn table_structure_commands_preserve_cells_boundaries_and_atomic_selection() {
    use super::tables::Operation;
    let original =
        "outside  \r\n\n| **A** | B |\r\n| --- | ---: |\n| x\\|y | `z` |\r\n\n[ref]: target";
    let normalized = original.replace("\r\n", "\n");
    let at = normalized.find("x\\|y").unwrap();
    let prefix = "outside  \r\n\n";
    let suffix = "\r\n\n[ref]: target";
    for (operation, table) in [
        (
            Operation::InsertRowBefore,
            "| **A** | B |\n| --- | ---: |\n|  |  |\n| x\\|y | `z` |",
        ),
        (
            Operation::InsertRowAfter,
            "| **A** | B |\n| --- | ---: |\n| x\\|y | `z` |\n|  |  |",
        ),
        (Operation::DeleteRow, "| **A** | B |\n| --- | ---: |"),
        (
            Operation::InsertColumnBefore,
            "|  | **A** | B |\n| --- | --- | ---: |\n|  | x\\|y | `z` |",
        ),
        (
            Operation::InsertColumnAfter,
            "| **A** |  | B |\n| --- | --- | ---: |\n| x\\|y |  | `z` |",
        ),
        (Operation::DeleteColumn, "| B |\n| ---: |\n| `z` |"),
        (
            Operation::AlignDefault,
            "| **A** | B |\n| --- | ---: |\n| x\\|y | `z` |",
        ),
        (
            Operation::AlignLeft,
            "| **A** | B |\n| :--- | ---: |\n| x\\|y | `z` |",
        ),
        (
            Operation::AlignCenter,
            "| **A** | B |\n| :---: | ---: |\n| x\\|y | `z` |",
        ),
        (
            Operation::AlignRight,
            "| **A** | B |\n| ---: | ---: |\n| x\\|y | `z` |",
        ),
    ] {
        let mut document = MarkdownDocument::new(original.into(), TextFormat::default());
        let selection = Selection::new(at, at + 4);
        let mut state = PresentationState::new(selection);
        document.command(&mut state, Command::Table(operation));
        let expected = format!("{prefix}{table}{suffix}");
        assert_eq!(document.encode().unwrap(), expected.as_bytes());
        let after = state.selection();
        // Typing at the command's selection must change a surviving real cell.
        document.command(&mut state, Command::Insert("中".into()));
        assert!(document.source().to_string().contains("中"));
        document.command(&mut state, Command::Undo);
        assert_eq!(state.selection(), after);
        assert_eq!(document.encode().unwrap(), expected.as_bytes());
        document.command(&mut state, Command::Undo);
        assert_eq!(document.encode().unwrap(), original.as_bytes());
        assert_eq!(state.selection(), selection);
        document.command(&mut state, Command::Redo);
        assert_eq!(document.encode().unwrap(), expected.as_bytes());
        assert_eq!(state.selection(), after);
    }
    for (source, token, operation, expected) in [
        (
            "| a | b |\n| --- | --- |\n| c | d |",
            "a",
            Operation::DeleteRow,
            "| c | d |\n| --- | --- |",
        ),
        (
            "| a | b |\n| --- | --- |\n| c | d |",
            "a",
            Operation::InsertRowBefore,
            "|  |  |\n| --- | --- |\n| a | b |\n| c | d |",
        ),
        ("| a |\n| --- |", "a", Operation::DeleteRow, "|  |\n| --- |"),
        (
            "| a |\n| --- |\n| b |",
            "b",
            Operation::DeleteColumn,
            "|  |\n| --- |\n|  |",
        ),
        (
            "| a | b |\n| --- | --- |\n| c | d |",
            "d",
            Operation::DeleteColumn,
            "| a |\n| --- |\n| c |",
        ),
        (
            "| a | b |\n| --- | --- |\n| c |",
            "c",
            Operation::InsertColumnAfter,
            "| a |  | b |\n| --- | --- | --- |\n| c |  |  |",
        ),
    ] {
        let mut document = MarkdownDocument::new(source.into(), TextFormat::default());
        let mut state = PresentationState::new(Selection::caret(source.find(token).unwrap()));
        let before = state.selection();
        document.command(&mut state, Command::Table(operation));
        assert_eq!(document.source().to_string(), expected);
        document.command(&mut state, Command::Insert("X".into()));
        assert!(document.source().to_string().contains(" X"));
        document.command(&mut state, Command::Undo);
        document.command(&mut state, Command::Undo);
        assert_eq!(document.source().to_string(), source);
        assert_eq!(state.selection(), before);
    }
}

#[gpui_kit::test]
fn native_mermaid_fence_edit_failure_history_and_save_keep_source_bytes(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::Focusable as _;
    use gpui_kit::test::TestWindowExt as _;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("diagram.md");
    let original = b"# Heading\r\n\r\n~~~~mermaid extra\r\nflowchart LR\r\nA[Start] --> B[End]\r\n~~~~\n\n[ref]: untouched  \r\ntail";
    std::fs::write(&path, original).unwrap();
    let source = String::from_utf8(original.to_vec())
        .unwrap()
        .replace("\r\n", "\n");
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    context.update(|_, cx| cx.set_global(super::super::MarkdownWorkbench));
    app.update_in(&mut context, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
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
    let identity = view.read_with(&context, |view, _| view.document().entity_id());
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    let fence = source.find("~~~~mermaid").unwrap();
    let image_id = format!("markdown-mermaid-image-{fence}");
    let error_id = format!("markdown-mermaid-error-{fence}");
    let settle = |context: &mut gpui_kit::VisualTestContext, id: &str| {
        loop {
            context
                .executor()
                .advance_clock(std::time::Duration::from_millis(100));
            context.run_until_parked();
            let visible = context.update(|window, cx| {
                window.render_frame(cx);
                window.try_find(id.to_owned()).is_some()
            });
            if visible {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Mermaid state did not settle: {id}"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    settle(&mut context, &image_id);
    let original_size = context.update(|window, _| {
        let size = window.find(image_id.clone()).bounds().size;
        assert!(size.height > gpui_kit::px(20.));
        size
    });
    context.update(|window, cx| {
        use gpui_kit::test::{TestEventExt as _, TestQueryExt as _};
        let label = crate::ui::i18n::t(crate::ui::i18n::L10nKey::EditorMarkdownMermaidEdit);
        let point = window.find_by_label(label).bounds().center();
        window.pointer_down(point, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(point, gpui_kit::MouseButton::Left, cx);
        window.render_frame(cx);
    });
    view.read_with(&context, |view, cx| {
        let body = source.find("flowchart").unwrap();
        assert_eq!(view.selection(), Selection::caret(body));
        assert!(view.range_to_bounds(&(body..body), cx).is_some());
    });
    let at = source.find("Start").unwrap();
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(at..at + 5, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| window.input("Changed", cx));
    settle(&mut context, &image_id);
    context.update(|window, _| {
        assert!(
            window.find(image_id.clone()).bounds().size.width > original_size.width,
            "the longer edited label must update the actual native graph, not retain its old raster"
        );
    });
    view.read_with(&context, |view, cx| {
        assert_eq!(view.selection(), Selection::caret(at + 7));
        assert_eq!(
            view.document().read(cx).source().to_string(),
            source.replacen("Start", "Changed", 1)
        );
    });
    let graph = source.find("flowchart").unwrap();
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(graph..graph + 9, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| window.input("not-a-diagram", cx));
    settle(&mut context, &error_id);
    view.read_with(&context, |view, cx| {
        assert!(
            view.document()
                .read(cx)
                .source()
                .to_string()
                .contains("not-a-diagram LR")
        );
        assert!(
            view.range_to_bounds(&(graph..graph), cx).is_some(),
            "failed source must remain editable"
        );
    });
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().entity_id(), identity);
        assert_eq!(view.document().read(cx).source().to_string(), source);
    });
    context.dispatch_action(gpui_kit::component::input::Redo);
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    settle(&mut context, &image_id);
    // Start work, replace its source before publication, and leave live mode.
    // Neither its successful image nor its failure may land in source mode.
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(graph..graph + 9, cx);
        view.command(Command::Insert("invalid".into()), window, cx);
        view.command(Command::Undo, window, cx);
        view.toggle_mode(window, cx);
    });
    context
        .executor()
        .advance_clock(std::time::Duration::from_secs(1));
    context.run_until_parked();
    context.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(image_id.clone()).is_none());
        assert!(window.try_find(error_id.clone()).is_none());
    });
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
    let expected = String::from_utf8(original.to_vec())
        .unwrap()
        .replacen("Start", "Changed", 1);
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
            reopened.document().read(cx).source().to_string(),
            expected.replace("\r\n", "\n")
        );
    });
}

#[gpui_kit::test]
fn native_mermaid_resource_constructs_fail_visibly_but_url_label_text_renders(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::Focusable as _;
    use gpui_kit::test::TestWindowExt as _;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("resources.md");
    let original = "# Heading\n\n```mermaid\nflowchart LR\nA[\"url(https://example.test/label.png)\"] --> B\n```\n\ntail";
    std::fs::write(&path, original).unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    context.update(|_, cx| cx.set_global(super::super::MarkdownWorkbench));
    app.update_in(&mut context, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
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
    let fence = original.find("```mermaid").unwrap();
    let start = original.find("flowchart").unwrap();
    let end = original.find("\n```\n").unwrap();
    let wait_for = |context: &mut gpui_kit::VisualTestContext, state: &str| {
        loop {
            context
                .executor()
                .advance_clock(std::time::Duration::from_millis(100));
            context.run_until_parked();
            if context.update(|window, cx| {
                window.render_frame(cx);
                window
                    .try_find(format!("markdown-mermaid-{state}-{fence}"))
                    .is_some()
            }) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Mermaid {state} state did not settle"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    wait_for(&mut context, "image");
    // The actual image construct, not arbitrary URL-looking label text, is
    // refused before native rasterization. No resource loader is introduced.
    let external =
        "flowchart LR\nA@{ img: \"https://example.test/private.png\", label: \"External\" } --> B";
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(start..end, cx);
        view.focus_handle(cx).focus(window, cx);
        view.command(
            Command::Replace {
                range: start..end,
                text: external.into(),
                kind: EditKind::Command,
            },
            window,
            cx,
        );
    });
    wait_for(&mut context, "error");
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).source().to_string(),
            format!("{}{}{}", &original[..start], external, &original[end..])
        );
        assert!(view.range_to_bounds(&(start..start), cx).is_some());
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    wait_for(&mut context, "image");
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).source().to_string(), original)
    });
}

#[gpui_kit::test]
fn native_mixed_selection_drag_cut_history_and_mixed_ending_save_reopen(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::TestEventExt as _;
    use gpui_kit::{Focusable as _, MouseButton};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mixed-selection.md");
    let original = "# Heading\r\n\nalpha \\* &amp; 中e\u{301} 👩‍💻\r\n\n* root\n  7) nested\r\n\n> quoted\n\n```rust\r\nlet x = 1;\n```\r\n\n| Name | Value |\n| :--- | ---: |\r\n| one | two |\n\n![引用][picture]\r\n\ntail  \n\n[picture]: missing.png \"title\"\r\n";
    let source = original.replace("\r\n", "\n");
    std::fs::write(&path, original).unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    cx.simulate_window_resize(
        context.window_handle(),
        gpui_kit::size(gpui_kit::px(1100.), gpui_kit::px(1800.)),
    );
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
    let start = source.find("alpha").unwrap();
    let end = source.find("tail").unwrap() + 2;
    for reverse in [false, true] {
        context.update(|window, cx| {
            window.render_frame(cx);
            let a = view
                .read(cx)
                .range_to_bounds(&(start..start), cx)
                .unwrap()
                .center();
            let b = view
                .read(cx)
                .range_to_bounds(&(end..end), cx)
                .unwrap()
                .center();
            let (from, to) = if reverse { (b, a) } else { (a, b) };
            window.pointer_down(from, MouseButton::Left, cx);
            window.pointer_move(to, Some(MouseButton::Left), cx);
            window.pointer_up(to, MouseButton::Left, cx);
            assert_eq!(
                view.read(cx).selection(),
                if reverse {
                    Selection::new(end, start)
                } else {
                    Selection::new(start, end)
                }
            );
            window.press("secondary-c", cx);
            assert_eq!(
                cx.read_from_clipboard().and_then(|item| item.text()),
                Some(source[start..end].into())
            );
        });
    }
    context.update(|window, cx| window.press("secondary-x", cx));
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).source().to_string(),
            "# Heading\n\nil  \n\n[picture]: missing.png \"title\"\n"
        );
        assert_eq!(view.selection(), Selection::caret(start));
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).source().to_string(), source);
        assert_eq!(view.selection(), Selection::new(end, start));
    });
    context.dispatch_action(gpui_kit::component::input::Redo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).source().to_string(),
            "# Heading\n\nil  \n\n[picture]: missing.png \"title\"\n"
        );
        assert_eq!(view.selection(), Selection::caret(start));
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    context.update(|window, cx| {
        window.press("secondary-c", cx);
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some(source[start..end].into())
        );
        window.press("delete", cx);
    });
    context.run_until_parked();
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    settle_live_geometry(&view, &mut context);
    let image = source.find("![引用]").unwrap();
    let image_end = source.find("\n\ntail").unwrap();
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(image..image, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| {
        window.press("shift-right", cx);
        assert_eq!(view.read(cx).selection(), Selection::new(image, image_end));
        window.press("secondary-c", cx);
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("![引用][picture]".into())
        );
        window.press("shift-left", cx);
        assert_eq!(view.read(cx).selection(), Selection::caret(image));
        window.press("right", cx);
        window.press("shift-left", cx);
        assert_eq!(view.read(cx).selection(), Selection::new(image_end, image));
        window.press("shift-right", cx);
        assert_eq!(view.read(cx).selection(), Selection::caret(image_end));
    });
    let one_end = source.find("one").unwrap() + 3;
    let two = source.find("two").unwrap();
    for (from, to, key) in [(one_end, two, "shift-right"), (two, one_end, "shift-left")] {
        view.update_in(&mut context, |view, _, cx| {
            view.select_range(from..from, cx)
        });
        context.update(|window, cx| {
            window.press(key, cx);
            assert_eq!(view.read(cx).selection(), Selection::new(from, to));
            window.press("secondary-c", cx);
            assert_eq!(
                cx.read_from_clipboard().and_then(|item| item.text()),
                Some(" | ".into())
            );
            assert_eq!(
                view.read(cx).document().read(cx).source().to_string(),
                source
            );
            window.press(
                if key == "shift-right" {
                    "shift-left"
                } else {
                    "shift-right"
                },
                cx,
            );
            assert_eq!(view.read(cx).selection(), Selection::caret(from));
        });
    }
    // A new edit after undo replaces the cut/delete branch, not earlier history.
    view.update_in(&mut context, |view, _, cx| {
        view.select_range(start..start + 5, cx)
    });
    context.update(|window, cx| {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("changed".into()));
        window.press("secondary-v", cx);
    });
    context.run_until_parked();
    context.dispatch_action(gpui_kit::component::input::Redo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).source().to_string(),
            source.replacen("alpha", "changed", 1)
        );
        assert_eq!(view.selection(), Selection::caret(start + 7));
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
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
    assert_eq!(
        std::fs::read(&path).unwrap(),
        original.replacen("alpha", "changed", 1).as_bytes()
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
        assert_eq!(
            reopened.document().read(cx).source().to_string(),
            source.replacen("alpha", "changed", 1)
        );
    });
}

#[gpui_kit::test]
fn native_mixed_shift_navigation_crosses_wrapped_rows_tables_and_image_atoms_both_ways(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::Focusable as _;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mixed-keyboard.md");
    let source = "# Heading\n\nstart 中e\u{301} 👩‍💻 escaped \\* &amp; text that wraps when the editor is narrow and continues on another visual row\n\n* root\n  + nested\n\n> quoted\n\n```\ncode\n```\n\n| Left | Right |\n| --- | --- |\n| one | two |\n\n![图](missing.png)\n\ntail";
    std::fs::write(&path, source).unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    cx.simulate_window_resize(
        context.window_handle(),
        gpui_kit::size(gpui_kit::px(720.), gpui_kit::px(1400.)),
    );
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
    let start = source.find("start").unwrap();
    let end = source.len();
    for (from, to, key) in [(start, end, "shift-right"), (end, start, "shift-left")] {
        view.update_in(&mut context, |view, window, cx| {
            view.select_range(from..from, cx);
            view.focus_handle(cx).focus(window, cx);
        });
        context.update(|window, cx| {
            for _ in 0..source.len() {
                if view.read(cx).selection().head() == to {
                    break;
                }
                let previous = view.read(cx).selection();
                window.press(key, cx);
                let selection = view.read(cx).selection();
                assert_ne!(
                    selection, previous,
                    "navigation must not stop at a structure boundary"
                );
                assert!(source.is_char_boundary(selection.head()));
                assert_ne!(
                    selection.head(),
                    source.find("e\u{301}").unwrap() + 1,
                    "a combining grapheme is indivisible"
                );
                let emoji = source.find("👩‍💻").unwrap();
                assert!(
                    !(emoji + 1..emoji + "👩‍💻".len()).contains(&selection.head()),
                    "a joined emoji is indivisible"
                );
            }
            assert_eq!(view.read(cx).selection(), Selection::new(from, to));
            window.press("secondary-c", cx);
            assert_eq!(
                cx.read_from_clipboard().and_then(|item| item.text()),
                Some(source[start..end].into())
            );
        });
    }
    // Up/down follow actual shaped rows, including the row after a tall image.
    let tail = source.find("tail").unwrap();
    for (from, key, forward) in [(start, "shift-down", true), (tail, "shift-up", false)] {
        view.update_in(&mut context, |view, window, cx| {
            view.select_range(from..from, cx);
            view.focus_handle(cx).focus(window, cx);
        });
        context.update(|window, cx| {
            for _ in 0..32 {
                let at = view.read(cx).selection().head();
                if (forward && at >= tail) || (!forward && at <= start) {
                    break;
                }
                window.press(key, cx);
                assert_ne!(
                    view.read(cx).selection().head(),
                    at,
                    "vertical navigation must cross block margins and image height"
                );
            }
            let selection = view.read(cx).selection();
            assert_eq!(selection, Selection::new(from, selection.head()));
            assert!(if forward {
                selection.head() >= tail
            } else {
                selection.head() <= start
            });
            assert_eq!(
                view.read(cx).document().read(cx).source().to_string(),
                source
            );
        });
    }
}

#[gpui_kit::test]
fn native_word_selection_keeps_image_and_entity_source_boundaries(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::Focusable as _;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("word-atoms.md");
    let source = "# Heading\n\nbefore ![图](missing.png) &amp; after\n\n| A | B |\n| --- | --- |\n| x | y |\n\ntail\n";
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
    let image = source.find("![图]").unwrap();
    let image_end = source.find(" &amp;").unwrap();
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(image..image, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.dispatch_action(gpui_kit::component::input::SelectToNextWordEnd);
    context.run_until_parked();
    view.read_with(&context, |view, _| {
        assert_eq!(view.selection(), Selection::new(image, image_end))
    });
    context.dispatch_action(gpui_kit::component::input::SelectToPreviousWordStart);
    context.run_until_parked();
    view.read_with(&context, |view, _| {
        assert_eq!(view.selection(), Selection::caret(image))
    });
    let entity = source.find("&amp;").unwrap();
    view.update_in(&mut context, |view, _, cx| {
        view.select_range(entity..entity, cx)
    });
    context.dispatch_action(gpui_kit::component::input::SelectToNextWordEnd);
    context.run_until_parked();
    view.read_with(&context, |view, _| {
        assert_eq!(view.selection(), Selection::new(entity, entity + 5))
    });
    // Word-delete with an existing cross-structure selection deletes that
    // selection, not an unrelated word beside its active endpoint.
    let tail = source.find("tail").unwrap();
    view.update_in(&mut context, |view, _, cx| {
        view.select_range(image..tail, cx)
    });
    context.dispatch_action(gpui_kit::component::input::DeleteToPreviousWordStart);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).source().to_string(),
            "# Heading\n\nbefore tail\n"
        );
        assert_eq!(view.selection(), Selection::caret(image));
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).source().to_string(), source);
        assert_eq!(view.selection(), Selection::new(image, tail));
    });
}

#[test]
fn mixed_reverse_replacement_and_composition_share_history_and_original_bytes() {
    let original = "before\r\n\n* root\n  7) nested\r\n\n> quote\n\n```\r\ncode\n```\r\n\n| A | B |\n| --- | --- |\r\n| one | two |\n\n![图][p]\r\n\nafter  \n\n[p]: missing.png\r\n";
    let source = original.replace("\r\n", "\n");
    let mut document = MarkdownDocument::new(original.into(), TextFormat::default());
    let mut live = PresentationState::default();
    let mut other = PresentationState::default();
    let start = source.find("before").unwrap() + 3;
    let end = source.find("\n\nafter").unwrap();
    document.command(&mut live, Command::Select(Selection::new(end, start)));
    document.command(
        &mut live,
        Command::Replace {
            range: start..end,
            text: "replacement".into(),
            kind: EditKind::Paste,
        },
    );
    assert_eq!(
        document.source().to_string(),
        "befreplacement\n\nafter  \n\n[p]: missing.png\n"
    );
    assert_eq!(live.selection(), Selection::caret(start + 11));
    document.command(&mut other, Command::Undo);
    assert_eq!(document.source().to_string(), source);
    assert_eq!(other.selection(), Selection::new(end, start));
    assert_eq!(document.encode().unwrap(), original.as_bytes());
    document.command(&mut live, Command::Redo);
    assert_eq!(
        document.source().to_string(),
        "befreplacement\n\nafter  \n\n[p]: missing.png\n"
    );
    assert_eq!(live.selection(), Selection::caret(start + 11));
    document.command(&mut live, Command::Undo);
    document.command(
        &mut live,
        Command::Compose {
            range: None,
            text: "ni".into(),
            selected: Some(2..2),
        },
    );
    document.command(&mut live, Command::CancelComposition);
    assert_eq!(document.source().to_string(), source);
    assert_eq!(live.selection(), Selection::new(end, start));
    assert_eq!(document.encode().unwrap(), original.as_bytes());
    document.command(&mut live, Command::Insert("new".into()));
    document.command(&mut other, Command::Redo);
    assert_eq!(
        document.source().to_string(),
        "befnew\n\nafter  \n\n[p]: missing.png\n"
    );
    document.command(&mut other, Command::Undo);
    assert_eq!(document.source().to_string(), source);
    assert_eq!(other.selection(), Selection::new(end, start));
    assert_eq!(document.encode().unwrap(), original.as_bytes());
}

#[gpui_kit::test]
fn native_math_renders_glyph_atoms_and_local_source_uses_document_history_and_save(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::{TestEventExt as _, TestInput, TestWindowExt as _};
    use gpui_kit::{Focusable as _, MouseButton};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("math.md");
    let formulas = [
        r"x_i^2",
        r"\frac{a+b}{c}",
        r"\sqrt{x}+\sqrt[3]{y}",
        r"\alpha+\Gamma",
        r"\lim_{x\to0}\sin x",
        r"\int_0^1 x\,dx",
        r"\sum_{i=1}^n i",
        r"x\quad\text{when }y>0",
    ];
    let source = format!(
        "# Math\r\n\r\n前 $x_i^2$ 后 $x$  \r\n\n$$\n{}\n$$\n\n错误 $\\unknowncommand{{x}}$ 与 $\\frac{{a}}$\n\n[keep]: target  ",
        formulas[1..].join(" + ")
    );
    std::fs::write(&path, source.as_bytes()).unwrap();
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
    let ready = "markdown-math-ready-x_i^2";
    loop {
        context.run_until_parked();
        context.update(|window, cx| window.render_frame(cx));
        if context.update(|window, _| window.try_find(ready).is_some()) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "native formula never became ready"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    context.update(|window, _| {
        assert!(window.find(ready).bounds().size.width > gpui_kit::px(4.));
        let block = format!("markdown-math-ready-{}", formulas[1..].join(" + "));
        assert!(
            window
                .find(gpui_kit::SharedString::from(block))
                .bounds()
                .size
                .height
                > gpui_kit::px(4.)
        );
        let error = window.find("markdown-math-error-\\unknowncommand{x}");
        assert!(
            error
                .label()
                .is_some_and(|reason| reason.contains("unknowncommand"))
        );
        assert!(window.try_find("markdown-math-error-\\frac{a}").is_some());
    });
    let normalized = source.replace("\r\n", "\n");
    let at = normalized.find("x_i^2").unwrap() + 5;
    let short = normalized.find("$x$").unwrap();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.range_to_bounds(&(short..short), cx),
            view.range_to_bounds(&(short + 1..short + 1), cx)
        );
    });
    context.update(|window, cx| {
        let point = window.find(ready).bounds().center();
        window.pointer_down(point, MouseButton::Left, cx);
        window.pointer_up(point, MouseButton::Left, cx);
    });
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(view.selection(), Selection::caret(at - 5));
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source.as_bytes()
        );
    });
    // Do not pump the executor between revisions: the first requested glyph
    // batch is still queued when its source is replaced by the second edit.
    for replacement in ["x_i^3", "x_i^4"] {
        view.update_in(&mut context, |view, window, cx| {
            view.command(
                Command::Replace {
                    range: at - 5..at,
                    text: replacement.into(),
                    kind: EditKind::Command,
                },
                window,
                cx,
            );
        });
    }
    view.update_in(&mut context, |view, _, cx| view.select_range(0..0, cx));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        context.run_until_parked();
        context.update(|window, cx| window.render_frame(cx));
        if context.update(|window, _| window.try_find("markdown-math-ready-x_i^4").is_some()) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "newest formula output did not replace the queued old revision"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    context.update(|window, _| assert!(window.try_find("markdown-math-ready-x_i^3").is_none()));
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source.replacen("x_i^2", "x_i^4", 1).as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source.as_bytes()
        )
    });
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(at..at, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| {
        window.render_frame(cx);
        let surface = window.find("markdown-source").bounds();
        let mut input = TestInput::new(
            gpui_kit::ElementInputHandler::new(surface, view.clone()),
            window,
        );
        input.compose_text(None, "ni", Some(2..2), cx);
        input.compose_text(None, "", Some(0..0), cx);
        input.commit_text("+1", cx);
    });
    context.run_until_parked();
    let edited = source.replacen("x_i^2", "x_i^2+1", 1);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).source().to_string(),
            edited.replace("\r\n", "\n")
        );
        assert_eq!(view.selection(), Selection::caret(at + 2));
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).source().to_string(), normalized)
    });
    context.dispatch_action(gpui_kit::component::input::Redo);
    context.run_until_parked();
    // Queue another revision before background rendering settles; late output
    // must never restore its old formula source, selection, or save snapshot.
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(0..0, cx);
        view.command(Command::Insert("正文\n\n".into()), window, cx);
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
        format!("正文\r\n\r\n{edited}").as_bytes()
    );
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).source().to_string(), normalized)
    });
    context.dispatch_action(gpui_kit::component::input::Redo);
    context.dispatch_action(gpui_kit::component::input::Redo);
    context.run_until_parked();
    app.update_in(&mut context, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.open_file_in_editor(&path, window, cx);
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        context.run_until_parked();
        if app.read_with(&context, |app, cx| {
            app.active_editor_input().is_some_and(|input| {
                input.read(cx).text().to_string()
                    == format!("正文\n\n{}", edited.replace("\r\n", "\n"))
            })
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "saved formula source did not reopen"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
