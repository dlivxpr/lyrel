use super::*;

#[gpui_kit::test]
fn links_selection_gestures_never_navigate_and_plain_click_edits(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::{
        Focusable as _, Modifiers, MouseButton, MouseDownEvent, PlatformInput,
        test::{TestEventExt as _, TestQueryExt as _, TestWindowExt as _},
    };
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("gestures.md");
    let original = "# Heading\n\noutside [target words](#heading) tail\n";
    std::fs::write(&path, original).unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut context, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let view = loop {
        context.run_until_parked();
        if let Some(view) = app.read_with(&context, |app, _| {
            app.active_editor_input().and_then(|input| input.markdown().cloned())
        }) {
            start_in_source(&view, &mut context);
            break view;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    let label = original.find("target").unwrap();
    let modifier = if cfg!(target_os = "macos") {
        Modifiers { platform: true, ..Modifiers::none() }
    } else {
        Modifiers::control()
    };
    for gesture in [0, 1, 2] {
        view.update_in(&mut context, |view, window, cx| {
            view.select_range(label..label, cx);
            view.focus_handle(cx).focus(window, cx);
        });
        settle_live_geometry(&view, &mut context);
        let (point, end) = context.update(|_, cx| {
            let view = view.read(cx);
            (
                view.range_to_bounds(&(label + 1..label + 1), cx).unwrap().center(),
                view.range_to_bounds(&(label + 5..label + 5), cx).unwrap().center(),
            )
        });
        match gesture {
            0 => {
                let shift = Modifiers { shift: true, ..modifier };
                context.simulate_mouse_down(point, MouseButton::Left, shift);
                context.simulate_mouse_up(point, MouseButton::Left, shift);
            }
            1 => {
                context.update(|window, cx| {
                    window.dispatch_event(PlatformInput::MouseDown(MouseDownEvent {
                        button: MouseButton::Left,
                        position: point,
                        modifiers: modifier,
                        click_count: 2,
                        first_mouse: false,
                    }), cx);
                });
                context.simulate_mouse_up(point, MouseButton::Left, modifier);
            }
            _ => {
                context.simulate_mouse_down(point, MouseButton::Left, modifier);
                context.simulate_mouse_move(end, Some(MouseButton::Left), modifier);
                // Returning to the same link must not turn a drag into a click.
                context.simulate_mouse_move(point, Some(MouseButton::Left), modifier);
                context.simulate_mouse_up(point, MouseButton::Left, modifier);
            }
        }
        context.run_until_parked();
        view.read_with(&context, |view, cx| {
            assert_ne!(view.selection(), Selection::caret(0), "selection gesture navigated");
            if gesture == 1 {
                assert_eq!(view.selection().range(), label..label + 6);
            }
            assert_eq!(view.document().read(cx).encode().unwrap(), original.as_bytes());
        });
    }
    settle_live_geometry(&view, &mut context);
    context.update(|window, cx| {
        let point = view.read(cx).range_to_bounds(&(label + 1..label + 1), cx).unwrap().center();
        window.pointer_down(point, MouseButton::Left, cx);
        window.pointer_up(point, MouseButton::Left, cx);
        window.input("X", cx);
    });
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).encode().unwrap(), original.replacen("target", "tXarget", 1).as_bytes());
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle_live_geometry(&view, &mut context);
    view.update_in(&mut context, |view, _, cx| view.select_range(label..label, cx));
    context.update(|window, cx| {
        window.right_click("markdown-source", cx);
        window.press("escape", cx);
        assert!(view.read(cx).focus_handle(cx).is_focused(window));
        assert_ne!(view.read(cx).selection(), Selection::caret(0));
        window.right_click("markdown-source", cx);
        let menu = window.within("popup-menu");
        let open = menu.find_by_label(crate::ui::i18n::t(crate::ui::i18n::L10nKey::MarkdownFollowLink)).bounds().center();
        window.pointer_down(open, MouseButton::Left, cx);
        window.pointer_up(open, MouseButton::Left, cx);
    });
    view.read_with(&context, |view, cx| {
        assert_eq!(view.selection(), Selection::caret(0));
        assert_eq!(view.document().read(cx).encode().unwrap(), original.as_bytes());
    });
}
