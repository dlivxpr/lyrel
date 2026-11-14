// Real PTY -> isolated production daemon -> ordered pane stream -> actual
// TerminalView/TerminalElement event handlers. GPUI's test platform exposes no
// native cursor reader: content_cursor is the production paint decision; native
// shape/hitbox priority is checked separately with pointer_scene.mjs.
#[gpui_kit::test]
fn pointer_real_pty_set_reset_fallback_and_command_cleanup(cx: &mut TestAppContext) {
    use gpui_kit::CursorStyle;
    let (daemon, pane, view, window) = graphics_layout_harness(cx);
    let other_pane = daemon.spawn();
    let (other_view, other_window) =
        graphics_rooted_consumer(cx, other_pane, daemon.stream(other_pane));
    for (serial, bytes, expected) in [
        (
            1,
            b"\x1b]22;pointer\x1b\\".as_slice(),
            CursorStyle::PointingHand,
        ),
        (
            2,
            b"\x1b]22;ew-resize\x07".as_slice(),
            CursorStyle::ResizeLeftRight,
        ),
        (
            3,
            b"\x1b]22;ne-resize\x1b\\".as_slice(),
            CursorStyle::ResizeUpRightDownLeft,
        ),
        (4, b"\x1b]22;help\x1b\\".as_slice(), CursorStyle::Arrow),
        (5, b"\x1b]22;\x07".as_slice(), CursorStyle::IBeam),
        (
            6,
            b"\x1b]22;pointer\x07\x1b]22;\x07\x1b]22;ns-resize\x07".as_slice(),
            CursorStyle::ResizeUpDown,
        ),
        (
            7,
            b"\x1b]22;not-a-shape\x07AFTER-UNKNOWN".as_slice(),
            CursorStyle::ResizeUpDown,
        ),
        (
            8,
            b"\x1b]22;\x1b\\AFTER-RESET".as_slice(),
            CursorStyle::IBeam,
        ),
        (
            9,
            b"\x1b]22;pointer\x07\x1b]133;D;0\x07".as_slice(),
            CursorStyle::IBeam,
        ),
        (
            10,
            b"\x1b]22;pointer\x07\x1bc".as_slice(),
            CursorStyle::IBeam,
        ),
    ] {
        graphics_send_fixture(&view, cx, serial, bytes);
        graphics_assert_production_paint(window, &view, cx);
        assert_eq!(
            view.read_with(cx, |view, _| view.content_cursor()),
            Some(expected),
            "PTY serial {serial}"
        );
        assert_eq!(
            other_view.read_with(cx, |view, _| view.content_cursor()),
            Some(CursorStyle::IBeam),
            "another pane never inherits this application's override"
        );
    }
    assert!(view.read_with(cx, |view, _| {
        view.scrollback_text().contains("KITTY_DONE_10;")
    }));
    graphics_send_fixture(
        &view,
        cx,
        11,
        b"\x1b[?1003h\x1b[?1006h\x1b]22;pointer\x07\x1b]22;\x07",
    );
    graphics_assert_production_paint(window, &view, cx);
    assert_eq!(
        view.read_with(cx, |view, _| view.content_cursor()),
        None,
        "reset in application mouse mode restores the native default, not a sticky I-beam"
    );
    daemon.panes().kill(pane).unwrap();
    graphics_recovery_destroy(view, window, cx);
    daemon.panes().kill(other_pane).unwrap();
    graphics_recovery_destroy(other_view, other_window, cx);
}

#[gpui_kit::test]
fn pointer_real_pty_selection_and_armed_link_run_before_the_app_claim(cx: &mut TestAppContext) {
    use gpui_kit::CursorStyle;
    use gpui_kit::test::TestEventExt as _;
    let (daemon, pane, view, window) = graphics_layout_harness(cx);
    graphics_send_fixture(&view, cx, 1, b"\x1b[2J\x1b[Hselect-me\x1b[3;1H\x1b]8;;https://example.org/pointer\x1b\\open-link\x1b]8;;\x1b\\\x1b]22;ew-resize\x07\x1b[?1003h\x1b[?1006h");
    graphics_assert_production_paint(window, &view, cx);
    let geom = view.read_with(cx, |view, _| view.cell_geom.unwrap());
    let at = |col: f32, row: f32| {
        point(
            geom.origin.x + geom.cell_width * col,
            geom.origin.y + geom.line_height * row,
        )
    };
    window
        .update(cx, |_, window, cx| {
            window.pointer_move(at(0.1, 0.5), None, cx);
            assert_eq!(
                view.read(cx).content_cursor(),
                Some(CursorStyle::ResizeLeftRight)
            );
            window.change_modifiers(
                Modifiers {
                    shift: true,
                    ..Default::default()
                },
                cx,
            );
            window.pointer_down(at(0.1, 0.5), MouseButton::Left, cx);
            window.pointer_move(at(8.9, 0.5), Some(MouseButton::Left), cx);
            assert_eq!(
                view.read(cx).content_cursor(),
                Some(CursorStyle::IBeam),
                "the running selection, not the app's resize affordance"
            );
            window.pointer_up(at(8.9, 0.5), MouseButton::Left, cx);
            window.change_modifiers(Modifiers::default(), cx);
            window.dispatch_action(Box::new(CopyText), cx);
        })
        .unwrap();
    assert_eq!(
        cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()))
            .as_deref(),
        Some("select-me")
    );
    window
        .update(cx, |_, window, cx| {
            let modifier = Modifiers {
                control: cfg!(windows),
                platform: cfg!(target_os = "macos"),
                ..Default::default()
            };
            window.change_modifiers(modifier, cx);
            window.pointer_move(at(2.5, 2.5), None, cx);
            assert_eq!(
                view.read(cx).content_cursor(),
                Some(CursorStyle::PointingHand)
            );
            window.pointer_down(at(2.5, 2.5), MouseButton::Left, cx);
            window.pointer_up(at(2.5, 2.5), MouseButton::Left, cx);
            window.change_modifiers(Modifiers::default(), cx);
            window.pointer_move(at(20.5, 4.5), None, cx);
            assert_eq!(
                view.read(cx).content_cursor(),
                Some(CursorStyle::ResizeLeftRight)
            );
        })
        .unwrap();
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://example.org/pointer")
    );
    daemon.panes().kill(pane).unwrap();
    graphics_recovery_destroy(view, window, cx);
}
