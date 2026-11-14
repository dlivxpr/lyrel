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
    let mut original = daemon
        .panes()
        .spawn(None, size, Some(theme_fixture_shell()), None, None)
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
        theme_fixture_send(&daemon, pane, serial, bytes, replies);
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
        theme_wait_result(&mut observer, cx, 10, b"\x1b[?4;6c");
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
        theme_wait_result(&mut observer, cx, 9, b"\x1b[?4;6c");
    }
    observer.detach().unwrap();
    daemon.panes().kill(pane).unwrap();
}

fn theme_fixture_shell() -> crate::daemon::protocol::ShellSpec {
    crate::daemon::protocol::ShellSpec {
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
    }
}

fn theme_fixture_send(
    daemon: &GraphicsDaemonProcess,
    pane: u64,
    serial: usize,
    bytes: &[u8],
    replies: usize,
) {
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
}

/// A program asking OSC 4/10/11/12 must hear the colours the pane actually
/// paints: the active theme's ANSI table and defaults, not the fixed palette
/// the emulator was constructed with. The whole path is real — PTY child,
/// daemon, ordered stream, production `TerminalView` — and the painted cells
/// are checked from the committed frame, not the wire answer alone.
#[gpui_kit::test]
fn color_queries_answer_the_pane_effective_palette(cx: &mut TestAppContext) {
    use gpui_kit::test::TestWindowExt as _;
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let daemon = GraphicsDaemonProcess::start();
    let size = crate::daemon::protocol::WinSize {
        cols: 80,
        rows: 24,
        cell_w: 10,
        cell_h: 20,
    };
    let mut original = daemon
        .panes()
        .spawn(None, size, Some(theme_fixture_shell()), None, None)
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

    // A detached pane has no renderer to answer a colour query; unlike DA1 /
    // CSI ? 996 n the daemon does not fabricate one. `replies = 0` still ends
    // on the APC barrier, so anything answered early would show up in-band.
    theme_fixture_send(&daemon, pane, 1, b"\x1b]4;1;?\x07", 0);
    theme_wait_result(&mut observer, cx, 1, b"");

    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    // The default preset is `light`: its authored ANSI 1 differs from the
    // dark table the pane was constructed with, which is exactly the
    // query-versus-paint divergence this fixes.
    cx.update(|cx| crate::ui::theme::apply_theme(None, cx));
    let (view, window) = graphics_build_rooted_consumer(cx, pane, daemon.stream(pane), Vec::new());

    // A replayed query must not leak a reply into the live pane, and the
    // first live answer afterwards must be the new query's, not a stale one.
    theme_fixture_send(&daemon, pane, 2, b"\x1b]4;3;?\x07", 1);
    theme_wait_result(&mut observer, cx, 2, b"\x1b]4;3;rgb:9a9a/6767/0000\x07");

    // Indexed, default foreground/background and cursor colours answer from
    // the same effective values the cells paint.
    theme_fixture_send(&daemon, pane, 3, b"\x1b]4;1;?\x07", 1);
    theme_wait_result(&mut observer, cx, 3, b"\x1b]4;1;rgb:d1d1/2424/2f2f\x07");
    theme_fixture_send(&daemon, pane, 4, b"\x1b]4;9;?\x1b\\", 1);
    theme_wait_result(&mut observer, cx, 4, b"\x1b]4;9;rgb:cfcf/2222/2e2e\x1b\\");
    theme_fixture_send(&daemon, pane, 5, b"\x1b]4;200;?\x07", 1);
    theme_wait_result(&mut observer, cx, 5, b"\x1b]4;200;rgb:ffff/0000/d7d7\x07");
    theme_fixture_send(
        &daemon,
        pane,
        6,
        b"\x1b]10;?\x07\x1b]11;?\x07\x1b]12;?\x07",
        3,
    );
    theme_wait_result(&mut observer, cx, 6, b"\x1b]10;rgb:1c1c/1c1c/1e1e\x07\x1b]11;rgb:fcfc/fcfc/fbfb\x07\x1b]12;rgb:1f1f/6b6b/f0f0\x07");

    // The committed frame resolves the same index through the same table:
    // 'R' painted in ANSI 1 carries the theme red, and the queried reply
    // above is its byte value.
    theme_fixture_send(
        &daemon,
        pane,
        7,
        b"\x1b[10;5H\x1b[31mR\x1b[0m\x1b[11;5HX",
        0,
    );
    theme_wait_result(&mut observer, cx, 7, b"");
    graphics_assert_production_paint(window, &view, cx);
    let (red, default_fg, default_bg) = view.read_with(cx, |view, _| {
        let cols = view.cell_geom.expect("production frame").cols;
        assert_eq!(view.grid_buf[9 * cols + 4].c, 'R');
        assert_eq!(view.grid_buf[10 * cols + 4].c, 'X');
        (
            crate::terminal::palette::hsla_to_rgb(view.grid_buf[9 * cols + 4].fg),
            crate::terminal::palette::hsla_to_rgb(view.grid_buf[10 * cols + 4].fg),
            crate::terminal::palette::hsla_to_rgb(view.grid_buf[10 * cols + 4].bg),
        )
    });
    assert_eq!(
        (red.r, red.g, red.b),
        (0xd1, 0x24, 0x2f),
        "painted ANSI 1 must be the queried colour"
    );
    assert_eq!(
        (default_fg.r, default_fg.g, default_fg.b),
        (0x1c, 0x1c, 0x1e),
        "painted default foreground must be the queried colour"
    );
    assert_eq!(
        (default_bg.r, default_bg.g, default_bg.b),
        (0xfc, 0xfc, 0xfb),
        "painted default background must be the queried colour"
    );

    // A query landing mid-frame is answered, but the unfinished frame stays
    // held: the reply cannot publish it early.
    theme_fixture_send(
        &daemon,
        pane,
        8,
        b"\x1b[?2026h\x1b[13;1Hheld\x1b]4;1;?\x07",
        1,
    );
    // Observe BSU before pumping the UI/PTY reply round trip. Only this
    // atomicity test extends the fail-safe deadline; production timeout
    // behavior remains covered by the synchronized-output consumers.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let parsed = view.read_with(cx, |view, _| {
            let mut term = view.terminal.term.lock();
            if term.grid[Line(12)][Column(0)].c() != 'h' {
                return false;
            }
            assert!(term.synchronized_output(), "query parsed inside BSU");
            term.sync_update_started =
                Some(rio_vt::time::Instant::now() + std::time::Duration::from_secs(30));
            true
        });
        if parsed {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "BSU output not parsed"
        );
        std::thread::yield_now();
    }
    theme_wait_result(&mut observer, cx, 8, b"\x1b]4;1;rgb:d1d1/2424/2f2f\x07");
    window
        .update(cx, |_, window, cx| {
            window.refresh();
            window.render_frame(cx);
        })
        .unwrap();
    view.read_with(cx, |view, _| {
        assert!(
            !sync_presented_row(view, 12).contains("held"),
            "a colour reply must not publish an unfinished frame"
        );
    });
    theme_fixture_send(&daemon, pane, 10, b"\x1b[?2026l", 0);
    theme_wait_result(&mut observer, cx, 10, b"");
    graphics_assert_production_paint(window, &view, cx);
    assert!(
        view.read_with(cx, |view, _| sync_presented_row(view, 12)
            .starts_with("held")),
        "the finished frame paints whole"
    );

    // Switching the application preset re-derives both sides coherently:
    // painted cells and query answers move to the new theme's ANSI table and
    // defaults together.
    cx.update(|cx| {
        let mut config = cx.global::<Config>().clone();
        config.theme_preset = "dracula".into();
        cx.set_global(config);
        crate::ui::theme::apply_theme(None, cx);
    });
    theme_fixture_send(
        &daemon,
        pane,
        11,
        b"\x1b]4;1;?\x07\x1b]10;?\x07\x1b]11;?\x07\x1b]12;?\x07",
        4,
    );
    theme_wait_result(&mut observer, cx, 11, b"\x1b]4;1;rgb:ffff/5555/5555\x07\x1b]10;rgb:f8f8/f8f8/f2f2\x07\x1b]11;rgb:2828/2a2a/3636\x07\x1b]12;rgb:ffff/7979/c6c6\x07");
    theme_fixture_send(&daemon, pane, 12, b"\x1b[14;5H\x1b[31mD\x1b[0m", 0);
    theme_wait_result(&mut observer, cx, 12, b"");
    graphics_assert_production_paint(window, &view, cx);
    let (dracula, default_fg, default_bg) = view.read_with(cx, |view, _| {
        let cols = view.cell_geom.expect("production frame").cols;
        assert_eq!(view.grid_buf[13 * cols + 4].c, 'D');
        (
            crate::terminal::palette::hsla_to_rgb(view.grid_buf[13 * cols + 4].fg),
            crate::terminal::palette::hsla_to_rgb(view.grid_buf[10 * cols + 4].fg),
            crate::terminal::palette::hsla_to_rgb(view.grid_buf[10 * cols + 4].bg),
        )
    });
    assert_eq!(
        (dracula.r, dracula.g, dracula.b),
        (0xff, 0x55, 0x55),
        "painted ANSI 1 follows the new theme"
    );
    assert_eq!(
        (default_fg.r, default_fg.g, default_fg.b),
        (0xf8, 0xf8, 0xf2)
    );
    assert_eq!(
        (default_bg.r, default_bg.g, default_bg.b),
        (0x28, 0x2a, 0x36)
    );

    // Detach, then ask while nothing renders: replaying that query to the
    // next consumer is suppressed, so the first live reply afterwards is the
    // new query's — never a historical answer injected into the pane.
    graphics_recovery_destroy(view, window, cx);
    theme_fixture_send(&daemon, pane, 13, b"\x1b]4;2;?\x07", 0);
    theme_wait_result(&mut observer, cx, 13, b"");
    let (view, window) = graphics_build_rooted_consumer(cx, pane, daemon.stream(pane), Vec::new());
    theme_fixture_send(&daemon, pane, 14, b"\x1b]4;2;?\x07", 1);
    theme_wait_result(&mut observer, cx, 14, b"\x1b]4;2;rgb:5050/fafa/7b7b\x07");

    graphics_recovery_destroy(view, window, cx);
    observer.detach().unwrap();
    daemon.panes().kill(pane).unwrap();
}
