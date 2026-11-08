//! Consumer regression for keyboard return from native MDX controls.
use crate::ui::app::test_window::harness_with_tabs;
use gpui_kit::{
    Focusable as _, VisualContext as _,
    test::TestWindowExt as _,
};

#[gpui_kit::test]
fn mdx_control_escape_returns_to_nested_text_without_changing_source_or_history(
    cx: &mut gpui_kit::TestAppContext,
) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("control-focus.mdx");
    let original = "---\r\nkeep: true\n---\r\n\n<Note title='keep'>\r\n\nnested **bold** text\n\n</Note>\r\n\nexport const value = () => neverExecute()";
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
    let at = original.replace("\r\n", "\n").find("bold").unwrap();
    view.update_in(&mut context, |view, window, cx| {
        assert!(view.is_live());
        view.select_range(at..at + 4, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    let revision = view.read_with(&context, |view, cx| view.document().read(cx).revision());
    // Traverse actual controls without activating them. Escape must return to
    // the existing nested selection, not select a property or recreate input.
    context.update(|window, cx| {
        for _ in 0..128 {
            window.focus_next(cx);
            window.render_frame(cx);
            if window.find("mdx-property-title").focused() == Some(true) {
                break;
            }
        }
        assert_eq!(window.find("mdx-property-title").focused(), Some(true));
        window.press("escape", cx);
        assert!(view.read(cx).focus_handle(cx).is_focused(window));
        assert_eq!(view.read(cx).selection().range(), at..at + 4);
        assert_eq!(view.read(cx).document().read(cx).revision(), revision);
        assert_eq!(
            view.read(cx).document().read(cx).encode().unwrap(),
            original.as_bytes()
        );
        window.input("changed", cx);
    });
    let edited = original.replacen("**bold**", "**changed**", 1);
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).encode().unwrap(), edited.as_bytes());
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        );
    });
    // The Action path used by menus/keymaps has the same focus-only behavior.
    context.update(|window, cx| {
        for _ in 0..128 {
            window.focus_next(cx);
            window.render_frame(cx);
            if window.find("callout-edit-source").focused() == Some(true) {
                break;
            }
        }
        assert_eq!(window.find("callout-edit-source").focused(), Some(true));
    });
    context.dispatch_action(gpui_kit::component::input::Escape);
    context.update(|window, cx| {
        assert!(view.read(cx).focus_handle(cx).is_focused(window));
        assert!(view.read(cx).is_live());
        assert_eq!(
            view.read(cx).document().read(cx).encode().unwrap(),
            original.as_bytes()
        );
    });
}
