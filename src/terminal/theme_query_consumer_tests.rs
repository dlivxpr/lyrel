#[test]
#[ignore = "real theme-query application in an isolated PTY"]
fn theme_fixture_sender() {
    graphics_pty_fixture::theme_query_sender();
}

fn theme_wait_result(
    observer: &mut lyrel_core::client::PaneSession,
    cx: &mut TestAppContext,
    serial: usize,
    expected: &[u8],
) {
    let marker = format!("THEME_RESULT_{serial} ");
    let mut output = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        match observer.recv() {
            Ok(DaemonMsg::Output(bytes) | DaemonMsg::Snapshot(bytes)) => output.extend(bytes),
            Ok(DaemonMsg::Exited { code }) => panic!("theme application exited: {code:?}"),
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => panic!("theme observer: {error}"),
        }
        let text = String::from_utf8_lossy(&output);
        if let Some((_, tail)) = text.split_once(&marker)
            && let Some((reply, _)) = tail.split_once(" END")
        {
            assert_eq!(reply, graphics_pty_fixture::hex(expected), "{text}");
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "missing {marker}: {text}"
        );
        graphics_consumer_progress(cx);
    }
}

#[gpui_kit::test]
fn theme_queries_cross_real_daemon_gui_replay_without_duplicates(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let daemon = GraphicsDaemonProcess::start();
    let size = crate::daemon::protocol::WinSize {
        cols: 80,
        rows: 24,
        cell_w: 10,
        cell_h: 20,
    };
    let shell = crate::daemon::protocol::ShellSpec {
        program: std::env::current_exe().unwrap().display().to_string(),
        args: vec![
            "--ignored".into(),
            "--exact".into(),
            format!(
                "{}::theme_fixture_sender",
                module_path!().split_once("::").unwrap().1
            ),
            "--nocapture".into(),
            "--color=never".into(),
            "--test-threads=1".into(),
        ],
        args_are_lyrel_defaults: false,
    };
    let mut original = daemon
        .panes()
        .spawn(None, size, Some(shell), None, None)
        .unwrap();
    original
        .set_recv_timeout(Some(std::time::Duration::from_secs(15)))
        .unwrap();
    let mut ready = Vec::new();
    while !ready
        .windows(b"THEME_READY".len())
        .any(|part| part == b"THEME_READY")
    {
        if let DaemonMsg::Output(bytes) | DaemonMsg::Snapshot(bytes) = original.recv().unwrap() {
            ready.extend(bytes);
        }
    }
    let pane = original.pane_id();
    original.detach().unwrap();
    let mut observer = daemon.panes().observe(pane, size).unwrap();
    observer
        .set_recv_timeout(Some(std::time::Duration::from_millis(20)))
        .unwrap();
    let send = |serial: usize, bytes: &[u8], replies: usize| {
        daemon
            .panes()
            .send_input(
                pane,
                format!(
                    "{serial} {replies} {}{}",
                    graphics_pty_fixture::hex(bytes),
                    if cfg!(windows) { "\r" } else { "\n" }
                )
                .as_bytes(),
            )
            .unwrap();
    };
    std::fs::write(
        daemon.dir.path().join("appearance.json"),
        r#"{"dark":true}"#,
    )
    .unwrap();
    send(1, b"\x1b[?996n", 1);
    theme_wait_result(&mut observer, cx, 1, b"\x1b[?997;1n");
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
        gpui_kit::component::Theme::global_mut(cx).background = gpui_kit::white();
    });
    let (view, window) = graphics_build_rooted_consumer(cx, pane, daemon.stream(pane), Vec::new());
    // Historical queries are suppressed. GUI authority differs deliberately
    // from the daemon's cached dark appearance, proving who answers live data.
    send(2, b"\x1b[?996n", 1);
    theme_wait_result(&mut observer, cx, 2, b"\x1b[?997;2n");
    // Native ConPTY forwarded the query and the daemon wrote its reply, but
    // the Windows fixture did not observe application delivery. Do not claim
    // that platform's DA1 receipt from a renderer or writer-only assertion.
    if cfg!(unix) {
        send(10, b"\x1b[c", 1);
        theme_wait_result(&mut observer, cx, 10, b"\x1b[?6c");
    }
    send(3, b"\x1b[?99", usize::MAX);
    graphics_recovery_wait(cx, "attached partial query flushed", |_| {
        daemon.dir.path().join("theme-fragment-3").exists()
    });
    graphics_recovery_destroy(view, window, cx);
    send(4, b"6n", 1);
    theme_wait_result(&mut observer, cx, 4, b"\x1b[?997;1n");
    send(5, b"\x1b[?99", usize::MAX);
    graphics_recovery_wait(cx, "detached partial query flushed", |_| {
        daemon.dir.path().join("theme-fragment-5").exists()
    });
    let (view, window) = graphics_build_rooted_consumer(cx, pane, daemon.stream(pane), Vec::new());
    send(6, b"6n", 1);
    theme_wait_result(&mut observer, cx, 6, b"\x1b[?997;2n");
    // A subsequent live query is not swallowed by replay draining.
    send(7, b"\x1b[?996n", 1);
    theme_wait_result(&mut observer, cx, 7, b"\x1b[?997;2n");
    graphics_recovery_destroy(view, window, cx);
    send(8, b"\x1b[?996n", 1);
    theme_wait_result(&mut observer, cx, 8, b"\x1b[?997;1n");
    if cfg!(unix) {
        send(9, b"\x1b[c", 1);
        theme_wait_result(&mut observer, cx, 9, b"\x1b[?6c");
    }
    observer.detach().unwrap();
    daemon.panes().kill(pane).unwrap();
}
