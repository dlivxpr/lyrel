fn xtgettcap_consumer_exchange(
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    serial: usize,
    bytes: &[u8],
    expected: &[&[u8]],
) {
    let request = format!(
        "SEND {serial} {} {} 1\n",
        expected.len(),
        graphics_pty_fixture::hex(bytes)
    );
    view.update(cx, |view, _| view.terminal.write(request.as_bytes()));
    graphics_wait_fixture(view, cx, serial, request.len());
    let text = view.read_with(cx, |view, _| view.scrollback_text());
    for reply in expected {
        assert!(
            text.contains(&format!(
                "KITTY_REPLY {serial} {}",
                graphics_pty_fixture::hex(reply)
            )),
            "actual application reply: {text}"
        );
    }
}

#[gpui_kit::test]
fn xtgettcap_real_daemon_rooted_consumer_never_replies_twice(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    let (view, window) = graphics_rooted_consumer(cx, pane, daemon.stream(pane));
    xtgettcap_consumer_exchange(
        &view,
        cx,
        1,
        b"\x1bP+q436f;544e;4d73\x1b\\after_discovery",
        &[
            b"\x1bP1+r436F=323536\x1b\\",
            b"\x1bP0+r544E\x1b\\",
            b"\x1bP0+r4D73\x1b\\",
        ],
    );
    xtgettcap_consumer_exchange(&view, cx, 2, b"\x1bP+q436f\x18after_cancel", &[]);
    let oversized = format!("\x1bP+q{}\x1b\\after_oversized", "436f;".repeat(2000));
    xtgettcap_consumer_exchange(&view, cx, 3, oversized.as_bytes(), &[]);
    graphics_assert_production_paint(window, &view, cx);
    let text = view.read_with(cx, |view, _| view.scrollback_text());
    assert!(text.contains("after_oversized"));
    assert!(
        !text.contains("436f;"),
        "query body never reaches the production consumer"
    );
    graphics_recovery_destroy(view, window, cx);
    let (view, window) = graphics_rooted_consumer(cx, pane, daemon.stream(pane));
    xtgettcap_consumer_exchange(
        &view,
        cx,
        4,
        b"\x1bP+q524742\x1b\\after_reconnect",
        &[b"\x1bP1+r524742=382F382F38\x1b\\"],
    );
    graphics_recovery_destroy(view, window, cx);
    daemon.panes().kill(pane).unwrap();
}
