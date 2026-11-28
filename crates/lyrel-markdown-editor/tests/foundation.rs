#![cfg(any(target_os = "windows", target_os = "macos"))]
use gpui_kit::{AppContext as _, Focusable as _, TestAppContext, test::TestWindowExt as _};
use lyrel_markdown_editor::{Document, DocumentEvent, EditorState};
use std::{cell::RefCell, rc::Rc};

#[gpui_kit::test]
fn two_presentations_share_host_edits_and_history_without_sharing_selection(
    cx: &mut TestAppContext,
) {
    let observed = Rc::new(RefCell::new(Vec::new()));
    let (document, first, second, _subscription) = cx.update(|cx| {
        let document = cx.new(|_| {
            Document::new(
                "one\r\n中🎉two\nlast".into(),
                lyrel_document::text::TextFormat::default(),
            )
        });
        let first = cx.new(|cx| EditorState::for_document(document.clone(), cx));
        let second = cx.new(|cx| EditorState::for_document(document.clone(), cx));
        let output = observed.clone();
        let subscription = cx.subscribe(&document, move |_, event: &DocumentEvent, _| {
            output.borrow_mut().push((
                event.revision(),
                event
                    .edits()
                    .iter()
                    .map(|edit| (edit.range(), edit.inserted().to_owned()))
                    .collect::<Vec<_>>(),
            ));
        });
        first.update(cx, |view, cx| view.select_range(0..3, cx));
        second.update(cx, |view, cx| view.select_range(4..4, cx));
        (document, first, second, subscription)
    });
    cx.update(|cx| second.update(cx, |view, cx| view.replace_range(4..4, "新", cx)));
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(document.read(cx).source(), "one\n新中🎉two\nlast");
        assert_eq!(first.read(cx).text(), second.read(cx).text());
        assert_eq!(first.read(cx).selected_range(), 0..3);
        assert_eq!(second.read(cx).selected_range(), 7..7);
    });
    assert_eq!(*observed.borrow(), vec![(1, vec![(4..4, "新".into())])]);
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, _| second.clone())
            .unwrap()
    });
    window
        .update(cx, |view, window, cx| {
            view.focus_handle(cx).focus(window, cx)
        })
        .unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.dispatch_action(Box::new(gpui_kit::component::input::Undo), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(
            document.read(cx).encode().unwrap(),
            "one\r\n中🎉two\nlast".as_bytes()
        );
        assert_eq!(first.read(cx).text(), second.read(cx).text());
        assert_eq!(first.read(cx).selected_range(), 0..3);
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.dispatch_action(Box::new(gpui_kit::component::input::Redo), cx)
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(document.read(cx).source(), "one\n新中🎉two\nlast"));
    assert_eq!(
        observed
            .borrow()
            .iter()
            .map(|(revision, _)| *revision)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
}

#[gpui_kit::test]
fn source_representation_is_atomic_for_batch_edits_and_native_undo(cx: &mut TestAppContext) {
    let original = b"\xef\xbb\xbfalpha  \r\nbeta\ngamma\r\nattached$$x$$literal\n<Unknown />";
    let decoded = lyrel_document::text::decode_source(original)
        .unwrap()
        .unwrap();
    let document = cx.update(|cx| cx.new(|_| Document::new(decoded.text, decoded.format)));
    let editor = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| EditorState::for_document(document.clone(), cx))
        })
        .unwrap()
    });
    editor
        .update(cx, |view, _, cx| {
            view.replace_ranges(vec![(0..0, "new\n".into()), (8..12, "BETA".into())], cx);
        })
        .unwrap();
    let expected = b"\xef\xbb\xbfnew\nalpha  \r\nBETA\ngamma\r\nattached$$x$$literal\n<Unknown />";
    cx.update(|cx| assert_eq!(document.read(cx).encode().unwrap(), expected));
    editor
        .update(cx, |view, window, cx| {
            view.focus_handle(cx).focus(window, cx)
        })
        .unwrap();
    cx.update_window(editor.into(), |_, window, cx| {
        window.render_frame(cx);
        window.dispatch_action(Box::new(gpui_kit::component::input::Undo), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(document.read(cx).encode().unwrap(), original));
    cx.update_window(editor.into(), |_, window, cx| {
        window.dispatch_action(Box::new(gpui_kit::component::input::Redo), cx)
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(document.read(cx).encode().unwrap(), expected));
}

#[gpui_kit::test]
fn unchanged_replacement_preserves_undo_and_redo_selection(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let document = cx.new(|_| Document::new("one two".into(), Default::default()));
        let view = cx.new(|cx| EditorState::for_document(document.clone(), cx));
        view.update(cx, |view, cx| {
            view.select_range(0..3, cx);
            view.replace_range(0..3, "first", cx);
            view.select_range(6..9, cx);
            view.replace_range(6..9, "two", cx);

            assert!(view.undo_document(cx));
            assert_eq!(view.text(), "one two");
            assert_eq!(view.selected_range(), 0..3);
            assert!(view.redo_document(cx));
            assert_eq!(view.text(), "first two");
            assert_eq!(view.selected_range(), 5..5);
        });
    });
}

#[gpui_kit::test]
fn independent_edits_release_historical_text_buffers(cx: &mut TestAppContext) {
    let (document, view, original, intermediate) = cx.update(|cx| {
        let document = cx.new(|_| Document::new("alpha\r\nbeta\n".into(), Default::default()));
        let view = cx.new(|cx| EditorState::for_document(document.clone(), cx));
        let original = std::sync::Arc::downgrade(&document.read(cx).snapshot().shared_text());
        view.update(cx, |view, cx| view.replace_range(0..1, "A", cx));
        let intermediate = std::sync::Arc::downgrade(&document.read(cx).snapshot().shared_text());
        view.update(cx, |view, cx| view.replace_range(6..7, "B", cx));
        (document, view, original, intermediate)
    });
    cx.run_until_parked();
    assert!(original.upgrade().is_none());
    assert!(intermediate.upgrade().is_none());
    for expected in [b"Alpha\r\nbeta\n", b"alpha\r\nbeta\n"] {
        cx.update(|cx| {
            assert!(view.update(cx, |view, cx| view.undo_document(cx)));
            assert_eq!(document.read(cx).encode().unwrap(), expected);
        });
    }
}

#[gpui_kit::test]
fn source_history_replays_representation_only_moves_across_format_conversion(
    cx: &mut TestAppContext,
) {
    use lyrel_document::text::{LineEnding, TextFormat};

    let original = "α\r\nα\ntail";
    let moved = "α\nα\r\ntail";
    let converted = "\u{feff}α\r\nα\r\ntail";
    let edited = "\u{feff}β\r\nγ\r\nα\r\n新\r\ntail";
    let format = TextFormat::default();
    let new_format = TextFormat {
        bom: true,
        line_ending: LineEnding::CrLf,
        ..format
    };

    let (document, view) = cx.update(|cx| {
        let document = cx.new(|_| Document::new(original.into(), format));
        let view = cx.new(|cx| EditorState::for_document(document.clone(), cx));
        (document, view)
    });
    let moved_snapshot = cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.replace_lines(0..6, vec![3..6, 0..3], false, true, cx)
        });
        let snapshot = document.read(cx).snapshot();
        assert_eq!(snapshot.text(), "α\nα\ntail");
        assert_eq!(snapshot.encode().unwrap(), moved.as_bytes());
        snapshot
    });
    cx.update(|cx| {
        view.update(cx, |view, cx| view.set_format(new_format, cx));
        assert_eq!(document.read(cx).encode().unwrap(), converted.as_bytes());
        view.update(cx, |view, cx| {
            view.replace_ranges(vec![(6..6, "新\n".into()), (0..3, "β\nγ\n".into())], cx)
        });
        assert_eq!(document.read(cx).encode().unwrap(), edited.as_bytes());
    });
    for expected in [converted, moved, original] {
        cx.update(|cx| {
            assert!(view.update(cx, |view, cx| view.undo_document(cx)));
            assert_eq!(document.read(cx).encode().unwrap(), expected.as_bytes());
        });
    }
    for expected in [moved, converted, edited] {
        cx.update(|cx| {
            assert!(view.update(cx, |view, cx| view.redo_document(cx)));
            assert_eq!(document.read(cx).encode().unwrap(), expected.as_bytes());
        });
    }
    assert_eq!(moved_snapshot.encode().unwrap(), moved.as_bytes());
}
