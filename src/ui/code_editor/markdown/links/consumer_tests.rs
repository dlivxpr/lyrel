use super::*;

#[gpui_kit::test]
fn links_selection_gestures_never_navigate_and_plain_click_edits(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui_kit::{
        Focusable as _, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, MouseButton,
        MouseDownEvent, PlatformInput,
        test::{TestEventExt as _, TestWindowExt as _},
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
    let label = original.find("target").unwrap();
    let modifier = if cfg!(target_os = "macos") {
        Modifiers {
            platform: true,
            ..Modifiers::none()
        }
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
                view.range_to_bounds(&(label + 1..label + 1), cx)
                    .unwrap()
                    .center(),
                view.range_to_bounds(&(label + 5..label + 5), cx)
                    .unwrap()
                    .center(),
            )
        });
        match gesture {
            0 => {
                let shift = Modifiers {
                    shift: true,
                    ..modifier
                };
                context.simulate_mouse_down(point, MouseButton::Left, shift);
                context.simulate_mouse_up(point, MouseButton::Left, shift);
            }
            1 => {
                context.update(|window, cx| {
                    window.dispatch_event(
                        PlatformInput::MouseDown(MouseDownEvent {
                            button: MouseButton::Left,
                            position: point,
                            modifiers: modifier,
                            click_count: 2,
                            first_mouse: false,
                        }),
                        cx,
                    );
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
            assert!(
                (label..=label + "target words".len()).contains(&view.selection().head()),
                "selection gesture left the link label"
            );
            if gesture == 1 {
                assert_eq!(view.selection().range(), label..label + 6);
            }
            assert_eq!(
                view.document().read(cx).encode().unwrap(),
                original.as_bytes()
            );
        });
    }
    // The explicit simulated gestures retain their modifiers in the window.
    // Release them through the real event route before testing an ordinary click.
    context.update(|window, cx| window.change_modifiers(Modifiers::none(), cx));
    settle_live_geometry(&view, &mut context);
    context.update(|window, cx| {
        assert_eq!(window.modifiers(), Modifiers::none());
        let point = view
            .read(cx)
            .range_to_bounds(&(label + 1..label + 1), cx)
            .unwrap()
            .center();
        window.pointer_down(point, MouseButton::Left, cx);
        window.pointer_up(point, MouseButton::Left, cx);
        assert_eq!(view.read(cx).selection(), Selection::caret(label + 1));
        window.input("X", cx);
    });
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.replacen("target", "tXarget", 1).as_bytes()
        );
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle_live_geometry(&view, &mut context);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        );
    });
    view.update_in(&mut context, |view, _, cx| {
        view.select_range(label..label, cx)
    });
    context.update(|window, cx| {
        window.right_click("markdown-source", cx);
    });
    // ContextMenu mounts its PopupMenu in a deferred app update.
    context.run_until_parked();
    let escape = context.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("popup-menu").focused(), Some(true));
        assert_eq!(view.read(cx).selection(), Selection::caret(label));
        let escape = Keystroke::parse_with_os("escape", cx.platform_info().os).unwrap();
        window.dispatch_event(
            PlatformInput::KeyDown(KeyDownEvent {
                keystroke: escape.clone(),
                is_held: false,
                prefer_character_input: false,
            }),
            cx,
        );
        escape
    });
    // Match native event boundaries: let dismissal remove the menu before
    // key-up can draw another frame of the still-open popup.
    context.run_until_parked();
    context.update(|window, cx| {
        window.dispatch_event(PlatformInput::KeyUp(KeyUpEvent { keystroke: escape }), cx);
    });
    context.run_until_parked();
    context.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("popup-menu").is_none());
        assert!(view.read(cx).focus_handle(cx).is_focused(window));
        assert_eq!(view.read(cx).selection(), Selection::caret(label));
        window.right_click("markdown-source", cx);
    });
    context.run_until_parked();
    context.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("popup-menu").focused(), Some(true));
        window.scroll(
            "popup-menu",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(-10000.))),
            cx,
        );
        let edit = window
            .within("popup-menu")
            .find_by_label(crate::ui::i18n::t(
                crate::ui::i18n::L10nKey::MarkdownEditLinkDestination,
            ))
            .bounds()
            .center();
        assert!(window.find("popup-menu").bounds().contains(&edit));
        assert!(
            edit.y < window.viewport_size().height && edit.x < window.viewport_size().width,
            "menu {:?}, click {:?}, viewport {:?}",
            window.find("popup-menu").bounds(),
            edit,
            window.viewport_size()
        );
        window.pointer_move(edit, None, cx);
        window.pointer_down(edit, MouseButton::Left, cx);
        window.pointer_up(edit, MouseButton::Left, cx);
    });
    // Popup actions are deferred; let the edit command consume the menu's
    // original selection before typing or selecting the label for Open link.
    context.run_until_parked();
    let destination = original.find("#heading").unwrap();
    context.update(|window, cx| {
        assert_eq!(
            view.read(cx).selection().range(),
            destination..destination + "#heading".len()
        );
        assert!(view.read(cx).focus_handle(cx).is_focused(window));
        window.input("#missing", cx);
    });
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.replacen("#heading", "#missing", 1).as_bytes()
        );
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle_live_geometry(&view, &mut context);
    view.update_in(&mut context, |view, _, cx| {
        view.select_range(label..label, cx)
    });
    settle_live_geometry(&view, &mut context);
    context.update(|window, cx| {
        window.right_click("markdown-source", cx);
    });
    context.run_until_parked();
    context.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("popup-menu").focused(), Some(true));
        window.scroll(
            "popup-menu",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(-10000.))),
            cx,
        );
        let menu = window.within("popup-menu");
        let open = menu
            .find_by_label(crate::ui::i18n::t(
                crate::ui::i18n::L10nKey::MarkdownFollowLink,
            ))
            .bounds()
            .center();
        assert!(window.find("popup-menu").bounds().contains(&open));
        window.pointer_move(open, None, cx);
        window.pointer_down(open, MouseButton::Left, cx);
        window.pointer_up(open, MouseButton::Left, cx);
    });
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.selection(),
            Selection::caret(original.find("Heading").unwrap())
        );
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        );
    });
}
