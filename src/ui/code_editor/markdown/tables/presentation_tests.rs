use super::{settle_live_geometry, start_in_source};
use crate::ui::app::test_window::harness_with_tabs;
use gpui_kit::{Focusable as _, VisualContext as _, MouseButton, point, px};
use gpui_kit::test::{TestEventExt as _, TestQueryExt as _, TestWindowExt as _};

#[gpui_kit::test]
fn table_presentation_drag_fit_scroll_edit_history_and_host_reopen(cx: &mut gpui_kit::TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("presentation.md");
    let original = format!("Before  \r\n\r\n<!-- table:striped cols=400.0,420 -->\r\n| A | B |\n| --- | --- |\r\n| one | two |\n\r\n<!-- table:grid cols=120,160 -->\n| 123 **אב** | גד |\n| --- | --- |\n| שלום | עולם |\n\r\nAfter  {}", "\n\nBelow".repeat(12));
    std::fs::write(&path, &original).unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut context, |app, window, cx| app.open_file_in_editor(&path, window, cx));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let view = loop {
        context.run_until_parked();
        if let Some(view) = app.read_with(&context, |app, _| app.active_editor_input().and_then(|input| input.markdown().cloned())) {
            start_in_source(&view, &mut context);
            break view;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    settle_live_geometry(&view, &mut context);
    let normalized = original.replace("\r\n", "\n");
    let rtl_first = normalized.find("אב").unwrap();
    let rtl_second = normalized.find("גד").unwrap();
    context.update(|window, cx| {
        let first = view.read(cx).range_to_bounds(&(rtl_first..rtl_first + "אב".len()), cx).unwrap();
        let second = view.read(cx).range_to_bounds(&(rtl_second..rtl_second + "גד".len()), cx).unwrap();
        assert!(first.left() > second.right(), "RTL columns must mirror the source order");
        window.pointer_down(first.center(), MouseButton::Left, cx);
        window.pointer_up(first.center(), MouseButton::Left, cx);
    });
    view.read_with(&context, |view, _| assert!((rtl_first..=rtl_first + "אב".len()).contains(&view.selection().head())));
    view.update_in(&mut context, |view, _, cx| view.select_range(rtl_second..rtl_second, cx));
    settle_live_geometry(&view, &mut context);
    context.update(|window, cx| window.press("right", cx));
    context.run_until_parked();
    let first_column = normalized.find("123").unwrap();
    view.read_with(&context, |view, _| assert!((first_column..=rtl_first + "אב".len()).contains(&view.selection().head())));
    context.update(|window, cx| window.press("left", cx));
    context.run_until_parked();
    view.read_with(&context, |view, _| assert!((rtl_second..=rtl_second + "גד".len()).contains(&view.selection().head())));
    view.update_in(&mut context, |view, _, cx| view.select_range(rtl_second..rtl_second, cx));
    context.update(|window, cx| window.press("tab", cx));
    context.run_until_parked();
    let body_first = normalized.find("שלום").unwrap();
    view.read_with(&context, |view, _| assert!((body_first..=body_first + "שלום".len()).contains(&view.selection().head())));
    let at = original.replace("\r\n", "\n").find("one").unwrap();
    view.update_in(&mut context, |view, window, cx| {
        view.select_range(at..at, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    settle_live_geometry(&view, &mut context);
    context.update(|window, cx| window.press("secondary-alt-shift-6", cx));
    context.run_until_parked();
    view.read_with(&context, |view, cx| assert_eq!(view.document().read(cx).encode().unwrap(), original.replace("table:striped", "table:header").as_bytes()));
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle_live_geometry(&view, &mut context);
    view.read_with(&context, |view, cx| assert_eq!(view.document().read(cx).encode().unwrap(), original.as_bytes()));
    let start = context.update(|window, _| window.find_all_by_label("Resize table column")[0].bounds().center());
    context.update(|window, cx| {
        window.pointer_down(start, MouseButton::Left, cx);
        window.pointer_move(start + point(px(32.), px(0.)), Some(MouseButton::Left), cx);
    });
    view.read_with(&context, |view, cx| assert_eq!(view.document().read(cx).encode().unwrap(), original.as_bytes()));
    context.update(|window, cx| window.pointer_up(start + point(px(32.), px(0.)), MouseButton::Left, cx));
    settle_live_geometry(&view, &mut context);
    let resized = original.replace("cols=400.0,420", "cols=432,420");
    view.read_with(&context, |view, cx| assert_eq!(view.document().read(cx).encode().unwrap(), resized.as_bytes()));
    // Horizontal wheel is presentation-only and cannot move prose horizontally.
    let prose = context.update(|window, cx| {
        let prose = view.read(cx).range_to_bounds(&(0..0), cx).unwrap();
        let cell = view.read(cx).range_to_bounds(&(at..at + 1), cx).unwrap();
        let handle = window.find_all_by_label("Resize table column")[0].bounds().center();
        window.dispatch_event(gpui_kit::PlatformInput::Scroll(gpui_kit::ScrollEvent {
            position: handle + point(px(-20.), px(20.)),
            delta: gpui_kit::ScrollDelta::Pixels(point(px(-120.), px(0.))),
            touch_phase: gpui_kit::TouchPhase::Moved,
            ..Default::default()
        }), cx);
        (prose, cell)
    });
    settle_live_geometry(&view, &mut context);
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).encode().unwrap(), resized.as_bytes());
        assert_eq!(view.range_to_bounds(&(0..0), cx).unwrap().left(), prose.0.left());
        assert!(view.range_to_bounds(&(at..at + 1), cx).unwrap().left() < prose.1.left());
        assert_eq!(view.scroll_offset().x, px(0.));
    });
    context.update(|window, cx| {
        let handle = window.find_all_by_label("Resize table column")[0].bounds().center();
        window.dispatch_event(gpui_kit::PlatformInput::Scroll(gpui_kit::ScrollEvent {
            position: handle + point(px(-20.), px(20.)),
            delta: gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-32.))),
            ..Default::default()
        }), cx);
    });
    settle_live_geometry(&view, &mut context);
    view.read_with(&context, |view, cx| {
        assert!(view.scroll_offset().y < px(0.));
        assert_eq!(view.document().read(cx).encode().unwrap(), resized.as_bytes());
    });
    context.update(|window, cx| window.scroll("markdown-source", gpui_kit::ScrollDelta::Pixels(point(px(0.), px(32.))), cx));
    settle_live_geometry(&view, &mut context);
    context.update(|window, cx| window.press("secondary-alt-f", cx));
    settle_live_geometry(&view, &mut context);
    let fitted = view.read_with(&context, |view, cx| view.document().read(cx).encode().unwrap());
    assert_ne!(fitted, resized.as_bytes());
    assert!(String::from_utf8(fitted.clone()).unwrap().contains("table:striped cols="));
    context.update(|window, cx| window.press("shift-right", cx));
    context.update(|window, cx| window.input("X", cx));
    context.run_until_parked();
    let edited = view.read_with(&context, |view, cx| view.document().read(cx).encode().unwrap());
    assert_eq!(edited, String::from_utf8(fitted.clone()).unwrap().replace("one", "Xne").as_bytes());
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| assert_eq!(view.document().read(cx).encode().unwrap(), fitted));
    view.read_with(&context, |view, _| assert_eq!(view.selection().range().len(), 1));
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| assert_eq!(view.document().read(cx).encode().unwrap(), resized.as_bytes()));
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| assert_eq!(view.document().read(cx).encode().unwrap(), original.as_bytes()));
    for _ in 0..3 { context.dispatch_action(gpui_kit::component::input::Redo); context.run_until_parked(); }
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    context.dispatch_action(crate::core::actions::EditorSave);
    loop {
        context.run_until_parked();
        if app.read_with(&context, |app, _| app.active_buffer().is_some_and(|file| !file.dirty && !file.saving)) { break; }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(std::fs::read(&path).unwrap(), edited);
    app.update_in(&mut context, |app, window, cx| { app.editor_close_file(0, window, cx); app.open_file_in_editor(&path, window, cx); });
    loop {
        context.run_until_parked();
        if app.read_with(&context, |app, _| app.active_buffer().is_some()) { break; }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    app.read_with(&context, |app, cx| assert_eq!(app.active_editor_input().unwrap().markdown().unwrap().read(cx).document().read(cx).encode().unwrap(), edited));
    let reopened = app.read_with(&context, |app, _| app.active_editor_input().unwrap().markdown().unwrap().clone());
    start_in_source(&reopened, &mut context);
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    settle_live_geometry(&reopened, &mut context);
    context.update(|window, _| {
        let handles = window.find_all_by_label("Resize table column");
        assert_eq!(handles[1].bounds().center().x - handles[0].bounds().center().x, window.rem_size() * (420. / 16.));
    });
    reopened.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).encode().unwrap(), edited);
        assert_eq!(view.document().read(cx).revision(), 0);
    });
}
