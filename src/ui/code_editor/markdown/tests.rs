use super::*;
use gpui_kit::{EntityInputHandler as _, VisualContext as _, test::TestWindowExt as _};

fn start_in_source(
    view: &gpui_kit::Entity<SourcePresentation>,
    cx: &mut gpui_kit::VisualTestContext,
) {
    view.update_in(cx, |view, window, cx| {
        if view.is_live() {
            view.toggle_mode(window, cx);
        }
        assert!(
            !view.is_live(),
            "this scenario starts in source before exercising live mode"
        );
    });
}

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
            start_in_source(&view, &mut view_context);
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
            view.is_live() && view.range_to_bounds(&(0..0), cx).is_some()
        }) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "live document geometry did not settle"
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
            start_in_source(&view, &mut view_context);
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
            start_in_source(&view, &mut view_context);
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
fn native_prose_delete_does_not_merge_table_cells(cx: &mut gpui_kit::TestAppContext) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::{Focusable as _, test::TestWindowExt as _};
    for (source, at, key) in [
        ("| a |\n| --- |\n| b |\n\nX", 21, "backspace"),
        ("X\n\n| a |\n| --- |", 1, "delete"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("table-boundary.md");
        std::fs::write(&path, source).unwrap();
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
            view.select_range(at..at, cx);
            view.focus_handle(cx).focus(window, cx);
        });
        context.update(|window, cx| window.press(key, cx));
        view.read_with(&context, |view, cx| {
            assert_eq!(
                view.document().read(cx).encode().unwrap(),
                source.as_bytes()
            );
            assert_eq!(view.selection().range(), at..at);
        });
    }
}

#[gpui_kit::test]
fn native_mdx_table_renders_each_embedded_control_once(cx: &mut gpui_kit::TestAppContext) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("table-components.mdx");
    let source = "| first | second |\n| --- | --- |\n| <Card title='A' /> | ordinary |\n| ordinary | <Card title='B' /> |";
    std::fs::write(&path, source).unwrap();
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
    context.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find_all_by_label("Edit card component name").len(),
            2
        );
    });
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source.as_bytes()
        )
    });
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
    let original = "* root\r\n\n  7) [ ] **child**\n+ neighbor\r\n\n> > *quote*\n";
    std::fs::write(&path, original).unwrap();
    let normalized = original.replace("\r\n", "\n");
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
            "* root\n\n  7) [ ] **child**\n  8) [ ] \n+ neighbor\n\n> > *quote*\n"
        )
    });
    context.update(|window, cx| window.press("enter", cx));
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).source().to_string(),
            "* root\n\n  7) [ ] **child**\n8) [ ] \n+ neighbor\n\n> > *quote*\n"
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
            "* root\n\n  7) [x] **child**\n+ neighbor\n\n> > *quote*\n"
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
            "* root\n\n  7) [x] **child**\n+ neighbor\n\n> > *quote*\n"
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
    let expected = "* root\n\n  7) [x] **child**\n+ neighbor\n\n> > *中文quote*\n";
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
        "* root\r\n\n  7) [x] **child**\n+ neighbor\r\n\n> > *中文quote*\n".as_bytes()
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
    assert!(reopened.read_with(&context, |view, _| view.is_live()));
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
    use gpui_kit::test::{TestEventExt as _, TestWindowExt as _};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("images.md");
    let source = "before ![图](missing.png) after\n\n![引用][picture]\n\n[picture]: other.png\n";
    std::fs::write(&path, source).unwrap();
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
    // Clicking the rendered atom must enter its original alt/source syntax.
    settle_live_geometry(&view, &mut context);
    let point = view.read_with(&context, |view, cx| {
        let a = view.range_to_bounds(&(start..start), cx).unwrap().center();
        let b = view.range_to_bounds(&(end..end), cx).unwrap().center();
        gpui_kit::point((a.x + b.x) / 2., a.y)
    });
    context.update(|window, cx| {
        window.pointer_down(point, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(point, gpui_kit::MouseButton::Left, cx);
        assert_eq!(view.read(cx).selection(), Selection::caret(start + 2));
        window.input("新", cx);
    });
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source.replacen("![图]", "![新图]", 1).as_bytes()
        );
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    settle_live_geometry(&view, &mut context);
    context.update(|window, cx| {
        let url_start = start + "![图](".len();
        let a = view
            .read(cx)
            .range_to_bounds(&(url_start..url_start), cx)
            .unwrap()
            .center();
        let b = view
            .read(cx)
            .range_to_bounds(&(end - 1..end - 1), cx)
            .unwrap()
            .center();
        window.pointer_down(a, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(a, gpui_kit::MouseButton::Left, cx);
        window.change_modifiers(gpui_kit::Modifiers::shift(), cx);
        window.pointer_down(b, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(b, gpui_kit::MouseButton::Left, cx);
        window.change_modifiers(gpui_kit::Modifiers::default(), cx);
        assert_eq!(view.read(cx).selection().range(), url_start..end - 1);
        window.input("new.png", cx);
    });
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
    context.update(|window, cx| window.press("shift-left", cx));
    context.run_until_parked();
    view.read_with(&context, |view, _| {
        assert_eq!(view.selection().range(), reference..reference_end)
    });
    context.update(|window, cx| window.press("shift-right", cx));
    settle_live_geometry(&view, &mut context);
    let point = view.read_with(&context, |view, cx| {
        let a = view
            .range_to_bounds(&(reference..reference), cx)
            .unwrap()
            .center();
        let b = view
            .range_to_bounds(&(reference_end..reference_end), cx)
            .unwrap()
            .center();
        gpui_kit::point((a.x + b.x) / 2., a.y)
    });
    context.update(|window, cx| {
        window.pointer_down(point, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(point, gpui_kit::MouseButton::Left, cx);
        window.input("新", cx);
    });
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source.replacen("![引用]", "![新引用]", 1).as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source.as_bytes()
        )
    });
}

#[gpui_kit::test]
fn opening_native_images_never_contacts_http_until_loading_intent(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::{TestEventExt as _, TestWindowExt as _};
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
            start_in_source(&view, &mut context);
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
    let start = source.find("![private]").unwrap();
    let end = source.find(" ![nested]").unwrap();
    let point = view.read_with(&context, |view, cx| {
        let a = view.range_to_bounds(&(start..start), cx).unwrap().center();
        let b = view.range_to_bounds(&(end..end), cx).unwrap().center();
        gpui_kit::point((a.x + b.x) / 2., a.y)
    });
    context.update(|window, cx| {
        window.pointer_down(point, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(point, gpui_kit::MouseButton::Left, cx);
    });
    context.run_until_parked();
    view.read_with(&context, |view, _| {
        assert_eq!(view.selection(), Selection::caret(start + 2))
    });
    view.update_in(&mut context, |view, _, cx| {
        view.select_range(source.len()..source.len(), cx)
    });
    settle_live_geometry(&view, &mut context);
    assert_eq!(
        requests.load(Ordering::SeqCst),
        0,
        "render, pointer activation, prefetch and size discovery must not request images"
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
            start_in_source(&view, &mut context);
            break view;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    let release = host.pause_next_read();
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    loop {
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
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
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
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
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
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
    loop {
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
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
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
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
    let after_row = "before  \r\n\n| **A** | B |\r\n| --- | ---: |\n| x\\|y | `z` |\r\n|  |  |\n\n[ref]: target  ";
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
    let expected = "before  \r\n\n| **A** |  | B |\r\n| --- | :---: | ---: |\n| x\\|y |  | `z` |\r\n|  |  |  |\n\n[ref]: target  ";
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
        assert_eq!(view.document().read(cx).encode().unwrap(), b"before  \r\n\n| **A** |  | B |\r\n| --- | :---: | ---: |\n| x\\|y |  | `z` |\r\n\n[ref]: target  ");
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
fn table_commands_keep_mdx_owner_and_reject_stale_source_bounds() {
    use super::projection::{Kind, Projection};
    let source = "| outside |\n| --- |\n| keep |\n\n<Note>\n\n| inside |\r\n| --- |\n| value |\r\n\n</Note>\n\n```md\n| code |\n| --- |\n```";
    let mut document = MarkdownDocument::new(source.into(), TextFormat::default());
    let normalized = source.replace("\r\n", "\n");
    let at = normalized.find("value").unwrap();
    let projection = Projection::parse(
        &normalized,
        document.revision(),
        true,
        super::mdx::Grammar::Mdx,
    );
    let owner = projection
        .leaves
        .iter()
        .find_map(|leaf| match leaf.kind {
            Kind::TableCell(cell) if leaf.range.contains(&at) => Some(cell.table()),
            _ => None,
        })
        .expect("table inside the owning MDX component");
    let mut state = PresentationState::new(Selection::caret(at));
    document.command(
        &mut state,
        Command::Table {
            operation: super::tables::Operation::InsertColumnAfter,
            range: owner.clone(),
            revision: projection.revision,
        },
    );
    let expected = source.replace(
        "| inside |\r\n| --- |\n| value |",
        "| inside |  |\r\n| --- | --- |\n| value |  |",
    );
    assert_eq!(document.encode().unwrap(), expected.as_bytes());
    document.command(&mut state, Command::Undo);
    assert_eq!(document.encode().unwrap(), source.as_bytes());
    let revision = document.revision();
    document.command(
        &mut state,
        Command::Table {
            operation: super::tables::Operation::DeleteRow,
            range: owner,
            revision: projection.revision,
        },
    );
    assert_eq!(document.revision(), revision);
    assert_eq!(document.encode().unwrap(), source.as_bytes());
    document.command(&mut state, Command::Redo);
    assert_eq!(document.encode().unwrap(), expected.as_bytes());
}

fn table_command(
    document: &mut MarkdownDocument,
    state: &mut PresentationState,
    operation: super::tables::Operation,
) {
    let projection = super::projection::Projection::parse(
        &document.source().to_string(),
        document.revision(),
        true,
        document.grammar(),
    );
    let at = state.selection().head();
    let range = projection
        .leaf_at(at)
        .and_then(|leaf| match leaf.kind {
            super::projection::Kind::TableCell(cell)
                if leaf.range.start <= at && at <= leaf.range.end =>
            {
                Some(cell.table())
            }
            _ => None,
        })
        .expect("owning table cell");
    document.command(
        state,
        Command::Table {
            operation,
            range,
            revision: projection.revision,
        },
    );
}

#[test]
fn table_structure_commands_preserve_cells_boundaries_and_atomic_selection() {
    use super::tables::Operation;
    let original =
        "outside  \r\n\n| **A** | B |\r\n| --- | ---: |\n| x\\|y | `z` |\r\n\n[ref]: target";
    let normalized = original.replace("\r\n", "\n");
    let at = normalized.find("x\\|y").unwrap();
    let prefix = "outside  \r\n\n";
    for (operation, table) in [
        (
            Operation::InsertRowBefore,
            "| **A** | B |\r\n| --- | ---: |\n|  |  |\n| x\\|y | `z` |",
        ),
        (
            Operation::InsertRowAfter,
            "| **A** | B |\r\n| --- | ---: |\n| x\\|y | `z` |\r\n|  |  |",
        ),
        (Operation::DeleteRow, "| **A** | B |\r\n| --- | ---: |"),
        (
            Operation::InsertColumnBefore,
            "|  | **A** | B |\r\n| --- | --- | ---: |\n|  | x\\|y | `z` |",
        ),
        (
            Operation::InsertColumnAfter,
            "| **A** |  | B |\r\n| --- | --- | ---: |\n| x\\|y |  | `z` |",
        ),
        (Operation::DeleteColumn, "| B |\r\n| ---: |\n| `z` |"),
        (
            Operation::AlignDefault,
            "| **A** | B |\r\n| --- | ---: |\n| x\\|y | `z` |",
        ),
        (
            Operation::AlignLeft,
            "| **A** | B |\r\n| :--- | ---: |\n| x\\|y | `z` |",
        ),
        (
            Operation::AlignCenter,
            "| **A** | B |\r\n| :---: | ---: |\n| x\\|y | `z` |",
        ),
        (
            Operation::AlignRight,
            "| **A** | B |\r\n| ---: | ---: |\n| x\\|y | `z` |",
        ),
    ] {
        let mut document = MarkdownDocument::new(original.into(), TextFormat::default());
        let selection = Selection::new(at, at + 4);
        let mut state = PresentationState::new(selection);
        table_command(&mut document, &mut state, operation);
        let suffix = if matches!(operation, Operation::DeleteRow | Operation::InsertRowAfter) {
            "\n\n[ref]: target"
        } else {
            "\r\n\n[ref]: target"
        };
        let expected = format!("{prefix}{table}{suffix}");
        assert_eq!(document.encode().unwrap(), expected.as_bytes());
        let after = state.selection();
        // Typing at the command's selection must change a surviving real cell.
        document.command(&mut state, Command::Insert("中".into()));
        assert!(document.source().to_string().contains("中"));
        let typed = document.encode().unwrap();
        let typed_selection = state.selection();
        document.command(&mut state, Command::Undo);
        assert_eq!(state.selection(), after);
        assert_eq!(document.encode().unwrap(), expected.as_bytes());
        document.command(&mut state, Command::Redo);
        assert_eq!(document.encode().unwrap(), typed);
        assert_eq!(state.selection(), typed_selection);
        document.command(&mut state, Command::Undo);
        assert_eq!(document.encode().unwrap(), expected.as_bytes());
        assert_eq!(state.selection(), after);
        if expected == original {
            // A no-op alignment does not invent a structural history entry.
            assert_eq!(after, selection);
        } else {
            document.command(&mut state, Command::Undo);
            assert_eq!(document.encode().unwrap(), original.as_bytes());
            assert_eq!(state.selection(), selection);
            document.command(&mut state, Command::Redo);
            assert_eq!(document.encode().unwrap(), expected.as_bytes());
            assert_eq!(state.selection(), after);
        }
    }
    // Even when alignment is already correct, the command separates typing
    // gestures without inserting a fake source change into shared history.
    let source = "| a |\r\n| --- |\n| b |";
    let mut document = MarkdownDocument::new(source.into(), TextFormat::default());
    let mut state = PresentationState::new(Selection::caret(3));
    document.command(&mut state, Command::Insert("X".into()));
    table_command(&mut document, &mut state, Operation::AlignDefault);
    document.command(&mut state, Command::Insert("Y".into()));
    assert_eq!(document.encode().unwrap(), b"| aXY |\r\n| --- |\n| b |");
    document.command(&mut state, Command::Undo);
    assert_eq!(document.encode().unwrap(), b"| aX |\r\n| --- |\n| b |");
    document.command(&mut state, Command::Undo);
    assert_eq!(document.encode().unwrap(), source.as_bytes());
    // Empty cells and content-end carets are legal editable insertion sites.
    for (source, at, expected) in [
        (
            "|  |\r\n| --- |\n|  |",
            2,
            "|  |  |\r\n| --- | --- |\n|  |  |",
        ),
        (
            "| a |\r\n| --- |\n| b |",
            3,
            "| a |  |\r\n| --- | --- |\n| b |  |",
        ),
    ] {
        let mut document = MarkdownDocument::new(source.into(), TextFormat::default());
        let mut state = PresentationState::new(Selection::caret(at));
        table_command(&mut document, &mut state, Operation::InsertColumnAfter);
        assert_eq!(document.encode().unwrap(), expected.as_bytes());
        document.command(&mut state, Command::Undo);
        assert_eq!(document.encode().unwrap(), source.as_bytes());
        assert_eq!(state.selection(), Selection::caret(at));
        document.command(&mut state, Command::Redo);
        assert_eq!(document.encode().unwrap(), expected.as_bytes());
    }
    for (source, token, operation, expected) in [
        (
            "| a |\r\n| --- |\n| b |\r\n| c |",
            "a",
            Operation::DeleteRow,
            "| b |\r\n| --- |\n| c |",
        ),
        (
            "| a |\r\n| --- |\n| b |",
            "b",
            Operation::InsertRowAfter,
            "| a |\r\n| --- |\n| b |\r\n|  |",
        ),
        (
            "| a |\r\n| --- |\n| b |",
            "b",
            Operation::DeleteRow,
            "| a |\r\n| --- |",
        ),
        (
            "- | a |\r\n  | --- |\n  | b |",
            "a",
            Operation::InsertRowBefore,
            "- |  |\r\n  | --- |\n  | a |\r\n  | b |",
        ),
        (
            "- parent\n  - child\n\n    | a |\r\n    | --- |\n    | b |",
            "b",
            Operation::InsertRowAfter,
            "- parent\n  - child\n\n    | a |\r\n    | --- |\n    | b |\r\n    |  |",
        ),
        (
            "> | a |\r\n>| --- |\n> | b |",
            "b",
            Operation::InsertColumnAfter,
            "> | a |  |\r\n>| --- | --- |\n> | b |  |",
        ),
    ] {
        let format = TextFormat {
            line_ending: LineEnding::CrLf,
            ..TextFormat::default()
        };
        let mut document = MarkdownDocument::new(source.into(), format);
        let at = source.replace("\r\n", "\n").find(token).unwrap();
        let mut state = PresentationState::new(Selection::caret(at));
        table_command(&mut document, &mut state, operation);
        assert_eq!(document.encode().unwrap(), expected.as_bytes());
        document.command(&mut state, Command::Undo);
        assert_eq!(document.encode().unwrap(), source.as_bytes());
        document.command(&mut state, Command::Redo);
        assert_eq!(document.encode().unwrap(), expected.as_bytes());
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
        table_command(&mut document, &mut state, operation);
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
fn native_mermaid_unchanged_diagrams_stay_visible_during_prose_and_neighbor_edits(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::Focusable as _;
    use gpui_kit::test::TestWindowExt as _;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("reuse.md");
    let source = "prose\n\n```mermaid\nflowchart LR\nA[First] --> B[End]\n```\n\n```mermaid\nflowchart LR\nC[Second] --> D[End]\n```\n\ntail";
    std::fs::write(&path, source).unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
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
            start_in_source(&view, &mut context);
            break view;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    let first = source.find("```mermaid").unwrap();
    let second = source.rfind("```mermaid").unwrap();
    let image_id = |offset| format!("markdown-mermaid-image-{offset}");
    let settle = |context: &mut gpui_kit::VisualTestContext, a, b| {
        loop {
            context
                .executor()
                .advance_clock(std::time::Duration::from_millis(100));
            context.run_until_parked();
            if context.update(|window, cx| {
                window.render_frame(cx);
                window.try_find(image_id(a)).is_some() && window.try_find(image_id(b)).is_some()
            }) {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    settle(&mut context, first, second);
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(0..0, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| window.input("new ", cx));
    // Inspect before advancing the debounced parse/render workers, then after
    // they settle. Neither unchanged native output may flash back to pending.
    context.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(image_id(first + 4)).is_some());
        assert!(window.try_find(image_id(second + 4)).is_some());
    });
    settle(&mut context, first + 4, second + 4);
    let at = source.find("First").unwrap() + 4;
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(at..at + 5, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| window.input("Changed", cx));
    context.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find(image_id(second + 6)).is_some(),
            "neighbor diagram must not enter pending"
        );
    });
    // Ordinary typing while the changed diagram is running must retain both
    // its request and the already usable neighboring output.
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(0..0, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| window.input("x", cx));
    context.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(image_id(second + 7)).is_some());
    });
    settle(&mut context, first + 5, second + 7);
    context.update(|_, cx| {
        gpui_kit::component::Theme::update(cx, |theme| theme.font_size = gpui_kit::px(20.))
    });
    settle(&mut context, first + 5, second + 7);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            format!("xnew {}", source.replacen("First", "Changed", 1)).as_bytes()
        )
    });
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
    // The released default minNodeWidth=120 makes Start and Changed equally
    // wide. This fixture explicitly measures labels, rather than mistaking a
    // valid minimum-width layout for a stale raster.
    let original = b"# Heading\r\n\r\n~~~~mermaid extra\r\n%%{init: {\"flowchart\": {\"minNodeWidth\": 0, \"wrappingWidth\": 1000, \"useMaxWidth\": false}}}%%\r\nflowchart LR\r\nA[Start] --> B[End]\r\n~~~~\n\n[ref]: untouched  \r\ntail";
    std::fs::write(&path, original).unwrap();
    let source = String::from_utf8(original.to_vec())
        .unwrap()
        .replace("\r\n", "\n");
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
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
            start_in_source(&view, &mut context);
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
        let body = source.find("%%{init:").unwrap();
        assert_eq!(view.selection(), Selection::caret(body));
        assert!(view.range_to_bounds(&(body..body), cx).is_some());
    });
    let at = source.find("Start").unwrap();
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(at..at + 5, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| window.input("Changed", cx));
    loop {
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
        context.run_until_parked();
        let rendered_size = context.update(|window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find(error_id.clone()).is_none(),
                "edited flowchart must remain a rendered diagram"
            );
            window
                .try_find(image_id.clone())
                .map(|image| image.bounds().size)
        });
        if rendered_size.is_some_and(|size| size.width > original_size.width) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the edited label must update the actual native graph: original={original_size:?}, observed={rendered_size:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    view.read_with(&context, |view, cx| {
        assert_eq!(view.selection(), Selection::caret(at + 7));
        assert_eq!(
            view.document().read(cx).source().to_string(),
            source.replacen("Start", "Changed", 1)
        );
    });
    let graph = source.find("\nflowchart LR\n").unwrap() + 1;
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
            start_in_source(&view, &mut context);
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
fn native_formula_pointer_inserts_inside_delimiters_and_reopens(cx: &mut gpui_kit::TestAppContext) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::{TestEventExt as _, TestWindowExt as _};
    let directory = tempfile::tempdir().unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    for (ix, formula) in [
        "$x$",
        "$$ x $$",
        "$$\nx\n$$",
        "$$$$  \n\n  x\n$$$$",
        "$$\n\n  x",
    ]
    .into_iter()
    .enumerate()
    {
        let path = directory.path().join(format!("pointer-{ix}.md"));
        let source = format!("before\r\n\r\n{formula}\n\nafter  ");
        std::fs::write(&path, &source).unwrap();
        app.update_in(&mut context, |app, window, cx| {
            app.open_file_in_editor(&path, window, cx)
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
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
        let identity = view.read_with(&context, |view, _| view.document().entity_id());
        context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
        settle_live_geometry(&view, &mut context);
        let normalized = source.replace("\r\n", "\n");
        let start = normalized.find('$').unwrap();
        let end = if formula.ends_with('$') {
            normalized.rfind('$').unwrap() + 1
        } else {
            normalized.len()
        };
        let insertion = normalized.find('x').unwrap();
        let point = view.read_with(&context, |view, cx| {
            let a = view.range_to_bounds(&(start..start), cx).unwrap().center();
            let b = view.range_to_bounds(&(end..end), cx).unwrap().center();
            gpui_kit::point((a.x + b.x) / 2., a.y)
        });
        context.update(|window, cx| {
            window.pointer_down(point, gpui_kit::MouseButton::Left, cx);
            window.pointer_up(point, gpui_kit::MouseButton::Left, cx);
        });
        context.run_until_parked();
        view.read_with(&context, |view, _| {
            assert_eq!(view.selection(), Selection::caret(insertion))
        });
        context.update(|window, cx| window.input("z+", cx));
        context.run_until_parked();
        let expected = source.replacen('x', "z+x", 1);
        view.read_with(&context, |view, cx| {
            assert_eq!(
                view.document().read(cx).encode().unwrap(),
                expected.as_bytes()
            )
        });
        context.dispatch_action(gpui_kit::component::input::Undo);
        context.run_until_parked();
        view.read_with(&context, |view, cx| {
            assert_eq!(
                view.document().read(cx).encode().unwrap(),
                source.as_bytes()
            )
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
            let reopened = app.read_with(&context, |app, _| {
                app.active_editor_input()
                    .and_then(|input| input.markdown().cloned())
            });
            if let Some(reopened) = reopened {
                if reopened.read_with(&context, |view, _| view.document().entity_id() != identity) {
                    reopened.read_with(&context, |view, cx| {
                        assert_eq!(
                            view.document().read(cx).encode().unwrap(),
                            expected.as_bytes()
                        )
                    });
                    break;
                }
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        app.update_in(&mut context, |app, window, cx| {
            app.editor_close_file(0, window, cx)
        });
        context.run_until_parked();
    }
}

#[gpui_kit::test]
fn native_math_renders_glyph_atoms_and_local_source_uses_document_history_and_save(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::{TestEventExt as _, TestInput, TestWindowExt as _};
    use gpui_kit::{Focusable as _, MouseButton};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("math.mdx");
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
        "# Math\r\n\r\n前 **$x_i^2$** 后 $x$  \r\n\n$$\n{}\n$$\n\n错误 $\\unknowncommand{{x}}$ 与 $\\frac{{a}}$\n\n[keep]: target  ",
        formulas[1..].join(" + ")
    );
    std::fs::write(&path, source.as_bytes()).unwrap();
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
        view.command(Command::Insert("正文\r\n\r\n".into()), window, cx);
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

#[gpui_kit::test]
fn structured_math_syntax_matrix_edits_renders_and_reopens_through_workbench(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::Focusable as _;
    use gpui_kit::test::TestWindowExt as _;

    // Each reference is a simpler expression with independently predictable
    // geometry. Literal TeX, a loading label, or one fixed placeholder cannot
    // satisfy the row/column/script relationships below.
    let fixtures = [
        ("scripts", r"x_i^2", r"x", true),
        ("fraction", r"\frac{a+b}{c}", r"a+b", true),
        ("root", r"\sqrt[3]{x}", r"x", false),
        ("greek", r"\alpha+\Gamma", r"\alpha", false),
        ("operators", r"\lim_{x\to0}\sin x+\cos x", r"\sin x", false),
        ("integral", r"\int_0^1 x\,dx", r"x", true),
        ("sum", r"\sum_{i=1}^n i", r"i", true),
        (
            "matrix",
            r"\begin{matrix}a&b\\c&d\end{matrix}",
            r"\begin{matrix}a&b\end{matrix}",
            true,
        ),
        (
            "parenthesized-matrix",
            r"\begin{pmatrix}a&b\\c&d\end{pmatrix}",
            r"\begin{pmatrix}a&b\end{pmatrix}",
            true,
        ),
        (
            "bracketed-matrix",
            r"\begin{bmatrix}a&b\\c&d\end{bmatrix}",
            r"\begin{bmatrix}a&b\end{bmatrix}",
            true,
        ),
        (
            "matrix-combinations",
            r"\begin{pmatrix}a_i^2&\frac{1}{x}\\\sqrt{x}&\sum_{i=1}^n i\end{pmatrix}",
            r"\begin{pmatrix}a&b\end{pmatrix}",
            true,
        ),
        (
            "cases",
            r"f(x)=\begin{cases}x_i^2&\text{if }x>0\\0&\text{otherwise}\end{cases}",
            r"f(x)=\begin{cases}0&x>0\end{cases}",
            true,
        ),
        (
            "aligned",
            r"\begin{aligned}a_i^2&=\sum_{i=1}^n i\\b&=\int_0^1 x\,dx\end{aligned}",
            r"\begin{aligned}a&=b\end{aligned}",
            true,
        ),
        (
            "text-and-operators",
            r"x_i^2+\text{when }x>0+\sum_{i=1}^n i",
            r"x_i^2",
            false,
        ),
    ];
    let directory = tempfile::tempdir().unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    let ready_bounds = |context: &mut gpui_kit::VisualTestContext, tex: &str| {
        let id = format!("markdown-math-ready-{tex}");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            context
                .executor()
                .advance_clock(std::time::Duration::from_millis(100));
            context.run_until_parked();
            context.update(|window, cx| window.render_frame(cx));
            if let Some(bounds) = context.update(|window, _| {
                window
                    .try_find(gpui_kit::SharedString::from(id.clone()))
                    .map(|image| {
                        assert_eq!(image.label(), Some(format!("Formula: {tex}").as_str()));
                        image.bounds()
                    })
            }) {
                break bounds;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "formula did not render: {tex}"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };

    for (name, tex, reference, taller) in fixtures {
        for block in [false, true] {
            let path = directory.path().join(format!("{name}-{block}.md"));
            let delimiter = if block { "$$\n" } else { "$" };
            let closing = if block { "\n$$" } else { "$" };
            let separator = if block { "\n\n" } else { " between " };
            let prose_gap = if block { "\n\n" } else { " " };
            let source = format!(
                "\u{feff}# Matrix\r\n\r\n前{prose_gap}{delimiter}{tex}{closing}{separator}{delimiter}{reference}{closing}{prose_gap}后  \r\n\n[keep]: target  "
            );
            std::fs::write(&path, source.as_bytes()).unwrap();
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
                assert!(
                    std::time::Instant::now() < deadline,
                    "{name}: workbench did not open"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            };
            let identity = view.read_with(&context, |view, _| view.document().entity_id());
            context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
            settle_live_geometry(&view, &mut context);
            let original_bounds = ready_bounds(&mut context, tex);
            let reference_bounds = ready_bounds(&mut context, reference);
            if taller {
                assert!(
                    original_bounds.size.height > reference_bounds.size.height,
                    "{name}, block={block}: structured rows/scripts must change the rendered height"
                );
            } else {
                assert!(
                    original_bounds.size.width > reference_bounds.size.width,
                    "{name}, block={block}: the extra notation must occupy rendered width"
                );
            }
            let normalized = source.trim_start_matches('\u{feff}').replace("\r\n", "\n");
            let start = normalized.find(tex).unwrap();
            let end = start + tex.len();
            let after = normalized.find('后').unwrap();
            view.read_with(&context, |view, cx| {
                let before_at = normalized.find('前').unwrap();
                let before = view.range_to_bounds(&(before_at..before_at), cx).unwrap();
                let after = view.range_to_bounds(&(after..after), cx).unwrap();
                if block {
                    assert!(after.top() > original_bounds.top());
                } else {
                    assert!(before.left() < original_bounds.left());
                    assert!(after.left() >= reference_bounds.right());
                }
                assert_eq!(
                    view.document().read(cx).encode().unwrap(),
                    source.as_bytes()
                );
            });

            // Enter through the real image hit target, then replace local TeX
            // through the focused production input handler, not a converter.
            context.update(|window, cx| {
                window.click(
                    gpui_kit::SharedString::from(format!("markdown-math-ready-{tex}")),
                    cx,
                )
            });
            context.run_until_parked();
            view.read_with(&context, |view, cx| {
                assert!(view.selection().head() >= start - delimiter.len());
                assert!(view.selection().head() <= end);
                assert_eq!(view.document().entity_id(), identity);
                assert_eq!(
                    view.document().read(cx).encode().unwrap(),
                    source.as_bytes()
                );
            });
            view.update_in(&mut context, |view, window, cx| {
                view.command(Command::Select(Selection::new(end, start)), window, cx);
                view.focus_handle(cx).focus(window, cx);
            });
            let edited_tex = format!("{tex}+z");
            context.update(|window, cx| window.input(&edited_tex, cx));
            context.run_until_parked();
            let edited_source = source.replacen(tex, &edited_tex, 1);
            view.read_with(&context, |view, cx| {
                assert_eq!(view.selection(), Selection::caret(start + edited_tex.len()));
                assert_eq!(
                    view.document().read(cx).encode().unwrap(),
                    edited_source.as_bytes()
                );
            });
            context.dispatch_action(gpui_kit::component::input::Undo);
            context.run_until_parked();
            view.read_with(&context, |view, cx| {
                assert_eq!(view.selection(), Selection::new(end, start));
                assert_eq!(
                    view.document().read(cx).encode().unwrap(),
                    source.as_bytes()
                );
            });
            context.dispatch_action(gpui_kit::component::input::Redo);
            context.run_until_parked();
            view.update_in(&mut context, |view, _, cx| view.select_range(0..0, cx));
            let edited_bounds = ready_bounds(&mut context, &edited_tex);
            assert!(
                edited_bounds.size.width > original_bounds.size.width,
                "{name}: local edit must update image geometry"
            );
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
                assert!(
                    std::time::Instant::now() < deadline,
                    "{name}: save did not settle"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert_eq!(std::fs::read(&path).unwrap(), edited_source.as_bytes());
            app.update_in(&mut context, |app, window, cx| {
                app.editor_close_file(0, window, cx);
                app.open_file_in_editor(&path, window, cx);
            });
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            let reopened = loop {
                context.run_until_parked();
                if let Some(view) = app.read_with(&context, |app, _| {
                    app.active_editor_input()
                        .and_then(|input| input.markdown().cloned())
                }) {
                    if view.read_with(&context, |view, _| view.document().entity_id() != identity) {
                        start_in_source(&view, &mut context);
                        break view;
                    }
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "{name}: saved document did not reopen"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            };
            reopened.read_with(&context, |view, cx| {
                assert_eq!(
                    view.document().read(cx).encode().unwrap(),
                    edited_source.as_bytes()
                )
            });
            context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
            settle_live_geometry(&reopened, &mut context);
            let reopened_bounds = ready_bounds(&mut context, &edited_tex);
            assert_eq!(
                reopened_bounds.size, edited_bounds.size,
                "{name}: reopening must render the edited expression, not the old source"
            );
            app.update_in(&mut context, |app, window, cx| {
                app.editor_close_file(0, window, cx)
            });
            context.run_until_parked();
        }
    }
}

#[gpui_kit::test]
fn structured_math_local_invalid_recovery_composition_and_selection_keep_one_history(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::{TestInput, TestWindowExt as _};
    use gpui_kit::{ElementInputHandler, Focusable as _};

    let fixtures = [
        r"\begin{pmatrix}a_i^2&\text{old}\\\sqrt{x}&\sum_{i=1}^n i\end{pmatrix}",
        r"f(x)=\begin{cases}x_i^2&\text{old}\\0&x\le0\end{cases}",
        r"\begin{aligned}a_i^2&=\text{old}\\b&=\int_0^1 x\,dx\end{aligned}",
        r"x_i^2+\sum_{i=1}^n i+\text{old}",
    ];
    let directory = tempfile::tempdir().unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    let await_output = |context: &mut gpui_kit::VisualTestContext, tex: &str, failed: bool| {
        let id = format!(
            "markdown-math-{}-{tex}",
            if failed { "error" } else { "ready" }
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            context
                .executor()
                .advance_clock(std::time::Duration::from_millis(100));
            context.run_until_parked();
            context.update(|window, cx| window.render_frame(cx));
            if context.update(|window, _| {
                window
                    .try_find(gpui_kit::SharedString::from(id.clone()))
                    .is_some()
            }) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "formula output never settled: {id}"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        context.update(|window, _| {
            let output = window.find(gpui_kit::SharedString::from(id.clone()));
            if failed {
                let prefix = crate::ui::i18n::t(crate::ui::i18n::L10nKey::EditorMarkdownMathError);
                let label = output.label().unwrap();
                assert!(label.starts_with(prefix));
                assert_ne!(label.trim(), prefix, "a failure must explain its cause");
                assert!(
                    window
                        .try_find(gpui_kit::SharedString::from(format!(
                            "markdown-math-ready-{tex}"
                        )))
                        .is_none()
                );
            } else {
                assert_eq!(output.label(), Some(format!("Formula: {tex}").as_str()));
                assert!(output.bounds().size.width > gpui_kit::px(20.));
                assert!(output.bounds().size.height > gpui_kit::px(4.));
            }
        });
    };

    for (ix, tex) in fixtures.into_iter().enumerate() {
        for block in [false, true] {
            let path = directory.path().join(format!("local-{ix}-{block}.md"));
            let source = if block {
                format!("# Local\r\n\r\n前  \r\n\n$$\n{tex}\n$$\n\n后🎉  \r\n[ref]: target  ")
            } else {
                format!("# Local\r\n\r\n前 ${tex}$ 后🎉  \r\n\n[ref]: target  ")
            };
            std::fs::write(&path, source.as_bytes()).unwrap();
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
            await_output(&mut context, tex, false);
            context.update(|window, cx| {
                window.click(
                    gpui_kit::SharedString::from(format!("markdown-math-ready-{tex}")),
                    cx,
                )
            });
            context.run_until_parked();
            let normalized = source.replace("\r\n", "\n");
            if block {
                if let Some(row) = tex.find(r"\\") {
                    let at = normalized.find(tex).unwrap() + row + 2;
                    view.update_in(&mut context, |view, window, cx| {
                        view.select_range(at..at, cx);
                        view.focus_handle(cx).focus(window, cx);
                    });
                    context.update(|window, cx| window.press("enter", cx));
                    context.run_until_parked();
                    view.read_with(&context, |view, cx| {
                        assert_eq!(
                            view.document().read(cx).source().to_string(),
                            normalized.replacen(r"\\", "\\\\\n", 1)
                        );
                        assert_eq!(view.selection(), Selection::caret(at + 1));
                    });
                    context.dispatch_action(gpui_kit::component::input::Undo);
                    context.run_until_parked();
                    view.read_with(&context, |view, cx| {
                        assert_eq!(
                            view.document().read(cx).encode().unwrap(),
                            source.as_bytes()
                        )
                    });
                }
            }
            let end = normalized.find(tex).unwrap() + tex.len();
            view.update_in(&mut context, |view, window, cx| {
                view.select_range(end..end, cx);
                view.focus_handle(cx).focus(window, cx);
            });
            context.update(|window, cx| window.press("backspace", cx));
            context.run_until_parked();
            let invalid_tex = &tex[..tex.len() - 1];
            let invalid_source = source.replacen(tex, invalid_tex, 1);
            view.read_with(&context, |view, cx| {
                assert_eq!(view.selection(), Selection::caret(end - 1));
                assert_eq!(
                    view.document().read(cx).encode().unwrap(),
                    invalid_source.as_bytes()
                );
            });
            view.update_in(&mut context, |view, _, cx| view.select_range(0..0, cx));
            await_output(&mut context, invalid_tex, true);
            // A malformed environment is still clickable local source, not a
            // dead error panel. Undo recovers both its original source and caret.
            context.update(|window, cx| {
                window.click(
                    gpui_kit::SharedString::from(format!("markdown-math-error-{invalid_tex}")),
                    cx,
                )
            });
            context.dispatch_action(gpui_kit::component::input::Undo);
            context.run_until_parked();
            view.read_with(&context, |view, cx| {
                assert_eq!(view.selection(), Selection::caret(end));
                assert_eq!(
                    view.document().read(cx).encode().unwrap(),
                    source.as_bytes()
                );
            });
            context.dispatch_action(gpui_kit::component::input::Redo);
            context.run_until_parked();
            view.read_with(&context, |view, cx| {
                assert_eq!(
                    view.document().read(cx).encode().unwrap(),
                    invalid_source.as_bytes()
                )
            });
            context.dispatch_action(gpui_kit::component::input::Undo);
            context.run_until_parked();

            let start = normalized.find("old").unwrap();
            let reversed = Selection::new(start + 3, start);
            view.update_in(&mut context, |view, window, cx| {
                view.command(Command::Select(reversed), window, cx);
                view.focus_handle(cx).focus(window, cx);
            });
            let utf16_start = normalized[..start].encode_utf16().count();
            context.update(|window, cx| {
                window.render_frame(cx);
                let bounds = window.find("markdown-source").bounds();
                let mut input =
                    TestInput::new(ElementInputHandler::new(bounds, view.clone()), window);
                let selected = input.selected_text_range(cx).unwrap();
                assert_eq!(selected.range, utf16_start..utf16_start + 3);
                assert!(selected.reversed);
                input.compose_text(None, "ni", Some(2..2), cx);
                assert_eq!(
                    input.marked_text_range(cx),
                    Some(utf16_start..utf16_start + 2)
                );
                input.compose_text(None, "", Some(0..0), cx);
                assert!(input.marked_text_range(cx).is_none());
            });
            context.run_until_parked();
            view.read_with(&context, |view, cx| {
                assert_eq!(view.selection(), reversed);
                assert_eq!(
                    view.document().read(cx).encode().unwrap(),
                    source.as_bytes()
                );
            });
            // Cancelling preedit must not discard the existing redo branch.
            context.dispatch_action(gpui_kit::component::input::Redo);
            context.run_until_parked();
            view.read_with(&context, |view, cx| {
                assert_eq!(
                    view.document().read(cx).encode().unwrap(),
                    invalid_source.as_bytes()
                )
            });
            context.dispatch_action(gpui_kit::component::input::Undo);
            context.run_until_parked();
            view.update_in(&mut context, |view, window, cx| {
                view.command(Command::Select(reversed), window, cx);
                view.focus_handle(cx).focus(window, cx);
            });
            context.update(|window, cx| {
                window.render_frame(cx);
                let bounds = window.find("markdown-source").bounds();
                let mut input =
                    TestInput::new(ElementInputHandler::new(bounds, view.clone()), window);
                input.compose_text(None, "ne", Some(2..2), cx);
                input.compose_text(None, "new", Some(3..3), cx);
                assert_eq!(
                    input.marked_text_range(cx),
                    Some(utf16_start..utf16_start + 3)
                );
                input.commit_text("new", cx);
                assert!(input.marked_text_range(cx).is_none());
            });
            context.run_until_parked();
            let edited_source = source.replacen("old", "new", 1);
            let edited_tex = tex.replacen("old", "new", 1);
            view.read_with(&context, |view, cx| {
                assert_eq!(view.selection(), Selection::caret(start + 3));
                assert_eq!(
                    view.document().read(cx).encode().unwrap(),
                    edited_source.as_bytes()
                );
            });
            // Cross the live/source boundary; composition is still one undo.
            context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
            context.dispatch_action(gpui_kit::component::input::Undo);
            context.run_until_parked();
            view.read_with(&context, |view, cx| {
                assert_eq!(view.selection(), reversed);
                assert_eq!(
                    view.document().read(cx).encode().unwrap(),
                    source.as_bytes()
                );
            });
            context.dispatch_action(gpui_kit::component::input::Redo);
            context.run_until_parked();
            view.read_with(&context, |view, cx| {
                assert_eq!(
                    view.document().read(cx).encode().unwrap(),
                    edited_source.as_bytes()
                )
            });
            context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
            view.update_in(&mut context, |view, _, cx| view.select_range(0..0, cx));
            await_output(&mut context, &edited_tex, false);
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
            assert_eq!(std::fs::read(&path).unwrap(), edited_source.as_bytes());
            app.update_in(&mut context, |app, window, cx| {
                app.editor_close_file(0, window, cx)
            });
            context.run_until_parked();
            app.update_in(&mut context, |app, window, cx| {
                app.open_file_in_editor(&path, window, cx)
            });
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            let reopened = loop {
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
            reopened.read_with(&context, |view, cx| {
                assert_eq!(
                    view.document().read(cx).encode().unwrap(),
                    edited_source.as_bytes()
                )
            });
            context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
            settle_live_geometry(&reopened, &mut context);
            await_output(&mut context, &edited_tex, false);
            app.update_in(&mut context, |app, window, cx| {
                app.editor_close_file(0, window, cx)
            });
            context.run_until_parked();
        }
    }
}
// Merman =0.9.2 / Mermaid 12.1.0. Sources use the released registry corpus
// (merman-core/src/tests/registry.rs), not a browser or a renderer-only seam.
// Each expansion is a separate workbench test: one failure must not prevent
// the remaining families from producing their own verification evidence.
fn mermaid_matrix_workbench(
    cx: &mut gpui_kit::TestAppContext,
    name: &str,
    diagram: &str,
    edited: &str,
    rendered: bool,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::Focusable as _;
    use gpui_kit::test::{TestEventExt as _, TestQueryExt as _, TestWindowExt as _};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(format!("{name}.md"));
    let envelope = |body: &str| {
        format!(
            "# Matrix\r\n\r\n```mermaid\r\n{}\r\n```\n\n[ref]: untouched  \r\ntail",
            body.replace('\n', "\r\n")
        )
    };
    let original = envelope(diagram);
    let normalized = original.replace("\r\n", "\n");
    std::fs::write(&path, original.as_bytes()).unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    cx.simulate_window_resize(
        context.window_handle(),
        gpui_kit::size(gpui_kit::px(1200.), gpui_kit::px(1400.)),
    );
    app.update_in(&mut context, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    let view = {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            context.run_until_parked();
            if let Some(view) = app.read_with(&context, |app, _| {
                app.active_editor_input()
                    .and_then(|input| input.markdown().cloned())
            }) {
                start_in_source(&view, &mut context);
                break view;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "{name}: document did not open"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    let identity = view.read_with(&context, |view, _| view.document().entity_id());
    let fence = normalized.find("```mermaid").unwrap();
    let body = fence + "```mermaid\n".len();
    let image_id = format!("markdown-mermaid-image-{fence}");
    let error_id = format!("markdown-mermaid-error-{fence}");
    let settle = |context: &mut gpui_kit::VisualTestContext, image: bool| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            context
                .executor()
                .advance_clock(std::time::Duration::from_millis(100));
            context.run_until_parked();
            let ready = context.update(|window, cx| {
                window.render_frame(cx);
                if image {
                    if let Some(error) = window.try_find(error_id.clone()) {
                        panic!("{name}: actual native renderer reported {:?}; source/error display is not support", error.label());
                    }
                    window.try_find(image_id.clone()).is_some_and(|image| {
                        let size = image.bounds().size;
                        size.width > gpui_kit::px(1.) && size.height > gpui_kit::px(1.)
                    })
                } else {
                    window.try_find(error_id.clone()).is_some() && window.try_find(image_id.clone()).is_none()
                }
            });
            if ready {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "{name}: native state did not settle (image={image})"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    settle(&mut context, rendered);
    context.dispatch_action(crate::core::actions::EditorSave);
    let wait_saved = |context: &mut gpui_kit::VisualTestContext| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            context.run_until_parked();
            if app.read_with(context, |app, _| {
                app.active_buffer()
                    .is_some_and(|file| !file.dirty && !file.saving)
            }) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "{name}: save did not complete"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    wait_saved(&mut context);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        original.as_bytes(),
        "{name}: no-edit rendering must not rewrite bytes"
    );
    context.update(|window, cx| {
        let label = crate::ui::i18n::t(crate::ui::i18n::L10nKey::EditorMarkdownMermaidEdit);
        let point = window.find_by_label(label).bounds().center();
        window.pointer_down(point, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(point, gpui_kit::MouseButton::Left, cx);
        window.render_frame(cx);
        assert_eq!(
            view.read(cx).selection(),
            Selection::caret(body),
            "{name}: contextual source activation"
        );
        assert!(view.read(cx).range_to_bounds(&(body..body), cx).is_some());
    });
    // An intervening real failure removes the old successful image, retaining
    // the editable source. The following successful edit cannot pass by merely
    // keeping the first raster visible.
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(body..body + diagram.len(), cx);
        view.focus_handle(cx).focus(window, cx);
        view.command(Command::Insert("not-a-mermaid-diagram".into()), window, cx);
    });
    settle(&mut context, false);
    view.read_with(&context, |view, cx| {
        assert!(
            view.document()
                .read(cx)
                .source()
                .to_string()
                .contains("not-a-mermaid-diagram")
        );
        assert!(
            view.range_to_bounds(&(body..body), cx).is_some(),
            "{name}: failed source remains editable"
        );
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context, rendered);
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).source().to_string(), normalized)
    });
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(body..body + diagram.len(), cx);
        view.focus_handle(cx).focus(window, cx);
        view.command(Command::Insert(edited.into()), window, cx);
    });
    settle(&mut context, rendered);
    let expected = envelope(edited);
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().entity_id(), identity);
        assert_eq!(
            view.document().read(cx).source().to_string(),
            expected.replace("\r\n", "\n")
        );
    });
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).source().to_string(), normalized)
    });
    context.dispatch_action(gpui_kit::component::input::Redo);
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    settle(&mut context, rendered);
    context.dispatch_action(crate::core::actions::EditorSave);
    wait_saved(&mut context);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        expected.as_bytes(),
        "{name}: unchanged Markdown and mixed line endings"
    );
    app.update_in(&mut context, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.open_file_in_editor(&path, window, cx);
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        context.run_until_parked();
        if app.read_with(&context, |app, cx| {
            app.active_editor_input().is_some_and(|input| {
                input.markdown().is_some_and(|view| {
                    view.read(cx).document().entity_id() != identity
                        && view.read(cx).document().read(cx).source().to_string()
                            == expected.replace("\r\n", "\n")
                })
            })
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{name}: saved document did not reopen"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(app.read_with(&context, |app, cx| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .read(cx)
            .is_live()
    }));
    settle(&mut context, rendered);
    assert_eq!(std::fs::read(&path).unwrap(), expected.as_bytes());
}

macro_rules! mermaid_matrix_case {
    ($name:ident, $source:expr, $edited:expr) => {
        #[gpui_kit::test]
        fn $name(cx: &mut gpui_kit::TestAppContext) {
            mermaid_matrix_workbench(cx, stringify!($name), $source, $edited, true);
        }
    };
}

mermaid_matrix_case!(
    native_mermaid_matrix_flowchart,
    "flowchart LR\nA[Original] --> B[End]",
    "flowchart LR\nA[Edited label] --> B[End]"
);

// Merman =0.9.2 / Mermaid 12.1.0; all-diagrams + complete-svg, without ELK.
// Inputs follow the released registry/native_font_families examples, with an
// independently specified changed label/value. Every row must really rasterize;
// these are acceptance fixtures, not declarations that an unrun row passed.
mermaid_matrix_case!(
    native_mermaid_matrix_agentflow,
    "agentflow-beta\nA[Original] --> B[End]",
    "agentflow-beta\nA[Edited label] --> B[End]"
);
mermaid_matrix_case!(
    native_mermaid_matrix_architecture,
    "architecture-beta\nservice api(server)[Original]",
    "architecture-beta\nservice api(server)[Edited label]"
);
mermaid_matrix_case!(
    native_mermaid_matrix_block,
    "block-beta\ncolumns 2\nA[\"Original\"] B[\"End\"]",
    "block-beta\ncolumns 2\nA[\"Edited label\"] B[\"End\"]"
);
mermaid_matrix_case!(
    native_mermaid_matrix_c4,
    "C4Context\nPerson(user, \"Original\")\nSystem(bank, \"Bank\")\nRel(user, bank, \"Uses\")",
    "C4Context\nPerson(user, \"Edited label\")\nSystem(bank, \"Bank\")\nRel(user, bank, \"Uses\")"
);
mermaid_matrix_case!(
    native_mermaid_matrix_class,
    "classDiagram\nclass Animal {\n+String name\n+eat()\n}\nAnimal <|-- Dog",
    "classDiagram\nclass Animal {\n+String name\n+sleep()\n}\nAnimal <|-- Dog"
);
mermaid_matrix_case!(
    native_mermaid_matrix_cynefin,
    "cynefin-beta\nclear\n \"Original\"\ncomplex\n \"Retrospective\"\nclear --> complex : \"Probe\"",
    "cynefin-beta\nclear\n \"Edited label\"\ncomplex\n \"Retrospective\"\nclear --> complex : \"Probe\""
);
mermaid_matrix_case!(
    native_mermaid_matrix_er,
    "erDiagram\nCUSTOMER ||--o{ ORDER : places\nCUSTOMER {\nstring name\n}",
    "erDiagram\nCUSTOMER ||--o{ ORDER : purchases\nCUSTOMER {\nstring name\n}"
);
mermaid_matrix_case!(
    native_mermaid_matrix_event_modeling,
    "eventmodeling\ntf 01 ui Shop.Cart\ntf 02 cmd Ordering.AddItem ->> 01 { sku: \"SKU-1\" }",
    "eventmodeling\ntf 01 ui Shop.Cart\ntf 02 cmd Ordering.RemoveItem ->> 01 { sku: \"SKU-1\" }"
);
mermaid_matrix_case!(
    native_mermaid_matrix_gantt,
    "gantt\ndateFormat YYYY-MM-DD\nsection Work\nOriginal :a, 2024-01-01, 1d",
    "gantt\ndateFormat YYYY-MM-DD\nsection Work\nEdited label :a, 2024-01-01, 1d"
);
mermaid_matrix_case!(
    native_mermaid_matrix_git_graph,
    "gitGraph\ncommit id: \"Original\"\nbranch feature\ncheckout feature\ncommit id: \"Work\"",
    "gitGraph\ncommit id: \"Edited label\"\nbranch feature\ncheckout feature\ncommit id: \"Work\""
);
mermaid_matrix_case!(native_mermaid_matrix_info, "info", "info showInfo");
// Merman 0.9.2 ignores showInfo in layout_info_diagram_typed: this is a
// fixed version-information graphic, not user-editable diagram labels.
mermaid_matrix_case!(
    native_mermaid_matrix_ishikawa,
    "ishikawa-beta\nBlurry Photo\n Process\n  Out of focus\n User\n  Wrong position",
    "ishikawa-beta\nBlurry Photo\n Process\n  Out of focus\n User\n  Shaky hands"
);
mermaid_matrix_case!(
    native_mermaid_matrix_journey,
    "journey\nsection Work\nOriginal: 5: Developer",
    "journey\nsection Work\nEdited label: 3: Developer"
);
mermaid_matrix_case!(
    native_mermaid_matrix_kanban,
    "kanban\n todo[Todo]\n  task[Original]",
    "kanban\n todo[Todo]\n  task[Edited label]"
);
mermaid_matrix_case!(
    native_mermaid_matrix_mindmap,
    "mindmap\n Root\n  Original\n  Other child",
    "mindmap\n Root\n  Edited label\n  Other child"
);
mermaid_matrix_case!(
    native_mermaid_matrix_packet,
    "packet-beta\n0-7: \"Original\"\n8-15: \"TYPE\"",
    "packet-beta\n0-7: \"Edited\"\n8-15: \"TYPE\""
);
mermaid_matrix_case!(
    native_mermaid_matrix_pie,
    "pie\n\"Original\": 40\n\"Pending\": 60",
    "pie\n\"Edited label\": 30\n\"Pending\": 70"
);
mermaid_matrix_case!(
    native_mermaid_matrix_quadrant_chart,
    "quadrantChart\nx-axis Low --> High\ny-axis Low --> High\nOriginal: [0.5, 0.5]",
    "quadrantChart\nx-axis Low --> High\ny-axis Low --> High\nEdited: [0.7, 0.8]"
);
mermaid_matrix_case!(
    native_mermaid_matrix_radar,
    "radar-beta\naxis Speed,Quality,Cost\ncurve Original{1,2,3}",
    "radar-beta\naxis Speed,Quality,Cost\ncurve Edited{3,2,1}"
);
mermaid_matrix_case!(
    native_mermaid_matrix_railroad,
    "railroad-beta\nrule = sequence(terminal(\"hello\"), nonterminal(\"other\")) ;\nother = terminal(\"world\") ;",
    "railroad-beta\nrule = sequence(terminal(\"edited\"), nonterminal(\"other\")) ;\nother = terminal(\"world\") ;"
);
mermaid_matrix_case!(
    native_mermaid_matrix_railroad_ebnf,
    "railroad-ebnf-beta\nrule = \"original\" ;",
    "railroad-ebnf-beta\nrule = \"edited\" ;"
);
mermaid_matrix_case!(
    native_mermaid_matrix_railroad_abnf,
    "railroad-abnf-beta\nrule = \"original\" ;",
    "railroad-abnf-beta\nrule = \"edited\" ;"
);
mermaid_matrix_case!(
    native_mermaid_matrix_railroad_peg,
    "railroad-peg-beta\nrule <- \"original\" ;",
    "railroad-peg-beta\nrule <- \"edited\" ;"
);
mermaid_matrix_case!(
    native_mermaid_matrix_requirement,
    "requirementDiagram\nrequirement req1 {\nid: 1\ntext: Original\nrisk: high\nverifymethod: analysis\n}",
    "requirementDiagram\nrequirement req1 {\nid: 1\ntext: Edited label\nrisk: high\nverifymethod: analysis\n}"
);
mermaid_matrix_case!(
    native_mermaid_matrix_sankey,
    "sankey-beta\nSource,Destination,10",
    "sankey-beta\nSource,Destination,20"
);
mermaid_matrix_case!(
    native_mermaid_matrix_sequence,
    "sequenceDiagram\nAlice->>Bob: Original\nBob-->>Alice: Reply",
    "sequenceDiagram\nAlice->>Bob: Edited label\nBob-->>Alice: Reply"
);
mermaid_matrix_case!(
    native_mermaid_matrix_state,
    "stateDiagram-v2\n[*] --> Idle\nIdle --> Active: Original\nActive --> Idle: stop",
    "stateDiagram-v2\n[*] --> Idle\nIdle --> Active: Edited label\nActive --> Idle: stop"
);
mermaid_matrix_case!(
    native_mermaid_matrix_swimlane,
    "swimlane-beta LR\nA[Original] --> B[End]",
    "swimlane-beta LR\nA[Edited label] --> B[End]"
);
mermaid_matrix_case!(
    native_mermaid_matrix_timeline,
    "timeline\nsection Preparation\n2024 : Original\n2025 : Release",
    "timeline\nsection Preparation\n2024 : Edited label\n2025 : Release"
);
mermaid_matrix_case!(
    native_mermaid_matrix_tree_view,
    "treeView-beta\n  \"packages\"\n    \"original\"\n      \"src\"",
    "treeView-beta\n  \"packages\"\n    \"edited\"\n      \"src\""
);
mermaid_matrix_case!(
    native_mermaid_matrix_treemap,
    "treemap-beta\n\"Root\"\n \"Original\": 40\n \"Pending\": 60",
    "treemap-beta\n\"Root\"\n \"Edited label\": 30\n \"Pending\": 70"
);
mermaid_matrix_case!(
    native_mermaid_matrix_usecase,
    "usecase-beta\nactor Customer(\"Customer\")\nsystemBoundary \"Order system\"\n Checkout(\"Original\")\nend\nCustomer --> Checkout",
    "usecase-beta\nactor Customer(\"Customer\")\nsystemBoundary \"Order system\"\n Checkout(\"Edited label\")\nend\nCustomer --> Checkout"
);
// Keep the boundary fixture above: the flat released registry example is an
// additional discriminator for parser/family admission versus compound layout.
mermaid_matrix_case!(
    native_mermaid_matrix_usecase_flat,
    "usecase-beta\nactor Customer(\"Customer\")\nCheckout(\"Original\")\nCustomer --> Checkout",
    "usecase-beta\nactor Customer(\"Customer\")\nCheckout(\"Edited label\")\nCustomer --> Checkout"
);
mermaid_matrix_case!(
    native_mermaid_matrix_venn,
    "venn-beta\nset A[\"Original\"]:10\nset B[\"Editor\"]:8\nunion A,B[\"Shared\"]:3",
    "venn-beta\nset A[\"Edited label\"]:10\nset B[\"Editor\"]:8\nunion A,B[\"Shared\"]:3"
);
mermaid_matrix_case!(
    native_mermaid_matrix_wardley,
    "wardley-beta\ncomponent Original [0.6, 0.7]",
    "wardley-beta\ncomponent Edited [0.5, 0.8]"
);
mermaid_matrix_case!(
    native_mermaid_matrix_xychart,
    "xychart-beta\nx-axis [Jan, Feb, Mar]\ny-axis \"Revenue\" 0 --> 100\nbar [20, 40, 80]",
    "xychart-beta\nx-axis [Jan, Feb, Mar]\ny-axis \"Revenue\" 0 --> 100\nbar [40, 60, 80]"
);
// zenuml remains the external comparison channel, not a 38th primary row.
mermaid_matrix_case!(
    native_mermaid_matrix_zenuml,
    "zenuml\nAlice->Bob: Original\nBob-->Alice: Reply",
    "zenuml\nAlice->Bob: Edited label\nBob-->Alice: Reply"
);
mermaid_matrix_case!(
    native_mermaid_matrix_unicode_html_labels,
    "flowchart LR\nA[\"<b>中文</b><br/>标签 é 👩‍💻\"] --> B[End]",
    "flowchart LR\nA[\"<b>中文更新</b><br/>标签 é 👩‍💻\"] --> B[End]"
);
mermaid_matrix_case!(
    native_mermaid_matrix_strict_callback_is_inert,
    "flowchart LR\nA[Original] --> B[End]\nclick A call arbitraryProjectFunction()",
    "flowchart LR\nA[Edited label] --> B[End]\nclick A call arbitraryProjectFunction()"
);

// Additional released syntax examples stay visible, rather than using a small
// flowchart success to imply that every variant/label grammar was exercised.
mermaid_matrix_case!(
    native_mermaid_matrix_graph_alias,
    "graph TD\nA[Original] --> B",
    "graph TD\nA[Edited label] --> B"
);
mermaid_matrix_case!(
    native_mermaid_matrix_flowchart_elk_without_elk,
    "flowchart-elk TD\nA[Original] --> B",
    "flowchart-elk TD\nA[Edited label] --> B"
);
mermaid_matrix_case!(
    native_mermaid_matrix_state_legacy,
    "stateDiagram\n[*] --> Original",
    "stateDiagram\n[*] --> Edited"
);
mermaid_matrix_case!(
    native_mermaid_matrix_flowchart_html_unicode_labels,
    "flowchart LR\nA[\"<b>中文 é 👩‍💻</b><br/>Long label\"] --> B[End]",
    "flowchart LR\nA[\"<b>更新中文 é 👩‍💻</b><br/>Long edited label\"] --> B[End]"
);
mermaid_matrix_case!(
    native_mermaid_matrix_flowchart_shapes_edges_styles,
    "flowchart LR\nsubgraph group[Group]\nA@{ shape: rect, label: Original } ==> B{Decision}\nend\nB -. label .-> C((Done))\nclassDef task font-weight:bold\nclass A task",
    "flowchart LR\nsubgraph group[Group]\nA@{ shape: rect, label: Edited } ==> B{Decision}\nend\nB -. changed .-> C((Done))\nclassDef task font-weight:bold\nclass A task"
);

#[gpui_kit::test]
fn native_mermaid_matrix_error_primary_is_not_successful_diagram(
    cx: &mut gpui_kit::TestAppContext,
) {
    mermaid_matrix_workbench(
        cx,
        "error-primary-boundary",
        "error",
        "error\n%% retained edit",
        false,
    );
}

#[gpui_kit::test]
fn native_mermaid_matrix_unknown_and_empty_sources_retain_bytes(cx: &mut gpui_kit::TestAppContext) {
    mermaid_matrix_workbench(
        cx,
        "empty-unknown-boundary",
        "",
        "unknownDiagram\nretained source",
        false,
    );
}

#[gpui_kit::test]
fn native_mermaid_matrix_external_file_and_network_resources_fail_closed(
    cx: &mut gpui_kit::TestAppContext,
) {
    mermaid_matrix_workbench(
        cx,
        "external-resource-boundary",
        "flowchart LR\nA@{ img: \"file:///private/mermaid-must-not-read.png\", label: \"File\" } --> B",
        "flowchart LR\nA@{ img: \"https://example.test/mermaid-must-not-fetch.png\", label: \"Network\" } --> B",
        false,
    );
}

mermaid_matrix_case!(
    native_mermaid_matrix_script_labels_are_inert_text,
    "zenuml\nA->B: <script>throw new Error('must not execute')</script>",
    "zenuml\nA->B: <script>fetch('https://example.test/must-not-fetch')</script>"
);
mermaid_matrix_case!(
    native_mermaid_matrix_flowchart_math_labels,
    "flowchart LR\nA[\"$$x^2 + y_1$$\"] --> B[End]",
    "flowchart LR\nA[\"$$\\frac{x}{y} + \\alpha$$\"] --> B[End]"
);
mermaid_matrix_case!(
    native_mermaid_matrix_flowchart_click_callbacks_are_inert,
    "flowchart LR\nA[Original] --> B\nclick A mustNotExecute \"Callback\"",
    "flowchart LR\nA[Edited label] --> B\nclick A mustNotExecute \"Changed callback\""
);

#[test]
fn mdx_component_rename_is_sparse_atomic_and_restores_selection() {
    let source = "before  \n\n<Mystery title='a&amp;b' {...props}>child</Mystery>\n\nafter";
    let mut document = MarkdownDocument::new(source.into(), TextFormat::default());
    let mut state = PresentationState::new(Selection::new(11, 18));
    document.command(
        &mut state,
        Command::RenameComponent {
            opening: 11..18,
            closing: Some(source.rfind("Mystery").unwrap()..source.rfind("Mystery").unwrap() + 7),
            name: "Box".into(),
        },
    );
    assert_eq!(
        document.source().to_string(),
        "before  \n\n<Box title='a&amp;b' {...props}>child</Box>\n\nafter"
    );
    assert_eq!(state.selection(), Selection::caret(14));
    document.command(&mut state, Command::Undo);
    assert_eq!(document.source().to_string(), source);
    assert_eq!(state.selection(), Selection::new(11, 18));
    document.command(&mut state, Command::Redo);
    assert_eq!(
        document.encode().unwrap(),
        b"before  \n\n<Box title='a&amp;b' {...props}>child</Box>\n\nafter"
    );
}

#[gpui_kit::test]
fn native_generic_mdx_edits_names_properties_children_and_saves_without_execution(
    cx: &mut gpui_kit::TestAppContext,
) {
    use super::super::test_host::FileHost;
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::{TestInput, TestWindowExt as _};
    use gpui_kit::{ElementInputHandler, Focusable as _};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("generic.mdx");
    let marker = directory.path().join("executed");
    std::fs::write(directory.path().join("trap.js"),
        "import { writeFileSync } from 'node:fs'; writeFileSync(new URL('./executed', import.meta.url), 'bad'); export default () => { throw Error('executed'); };"
    ).unwrap();
    let source = "---\r\nreview: preserve\n---\r\n\nimport Trap from './trap.js'\r\n\n  <Mystery title='a&amp;b' action={() => Trap()} {...props}>\r\n\n  # Child\n\n  text **bold**\r\n\n  {() => Trap()}\n\n  </Mystery>\r\n\n[ref]: /keep  ";
    std::fs::write(&path, source).unwrap();
    let host = FileHost::new(lyrel_core::host::HostId(724));
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
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
            start_in_source(&view, &mut context);
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
        let ready = context.update(|window, cx| {
            window.render_frame(cx);
            window.try_find("mdx-name").is_some()
        });
        if ready {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    context.update(|window, cx| {
        window.click("mdx-name", cx);
        window.input("B", cx);
        assert_eq!(
            view.read(cx).document().read(cx).source().to_string(),
            source.replace("\r\n", "\n").replace("Mystery", "B")
        );
        window.input("ox", cx);
    });
    let renamed = source.replace("Mystery", "Box");
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            renamed.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    // Each native character is one paired transaction, never two independent tag edits.
    context.dispatch_action(gpui_kit::component::input::Redo);
    loop {
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
        context.run_until_parked();
        let ready = context.update(|window, cx| {
            window.render_frame(cx);
            window.try_find("mdx-property-title").is_some()
        });
        if ready {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
    }
    context.update(|window, cx| {
        window.click("mdx-name", cx);
        let bounds = window.find("markdown-source").bounds();
        let mut input = TestInput::new(ElementInputHandler::new(bounds, view.clone()), window);
        input.compose_text(None, "ming", Some(4..4), cx);
        input.commit_text("Panel", cx);
    });
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source.replace("Mystery", "Panel").as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            renamed.as_bytes()
        )
    });
    context.update(|window, cx| {
        window.render_frame(cx);
        window.click("mdx-name", cx);
        let bounds = window.find("markdown-source").bounds();
        let mut input = TestInput::new(ElementInputHandler::new(bounds, view.clone()), window);
        input.compose_text(None, "cancel", Some(6..6), cx);
        input.compose_text(None, "", None, cx);
    });
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            renamed.as_bytes()
        )
    });
    context.update(|window, cx| {
        window.click("mdx-property-title", cx);
        let bounds = window.find("markdown-source").bounds();
        let mut input = TestInput::new(ElementInputHandler::new(bounds, view.clone()), window);
        input.compose_text(None, "zhong", Some(5..5), cx);
        assert!(input.marked_text_range(cx).is_some());
        input.commit_text("中文🎉", cx);
        assert!(input.marked_text_range(cx).is_none());
    });
    let property_edit = renamed.replacen("a&amp;b", "中文🎉", 1);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            property_edit.as_bytes()
        )
    });
    let normalized = property_edit.replace("\r\n", "\n");
    let child = normalized.find("text ").unwrap() + 5;
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(child..child, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| window.input("nested ", cx));
    let expected = property_edit.replacen("text **bold**", "text nested **bold**", 1);
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            property_edit.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Redo);
    context.dispatch_action(crate::core::actions::EditorSave);
    loop {
        context.run_until_parked();
        if app.read_with(&context, |app, _| {
            app.active_buffer()
                .is_some_and(|buffer| !buffer.dirty && !buffer.saving)
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(std::fs::read(&path).unwrap(), expected.as_bytes());
    assert_eq!(host.writes.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(
        !marker.exists(),
        "opening/editing MDX must not execute its module or expressions"
    );
    app.update_in(&mut context, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.editor_open_on_host(host.clone(), &path, window, cx);
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
        assert_eq!(
            app.active_buffer().unwrap().host.id(),
            lyrel_core::host::HostId(724)
        );
        let view = app
            .active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .read(cx);
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            expected.as_bytes()
        );
    });
    assert!(!marker.exists());
}

#[test]
fn mdx_name_preedit_commit_cancel_and_undo_keep_both_tags_coherent() {
    let original = "<Box title='keep'>child</Box>";
    let closing = original.rfind("Box").unwrap();
    let mut document = MarkdownDocument::new(original.into(), TextFormat::default());
    let mut state = PresentationState::new(Selection::new(1, 4));
    document.command(
        &mut state,
        Command::ComposePaired {
            range: 1..4,
            mirror: closing..closing + 3,
            text: "ming".into(),
            selected: Some(4..4),
        },
    );
    assert_eq!(
        document.source().to_string(),
        "<ming title='keep'>child</ming>"
    );
    document.command(&mut state, Command::Commit("名称".into()));
    assert_eq!(
        document.source().to_string(),
        "<名称 title='keep'>child</名称>"
    );
    document.command(&mut state, Command::Undo);
    assert_eq!(
        document.source().to_string(),
        "<Box title='keep'>child</Box>"
    );
    assert_eq!(state.selection(), Selection::new(1, 4));
    document.command(&mut state, Command::Redo);
    assert_eq!(
        document.source().to_string(),
        "<名称 title='keep'>child</名称>"
    );
    document.command(&mut state, Command::Undo);
    document.command(
        &mut state,
        Command::ComposePaired {
            range: 1..4,
            mirror: closing..closing + 3,
            text: "cancel".into(),
            selected: None,
        },
    );
    document.command(&mut state, Command::CancelComposition);
    assert_eq!(
        document.source().to_string(),
        "<Box title='keep'>child</Box>"
    );
    assert_eq!(state.selection(), Selection::new(1, 4));
}

#[test]
fn mdx_self_closing_children_expand_without_rewriting_properties() {
    let original = "<Box title='&amp;' {...dynamic} />\nkeep  ";
    let mut document = MarkdownDocument::new(original.into(), TextFormat::default());
    let at = original.find("/>").unwrap();
    let mut state = PresentationState::new(Selection::caret(at));
    document.command(
        &mut state,
        Command::ExpandComponent {
            ending: at..at + 2,
            name: "Box".into(),
            block: true,
        },
    );
    assert_eq!(
        document.source().to_string(),
        "<Box title='&amp;' {...dynamic} >\n\n</Box>\nkeep  "
    );
    assert_eq!(state.selection(), Selection::caret(at + 2));
    document.command(&mut state, Command::Insert("child **Markdown**".into()));
    assert_eq!(
        document.source().to_string(),
        "<Box title='&amp;' {...dynamic} >\nchild **Markdown**\n</Box>\nkeep  "
    );
    document.command(&mut state, Command::Undo);
    document.command(&mut state, Command::Undo);
    assert_eq!(document.source().to_string(), original);
    assert_eq!(state.selection(), Selection::caret(at));
}

#[gpui_kit::test]
fn native_mdx_grammar_follows_rename_save_as_and_recovers_invalid_property_source(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::TestWindowExt as _;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("grammar.md");
    let renamed = directory.path().join("grammar.mdx");
    let saved = directory.path().join("saved.md");
    let source = "<Box title='keep'>\n\n# Child\n\n</Box>\n";
    std::fs::write(&path, source).unwrap();
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
    // Change grammar without changing revision, while the Markdown parse is queued.
    std::fs::rename(&path, &renamed).unwrap();
    app.update_in(&mut context, |app, _, cx| {
        let host = app.active_buffer().unwrap().host.id();
        app.editor_path_moved(host, &path, &renamed, cx);
    });
    loop {
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
        context.run_until_parked();
        if context.update(|window, cx| {
            window.render_frame(cx);
            window.try_find("mdx-property-title").is_some()
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).revision(), 0);
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source.as_bytes()
        );
    });
    context.update(|window, cx| {
        window.click("mdx-property-title", cx);
        window.input("broken'", cx);
    });
    let invalid = source.replacen("keep", "broken'", 1);
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).source().to_string(), invalid)
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    // Native character typing is grouped after the initial replacement; undo
    // both the appended characters and the initial selection replacement.
    if view.read_with(&context, |view, cx| {
        view.document().read(cx).source().to_string() != source
    }) {
        context.dispatch_action(gpui_kit::component::input::Undo);
    }
    loop {
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
        context.run_until_parked();
        if context.update(|window, cx| {
            window.render_frame(cx);
            window.try_find("mdx-property-title").is_some()
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source.as_bytes()
        )
    });
    context.dispatch_action(crate::core::actions::EditorSaveAs);
    context.run_until_parked();
    cx.simulate_new_path_selection(|_| Some(saved.clone()));
    loop {
        context.run_until_parked();
        if app.read_with(&context, |app, _| {
            app.active_buffer()
                .is_some_and(|buffer| buffer.path == saved && !buffer.saving)
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    context
        .executor()
        .advance_clock(std::time::Duration::from_millis(200));
    context.run_until_parked();
    context.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("mdx-name").is_none());
    });
    assert_eq!(std::fs::read(&saved).unwrap(), source.as_bytes());
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source.as_bytes()
        )
    });
}

#[gpui_kit::test]
fn native_nested_mdx_keeps_dynamic_source_and_expands_children_in_shared_history(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::{
        Focusable as _, MouseButton,
        test::{TestEventExt as _, TestWindowExt as _},
    };
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("nested.mdx");
    let original = "export const data = {\n  title: '} preserved',\n}\n\n<Outer flag {...data}>\n\n<Mystery title='&amp;' run={() => ({ text: '}' })} />\n\ntext **bold**\n\n</Outer>\n\nkeep  ";
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
    loop {
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
        context.run_until_parked();
        if context.update(|window, cx| {
            window.render_frame(cx);
            window.find_all("mdx-name").len() == 2
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    context.update(|window, cx| {
        // The nested editor is a field of this same surface, not another input.
        let target = window.find_all("mdx-name")[1].bounds().center();
        window.pointer_down(target, MouseButton::Left, cx);
        window.pointer_up(target, MouseButton::Left, cx);
        window.input("Inner", cx);
    });
    let renamed = original.replacen("Mystery", "Inner", 1);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            renamed.as_bytes()
        )
    });
    loop {
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
        context.run_until_parked();
        if context.update(|window, cx| {
            window.render_frame(cx);
            window.find_all("mdx-children").len() == 2
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
    }
    context.update(|window, cx| {
        let target = window.find_all("mdx-children")[1].bounds().center();
        window.pointer_down(target, MouseButton::Left, cx);
        window.pointer_up(target, MouseButton::Left, cx);
        assert!(view.read(cx).focus_handle(cx).is_focused(window));
        window.input("child **Markdown**", cx);
    });
    let expanded = renamed.replacen(" />", " >\nchild **Markdown**\n</Inner>", 1);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            expanded.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            renamed.as_bytes()
        )
    });
    // The spread and expression remain normal editable source at the command seam.
    let dynamic = renamed.find("text: '}'").unwrap();
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(dynamic..dynamic + 9, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| window.input("text: '中文'", cx));
    let edited = renamed.replacen("text: '}'", "text: '中文'", 1);
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            edited.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            renamed.as_bytes()
        )
    });
}

#[gpui_kit::test]
fn native_mdx_embedded_in_heading_exposes_property_edits_without_replacing_its_surface(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::TestWindowExt as _;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("inline.mdx");
    let original = "# Heading **<Mystery title='keep' />** tail\n\nuntouched  ";
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
    loop {
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
        context.run_until_parked();
        if context.update(|window, cx| {
            window.render_frame(cx);
            window.try_find("mdx-property-title").is_some()
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
    }
    context.update(|window, cx| {
        window.click("mdx-property-title", cx);
        window.input("changed", cx);
    });
    let property = original.replacen("keep", "changed", 1);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            property.as_bytes()
        )
    });
    loop {
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
        context.run_until_parked();
        if context.update(|window, cx| {
            window.render_frame(cx);
            window.try_find("mdx-children").is_some()
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
    }
    context.update(|window, cx| {
        window.click("mdx-children", cx);
        window.input("child", cx);
    });
    let expected = property.replacen(" />", " >child</Mystery>", 1);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            expected.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            property.as_bytes()
        )
    });
}

#[gpui_kit::test]
fn native_cards_edit_properties_children_follow_host_links_and_save_reopen(
    cx: &mut gpui_kit::TestAppContext,
) {
    use super::super::test_host::FileHost;
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::{Focusable as _, test::TestWindowExt as _};
    let directory = tempfile::tempdir().unwrap();
    let path = std::path::PathBuf::from("/docs/cards.mdx");
    let target = std::path::PathBuf::from("/docs/next&one.md");
    let disk = directory.path().join("docs");
    std::fs::create_dir(&disk).unwrap();
    std::fs::write(disk.join("next&one.md"), "# Host destination\n").unwrap();
    let original = "<CardGroup cols=\"2\">\r\n\n<Card title='Guide' href='./unused.md' icon='book' run={() => { throw Error('never execute') }}>\n\nchild **bold**\r\n\n</Card>\n\n</CardGroup>\r\n\n[keep]: /unchanged  ";
    std::fs::write(disk.join("cards.mdx"), original).unwrap();
    let host = FileHost::mapped(lyrel_core::host::HostId(726), directory.path());
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
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
            start_in_source(&view, &mut context);
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
        if context.update(|window, cx| {
            window.render_frame(cx);
            window.try_find("mdx-card-title").is_some()
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    context.update(|window, cx| {
        window.click("mdx-card-title", cx);
        assert!(view.read(cx).focus_handle(cx).is_focused(window));
        window.input("中文", cx);
    });
    let titled = original.replacen("Guide", "中文", 1);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            titled.as_bytes()
        )
    });
    assert!(app.read_with(&context, |app, _| app.active_buffer().unwrap().path == path));
    loop {
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
        context.run_until_parked();
        if context.update(|window, cx| {
            window.render_frame(cx);
            window.try_find("mdx-card-href").is_some()
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
    }
    context.update(|window, cx| {
        window.click("mdx-card-href", cx);
        window.input("./next&#38;one.md", cx);
    });
    let titled = titled.replacen("./unused.md", "./next&#38;one.md", 1);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            titled.as_bytes()
        )
    });
    assert!(app.read_with(&context, |app, _| app.active_buffer().unwrap().path == path));
    loop {
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
        context.run_until_parked();
        if context.update(|window, cx| {
            window.render_frame(cx);
            window.try_find("mdx-card-cols").is_some()
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
    }
    context.update(|window, cx| {
        window.click("mdx-card-cols", cx);
        window.input("3", cx);
    });
    let grouped = titled.replacen("cols=\"2\"", "cols=\"3\"", 1);
    let child = grouped.replace("\r\n", "\n").find("child ").unwrap() + 6;
    view.update_in(&mut context, |view, window, cx| {
        view.command(Command::Select(Selection::caret(child)), window, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| window.input("nested ", cx));
    let expected = grouped.replacen("child **bold**", "child nested **bold**", 1);
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            grouped.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            titled.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Redo);
    context.dispatch_action(gpui_kit::component::input::Redo);
    context.dispatch_action(crate::core::actions::EditorSave);
    loop {
        context.run_until_parked();
        if app.read_with(&context, |app, _| {
            app.active_buffer()
                .is_some_and(|buffer| !buffer.dirty && !buffer.saving)
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        std::fs::read(disk.join("cards.mdx")).unwrap(),
        expected.as_bytes()
    );
    assert_eq!(host.writes.load(std::sync::atomic::Ordering::SeqCst), 1);
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    loop {
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
        context.run_until_parked();
        if context.update(|window, cx| {
            window.render_frame(cx);
            window.try_find("mdx-card-follow").is_some()
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
    }
    // Selecting the href only edits; the separate native button follows it.
    context.update(|window, cx| window.click("mdx-card-href", cx));
    assert!(app.read_with(&context, |app, _| app.active_buffer().unwrap().path == path));
    context.update(|window, cx| window.click("mdx-card-follow", cx));
    loop {
        context.run_until_parked();
        if app.read_with(&context, |app, _| {
            app.active_buffer()
                .is_some_and(|buffer| buffer.path == target)
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "relative card link did not open through the document Host"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    app.read_with(&context, |app, _| {
        assert_eq!(
            app.active_buffer().unwrap().host.id(),
            lyrel_core::host::HostId(726)
        )
    });
    app.update_in(&mut context, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.editor_open_on_host(host.clone(), &path, window, cx);
    });
    loop {
        context.run_until_parked();
        if app.read_with(&context, |app, _| {
            app.active_buffer()
                .is_some_and(|buffer| buffer.path == path)
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    app.read_with(&context, |app, cx| {
        let view = app
            .active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .read(cx);
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            expected.as_bytes()
        );
    });
}

#[gpui_kit::test]
fn native_cards_add_missing_properties_expand_children_and_never_follow_dynamic_href(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::TestWindowExt as _;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("empty-cards.mdx");
    let original = "<CardGroup>\n\n<Card href={chooseDestination()} {...props} />  \r\n\n</CardGroup>\n\nkeep  ";
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
    loop {
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
        context.run_until_parked();
        if context.update(|window, cx| {
            window.render_frame(cx);
            window.try_find("mdx-card-add-title").is_some()
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    context.update(|window, cx| {
        assert!(
            window.try_find("mdx-card-follow").is_none(),
            "dynamic href is source, not an executable destination"
        );
        window.click("mdx-card-add-title", cx);
        window.input("Title", cx);
    });
    let titled = original.replacen("{...props}", "{...props} title=\"Title\"", 1);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            titled.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        )
    });
    loop {
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
        context.run_until_parked();
        if context.update(|window, cx| {
            window.render_frame(cx);
            window.try_find("mdx-card-add-cols").is_some()
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
    }
    context.update(|window, cx| {
        window.click("mdx-card-add-cols", cx);
        assert_eq!(
            view.read(cx).document().read(cx).encode().unwrap(),
            original
                .replacen("<CardGroup>", "<CardGroup cols=\"\">", 1)
                .as_bytes(),
            "adding a property must never rename the adjacent opening/closing tag",
        );
        assert_eq!(
            view.read(cx).selection(),
            Selection::caret("<CardGroup cols=\"".len())
        );
        window.input("2", cx);
    });
    let grouped = original.replacen("<CardGroup>", "<CardGroup cols=\"2\">", 1);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            grouped.as_bytes()
        )
    });
    loop {
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
        context.run_until_parked();
        if context.update(|window, cx| {
            window.render_frame(cx);
            window.try_find("mdx-card-children").is_some()
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
    }
    context.update(|window, cx| {
        window.click("mdx-card-children", cx);
        window.input("nested **child**", cx);
    });
    let expanded = grouped.replacen(" />", " >\nnested **child**\n</Card>", 1);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            expanded.as_bytes()
        )
    });
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            grouped.as_bytes()
        )
    });
}

#[gpui_kit::test]
fn native_long_document_scroll_return_reflow_and_pending_parse_keep_selection_and_history(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::Focusable as _;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("long-mixed.md");
    let mut source = String::from("# Long document\n\n");
    for ix in 0..1200 {
        source.push_str(&format!("Paragraph {ix}: wrapped prose 中e\u{301} with **emphasis** and enough words to wrap on a narrow surface.\n\n* parent\n  + nested\n\n```rust\nlet n = {ix};\n```\n\n| Left | Right |\n| --- | --- |\n| a wide cell with retained source | value |\n\n"));
    }
    source.push_str("last editable paragraph");
    source.push_str(&" wide 中e\u{301} 👩‍💻".repeat(3000));
    std::fs::write(&path, &source).unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut context, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
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
    let start = source.find("Paragraph 0:").unwrap();
    let end = source.find("last editable").unwrap();
    let settle_at = |at: usize, context: &mut gpui_kit::VisualTestContext| {
        loop {
            context
                .executor()
                .advance_clock(std::time::Duration::from_millis(100));
            context.run_until_parked();
            if context.update(|window, cx| {
                window.render_frame(cx);
                view.read(cx)
                    .range_to_bounds(&(at..at), cx)
                    .is_some_and(|caret| {
                        caret.top() >= gpui_kit::px(0.)
                            && caret.bottom() <= window.viewport_size().height
                    })
            }) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "current source position {at} must reenter the viewport; selection={:?}, offset={:?}, caret={:?}",
                view.read_with(context, |view, _| view.selection()),
                view.read_with(context, |view, _| view.scroll_offset()),
                view.read_with(context, |view, cx| view.range_to_bounds(&(at..at), cx))
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(end..end, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    settle_at(end, &mut context);
    view.read_with(&context, |view, cx| {
        assert!(view.range_to_bounds(&(end..end), cx).is_some());
        assert!(
            view.range_to_bounds(&(start..start), cx).is_none(),
            "offscreen source must not retain frame geometry"
        );
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source.as_bytes()
        );
    });
    // Edit before the debounced full parse, then leave and return. Old results
    // must not restore the original source or lose the unified history.
    view.update_in(&mut context, |view, window, cx| {
        view.command(Command::Insert("new ".into()), window, cx);
        view.select_range(start..start, cx);
    });
    settle_at(start, &mut context);
    view.read_with(&context, |view, cx| {
        assert!(view.range_to_bounds(&(start..start), cx).is_some())
    });
    view.update_in(&mut context, |view, _, cx| {
        view.select_range(end + 4..end + 4, cx)
    });
    cx.simulate_window_resize(
        context.window_handle(),
        gpui_kit::size(gpui_kit::px(760.), gpui_kit::px(650.)),
    );
    context.update(|_, cx| {
        gpui_kit::component::Theme::update(cx, |theme| theme.font_size = gpui_kit::px(20.))
    });
    settle_at(end + 4, &mut context);
    context
        .executor()
        .advance_clock(std::time::Duration::from_secs(1));
    context.run_until_parked();
    settle_at(end + 4, &mut context);
    context.update(|window, cx| {
        window.render_frame(cx);
        let caret = view
            .read(cx)
            .range_to_bounds(&(end + 4..end + 4), cx)
            .unwrap();
        assert!(caret.top() >= gpui_kit::px(0.) && caret.bottom() <= window.viewport_size().height);
        assert_eq!(view.read(cx).selection(), Selection::caret(end + 4));
        assert_eq!(
            view.read(cx).document().read(cx).source().to_string(),
            format!("{}new {}", &source[..end], &source[end..])
        );
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source.as_bytes()
        )
    });
}

#[gpui_kit::test]
fn native_theme_change_preserves_scrolled_reading_anchor_and_offscreen_selection(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::{Focusable as _, px};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("reading-anchor.mdx");
    let source = (0..120)
        .map(|ix| {
            let resources = if ix % 12 == 1 {
                "$x_i^2+\\sqrt{y}$\n\n```mermaid\nflowchart LR\nA[开始] --> B[结束]\n```\n\n"
            } else {
                ""
            };
            format!(
                "## Section-{ix:03}\n\n{}\n\n- first **item**\n- second `item`\n\n{resources}",
                "锚点正文：中文阅读位置，e\u{301} 与 👩‍💻。这里是需要随窗口宽度换行的连续正文，用于检查字体变化后的光标和阅读锚点。".repeat(5)
            )
        })
        .collect::<String>();
    std::fs::write(&path, &source).unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    context.simulate_resize(gpui_kit::size(px(1008.), px(901.)));
    context.update(|window, cx| {
        let config = cx.global_mut::<crate::core::config::Config>();
        config.theme_follow_system = false;
        config.theme_preset = "light".into();
        crate::ui::theme::apply_theme(Some(window), cx);
    });
    app.update_in(&mut context, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
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
    for (section, scroll) in [("Section-060", -1800.), ("Section-100", -100_000.)] {
        let caret = source.find(section).unwrap();
        view.update_in(&mut context, |view, window, cx| {
            view.select_range(caret..caret, cx);
            view.focus_handle(cx).focus(window, cx);
        });
        for _ in 0..8 {
            context.update(|window, cx| window.render_frame(cx));
            context.run_until_parked();
        }
        context.update(|window, cx| {
            window.scroll(
                "markdown-source",
                gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(scroll))),
                cx,
            );
        });
        for _ in 0..8 {
            context.update(|window, cx| window.render_frame(cx));
            context.run_until_parked();
        }
        let (anchor, before) = context.update(|window, cx| {
            let viewport = window.find("markdown-source").bounds();
            source
                .match_indices("Section-")
                .find_map(|(at, _)| {
                    let bounds = view.read(cx).range_to_bounds(&(at..at), cx)?;
                    (bounds.top() >= viewport.top() && bounds.bottom() < viewport.bottom())
                        .then_some((at, bounds.top()))
                })
                .expect("a scrolled section heading is visible")
        });
        assert!(
            anchor > caret,
            "the reading position has left the caret behind"
        );
        context.update(|window, cx| {
            cx.global_mut::<crate::core::config::Config>().theme_preset = "dark".into();
            crate::ui::theme::apply_theme(Some(window), cx);
        });
        for _ in 0..8 {
            context.update(|window, cx| window.render_frame(cx));
            context.run_until_parked();
        }
        view.read_with(&context, |view, cx| {
            let after = view
                .range_to_bounds(&(anchor..anchor), cx)
                .expect("the same reading anchor remains laid out")
                .top();
            assert!(
                (after - before).abs() <= px(1.),
                "theme-only change moved the reading anchor: {before:?} -> {after:?}"
            );
            assert_eq!(view.selection(), Selection::caret(caret));
            assert_eq!(
                view.document().read(cx).encode().unwrap(),
                source.as_bytes()
            );
        });
        for font_size in [12., 18., 16.] {
            context.update(|_window, cx| {
                cx.global_mut::<crate::core::config::Config>().ui_font_size = font_size;
                gpui_kit::component::Theme::update(cx, |theme| theme.font_size = px(font_size));
            });
            for _ in 0..8 {
                context.update(|window, cx| window.render_frame(cx));
                context.run_until_parked();
            }
            context.update(|window, cx| {
                let viewport = window.find("markdown-source").bounds();
                let view = view.read(cx);
                let bounds = view
                    .range_to_bounds(&(anchor..anchor), cx)
                    .expect("font reflow must materialize the original reading anchor");
                assert!(
                    bounds.bottom() >= viewport.top() && bounds.top() < viewport.bottom(),
                    "font {font_size} reflow lost the reading position: {bounds:?}, viewport={viewport:?}"
                );
                if font_size >= 16. {
                    assert!(
                        (bounds.top() - before).abs() <= window.rem_size(),
                        "font {font_size} moved {section} reading anchor: {before:?} -> {bounds:?}"
                    );
                }
                assert_eq!(view.selection(), Selection::caret(caret));
                assert_eq!(
                    view.document().read(cx).encode().unwrap(),
                    source.as_bytes()
                );
            });
        }
    }
}

#[test]
fn wide_single_line_transactions_keep_chunk_crossing_graphemes_atomic() {
    let prefix = "wide ".repeat(20_000);
    let cluster = "👩‍💻e\u{301}";
    let source = format!("{prefix}{cluster}{}", " tail".repeat(20_000));
    let mut document = MarkdownDocument::new(source.clone(), TextFormat::default());
    let mut state = PresentationState::default();
    document.command(
        &mut state,
        Command::Select(Selection::caret(prefix.len() + "👩‍💻".len())),
    );
    document.command(&mut state, Command::Backspace);
    assert_eq!(
        document.source().to_string(),
        format!("{prefix}e\u{301}{}", " tail".repeat(20_000))
    );
    document.command(&mut state, Command::Undo);
    assert_eq!(document.encode().unwrap(), source.as_bytes());
    document.command(
        &mut state,
        Command::Select(Selection::caret(prefix.len() + cluster.len())),
    );
    document.command(&mut state, Command::Backspace);
    assert_eq!(
        document.source().to_string(),
        format!("{prefix}👩‍💻{}", " tail".repeat(20_000))
    );
    document.command(&mut state, Command::Undo);
    assert_eq!(document.encode().unwrap(), source.as_bytes());
}

#[gpui_kit::test]
fn native_steps_edit_properties_nested_content_source_history_and_host_reopen(
    cx: &mut gpui_kit::TestAppContext,
) {
    use super::super::test_host::FileHost;
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::{
        Focusable as _, MouseButton,
        test::{TestEventExt as _, TestQueryExt as _, TestWindowExt as _},
    };
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("procedure.mdx");
    let original = "before  \r\n\n<Steps compact>\r\n\n<Step title='Install' icon='terminal'>\n\nfirst **nested** text\r\n\n</Step>\n\n<Step title=\"Run\" value={two}>\r\n\n## Finish\n\nsecond `code`\r\n\n</Step>\n\n</Steps>\r\n\n[ref]: /keep  ";
    std::fs::write(&path, original).unwrap();
    let host = FileHost::new(lyrel_core::host::HostId(728));
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    cx.simulate_window_resize(
        context.window_handle(),
        gpui_kit::size(gpui_kit::px(1200.), gpui_kit::px(1400.)),
    );
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
            start_in_source(&view, &mut context);
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
        if context.update(|window, cx| {
            window.render_frame(cx);
            window.find_all_by_label("Edit step title").len() == 2
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    context.update(|window, cx| {
        assert!(window.find_by_label("Step 1").visible());
        assert!(window.find_by_label("Step 2").visible());
        let target = window.find_all_by_label("Edit step title")[1]
            .bounds()
            .center();
        window.pointer_down(target, MouseButton::Left, cx);
        window.pointer_up(target, MouseButton::Left, cx);
        assert!(view.read(cx).focus_handle(cx).is_focused(window));
        window.input("R", cx);
    });
    let titled = original.replacen("title=\"Run\"", "title=\"R\"", 1);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            titled.as_bytes()
        )
    });
    // A second property uses the common property control, not another editor.
    context.update(|window, cx| {
        window.click("mdx-property-icon", cx);
        window.input("i", cx);
    });
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            titled.replacen("icon='terminal'", "icon='i'", 1).as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            titled.as_bytes()
        )
    });
    let normalized = titled.replace("\r\n", "\n");
    let nested = normalized.find("nested").unwrap();
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(nested..nested + 6, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| window.input("N", cx));
    let nested_edit = titled.replacen("**nested**", "**N**", 1);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            nested_edit.as_bytes()
        )
    });
    // Contextual source activation selects only the second Step in the same
    // input and changes presentation without creating a document edit.
    loop {
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
        context.run_until_parked();
        if context.update(|window, cx| {
            window.render_frame(cx);
            window.find_all_by_label("Edit step source").len() == 2
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
    }
    let revision = view.read_with(&context, |view, cx| view.document().read(cx).revision());
    context.update(|window, cx| {
        let target = window.find_all_by_label("Edit step source")[1]
            .bounds()
            .center();
        window.pointer_down(target, MouseButton::Left, cx);
        window.pointer_up(target, MouseButton::Left, cx);
        let view = view.read(cx);
        assert!(!view.is_live());
        assert_eq!(view.document().read(cx).revision(), revision);
        assert_eq!(
            view.document()
                .read(cx)
                .source()
                .slice(view.selection().range())
                .to_string(),
            "<Step title=\"R\" value={two}>\n\n## Finish\n\nsecond `code`\n\n</Step>"
        );
    });
    let dynamic = nested_edit.replace("\r\n", "\n").find("two").unwrap();
    view.update_in(&mut context, |view, _, cx| {
        view.select_range(dynamic..dynamic + 3, cx)
    });
    context.update(|window, cx| window.input("3", cx));
    let expected = nested_edit.replacen("{two}", "{3}", 1);
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            nested_edit.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            titled.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        )
    });
    for _ in 0..3 {
        context.dispatch_action(gpui_kit::component::input::Redo);
    }
    context.dispatch_action(crate::core::actions::EditorSave);
    loop {
        context.run_until_parked();
        if app.read_with(&context, |app, _| {
            app.active_buffer()
                .is_some_and(|buffer| !buffer.dirty && !buffer.saving)
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(std::fs::read(&path).unwrap(), expected.as_bytes());
    assert_eq!(host.writes.load(std::sync::atomic::Ordering::SeqCst), 1);
    app.update_in(&mut context, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.editor_open_on_host(host.clone(), &path, window, cx);
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
        assert_eq!(
            app.active_buffer().unwrap().host.id(),
            lyrel_core::host::HostId(728)
        );
        let view = app
            .active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .read(cx);
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            expected.as_bytes()
        );
    });
}

#[gpui_kit::test]
fn native_steps_keyboard_source_activation_finishes_preedit_without_numbering_edits(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::{
        ElementInputHandler, Focusable as _,
        test::{TestInput, TestQueryExt as _, TestWindowExt as _},
    };
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("keyboard-steps.mdx");
    let original = "<Steps>\n\n<Step title='One'>\n\n**body**\n\n</Step>\n\n</Steps>\n\nkeep  ";
    std::fs::write(&path, original).unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    cx.simulate_window_resize(
        context.window_handle(),
        gpui_kit::size(gpui_kit::px(1200.), gpui_kit::px(1000.)),
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
    loop {
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
        context.run_until_parked();
        if context.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label("Edit step title").is_some()
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).revision(), 0);
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        );
    });
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(1..6, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| {
        // Tab reaches the real Button, Enter selects the title in the single
        // document input. No test-only callback or duplicate field state.
        view.read(cx).focus_handle(cx).focus(window, cx);
        for _ in 0..24 {
            window.press("tab", cx);
            if window.find_by_label("Edit step title").focused() == Some(true) {
                break;
            }
        }
        assert_eq!(
            window.find_by_label("Edit step title").focused(),
            Some(true)
        );
        window.press("enter", cx);
        assert!(view.read(cx).focus_handle(cx).is_focused(window));
        let title = original.find("One").unwrap();
        assert_eq!(view.read(cx).selection().range(), title..title + 3);
        let bounds = window.find("markdown-source").bounds();
        let mut input = TestInput::new(ElementInputHandler::new(bounds, view.clone()), window);
        input.compose_text(None, "中文", Some(2..2), cx);
        assert!(input.marked_text_range(cx).is_some());
    });
    context.update(|window, cx| {
        for _ in 0..24 {
            window.press("tab", cx);
            if window.find_by_label("Edit step source").focused() == Some(true) {
                break;
            }
        }
        assert_eq!(
            window.find_by_label("Edit step source").focused(),
            Some(true)
        );
        window.press("enter", cx);
        let view = view.read(cx);
        assert!(!view.is_live());
        assert!(!view.is_composing());
        assert!(view.focus_handle(cx).is_focused(window));
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.replacen("'One'", "'中文'", 1).as_bytes()
        );
        let committed = original.replacen("'One'", "'中文'", 1);
        assert_eq!(
            view.selection().range(),
            committed.find("<Step ").unwrap()..committed.find("</Step>").unwrap() + 7
        );
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Redo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.replacen("'One'", "'中文'", 1).as_bytes()
        )
    });
}

fn exercise_native_callout_consumer(cx: &mut gpui_kit::TestAppContext, name: &str) {
    use super::super::test_host::FileHost;
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::{TestInput, TestWindowExt as _};
    use gpui_kit::{ElementInputHandler, Focusable as _};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("callout.mdx");
    let original = format!(
        "before  \r\n\n<{name} title='keep' action={{() => {{ throw Error('never execute') }}}}>\r\n\ntext **bold** and *nested*\n\n</{name}>\r\n\n[ref]: /untouched  "
    );
    std::fs::write(&path, &original).unwrap();
    let host = FileHost::new(lyrel_core::host::HostId(725));
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
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
            start_in_source(&view, &mut context);
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
        if context.update(|window, cx| {
            window.render_frame(cx);
            window.try_find("callout-edit-source").is_some()
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
    }
    context.update(|window, cx| {
        assert!(
            window.try_find("mdx-name").is_none(),
            "callouts are not unknown-component boxes"
        );
        window.click("mdx-property-title", cx);
        assert!(view.read(cx).focus_handle(cx).is_focused(window));
        let bounds = window.find("markdown-source").bounds();
        let mut input = TestInput::new(ElementInputHandler::new(bounds, view.clone()), window);
        input.compose_text(None, "zhong", Some(5..5), cx);
        input.commit_text("中文", cx);
    });
    let property = original.replacen("keep", "中文", 1);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            property.as_bytes()
        )
    });
    settle_live_geometry(&view, &mut context);
    context.update(|window, cx| {
        for _ in 0..96 {
            if window.find("callout-edit-source").focused() == Some(true) {
                break;
            }
            window.press("tab", cx);
        }
        assert_eq!(
            window.find("callout-edit-source").focused(),
            Some(true),
            "callout source must be keyboard reachable"
        );
        assert_eq!(
            window.find("callout-edit-source").label(),
            Some(crate::ui::i18n::t(
                crate::ui::i18n::L10nKey::EditorCalloutEditSource
            ))
        );
        window.press("enter", cx);
        assert!(view.read(cx).focus_handle(cx).is_focused(window));
    });
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            property.as_bytes()
        )
    });
    let normalized = property.replace("\r\n", "\n");
    let bold = normalized.find("bold").unwrap();
    view.update_in(&mut context, |view, _, cx| {
        view.select_range(bold..bold + 4, cx)
    });
    context.update(|window, cx| {
        let bounds = window.find("markdown-source").bounds();
        let mut input = TestInput::new(ElementInputHandler::new(bounds, view.clone()), window);
        input.compose_text(None, "nei", Some(3..3), cx);
    });
    context.update(|window, cx| {
        window.render_frame(cx);
        window.click("callout-return-content", cx);
    });
    let source_edit = property.replacen("bold", "nei", 1);
    view.read_with(&context, |view, cx| {
        assert!(!view.is_composing());
        assert_eq!(view.selection(), Selection::caret(bold + 3));
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source_edit.as_bytes()
        );
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            property.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Redo);
    settle_live_geometry(&view, &mut context);
    let nested = source_edit.replace("\r\n", "\n").find("nested").unwrap();
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(nested..nested + 6, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| {
        let bounds = window.find("markdown-source").bounds();
        let mut input = TestInput::new(ElementInputHandler::new(bounds, view.clone()), window);
        input.commit_text("edited", cx);
    });
    let expected = source_edit.replacen("nested", "edited", 1);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            expected.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source_edit.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Redo);
    for _ in 0..3 {
        context.dispatch_action(gpui_kit::component::input::Undo);
    }
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        )
    });
    for _ in 0..3 {
        context.dispatch_action(gpui_kit::component::input::Redo);
    }
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            expected.as_bytes()
        )
    });
    context.dispatch_action(crate::core::actions::EditorSave);
    loop {
        context.run_until_parked();
        if app.read_with(&context, |app, _| {
            app.active_buffer()
                .is_some_and(|buffer| !buffer.dirty && !buffer.saving)
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(std::fs::read(&path).unwrap(), expected.as_bytes());
    assert_eq!(host.writes.load(std::sync::atomic::Ordering::SeqCst), 1);
    app.update_in(&mut context, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.editor_open_on_host(host.clone(), &path, window, cx);
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
        assert_eq!(
            app.active_buffer().unwrap().host.id(),
            lyrel_core::host::HostId(725)
        );
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
fn native_callout_note_edits_source_history_and_host_reopen(cx: &mut gpui_kit::TestAppContext) {
    exercise_native_callout_consumer(cx, "Note");
}
#[gpui_kit::test]
fn native_callout_tip_edits_source_history_and_host_reopen(cx: &mut gpui_kit::TestAppContext) {
    exercise_native_callout_consumer(cx, "Tip");
}
#[gpui_kit::test]
fn native_callout_info_edits_source_history_and_host_reopen(cx: &mut gpui_kit::TestAppContext) {
    exercise_native_callout_consumer(cx, "Info");
}
#[gpui_kit::test]
fn native_callout_warning_edits_source_history_and_host_reopen(cx: &mut gpui_kit::TestAppContext) {
    exercise_native_callout_consumer(cx, "Warning");
}

#[gpui_kit::test]
fn native_accordions_preserve_hidden_children_focus_history_and_host_save(
    cx: &mut gpui_kit::TestAppContext,
) {
    use super::super::test_host::FileHost;
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::{TestEventExt as _, TestInput, TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{ElementInputHandler, Focusable as _, MouseButton};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("accordions.mdx");
    let marker = directory.path().join("executed");
    std::fs::write(directory.path().join("trap.js"),
        "import {writeFileSync} from 'node:fs'; writeFileSync(new URL('./executed', import.meta.url), 'bad'); export default () => {throw Error('executed')};"
    ).unwrap();
    let original = "import Trap from './trap.js'\r\n\n<AccordionGroup>\r\n\n<Accordion title='Outer' icon='book' defaultOpen action={() => Trap()}>\n\nouter **bold**\r\n\n<Accordion title='Inner'>\n\nsecret child\r\n\n</Accordion>\n\n</Accordion>\r\n\n</AccordionGroup>\n\n[ref]: /keep  ";
    std::fs::write(&path, original).unwrap();
    let host = FileHost::new(lyrel_core::host::HostId(729));
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
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
            start_in_source(&view, &mut context);
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
        if context.update(|window, cx| {
            window.render_frame(cx);
            window
                .try_find_by_label("Collapse accordion: Outer")
                .is_some()
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
    }
    // Drive the real named disclosure, not ViewState or parser metadata.
    context.update(|window, cx| {
        let at = window
            .find_by_label("Collapse accordion: Outer")
            .bounds()
            .center();
        window.pointer_down(at, MouseButton::Left, cx);
        window.pointer_up(at, MouseButton::Left, cx);
        assert!(
            window
                .try_find_by_label("Collapse accordion: Inner")
                .is_none()
        );
        assert!(
            window
                .try_find_by_label("Expand accordion: Inner")
                .is_none()
        );
        let hidden = original.replace("\r\n", "\n").find("secret child").unwrap();
        assert!(
            view.read(cx)
                .range_to_bounds(&(hidden..hidden), cx)
                .is_none()
        );
        assert!(view.read(cx).focus_handle(cx).is_focused(window));
        let at = window
            .find_by_label("Expand accordion: Outer")
            .bounds()
            .center();
        window.pointer_down(at, MouseButton::Left, cx);
        window.pointer_up(at, MouseButton::Left, cx);
    });
    context.update(|window, cx| {
        // Traverse the real focus order, then activate the native Button by key.
        for _ in 0..128 {
            window.focus_next(cx);
            window.render_frame(cx);
            if window.find_by_label("Collapse accordion: Outer").focused() == Some(true) {
                break;
            }
        }
        assert_eq!(
            window.find_by_label("Collapse accordion: Outer").focused(),
            Some(true)
        );
        assert_eq!(
            window.find_by_label("Collapse accordion: Outer").expanded(),
            Some(true)
        );
        window.press("enter", cx);
        assert_eq!(
            window.find_by_label("Expand accordion: Outer").expanded(),
            Some(false)
        );
        assert!(view.read(cx).focus_handle(cx).is_focused(window));
        for _ in 0..128 {
            window.focus_next(cx);
            window.render_frame(cx);
            if window.find_by_label("Expand accordion: Outer").focused() == Some(true) {
                break;
            }
        }
        assert_eq!(
            window.find_by_label("Expand accordion: Outer").focused(),
            Some(true)
        );
        window.press("space", cx);
        assert!(
            window
                .try_find_by_label("Collapse accordion: Inner")
                .is_some()
        );
    });
    app.read_with(&context, |app, _| {
        assert!(!app.active_buffer().unwrap().dirty)
    });
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        )
    });
    // Property editing and nested prose use the one document/input/history.
    context.update(|window, cx| {
        let at = window.find_all("mdx-property-title")[0].bounds().center();
        window.pointer_down(at, MouseButton::Left, cx);
        window.pointer_up(at, MouseButton::Left, cx);
        window.input("Changed", cx);
    });
    let property = original.replacen("title='Outer'", "title='Changed'", 1);
    let normalized = property.replace("\r\n", "\n");
    let child = normalized.find("secret child").unwrap() + "secret ".len();
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(child..child, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| window.input("edited ", cx));
    let edited = property.replacen("secret child", "secret edited child", 1);
    // A native handler preedit is finalized before hiding its active descendant.
    context.update(|window, cx| {
        let bounds = window.find("markdown-source").bounds();
        let mut input = TestInput::new(ElementInputHandler::new(bounds, view.clone()), window);
        input.compose_text(None, "preedit", Some(7..7), cx);
        assert!(input.marked_text_range(cx).is_some());
    });
    loop {
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
        context.run_until_parked();
        if context.update(|window, cx| {
            window.render_frame(cx);
            window
                .try_find_by_label("Collapse accordion: Changed")
                .is_some()
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
    }
    context.update(|window, cx| {
        let at = window
            .find_by_label("Collapse accordion: Changed")
            .bounds()
            .center();
        window.pointer_down(at, MouseButton::Left, cx);
        window.pointer_up(at, MouseButton::Left, cx);
        assert!(!view.read(cx).is_composing());
        assert!(view.read(cx).focus_handle(cx).is_focused(window));
        assert!(
            window
                .try_find_by_label("Collapse accordion: Inner")
                .is_none()
        );
        // Input after collapse goes to the visible header, never the old child.
        let selection = view.read(cx).selection();
        let source = view.read(cx).document().read(cx).source().to_string();
        assert_eq!(
            selection.range(),
            source.find("Accordion title='Changed'").unwrap()
                ..source.find("Accordion title='Changed'").unwrap()
        );
    });
    let composed = edited.replacen("secret edited child", "secret edited preeditchild", 1);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            composed.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            edited.as_bytes()
        )
    });
    context.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .try_find_by_label("Collapse accordion: Inner")
                .is_some(),
            "undo into hidden children must reveal every collapsed ancestor"
        );
    });
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            property.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Redo);
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    context.dispatch_action(crate::core::actions::EditorSave);
    loop {
        context.run_until_parked();
        if app.read_with(&context, |app, _| {
            app.active_buffer()
                .is_some_and(|buffer| !buffer.dirty && !buffer.saving)
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(std::fs::read(&path).unwrap(), edited.as_bytes());
    assert!(!marker.exists());
    app.update_in(&mut context, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.editor_open_on_host(host.clone(), &path, window, cx);
    });
    loop {
        context.run_until_parked();
        if app.read_with(&context, |app, cx| {
            app.active_editor_input()
                .and_then(|input| input.markdown().cloned())
                .is_some_and(|view| {
                    view.read(cx).document().read(cx).encode().unwrap() == edited.as_bytes()
                })
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    app.read_with(&context, |app, _| {
        assert_eq!(
            app.active_buffer().unwrap().host.id(),
            lyrel_core::host::HostId(729)
        )
    });
    assert!(!marker.exists());
}

#[gpui_kit::test]
fn native_table_inline_disclosures_hide_content_without_losing_source(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::{TestEventExt as _, TestQueryExt as _, TestWindowExt as _};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("inline-disclosures.mdx");
    let original = "| content |\n| --- |\n| <Tabs><Tab title='A'>alpha</Tab><Tab title='B'>bravo</Tab><Tab title='C'></Tab></Tabs> |\n| <Accordion title='Details'>secret</Accordion> |";
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
    let alpha = original.find("alpha").unwrap();
    let bravo = original.find("bravo").unwrap();
    let secret = original.find("secret").unwrap();
    let width = |view: &SourcePresentation, at, cx: &gpui_kit::App| {
        let start = view.range_to_bounds(&(at..at), cx).unwrap().origin;
        let end = view.range_to_bounds(&(at + 5..at + 5), cx).unwrap().origin;
        (end.x - start.x).abs() + (end.y - start.y).abs()
    };
    context.update(|window, cx| {
        window.render_frame(cx);
        assert!(width(view.read(cx), alpha, cx) > gpui_kit::px(0.));
        assert_eq!(width(view.read(cx), bravo, cx), gpui_kit::px(0.));
        let choice = window
            .find_all("mdx-tabs-choice")
            .into_iter()
            .find(|target| target.label() == Some("B"))
            .unwrap()
            .bounds()
            .center();
        window.pointer_down(choice, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(choice, gpui_kit::MouseButton::Left, cx);
    });
    settle_live_geometry(&view, &mut context);
    context.update(|window, cx| {
        assert_eq!(width(view.read(cx), alpha, cx), gpui_kit::px(0.));
        assert!(width(view.read(cx), bravo, cx) > gpui_kit::px(0.));
        let disclosure = window
            .find_by_label("Collapse accordion: Details")
            .bounds()
            .center();
        window.pointer_down(disclosure, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(disclosure, gpui_kit::MouseButton::Left, cx);
    });
    settle_live_geometry(&view, &mut context);
    view.read_with(&context, |view, cx| {
        assert_eq!(width(view, secret, cx), gpui_kit::px(0.));
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        );
    });
    context.update(|window, cx| {
        let choice = window
            .find_all("mdx-tabs-choice")
            .into_iter()
            .find(|target| target.label() == Some("C"))
            .unwrap()
            .bounds()
            .center();
        window.pointer_down(choice, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(choice, gpui_kit::MouseButton::Left, cx);
        window.input("empty child", cx);
    });
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original
                .replacen("title='C'>", "title='C'>empty child", 1)
                .as_bytes()
        );
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        );
    });
    settle_live_geometry(&view, &mut context);
    context.update(|window, cx| {
        let choice = window
            .find_all("mdx-tabs-choice")
            .into_iter()
            .find(|target| target.label() == Some("B"))
            .unwrap()
            .bounds()
            .center();
        window.pointer_down(choice, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(choice, gpui_kit::MouseButton::Left, cx);
        window.input("edited", cx);
    });
    let edited = original.replacen("bravo", "editedbravo", 1);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            edited.as_bytes()
        );
    });
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Redo);
    let source_deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
        context.run_until_parked();
        context.update(|window, cx| window.render_frame(cx));
        if view.read_with(&context, |view, cx| {
            !view.is_live() && view.range_to_bounds(&(alpha..alpha), cx).is_some()
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < source_deadline,
            "source geometry after toggle/undo/redo did not settle"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    view.read_with(&context, |view, cx| {
        assert!(!view.is_live());
        assert!(width(view, alpha, cx) > gpui_kit::px(0.));
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            edited.as_bytes()
        );
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
    assert_eq!(std::fs::read(&path).unwrap(), edited.as_bytes());
}

#[gpui_kit::test]
fn native_tabs_switch_preserves_hidden_edits_history_composition_and_host_bytes(
    cx: &mut gpui_kit::TestAppContext,
) {
    use super::super::test_host::FileHost;
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::{
        ElementInputHandler, Focusable as _,
        test::{TestEventExt as _, TestInput, TestWindowExt as _},
    };
    let has = |window: &gpui_kit::Window, kind: &'static str, label: &str| {
        window
            .find_all(kind)
            .iter()
            .any(|target| target.label() == Some(label))
    };
    let choose = |window: &mut gpui_kit::Window, cx: &mut gpui_kit::App, label: &str| {
        let target = window
            .find_all("mdx-tabs-choice")
            .into_iter()
            .find(|target| target.label() == Some(label))
            .unwrap()
            .bounds()
            .center();
        window.pointer_down(target, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(target, gpui_kit::MouseButton::Left, cx);
    };
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("tabs.mdx");
    let original = "before  \r\n\n<Tabs syncKey='keep'>\n\n<Tab title='One'>\n\nfirst **bold**\r\n\n</Tab>\n\n<Tab title='Two'>\n\nsecond 🎉\r\n\n</Tab>\n\n</Tabs>\r\n\n[ref]: /keep  ";
    std::fs::write(&path, original).unwrap();
    let host = FileHost::new(lyrel_core::host::HostId(727));
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
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
            start_in_source(&view, &mut context);
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
        if context.update(|window, cx| {
            window.render_frame(cx);
            has(window, "mdx-tabs-choice", "Two")
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
    }
    let revision = view.read_with(&context, |view, cx| view.document().read(cx).revision());
    context.update(|window, cx| {
        assert!(has(window, "mdx-tabs-panel", "One"));
        assert!(!has(window, "mdx-tabs-panel", "Two"));
        choose(window, cx, "Two");
        window.render_frame(cx);
        assert!(!has(window, "mdx-tabs-panel", "One"));
        assert!(has(window, "mdx-tabs-panel", "Two"));
        assert!(view.read(cx).focus_handle(cx).is_focused(window));
        assert_eq!(view.read(cx).document().read(cx).revision(), revision);
        assert_eq!(
            view.read(cx).document().read(cx).encode().unwrap(),
            original.as_bytes()
        );
    });
    app.read_with(&context, |app, _| {
        assert!(!app.active_buffer().unwrap().dirty)
    });
    context.update(|window, cx| {
        // The continuous editor keeps Tab for indentation/table navigation.
        // Its explicit tab-switch keys use the same native control operation.
        window.press("alt-pageup", cx);
        assert!(has(window, "mdx-tabs-panel", "One"));
        assert!(!has(window, "mdx-tabs-panel", "Two"));
        window.press("alt-pagedown", cx);
        assert!(has(window, "mdx-tabs-panel", "Two"));
        assert!(!has(window, "mdx-tabs-panel", "One"));
        assert_eq!(view.read(cx).document().read(cx).revision(), revision);
    });
    // A switch is not an undo item, including the clean initial state.
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).revision(), revision)
    });
    let at = original.replace("\r\n", "\n").find("second").unwrap();
    view.update_in(&mut context, |view, window, cx| {
        view.command(Command::Select(Selection::caret(at)), window, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| window.input("edited ", cx));
    let edited = original.replacen("second", "edited second", 1);
    context.update(|window, cx| {
        window.render_frame(cx);
        choose(window, cx, "One");
    });
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            edited.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            has(window, "mdx-tabs-panel", "Two"),
            "undo restores the edited child as a visible input target"
        );
        assert!(!has(window, "mdx-tabs-panel", "One"));
    });
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Redo);
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.dispatch_action(gpui_kit::component::input::Redo);
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    loop {
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
        context.run_until_parked();
        if context.update(|window, cx| {
            window.render_frame(cx);
            has(window, "mdx-tabs-choice", "One")
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
    }
    context.update(|window, cx| {
        let bounds = window.find("markdown-source").bounds();
        let mut input = TestInput::new(ElementInputHandler::new(bounds, view.clone()), window);
        input.compose_text(None, "cancel", Some(6..6), cx);
        input.compose_text(None, "", None, cx);
        assert!(!view.read(cx).is_composing());
    });
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            edited.as_bytes()
        )
    });
    context.update(|window, cx| {
        let bounds = window.find("markdown-source").bounds();
        let mut input = TestInput::new(ElementInputHandler::new(bounds, view.clone()), window);
        input.compose_text(None, "中文", Some(2..2), cx);
    });
    context.update(|window, cx| {
        window.render_frame(cx);
        choose(window, cx, "One");
    });
    let committed = edited.replacen("edited second", "edited 中文second", 1);
    view.read_with(&context, |view, cx| {
        assert!(!view.is_composing());
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            committed.as_bytes()
        );
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            edited.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Redo);
    context.dispatch_action(crate::core::actions::EditorSave);
    loop {
        context.run_until_parked();
        if app.read_with(&context, |app, _| {
            app.active_buffer()
                .is_some_and(|buffer| !buffer.dirty && !buffer.saving)
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(std::fs::read(&path).unwrap(), committed.as_bytes());
    assert_eq!(host.writes.load(std::sync::atomic::Ordering::SeqCst), 1);
    app.update_in(&mut context, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.editor_open_on_host(host.clone(), &path, window, cx);
    });
    loop {
        context.run_until_parked();
        if app.read_with(&context, |app, _| app.active_buffer().is_some()) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
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
            committed.as_bytes()
        );
        assert_eq!(
            app.active_buffer().unwrap().host.id(),
            lyrel_core::host::HostId(727)
        );
    });
}

#[gpui_kit::test]
fn native_tabs_nested_ancestor_visibility_and_property_undo_restore_the_right_child(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::{TestEventExt as _, TestWindowExt as _};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("nested-tabs.mdx");
    let original = "<Tabs>\n\n<Tab title='Outer'>\n\n<Tabs>\n\n<Tab title='Inner'>\n\nvisible **inner**\n\n</Tab>\n\n<Tab title='Hidden'>\n\nkept hidden\n\n</Tab>\n\n</Tabs>\n\n</Tab>\n\n<Tab title='Other'>\n\nother child\n\n</Tab>\n\n</Tabs>\n\nkeep  ";
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
    let has = |window: &gpui_kit::Window, kind: &'static str, label: &str| {
        window
            .find_all(kind)
            .iter()
            .any(|target| target.label() == Some(label))
    };
    let choose = |window: &mut gpui_kit::Window, cx: &mut gpui_kit::App, label: &str| {
        let target = window
            .find_all("mdx-tabs-choice")
            .into_iter()
            .find(|target| target.label() == Some(label))
            .unwrap()
            .bounds()
            .center();
        window.pointer_down(target, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(target, gpui_kit::MouseButton::Left, cx);
        window.render_frame(cx);
    };
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    loop {
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
        context.run_until_parked();
        if context.update(|window, cx| {
            window.render_frame(cx);
            has(window, "mdx-tabs-choice", "Hidden")
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
    }
    context.update(|window, cx| {
        assert!(has(window, "mdx-tabs-panel", "Inner"));
        assert!(!has(window, "mdx-tabs-panel", "Hidden"));
        choose(window, cx, "Hidden");
        assert!(has(window, "mdx-tabs-panel", "Hidden"));
        assert!(!has(window, "mdx-tabs-panel", "Inner"));
        choose(window, cx, "Other");
        assert!(has(window, "mdx-tabs-panel", "Other"));
        assert!(!has(window, "mdx-tabs-panel", "Hidden"));
        assert!(
            !has(window, "mdx-tabs-choice", "Hidden"),
            "inactive ancestor hides its whole nested strip"
        );
        choose(window, cx, "Outer");
        assert!(
            has(window, "mdx-tabs-panel", "Hidden"),
            "nested active child survives ancestor switching"
        );
        assert!(!has(window, "mdx-tabs-panel", "Inner"));
        // Only the outer and selected nested Tab expose property controls.
        let target = window
            .find_all("mdx-property-title")
            .last()
            .unwrap()
            .bounds()
            .center();
        window.pointer_down(target, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(target, gpui_kit::MouseButton::Left, cx);
        window.input("Renamed", cx);
    });
    let edited = original.replacen("title='Hidden'", "title='Renamed'", 1);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            edited.as_bytes()
        )
    });
    loop {
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
        context.run_until_parked();
        if context.update(|window, cx| {
            window.render_frame(cx);
            has(window, "mdx-tabs-choice", "Renamed")
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
    }
    context.update(|window, cx| {
        assert!(
            has(window, "mdx-tabs-panel", "Renamed"),
            "editing a title does not replace active stable identity"
        );
        choose(window, cx, "Other");
    });
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        )
    });
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    loop {
        context
            .executor()
            .advance_clock(std::time::Duration::from_millis(100));
        context.run_until_parked();
        if context.update(|window, cx| {
            window.render_frame(cx);
            has(window, "mdx-tabs-panel", "Hidden")
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
    }
    context.update(|window, _| {
        assert!(!has(window, "mdx-tabs-panel", "Other"));
        assert!(!has(window, "mdx-tabs-panel", "Inner"));
    });
}

#[gpui_kit::test]
fn native_empty_quote_line_exits_one_level_and_undo_restores_source(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::Focusable as _;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("quote.md");
    let original = "> first\n>\n> next\n\nkeep  ";
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
    let at = original.find("\n>\n").unwrap() + 2;
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(at..at, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| window.press("enter", cx));
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            b"> first\n\n> next\n\nkeep  "
        );
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        );
        assert_eq!(view.selection(), Selection::caret(at));
    });
}

#[test]
fn installed_mdx_children_boundary_never_aliases_its_inline_content() {
    use super::projection::Projection;
    use std::collections::HashSet;

    let original = "<Tabs>\n<Tab title='One'>标签正文。</Tab>\n</Tabs>\n\n<Steps>\n<Step title='Step'>步骤正文。</Step>\n</Steps>\n";
    let mut projection = Projection::parse(original, 0, false, super::mdx::Grammar::Mdx);
    for (revision, source) in [
        (0, original.to_owned()),
        (1, original.replace("标签", "输入标签")),
    ] {
        projection.install(Projection::parse(
            &source,
            revision,
            true,
            super::mdx::Grammar::Mdx,
        ));
        let mut identities = HashSet::new();
        for leaf in &projection.leaves {
            assert!(
                identities.insert(leaf.id),
                "each native block needs its own render/input identity"
            );
        }
        for text in ["标签正文。", "步骤正文。"] {
            let at = source.find(text).unwrap();
            let content = projection
                .leaves
                .iter()
                .find(|leaf| leaf.range.contains(&at) && !leaf.range.is_empty())
                .unwrap();
            let boundary = projection
                .leaves
                .iter()
                .find(|leaf| leaf.range.start == content.range.start && leaf.range.is_empty())
                .unwrap();
            assert_ne!(content.id, boundary.id);
            assert_eq!(boundary.text, "");
            assert!(content.text.ends_with(text));
        }
    }
}

#[gpui_kit::test]
fn native_full_wide_table_reenters_distant_cells_with_wrapped_height_and_undo(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::Focusable as _;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("full-wide-table.md");
    let mut source = String::from("# Full table\n\n|");
    for column in 0..120 {
        source.push_str(&format!(" Header-{column} |"));
    }
    source.push_str("\n|");
    for _ in 0..120 {
        source.push_str(" --- |");
    }
    source.push('\n');
    for row in 0..160 {
        if row != 3 {
            source.push('|');
        }
        for column in 0..120 {
            match (row, column) {
                (1, 0) => source.push_str("a|"),
                (1, 1) => source.push_str("b|"),
                (2, 1) => source.push_str(" |"),
                (3, 0) => source.push_str("first|"),
                (3, 1) => source.push_str("# x |"),
                _ => source.push_str(&format!(" Cell-{row:03}-{column:03} |")),
            }
        }
        if row == 159 {
            source.pop(); // The last cell has neither padding nor an outer pipe.
            source.pop();
        }
        source.push('\n');
    }
    source = source.replacen(
        "Cell-080-119",
        &format!("Tall-080-119 {}", "wrapped 中文 e\u{301} 🎉 ".repeat(24)),
        1,
    );
    source.push_str("\nUnchanged tail  ");
    std::fs::write(&path, &source).unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut context, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(40);
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
    view.update_in(&mut context, |view, _, cx| view.set_soft_wrap(true, cx));
    settle_live_geometry(&view, &mut context);
    let settle_at = |at, context: &mut gpui_kit::VisualTestContext| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(40);
        view.update_in(context, |view, window, cx| {
            view.select_range(at..at, cx);
            view.focus_handle(cx).focus(window, cx);
        });
        assert_eq!(
            view.read_with(context, |view, _| view.selection()),
            Selection::caret(at),
            "explicit table caret target must not move before layout"
        );
        loop {
            context
                .executor()
                .advance_clock(std::time::Duration::from_millis(100));
            context.run_until_parked();
            let bounds = context.update(|window, cx| {
                window.render_frame(cx);
                let surface = window.find("markdown-source").bounds();
                view.read(cx)
                    .range_to_bounds(&(at..at), cx)
                    .filter(|bounds| {
                        bounds.left() >= surface.left()
                            && bounds.right() <= surface.right()
                            && bounds.top() >= surface.top() + bounds.size.height
                            && bounds.bottom() <= surface.bottom() - bounds.size.height
                    })
            });
            if let Some(bounds) = bounds {
                break bounds;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "wide-table cell {at} must enter the real viewport; offset={:?}; caret={:?}",
                view.read_with(context, |view, _| view.scroll_offset()),
                view.read_with(context, |view, cx| view.range_to_bounds(&(at..at), cx))
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    for token in [
        "Cell-000-000",
        "Cell-159-119",
        "Cell-080-000",
        "Tall-080-119",
    ] {
        settle_at(source.find(token).unwrap(), &mut context);
    }
    let start = source.find("Tall-080-119").unwrap();
    let later = start + "Tall-080-119 ".len() + "wrapped 中文 e\u{301} 🎉 ".len() * 8;
    let first = settle_at(start, &mut context);
    let later_bounds = view
        .read_with(&context, |view, cx| {
            view.range_to_bounds(&(later..later), cx)
        })
        .unwrap();
    context.update(|window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        let next = source.find("Cell-081-119").unwrap();
        assert!(
            view.read(cx).range_to_bounds(&(next..next), cx).is_some(),
            "a caret-only materialization must refill the surrounding continuous document"
        );
    });
    assert!(
        later_bounds.top() > first.top(),
        "the distant cell must wrap rather than use a fixed row height"
    );
    context.update(|window, cx| {
        use gpui_kit::{
            ElementInputHandler,
            test::{TestInput, TestWindowExt as _},
        };
        window.render_frame(cx);
        let before = view.read(cx).range_to_bounds(&(start..start), cx).unwrap();
        let previous = source.find("Cell-079-119").unwrap();
        let previous_before = view
            .read(cx)
            .range_to_bounds(&(previous..previous), cx)
            .unwrap();
        let surface = window.find("markdown-source").bounds();
        TestInput::new(ElementInputHandler::new(surface, view.clone()), window).compose_text(
            None,
            "zhong'wen",
            Some(9..9),
            cx,
        );
        // Do not advance the parse debounce: the first OS candidate query must
        // use the edited cell, not a document-sized temporary source block.
        window.render_frame(cx);
        window.render_frame(cx);
        let marked = view
            .update(cx, |view, cx| view.marked_text_range(window, cx))
            .unwrap();
        let candidate = view
            .update(cx, |view, cx| {
                view.bounds_for_range(marked, surface, window, cx)
            })
            .unwrap();
        let previous_after = view
            .read(cx)
            .range_to_bounds(&(previous..previous), cx)
            .unwrap();
        assert_eq!(
            candidate.left() - previous_after.left(),
            before.left() - previous_before.left(),
            "preedit retains its table column relative to the previous row"
        );
        assert_eq!(
            candidate.top() - previous_after.top(),
            before.top() - previous_before.top(),
            "first candidate retains its table row, including when the viewport scrolls"
        );
        TestInput::new(ElementInputHandler::new(surface, view.clone()), window)
            .commit_text("中文", cx);
    });
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source
                .replacen("Tall-080-119", "中文Tall-080-119", 1)
                .as_bytes()
        );
    });
    context.update(|window, cx| {
        use gpui_kit::{
            ElementInputHandler,
            test::{TestInput, TestWindowExt as _},
        };
        let handler =
            ElementInputHandler::new(window.find("markdown-source").bounds(), view.clone());
        TestInput::new(handler, window).compose_text(None, "ce'shi", Some(6..6), cx);
        window.press("escape", cx);
    });
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source
                .replacen("Tall-080-119", "中文Tall-080-119", 1)
                .as_bytes()
        );
    });
    context.update(|window, cx| window.press("tab", cx));
    view.read_with(&context, |view, _| {
        assert_eq!(
            view.selection(),
            Selection::caret(source.find("Cell-081-000").unwrap() + "中文".len()),
            "preedit updates must retain the next row's rebased source coordinate"
        );
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle_at(start, &mut context);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source.as_bytes()
        );
    });
    context.update(|window, cx| window.input("X", cx));
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source
                .replacen("Tall-080-119", "XTall-080-119", 1)
                .as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle_at(source.find("Cell-000-000").unwrap(), &mut context);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source.as_bytes()
        )
    });
    let dense = source.find("|a|b|").unwrap() + 2;
    settle_at(dense, &mut context);
    context.update(|window, cx| {
        window.input("X", cx);
        window.press("right", cx);
        window.input("Y", cx);
    });
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source.replacen("|a|b|", "|aX|Yb|", 1).as_bytes()
        );
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.dispatch_action(gpui_kit::component::input::Undo);
    let empty = source.find("Cell-002-000 | | Cell-002-002").unwrap() + "Cell-002-000 |".len();
    settle_at(empty, &mut context);
    context.update(|window, cx| {
        use gpui_kit::{ElementInputHandler, test::TestInput};
        let handler =
            ElementInputHandler::new(window.find("markdown-source").bounds(), view.clone());
        TestInput::new(handler, window).compose_text(None, "ni", Some(2..2), cx);
        window.press("escape", cx);
        window.press("tab", cx);
    });
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.selection(),
            Selection::caret(source.find("Cell-002-002").unwrap())
        );
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source.as_bytes()
        );
    });
    let inline_heading = source.find("first|# x | Cell-003-002").unwrap() + "first|# ".len();
    settle_at(inline_heading, &mut context);
    context.update(|window, cx| {
        window.input("Q", cx);
        window.press("tab", cx);
    });
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.selection(),
            Selection::caret(source.find("Cell-003-002").unwrap() + 1)
        );
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source
                .replacen("first|# x | Cell-003-002", "first|# Qx | Cell-003-002", 1)
                .as_bytes()
        );
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    let end = source.find("Cell-159-119").unwrap() + "Cell-159-119".len();
    settle_at(end, &mut context);
    context.update(|window, cx| window.input("Z", cx));
    context.dispatch_action(super::tables::AlignRight);
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source
                .replacen("Cell-159-119", "Cell-159-119Z", 1)
                .as_bytes(),
            "alignment at the grown table end must be a separate undo transaction"
        );
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source.as_bytes()
        );
    });
    settle_at(end, &mut context);
    context.update(|window, cx| {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("X\n".into()));
        window.press("secondary-v", cx);
        window.input("Y", cx);
        window.render_frame(cx);
        window.render_frame(cx);
        let caret = end + "X\nY".len();
        assert_eq!(view.read(cx).selection(), Selection::caret(caret));
        let previous_line = view
            .read(cx)
            .range_to_bounds(&(end + 1..end + 1), cx)
            .unwrap();
        let next_line = view.read(cx).range_to_bounds(&(caret..caret), cx).unwrap();
        assert!(
            next_line.top() > previous_line.top(),
            "the newly pasted line must map below the previous line before the background parse"
        );
    });
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source
                .replacen("Cell-159-119", "Cell-159-119X\nY", 1)
                .as_bytes()
        );
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source
                .replacen("Cell-159-119", "Cell-159-119X\n", 1)
                .as_bytes()
        );
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            source.as_bytes()
        );
    });
}
