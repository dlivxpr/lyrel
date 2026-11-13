fn xtgettcap_consumer_exchange(
    view: &Entity<TerminalView>,
    observer: &mut lyrel_core::client::PaneSession,
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
    let input_bytes = request.len();
    view.read_with(cx, |view, _| view.terminal.write(request.into_bytes()));
    // The fixture deliberately rewrites one screen row for each reply. Read
    // all application receipts from the public observer stream rather than
    // mistaking the final overwritten grid row for the whole reply batch.
    let marker = format!("KITTY_DONE_{serial};");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut output = Vec::new();
    loop {
        match observer.recv() {
            Ok(DaemonMsg::Output(bytes) | DaemonMsg::Snapshot(bytes)) => output.extend(bytes),
            Ok(DaemonMsg::Exited { code }) => panic!("discovery application exited: {code:?}"),
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => panic!("discovery observer: {error}"),
        }
        if output
            .windows(marker.len())
            .any(|bytes| bytes == marker.as_bytes())
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "missing {marker}: {}",
            String::from_utf8_lossy(&output)
        );
        graphics_consumer_progress(cx);
    }
    graphics_wait_fixture(view, cx, serial, input_bytes);
    let text = String::from_utf8_lossy(&output);
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
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    let mut observer = daemon
        .panes()
        .observe(
            pane,
            crate::daemon::protocol::WinSize {
                cols: 80,
                rows: 24,
                cell_w: 10,
                cell_h: 20,
            },
        )
        .unwrap();
    observer
        .set_recv_timeout(Some(std::time::Duration::from_millis(20)))
        .unwrap();
    let (view, window) = graphics_rooted_consumer(cx, pane, daemon.stream(pane));
    xtgettcap_consumer_exchange(
        &view,
        &mut observer,
        cx,
        1,
        b"\x1bP+q436f;544e;4d73\x1b\\after_discovery",
        &[
            b"\x1bP1+r436F=323536\x1b\\",
            b"\x1bP0+r544E\x1b\\",
            b"\x1bP0+r4D73\x1b\\",
        ],
    );
    xtgettcap_consumer_exchange(
        &view,
        &mut observer,
        cx,
        2,
        b"\x1bP+q436f\x18after_cancel",
        &[],
    );
    let oversized = format!("\x1bP+q{}\x1b\\after_oversized", "436f;".repeat(2000));
    xtgettcap_consumer_exchange(&view, &mut observer, cx, 3, oversized.as_bytes(), &[]);
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
        &mut observer,
        cx,
        4,
        b"\x1bP+q524742\x1b\\after_reconnect",
        &[b"\x1bP1+r524742=382F382F38\x1b\\"],
    );
    graphics_recovery_destroy(view, window, cx);
    observer.detach().unwrap();
    daemon.panes().kill(pane).unwrap();
}
