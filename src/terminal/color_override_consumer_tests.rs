// Real PTY -> daemon -> production `TerminalView` pane-colour consumers.
//
// These use the shared theme fixture (a real PTY application that sends
// arbitrary OSC bytes and reports the replies it received) and the production
// rooted terminal consumer. Assertions read the committed frame's resolved
// cells and the wire replies, never a private map.

fn color_override(
    daemon: &GraphicsDaemonProcess,
    pane: u64,
) -> lyrel_core::core::pane_colors::PaneColors {
    let mut colors = lyrel_core::core::pane_colors::PaneColors::default();
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
        .set_recv_timeout(Some(std::time::Duration::from_secs(15)))
        .unwrap();
    let mut text = Vec::new();
    loop {
        match observer.recv() {
            Ok(DaemonMsg::Snapshot(bytes) | DaemonMsg::Output(bytes)) => text.extend(bytes),
            Ok(DaemonMsg::Colors(frame)) => {
                let frame = lyrel_core::core::pane_colors::ColorFrame::decode(&frame)
                    .expect("decodable colour frame");
                assert!(frame.is_snapshot(), "an attach must carry a full snapshot");
                frame.apply(&mut colors);
                break;
            }
            Ok(_) => {}
            Err(e) => panic!("colour replay did not arrive: {e}"),
        }
    }
    assert!(
        !text
            .windows(b"\x1b]4;1;#445566".len())
            .any(|window| window == b"\x1b]4;1;#445566"),
        "the original set must have left the replay ring; the state below came from the projection"
    );
    let _ = observer.detach();
    colors
}

#[gpui_kit::test]
fn color_overrides_change_production_paint_and_queries(cx: &mut TestAppContext) {
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

    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
        crate::ui::theme::apply_theme(None, cx);
    });
    let (view, window) = graphics_build_rooted_consumer(cx, pane, daemon.stream(pane), Vec::new());

    // A one-shot program sets the pane's indexed/default colours and exits.
    // Nothing about that exit may clear them.
    theme_fixture_send(
        &daemon,
        pane,
        1,
        b"\x1b]4;1;#112233\x07\x1b]10;#0a0b0c\x07\x1b]11;#101112\x07\x1b]12;#131415\x07",
        0,
    );
    theme_wait_result(&mut observer, cx, 1, b"");
    graphics_recovery_wait(cx, "pane overrides applied", |cx| {
        view.read_with(cx, |view, _| {
            let colors = *view
                .terminal
                .color_overrides
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            colors.get(1) == Some([0x11, 0x22, 0x33])
                && colors.get(lyrel_core::core::pane_colors::DEFAULT_FG)
                    == Some([0x0a, 0x0b, 0x0c])
        })
    });

    // Probe replies are the effective values, including the named defaults.
    theme_fixture_send(
        &daemon,
        pane,
        2,
        b"\x1b]4;1;?\x07\x1b]10;?\x07\x1b]11;?\x07\x1b]12;?\x07",
        4,
    );
    theme_wait_result(
        &mut observer,
        cx,
        2,
        b"\x1b]4;1;rgb:1111/2222/3333\x07\x1b]10;rgb:0a0a/0b0b/0c0c\x07\x1b]11;rgb:1010/1111/1212\x07\x1b]12;rgb:1313/1414/1515\x07",
    );

    // The committed frame resolves ANSI 1, indexed 1, default fg and default
    // bg through the same values the probes returned.
    theme_fixture_send(
        &daemon,
        pane,
        3,
        b"\x1b[5;5H\x1b[31mO\x1b[0m\x1b[6;5H\x1b[38;5;1mP\x1b[0m\x1b[7;5H\x1b[39mQ\x1b[0m\x1b[8;5H\x1b[49mR",
        0,
    );
    theme_wait_result(&mut observer, cx, 3, b"");
    graphics_assert_production_paint(window, &view, cx);
    view.read_with(cx, |view, _| {
        let cols = view.cell_geom.expect("production frame").cols;
        let rgb = crate::terminal::palette::hsla_to_rgb;
        let ansi1 = rgb(view.grid_buf[4 * cols + 4].fg);
        let indexed1 = rgb(view.grid_buf[5 * cols + 4].fg);
        let default_fg = rgb(view.grid_buf[6 * cols + 4].fg);
        let default_bg = rgb(view.grid_buf[7 * cols + 4].bg);
        assert_eq!((ansi1.r, ansi1.g, ansi1.b), (0x11, 0x22, 0x33));
        assert_eq!((indexed1.r, indexed1.g, indexed1.b), (0x11, 0x22, 0x33));
        assert_eq!(
            (default_fg.r, default_fg.g, default_fg.b),
            (0x0a, 0x0b, 0x0c)
        );
        assert_eq!(
            (default_bg.r, default_bg.g, default_bg.b),
            (0x10, 0x11, 0x12)
        );
    });

    // A theme switch updates only the slots the pane did not override.
    cx.update(|cx| {
        let mut config = cx.global::<Config>().clone();
        config.theme_preset = "dracula".into();
        cx.set_global(config);
        crate::ui::theme::apply_theme(None, cx);
    });
    theme_fixture_send(
        &daemon,
        pane,
        4,
        b"\x1b]4;2;?\x07\x1b]4;1;?\x07\x1b]10;?\x07\x1b]11;?\x07",
        4,
    );
    theme_wait_result(
        &mut observer,
        cx,
        4,
        b"\x1b]4;2;rgb:5050/fafa/7b7b\x07\x1b]4;1;rgb:1111/2222/3333\x07\x1b]10;rgb:0a0a/0b0b/0c0c\x07\x1b]11;rgb:1010/1111/1212\x07",
    );

    // Reset each layer: indexed, foreground, background, cursor. Queries now
    // reveal the current (dracula) theme, not the light theme at set time.
    theme_fixture_send(
        &daemon,
        pane,
        5,
        b"\x1b]104;1\x07\x1b]110\x07\x1b]111\x07\x1b]112\x07",
        0,
    );
    theme_wait_result(&mut observer, cx, 5, b"");
    theme_fixture_send(
        &daemon,
        pane,
        6,
        b"\x1b]4;1;?\x07\x1b]10;?\x07\x1b]11;?\x07\x1b]12;?\x07",
        4,
    );
    theme_wait_result(
        &mut observer,
        cx,
        6,
        b"\x1b]4;1;rgb:ffff/5555/5555\x07\x1b]10;rgb:f8f8/f8f8/f2f2\x07\x1b]11;rgb:2828/2a2a/3636\x07\x1b]12;rgb:ffff/7979/c6c6\x07",
    );
    theme_fixture_send(
        &daemon,
        pane,
        7,
        b"\x1b[10;5H\x1b[31mD\x1b[0m\x1b[11;5H\x1b[39mE\x1b[0m",
        0,
    );
    theme_wait_result(&mut observer, cx, 7, b"");
    graphics_assert_production_paint(window, &view, cx);
    view.read_with(cx, |view, app| {
        let cols = view.cell_geom.expect("production frame").cols;
        let rgb = crate::terminal::palette::hsla_to_rgb;
        let reset_red = rgb(view.grid_buf[9 * cols + 4].fg);
        let reset_fg = rgb(view.grid_buf[10 * cols + 4].fg);
        assert_eq!((reset_red.r, reset_red.g, reset_red.b), (0xff, 0x55, 0x55));
        assert_eq!((reset_fg.r, reset_fg.g, reset_fg.b), (0xf8, 0xf8, 0xf2));
        assert_eq!(
            app.global::<Config>().theme_preset,
            "dracula",
            "pane colour commands never mutate user configuration"
        );
    });

    // A second pane on the same daemon starts with no inherited overrides.
    let mut second = daemon
        .panes()
        .spawn(None, size, Some(theme_fixture_shell()), None, None)
        .unwrap();
    second
        .set_recv_timeout(Some(std::time::Duration::from_secs(15)))
        .unwrap();
    let second_id = second.pane_id();
    let mut inherited = None;
    while inherited.is_none() {
        match second.recv().unwrap() {
            DaemonMsg::Colors(frame) => {
                let frame = lyrel_core::core::pane_colors::ColorFrame::decode(&frame).unwrap();
                let mut colors = lyrel_core::core::pane_colors::PaneColors::default();
                frame.apply(&mut colors);
                inherited = Some(colors);
            }
            DaemonMsg::Exited { code } => panic!("second pane exited: {code:?}"),
            _ => {}
        }
    }
    assert!(
        inherited.unwrap().is_empty(),
        "a new pane must not inherit another pane's temporary colours"
    );
    let _ = second.detach();
    let _ = daemon.panes().kill(second_id);

    // Held frame: a set and its query inside BSU answer correctly but do not
    // publish the unfinished frame; ending the hold paints it whole.
    theme_fixture_send(
        &daemon,
        pane,
        8,
        b"\x1b[?2026h\x1b[13;1Hheld\x1b]4;1;?\x07",
        1,
    );
    theme_wait_result(&mut observer, cx, 8, b"\x1b]4;1;rgb:ffff/5555/5555\x07");
    window
        .update(cx, |_, window, cx| {
            window.refresh();
            window.render_frame(cx);
        })
        .unwrap();
    view.read_with(cx, |view, _| {
        assert!(
            !sync_presented_row(view, 12).contains("held"),
            "a colour reply must not publish an unfinished synchronized frame"
        );
    });
    theme_fixture_send(&daemon, pane, 9, b"\x1b[?2026l", 0);
    theme_wait_result(&mut observer, cx, 9, b"");
    graphics_assert_production_paint(window, &view, cx);
    assert!(
        view.read_with(cx, |view, _| sync_presented_row(view, 12).starts_with("held")),
        "the finished frame paints whole"
    );

    graphics_recovery_destroy(view, window, cx);
    observer.detach().unwrap();
    daemon.panes().kill(pane).unwrap();
}

#[gpui_kit::test]
fn color_state_outlives_the_replay_ring_for_a_fresh_consumer(cx: &mut TestAppContext) {
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
    theme_fixture_send(
        &daemon,
        pane,
        1,
        b"\x1b]4;1;#445566\x07\x1b]10;#0d0e0f\x07",
        0,
    );
    theme_wait_result(&mut observer, cx, 1, b"");
    observer.detach().unwrap();

    // Force the original set bytes out of the real 8 MiB replay ring.
    let mut monitor = daemon.panes().observe(pane, size).unwrap();
    monitor
        .set_recv_timeout(Some(std::time::Duration::from_secs(180)))
        .unwrap();
    let monitor_thread = std::thread::spawn(move || {
        let mut bytes_seen = 0usize;
        loop {
            match monitor.recv().unwrap() {
                DaemonMsg::Snapshot(bytes) | DaemonMsg::Output(bytes) => {
                    bytes_seen += bytes.len();
                    if bytes
                        .windows(b"RING_COLOR_DONE".len())
                        .any(|w| w == b"RING_COLOR_DONE")
                    {
                        break;
                    }
                }
                DaemonMsg::Error(error) => panic!("ring monitor failed: {error}"),
                DaemonMsg::Exited { code } => panic!("ring sender exited: {code:?}"),
                _ => {}
            }
        }
        let _ = monitor.detach();
        bytes_seen
    });
    let mut row = b"\x1b[0m".repeat(250);
    row.extend_from_slice(b"\r\nCOLOR_ROW_00000\r\n");
    let line = format!(
        "2 0 {} 10500{}",
        graphics_pty_fixture::hex(&row),
        if cfg!(windows) { "\r" } else { "\n" }
    );
    daemon.panes().send_input(pane, line.as_bytes()).unwrap();
    theme_fixture_send(&daemon, pane, 999, b"RING_COLOR_DONE", 0);
    let pty_bytes = monitor_thread.join().unwrap();
    assert!(
        pty_bytes > 8 * 1024 * 1024,
        "actual daemon Snapshot/Output bytes must exceed the ring cap: {pty_bytes}"
    );

    // The replay no longer carries the original OSC, but the decoded layer is
    // still there; a fresh production consumer needs no resend.
    let replay_colors = color_override(&daemon, pane);
    assert_eq!(replay_colors.get(1), Some([0x44, 0x55, 0x66]));
    assert_eq!(
        replay_colors.get(lyrel_core::core::pane_colors::DEFAULT_FG),
        Some([0x0d, 0x0e, 0x0f])
    );

    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
        crate::ui::theme::apply_theme(None, cx);
    });
    let (view, window) = graphics_build_rooted_consumer(cx, pane, daemon.stream(pane), Vec::new());
    let mut query_observer = daemon.panes().observe(pane, size).unwrap();
    query_observer
        .set_recv_timeout(Some(std::time::Duration::from_millis(20)))
        .unwrap();
    theme_fixture_send(&daemon, pane, 1000, b"\x1b]4;1;?\x07\x1b]10;?\x07", 2);
    theme_wait_result(
        &mut query_observer,
        cx,
        1000,
        b"\x1b]4;1;rgb:4444/5555/6666\x07\x1b]10;rgb:0d0d/0e0e/0f0f\x07",
    );
    graphics_recovery_destroy(view, window, cx);
    query_observer.detach().unwrap();
    daemon.panes().kill(pane).unwrap();
}
