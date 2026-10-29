use super::*;

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
    let mut document = MarkdownDocument::new("# 标题\n\nfirst second\n\nlast".into(), TextFormat::default());
    let mut state = PresentationState::new(Selection::caret(16));
    document.command(&mut state, Command::ParagraphBreak { hard: false });
    assert_eq!(document.source().to_string(), "# 标题\n\nfirst \n\nsecond\n\nlast");
    document.command(&mut state, Command::ParagraphBreak { hard: true });
    assert_eq!(document.source().to_string(), "# 标题\n\nfirst \n\n  \nsecond\n\nlast");
    document.command(&mut state, Command::Undo);
    document.command(&mut state, Command::Undo);
    assert_eq!(document.source().to_string(), "# 标题\n\nfirst second\n\nlast");
    assert_eq!(state.selection(), Selection::caret(16));
    document.command(&mut state, Command::Replace {
        range: 13..28,
        text: "正文".into(),
        kind: EditKind::Command,
    });
    assert_eq!(document.source().to_string(), "# 标题\n\nfir正文");
    document.command(&mut state, Command::Undo);
    assert_eq!(document.source().to_string(), "# 标题\n\nfirst second\n\nlast");
}

#[gpui_kit::test]
fn native_live_split_join_and_source_switch_keep_one_document_and_history(cx: &mut gpui_kit::TestAppContext) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{Entity, Focusable as _};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("prose.md");
    std::fs::write(&path, "# Heading\n\nalpha beta\n\nlast\n").unwrap();
    let (app, mut view_context, _streams) = harness_with_tabs(cx, 1);
    view_context.update(|_, cx| cx.set_global(super::super::MarkdownWorkbench));
    app.update_in(&mut view_context, |app, window, cx| app.open_file_in_editor(&path, window, cx));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let presentation: Entity<SourcePresentation> = loop {
        view_context.run_until_parked();
        if let Some(view) = app.read_with(&view_context, |app, _| app.active_editor_input().and_then(|input| input.markdown()).cloned()) { break view; }
        assert!(std::time::Instant::now() < deadline, "opening Markdown did not settle");
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    let identity = presentation.read_with(&view_context, |view, _| view.document().entity_id());
    view_context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    settle_live_geometry(&presentation, &mut view_context);
    presentation.update_in(&mut view_context, |view, window, cx| {
        assert!(view.is_live());
        assert_eq!(view.document().entity_id(), identity);
        assert_eq!(view.document().read(cx).revision(), 0);
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
        assert_eq!(view.document().read(cx).source().to_string(), "# Heading\n\nalpha 中🎉\n\nbeta\n\nlast\n");
    });
    view_context.update(|window, cx| window.press("backspace", cx));
    view_context.run_until_parked();
    presentation.read_with(&view_context, |view, cx| {
        assert_eq!(view.document().read(cx).source().to_string(), "# Heading\n\nalpha 中🎉beta\n\nlast\n");
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
        assert_eq!(view.document().read(cx).source().to_string(), "# Heading\n\nalpha 中🎉\n\nbeta\n\nlast\n");
    });
    view_context.dispatch_action(gpui_kit::component::input::Undo);
    view_context.dispatch_action(gpui_kit::component::input::Undo);
    view_context.run_until_parked();
    presentation.read_with(&view_context, |view, cx| {
        assert_eq!(view.document().read(cx).source().to_string(), "# Heading\n\nalpha beta\n\nlast\n");
    });
    app.read_with(&view_context, |app, _| assert!(!app.active_buffer().unwrap().dirty));
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
        if app.read_with(&view_context, |app, _| app.active_buffer().is_some_and(|file| !file.dirty && !file.saving)) { break; }
        assert!(std::time::Instant::now() < deadline, "saving live Markdown did not settle");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(std::fs::read(&path).unwrap(), "# Heading\n\nalpha 写beta\n\nlast\n".as_bytes());
    app.update_in(&mut view_context, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.open_file_in_editor(&path, window, cx);
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        view_context.run_until_parked();
        if app.read_with(&view_context, |app, _| app.active_buffer().is_some()) { break; }
        assert!(std::time::Instant::now() < deadline, "reopening live Markdown did not settle");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    app.read_with(&view_context, |app, cx| {
        let reopened = app.active_editor_input().unwrap().markdown().unwrap().read(cx);
        assert_ne!(reopened.document().entity_id(), identity);
        assert_eq!(reopened.document().read(cx).source().to_string(), "# Heading\n\nalpha 写beta\n\nlast\n");
    });
}

fn settle_live_geometry(view: &gpui_kit::Entity<SourcePresentation>, cx: &mut gpui_kit::VisualTestContext) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        cx.executor().advance_clock(std::time::Duration::from_millis(100));
        cx.run_until_parked();
        cx.update(|window, cx| window.render_frame(cx));
        if view.read_with(cx, |view, cx| {
            let start = view.range_to_bounds(&(0..0), cx);
            start.is_some() && start == view.range_to_bounds(&(2..2), cx)
        }) { return; }
        assert!(std::time::Instant::now() < deadline, "live heading geometry did not settle");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[gpui_kit::test]
fn native_live_pointer_selection_cuts_raw_source_across_adjacent_prose(cx: &mut gpui_kit::TestAppContext) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::{TestEventExt as _, TestWindowExt as _};
    use gpui_kit::{Modifiers, MouseButton};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("selection.md");
    std::fs::write(&path, "# Heading\n\nalpha beta\n\nlast\n").unwrap();
    let (app, mut view_context, _streams) = harness_with_tabs(cx, 1);
    view_context.update(|_, cx| cx.set_global(super::super::MarkdownWorkbench));
    app.update_in(&mut view_context, |app, window, cx| app.open_file_in_editor(&path, window, cx));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let presentation = loop {
        view_context.run_until_parked();
        if let Some(view) = app.read_with(&view_context, |app, _| app.active_editor_input().and_then(|input| input.markdown()).cloned()) { break view; }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    view_context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    settle_live_geometry(&presentation, &mut view_context);
    view_context.update(|window, cx| {
        let start = presentation.read(cx).range_to_bounds(&(11..11), cx).unwrap().center();
        let end = presentation.read(cx).range_to_bounds(&(25..25), cx).unwrap().center();
        window.pointer_down(start, MouseButton::Left, cx);
        window.pointer_up(start, MouseButton::Left, cx);
        window.change_modifiers(Modifiers::shift(), cx);
        window.pointer_down(end, MouseButton::Left, cx);
        window.pointer_up(end, MouseButton::Left, cx);
        window.change_modifiers(Modifiers::default(), cx);
        assert_eq!(presentation.read(cx).selection(), Selection::new(11, 25));
        window.press("secondary-x", cx);
        assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()), Some("alpha beta\n\nla".into()));
    });
    view_context.run_until_parked();
    presentation.read_with(&view_context, |view, cx| assert_eq!(view.document().read(cx).source().to_string(), "# Heading\n\nst\n"));
    view_context.dispatch_action(gpui_kit::component::input::Undo);
    view_context.run_until_parked();
    presentation.read_with(&view_context, |view, cx| {
        assert_eq!(view.document().read(cx).source().to_string(), "# Heading\n\nalpha beta\n\nlast\n");
        assert_eq!(view.selection(), Selection::new(11, 25));
    });
}

#[gpui_kit::test]
fn native_live_break_mapping_resize_and_composition_keep_source_ranges(cx: &mut gpui_kit::TestAppContext) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::test::{TestEventExt as _, TestInput, TestWindowExt as _};
    use gpui_kit::{ElementInputHandler, EntityInputHandler as _, Focusable as _, MouseButton, VisualContext as _, px, size};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mapping.md");
    let source = format!("# Heading\n\nA &amp; e\u{301}🎉\nsoft  \nhard\n\n\\*literal\\*\n\n---\n\n{}", "wrapping words ".repeat(240));
    std::fs::write(&path, &source).unwrap();
    let (app, mut view_context, _streams) = harness_with_tabs(cx, 1);
    view_context.update(|_, cx| cx.set_global(super::super::MarkdownWorkbench));
    app.update_in(&mut view_context, |app, window, cx| app.open_file_in_editor(&path, window, cx));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let presentation = loop {
        view_context.run_until_parked();
        if let Some(view) = app.read_with(&view_context, |app, _| app.active_editor_input().and_then(|input| input.markdown()).cloned()) { break view; }
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
        assert_eq!(before_soft.top(), after_soft.top(), "soft LF flows on the same visual line");
        assert!(after_hard.top() > after_soft.top(), "two-space hard break starts a visual line");
        let after_entity = view.range_to_bounds(&(18..18), cx).unwrap().center();
        window.pointer_down(after_entity, MouseButton::Left, cx);
        window.pointer_up(after_entity, MouseButton::Left, cx);
        window.press("backspace", cx);
    });
    view_context.run_until_parked();
    presentation.read_with(&view_context, |view, cx| assert_eq!(view.document().read(cx).source().to_string(), source.replacen("&amp;", "", 1)));
    view_context.dispatch_action(gpui_kit::component::input::Undo);
    view_context.run_until_parked();
    view_context.update(|window, cx| {
        window.render_frame(cx);
        let after_escape = presentation.read(cx).range_to_bounds(&(42..42), cx).unwrap().center();
        window.pointer_down(after_escape, MouseButton::Left, cx);
        window.pointer_up(after_escape, MouseButton::Left, cx);
        window.press("backspace", cx);
    });
    view_context.run_until_parked();
    presentation.read_with(&view_context, |view, cx| assert_eq!(view.document().read(cx).source().to_string(), source.replacen("\\*literal", "literal", 1)));
    view_context.dispatch_action(gpui_kit::component::input::Undo);
    view_context.run_until_parked();
    presentation.update_in(&mut view_context, |view, window, cx| {
        view.select_range(22..22, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    view_context.update(|window, cx| window.press("backspace", cx));
    view_context.run_until_parked();
    presentation.read_with(&view_context, |view, cx| assert_eq!(view.document().read(cx).source().to_string(), source.replacen("e\u{301}", "", 1)));
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
        window.scroll("markdown-source", gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-64.))), cx);
        assert!(presentation.read(cx).scroll_offset().y < px(0.));
        let caret = presentation.read(cx).range_to_bounds(&(158..158), cx).unwrap();
        let native = presentation.update(cx, |view, cx| view.character_index_for_point(caret.center(), window, cx)).unwrap();
        assert_eq!(native, source[..158].encode_utf16().count());
        window.pointer_down(caret.center(), MouseButton::Left, cx);
        window.pointer_up(caret.center(), MouseButton::Left, cx);
        window.press("left", cx);
        window.input("X", cx);
    });
    view_context.run_until_parked();
    presentation.read_with(&view_context, |view, cx| {
        assert_eq!(view.document().read(cx).source().to_string(), format!("{}X{}", &source[..157], &source[157..]));
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
        let caret = presentation.read(cx).range_to_bounds(&(22..22), cx).unwrap();
        let native_offset = presentation.update(cx, |view, cx| view.character_index_for_point(caret.center(), window, cx)).unwrap();
        assert_eq!(native_offset, "# Heading\n\nA &amp; e\u{301}".encode_utf16().count());
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
        assert_eq!(view.document().read(cx).source().to_string(), source.replacen("e\u{301}🎉", "e\u{301}你好🎉🎉", 1));
    });
    view_context.dispatch_action(gpui_kit::component::input::Undo);
    view_context.run_until_parked();
    presentation.read_with(&view_context, |view, cx| assert_eq!(view.document().read(cx).source().to_string(), source));
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
    document.command(&mut view, Command::Compose { range: None, text: "replacement\n".into(), selected: None });
    document.command(&mut view, Command::CancelComposition);
    assert_eq!(document.encode().unwrap(), original);
    document.command(&mut view, Command::Compose { range: None, text: "replacement\n".into(), selected: None });
    document.command(&mut view, Command::Commit("done\n".into()));
    assert_eq!(document.encode().unwrap(), b"adone\r\n\r\ntail  ");
    document.command(&mut view, Command::Undo);
    assert_eq!(document.encode().unwrap(), original);
}

#[test]
fn changing_only_a_terminator_is_an_undoable_edit_and_reload_replaces_its_representation() {
    let mut document = MarkdownDocument::new("a\nb\r\nc".into(), TextFormat::default());
    let mut view = PresentationState::default();
    document.command(&mut view, Command::Replace { range: 1..2, text: "\r\n".into(), kind: EditKind::Command });
    assert_eq!(document.source().to_string(), "a\nb\nc");
    assert_eq!(document.encode().unwrap(), b"a\r\nb\r\nc");
    document.command(&mut view, Command::Undo);
    assert_eq!(document.encode().unwrap(), b"a\nb\r\nc");
    document.command(&mut view, Command::Reload { text: "a\r\nb\nc".into(), format: TextFormat::default() });
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
    document.command(&mut view, Command::BatchReplace {
        edits: vec![(0..3, "kitten".into()), (13..16, "kitten".into()), (32..35, "kitten".into())],
        kind: EditKind::Command,
    });
    assert_eq!(document.encode().unwrap(), b"kitten\r\n* keep  \nkitten\r\n[ref]: /target\nkitten");
    document.command(&mut view, Command::Undo);
    assert_eq!(document.encode().unwrap(), original);
    assert_eq!(view.selection(), before);
    document.command(&mut view, Command::Redo);
    assert_eq!(document.encode().unwrap(), b"kitten\r\n* keep  \nkitten\r\n[ref]: /target\nkitten");
}
