#![cfg(any(target_os = "windows", target_os = "macos"))]
use gpui_kit::{AppContext as _, Focusable as _, TestAppContext, test::TestWindowExt as _};
use lyrel_markdown_editor::{Document, DocumentEvent, EditorState};
use std::{cell::RefCell, rc::Rc};

#[gpui_kit::test]
fn two_presentations_share_host_edits_and_history_without_sharing_selection(cx: &mut TestAppContext) {
    let observed = Rc::new(RefCell::new(Vec::new()));
    let (document, first, second, _subscription) = cx.update(|cx| {
        let document = cx.new(|_| Document::new("one\r\n中🎉two\nlast".into(), lyrel_text::TextFormat::default()));
        let first = cx.new(|cx| EditorState::for_document(document.clone(), cx));
        let second = cx.new(|cx| EditorState::for_document(document.clone(), cx));
        let output = observed.clone();
        let subscription = cx.subscribe(&document, move |_, event: &DocumentEvent, _| {
            output.borrow_mut().push((event.revision(), event.edits().iter().map(|edit| (edit.range(), edit.inserted().to_owned())).collect::<Vec<_>>()));
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
    let window = cx.update(|cx| cx.open_window(Default::default(), |_, _| second.clone()).unwrap());
    window.update(cx, |view, window, cx| view.focus_handle(cx).focus(window, cx)).unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.dispatch_action(Box::new(gpui_kit::component::input::Undo), cx);
    }).unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(document.read(cx).encode().unwrap(), "one\r\n中🎉two\nlast".as_bytes());
        assert_eq!(first.read(cx).text(), second.read(cx).text());
        assert_eq!(first.read(cx).selected_range(), 0..3);
    });
    cx.update_window(window.into(), |_, window, cx| window.dispatch_action(Box::new(gpui_kit::component::input::Redo), cx)).unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(document.read(cx).source(), "one\n新中🎉two\nlast"));
    assert_eq!(observed.borrow().iter().map(|(revision, _)| *revision).collect::<Vec<_>>(), vec![1, 2, 3]);
}

#[gpui_kit::test]
fn source_representation_is_atomic_for_batch_edits_and_native_undo(cx: &mut TestAppContext) {
    let original = b"\xef\xbb\xbfalpha  \r\nbeta\ngamma\r\nattached$$x$$literal\n<Unknown />";
    let decoded = lyrel_text::decode_source(original).unwrap().unwrap();
    let document = cx.update(|cx| cx.new(|_| Document::new(decoded.text, decoded.format)));
    let editor = cx.update(|cx| cx.open_window(Default::default(), |_, cx| cx.new(|cx| EditorState::for_document(document.clone(), cx))).unwrap());
    editor.update(cx, |view, _, cx| {
        view.replace_ranges(vec![(0..0, "new\n".into()), (8..12, "BETA".into())], cx);
    }).unwrap();
    let expected = b"\xef\xbb\xbfnew\nalpha  \r\nBETA\ngamma\r\nattached$$x$$literal\n<Unknown />";
    cx.update(|cx| assert_eq!(document.read(cx).encode().unwrap(), expected));
    editor.update(cx, |view, window, cx| view.focus_handle(cx).focus(window, cx)).unwrap();
    cx.update_window(editor.into(), |_, window, cx| {
        window.render_frame(cx);
        window.dispatch_action(Box::new(gpui_kit::component::input::Undo), cx);
    }).unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(document.read(cx).encode().unwrap(), original));
    cx.update_window(editor.into(), |_, window, cx| window.dispatch_action(Box::new(gpui_kit::component::input::Redo), cx)).unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(document.read(cx).encode().unwrap(), expected));
}
