// Real PTY -> daemon/ordered transport -> production TerminalView -> painted
// picture for the monochrome Glyph Protocol slice. The producer is the same
// fixture child the graphics consumers use; every byte here is real output and
// every reply is read back from the actual PTY input through the request's
// gate file. The glyph producer/paint helpers live in
// `graphics_pty_consumer_support.rs` so the native-font consumer reuses them.

#[gpui_kit::test]
fn glyph_consumer_real_pty_registers_queries_replaces_and_clears_a_monochrome_glyph(
    cx: &mut TestAppContext,
) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let (view, window) = graphics_rooted_consumer(cx, pane, daemon.stream(pane));
    let replies = tempfile::TempDir::new().unwrap();

    // The pane is real: a query for the target codepoint is answered by the
    // production consumer from its actual font fallback before any
    // registration exists. (The headless test text system reports a glyph for
    // every codepoint, so this only pins the reply path; the native consumer
    // checks real system coverage.)
    let reply = glyph_send(
        &view,
        cx,
        1,
        &replies.path().join("r1"),
        &glyph_query(GLYPH_CP),
        1,
    );
    assert_eq!(
        reply,
        format!("\x1b_25a1;q;cp={GLYPH_CP:x};status=system\x1b\\").into_bytes(),
        "query before registration answers the live font fallback"
    );

    // r -> accepted, and the daemon's reply reaches the real PTY input.
    let reply = glyph_send(
        &view,
        cx,
        2,
        &replies.path().join("r2"),
        &glyph_register(GLYPH_CP, &glyph_square(1000), 1),
        1,
    );
    assert_eq!(
        reply,
        format!("\x1b_25a1;r;cp={GLYPH_CP:x};status=0\x1b\\").into_bytes(),
        "registration accepted"
    );

    // q -> the same consumer now reports the accepted registry entry.
    let reply = glyph_send(
        &view,
        cx,
        3,
        &replies.path().join("r3"),
        &glyph_query(GLYPH_CP),
        1,
    );
    assert_eq!(
        reply,
        format!("\x1b_25a1;q;cp={GLYPH_CP:x};status=system,glossary\x1b\\").into_bytes(),
        "query reflects the accepted registry, not just parsed bytes"
    );

    // Display the codepoint in a real cell.
    let display = format!("\x1b[3;5H{cp}", cp = char::from_u32(GLYPH_CP).unwrap());
    graphics_send_fixture(&view, cx, 4, display.as_bytes());
    glyph_wait(cx, "glyph cell painted", |cx| {
        view.read_with(cx, |view, _| {
            view.cell_geom.is_some()
                && glyph_presented_row(view, 2).chars().nth(4) == Some('\u{100000}')
        })
    });

    let cell = glyph_cell_bounds(window, &view, cx, 2, 4);
    let first = glyph_paths_in_cell(window, &view, cx, cell);
    assert_eq!(
        first.len(),
        1,
        "one registered glyph path in its cell: {first:?}"
    );
    let expected_fg = view.read_with(cx, |view, _| {
        let cols = view.cell_geom.unwrap().cols;
        view.grid_buf[2 * cols + 4].fg
    });
    assert_eq!(
        first[0].1.as_solid(),
        Some(expected_fg),
        "monochrome glyph uses the cell's effective foreground"
    );

    // The raw codepoint is still what text extraction and the frame carry.
    assert!(
        view.read_with(cx, |view, _| view.scrollback_text().contains('\u{100000}')),
        "raw codepoint survives rendering"
    );

    // Replace with a wide rectangle: the painted picture must change.
    let reply = glyph_send(
        &view,
        cx,
        5,
        &replies.path().join("r5"),
        &glyph_register(GLYPH_CP, &glyph_rect(2000, 500), 1),
        1,
    );
    assert_eq!(
        reply,
        format!("\x1b_25a1;r;cp={GLYPH_CP:x};status=0\x1b\\").into_bytes(),
        "replacement accepted"
    );
    glyph_wait(cx, "replacement repainted", |cx| {
        glyph_paths_in_cell(window, &view, cx, cell)
            .first()
            .is_some_and(|(bounds, _)| *bounds != first[0].0)
    });
    let second = glyph_paths_in_cell(window, &view, cx, cell);
    assert_eq!(second.len(), 1, "replacement is one glyph path: {second:?}");
    assert_ne!(
        second[0].0, first[0].0,
        "replacement changes the painted extent"
    );

    // Clear: the derived picture falls back to ordinary font rendering.
    let reply = glyph_send(
        &view,
        cx,
        6,
        &replies.path().join("r6"),
        &glyph_clear(Some(GLYPH_CP)),
        1,
    );
    assert_eq!(
        reply,
        format!("\x1b_25a1;c;cp={GLYPH_CP:x};status=0\x1b\\").into_bytes(),
        "clear succeeds"
    );
    glyph_wait(cx, "clear drops the registered picture", |cx| {
        glyph_paths_in_cell(window, &view, cx, cell).is_empty()
    });

    // And the query says the glossary entry is gone.
    let reply = glyph_send(
        &view,
        cx,
        7,
        &replies.path().join("r7"),
        &glyph_query(GLYPH_CP),
        1,
    );
    assert_eq!(
        reply,
        format!("\x1b_25a1;q;cp={GLYPH_CP:x};status=system\x1b\\").into_bytes(),
        "query after clear no longer reports the glossary"
    );
}

// ---- Retained glyph authority across detach and fresh consumers -----------
//
// The producer below is a second real PTY child that emits exactly the bytes a
// test asks for. It deliberately does not emulate a terminal: the old-client
// regression reads raw daemon frames, and the recovery consumer only needs the
// original registration bytes to leave the ring before it attaches.

#[test]
#[ignore = "real PTY child for the retained-glyph consumer tests"]
fn glyph_consumer_fixture_sender() {
    if std::env::var_os("LYREL_PANE").is_none() {
        return;
    }
    use std::io::{Read as _, Write as _};
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    output.write_all(b"\r\nGLYPH_READY\r\n").unwrap();
    output.flush().unwrap();
    loop {
        let mut line = Vec::new();
        loop {
            let mut byte = [0u8];
            if input.read(&mut byte).unwrap() == 0 {
                return;
            }
            if byte[0] == b'\n' || (cfg!(windows) && byte[0] == b'\r') {
                break;
            }
            line.push(byte[0]);
        }
        let line = String::from_utf8_lossy(&line);
        let mut fields = line.split_whitespace();
        match fields.next() {
            Some("SEND") => {
                let bytes = graphics_pty_fixture::unhex(fields.next().expect("hex payload"));
                output.write_all(&bytes).unwrap();
                output.flush().unwrap();
            }
            Some("FLOOD") => {
                // Generated here so a ring-cap write never becomes a huge hex
                // command line; the count is exact and the producer exits after.
                let mut left: usize = fields.next().expect("flood bytes").parse().unwrap();
                let block = b"glyph replay filler\r\n";
                while left > 0 {
                    let take = block.len().min(left);
                    output.write_all(&block[..take]).unwrap();
                    left -= take;
                }
                output.flush().unwrap();
            }
            Some("EXIT") => return,
            // ConPTY may echo or split its own line endings; ignore them.
            _ => {}
        }
    }
}

fn glyph_fixture_shell() -> crate::daemon::protocol::ShellSpec {
    crate::daemon::protocol::ShellSpec {
        program: std::env::current_exe().unwrap().display().to_string(),
        args: vec![
            "--ignored".into(),
            "--exact".into(),
            format!(
                "{}::glyph_consumer_fixture_sender",
                module_path!().split_once("::").unwrap().1
            ),
            "--nocapture".into(),
            "--color=never".into(),
            "--test-threads=1".into(),
        ],
        args_are_lyrel_defaults: false,
    }
}

fn glyph_test_size() -> crate::daemon::protocol::WinSize {
    crate::daemon::protocol::WinSize {
        cols: 80,
        rows: 24,
        cell_w: 10,
        cell_h: 20,
    }
}

fn glyph_fixture_pane(daemon: &GraphicsDaemonProcess) -> u64 {
    let mut session = daemon
        .panes()
        .spawn(
            None,
            glyph_test_size(),
            Some(glyph_fixture_shell()),
            None,
            None,
        )
        .expect("spawn the glyph producer");
    session
        .set_recv_timeout(Some(std::time::Duration::from_secs(30)))
        .expect("bound the producer readiness read");
    let mut text = Vec::new();
    while !text
        .windows(b"GLYPH_READY".len())
        .any(|window| window == b"GLYPH_READY")
    {
        match session.recv().expect("the producer reports readiness") {
            crate::daemon::protocol::DaemonMsg::Snapshot(bytes)
            | crate::daemon::protocol::DaemonMsg::Output(bytes) => text.extend(bytes),
            crate::daemon::protocol::DaemonMsg::Error(error) => {
                panic!("glyph producer failed: {error}")
            }
            crate::daemon::protocol::DaemonMsg::Exited { code } => {
                panic!("glyph producer exited before readiness: {code:?}")
            }
            _ => {}
        }
    }
    let pane = session.pane_id();
    session.detach().expect("detach the spawn link");
    pane
}

fn glyph_fixture_write(daemon: &GraphicsDaemonProcess, pane: u64, bytes: &[u8]) {
    let request = format!(
        "SEND {}{}",
        graphics_pty_fixture::hex(bytes),
        if cfg!(windows) { "\r\n" } else { "\n" }
    );
    daemon
        .panes()
        .send_input(pane, request.as_bytes())
        .expect("the producer takes input");
}

/// A controller that never sends `SubscribeGlyphState` predates the retained
/// projection. The daemon must keep it attached and must not put a glyph frame
/// kind on its stream: its decoder has no arm for one.
#[test]
fn a_pane_client_that_did_not_opt_in_receives_no_glyph_kinds_and_stays_attached() {
    use crate::daemon::protocol::{ClientMsg, DaemonMsg, PROTOCOL_VERSION, read_frame};

    let daemon = GraphicsDaemonProcess::start();
    let pane = glyph_fixture_pane(&daemon);
    let mut stream =
        crate::daemon::transport::connect_endpoint_at(&daemon.endpoint("daemon")).unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(30)))
        .unwrap();

    ClientMsg::Negotiate {
        protocol: PROTOCOL_VERSION,
    }
    .encode(&mut stream)
    .unwrap();
    let version = match DaemonMsg::read(&mut stream).unwrap() {
        DaemonMsg::Version(version) => version,
        other => panic!("expected the daemon version, got {other:?}"),
    };
    // Deliberately no `SubscribeGlyphState`: this is an old client on the same
    // protocol version, which is exactly the peer that cannot decode kind 72
    // or 73.
    ClientMsg::GraphicsCapacity {
        encoded_bytes: 64 << 20,
        decoded_bytes: 128 << 20,
        offers: false,
    }
    .encode(&mut stream)
    .unwrap();
    ClientMsg::Attach {
        pane_id: pane,
        size: glyph_test_size(),
        allow_remote_clipboard_write: false,
    }
    .encode(&mut stream)
    .unwrap();

    let mut ready = false;
    while !ready {
        let (kind, payload) = read_frame(&mut stream).expect("the attach replay arrives");
        assert_ne!(kind, 72, "un-opted-in controller received GlyphQuery");
        assert_ne!(kind, 73, "un-opted-in consumer received GlyphState");
        ready = payload
            .windows(b"GLYPH_READY".len())
            .any(|window| window == b"GLYPH_READY");
    }

    glyph_fixture_write(&daemon, pane, b"\x1b_25a1;q;cp=100000\x1b\\");
    glyph_fixture_write(
        &daemon,
        pane,
        b"\x1b_25a1;r;cp=100000;fmt=glyf;reply=0;upm=1000;AAAA\x1b\\",
    );
    glyph_fixture_write(&daemon, pane, b"\x1b[1;1H\r\nGLYPH_MARK\r\n");

    let mut saw_mark = false;
    while !saw_mark {
        let (kind, payload) = read_frame(&mut stream).expect("the producer output arrives");
        assert_ne!(kind, 72, "un-opted-in controller received GlyphQuery");
        assert_ne!(kind, 73, "un-opted-in consumer received GlyphState");
        saw_mark = payload
            .windows(b"GLYPH_MARK".len())
            .any(|window| window == b"GLYPH_MARK");
    }

    // Still a live controller: a resize is acknowledged in stream order.
    ClientMsg::Resize(crate::daemon::protocol::WinSize {
        cols: 90,
        ..glyph_test_size()
    })
    .encode(&mut stream)
    .unwrap();
    loop {
        let (kind, _) = read_frame(&mut stream).expect("the resize echo arrives");
        if kind == 9 {
            break;
        }
    }
    assert!(
        version.has_feature("glyph-state"),
        "this daemon retains glyph state outside the text ring"
    );
    ClientMsg::Detach.encode(&mut stream).unwrap();
    daemon.panes().kill(pane).unwrap();
}

/// Registration bytes leave the 8 MiB ring, the producer stops, the consumer
/// detaches, and a fresh production consumer still paints the replacement.
#[gpui_kit::test]
fn glyph_state_outlives_the_replay_ring_for_a_fresh_production_consumer(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    let (view, window) = graphics_rooted_consumer(cx, pane, daemon.stream(pane));
    let replies = tempfile::TempDir::new().unwrap();

    let reply = glyph_send(
        &view,
        cx,
        1,
        &replies.path().join("r1"),
        &glyph_register(GLYPH_CP, &glyph_square(500), 1),
        1,
    );
    assert_eq!(
        reply,
        format!("\x1b_25a1;r;cp={GLYPH_CP:x};status=0\x1b\\").into_bytes()
    );
    let reply = glyph_send(
        &view,
        cx,
        2,
        &replies.path().join("r2"),
        &glyph_register(GLYPH_CP, &glyph_rect(2000, 500), 1),
        1,
    );
    assert_eq!(
        reply,
        format!("\x1b_25a1;r;cp={GLYPH_CP:x};status=0\x1b\\").into_bytes(),
        "the replacement is accepted"
    );

    // The producer writes a finite file once, then blocks on stdin. No glyph
    // byte is repeated after this point.
    let filler = daemon.dir.path().join("glyph-ring-trim.txt");
    std::fs::write(&filler, b"glyph replay filler\r\n".repeat(700_000)).unwrap();
    let request = format!(
        "FILE_SEND 3 0 {} 8192\n",
        graphics_pty_fixture::hex(filler.to_str().unwrap().as_bytes())
    );
    let input_bytes = request.len();
    view.read_with(cx, |view, _| view.terminal.write(request.into_bytes()));
    graphics_wait_fixture(&view, cx, 3, input_bytes);

    // Keep the codepoint on screen after the flood; its registration body is
    // long out of the ring now.
    let place = format!("\x1b[2J\x1b[3;5H{}", char::from_u32(GLYPH_CP).unwrap());
    glyph_send(
        &view,
        cx,
        4,
        &replies.path().join("place"),
        place.as_bytes(),
        0,
    );
    glyph_wait(cx, "the replacement paints before detach", |cx| {
        view.read_with(cx, |view, _| {
            view.cell_geom.is_some() && glyph_presented_row(view, 2).chars().nth(4) == Some('\u{100000}')
        })
    });
    let cell = glyph_cell_bounds(window, &view, cx, 2, 4);
    let live = glyph_paths_in_cell(window, &view, cx, cell);
    assert_eq!(live.len(), 1, "the live replacement paints one path");
    assert!(
        live[0].0.size.width.0 > live[0].0.size.height.0 * 1.5,
        "the wide replacement, not the first square: {:?}",
        live[0].0
    );

    graphics_recovery_destroy(view, window, cx);

    // Same daemon, same pane, no retransmission: only the retained authority
    // can supply this definition.
    let (fresh, fresh_window) = graphics_rooted_consumer(cx, pane, daemon.stream(pane));
    glyph_wait(cx, "the retained glyph paints for the fresh consumer", |cx| {
        fresh.read_with(cx, |view, _| {
            view.cell_geom.is_some() && glyph_presented_row(view, 2).chars().nth(4) == Some('\u{100000}')
        })
    });
    let cell = glyph_cell_bounds(fresh_window, &fresh, cx, 2, 4);
    let recovered = glyph_paths_in_cell(fresh_window, &fresh, cx, cell);
    assert_eq!(
        recovered.len(),
        1,
        "the retained registration paints without the producer resending"
    );
    assert!(
        recovered[0].0.size.width.0 > recovered[0].0.size.height.0 * 1.5,
        "the retained version is the replacement: {:?}",
        recovered[0].0
    );
    graphics_recovery_destroy(fresh, fresh_window, cx);
    daemon.panes().kill(pane).unwrap();
}

/// Clear and RIS are authoritative mutations: a fresh consumer must see the
/// cleared state even though the ring no longer holds either sequence.
#[gpui_kit::test]
fn glyph_clear_and_ris_reach_a_fresh_production_consumer_as_empty_state(
    cx: &mut TestAppContext,
) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    let (view, window) = graphics_rooted_consumer(cx, pane, daemon.stream(pane));
    let replies = tempfile::TempDir::new().unwrap();
    let filler = daemon.dir.path().join("glyph-clear-trim.txt");
    std::fs::write(&filler, b"glyph clear filler\r\n".repeat(700_000)).unwrap();

    let reply = glyph_send(
        &view,
        cx,
        1,
        &replies.path().join("r1"),
        &glyph_register(GLYPH_CP, &glyph_square(500), 1),
        1,
    );
    assert_eq!(
        reply,
        format!("\x1b_25a1;r;cp={GLYPH_CP:x};status=0\x1b\\").into_bytes()
    );
    let reply = glyph_send(
        &view,
        cx,
        2,
        &replies.path().join("c1"),
        &glyph_clear(Some(GLYPH_CP)),
        1,
    );
    assert_eq!(
        reply,
        format!("\x1b_25a1;c;cp={GLYPH_CP:x};status=0\x1b\\").into_bytes()
    );
    let request = format!(
        "FILE_SEND 3 0 {} 8192\n",
        graphics_pty_fixture::hex(filler.to_str().unwrap().as_bytes())
    );
    let input_bytes = request.len();
    view.read_with(cx, |view, _| view.terminal.write(request.into_bytes()));
    graphics_wait_fixture(&view, cx, 3, input_bytes);
    let place = format!("\x1b[2J\x1b[3;5H{}", char::from_u32(GLYPH_CP).unwrap());
    glyph_send(
        &view,
        cx,
        4,
        &replies.path().join("place"),
        place.as_bytes(),
        0,
    );
    glyph_wait(cx, "the cleared codepoint is on screen", |cx| {
        view.read_with(cx, |view, _| {
            view.cell_geom.is_some() && glyph_presented_row(view, 2).chars().nth(4) == Some('\u{100000}')
        })
    });
    graphics_recovery_destroy(view, window, cx);

    let (fresh, fresh_window) = graphics_rooted_consumer(cx, pane, daemon.stream(pane));
    glyph_wait(cx, "the cleared codepoint replays as text", |cx| {
        fresh.read_with(cx, |view, _| {
            view.cell_geom.is_some() && glyph_presented_row(view, 2).chars().nth(4) == Some('\u{100000}')
        })
    });
    let cell = glyph_cell_bounds(fresh_window, &fresh, cx, 2, 4);
    assert!(
        glyph_paths_in_cell(fresh_window, &fresh, cx, cell).is_empty(),
        "a cleared registration must not be revived by the ring or the projection"
    );

    // Now RIS, from a live registration, with its own eviction.
    let reply = glyph_send(
        &fresh,
        cx,
        5,
        &replies.path().join("r5"),
        &glyph_register(GLYPH_CP, &glyph_rect(2000, 500), 1),
        1,
    );
    assert_eq!(
        reply,
        format!("\x1b_25a1;r;cp={GLYPH_CP:x};status=0\x1b\\").into_bytes()
    );
    graphics_send_fixture(&fresh, cx, 6, b"\x1bc");
    let request = format!(
        "FILE_SEND 7 0 {} 8192\n",
        graphics_pty_fixture::hex(filler.to_str().unwrap().as_bytes())
    );
    let input_bytes = request.len();
    fresh.read_with(cx, |view, _| view.terminal.write(request.into_bytes()));
    graphics_wait_fixture(&fresh, cx, 7, input_bytes);
    let place = format!("\x1b[2J\x1b[3;5H{}", char::from_u32(GLYPH_CP).unwrap());
    glyph_send(
        &fresh,
        cx,
        8,
        &replies.path().join("place-ris"),
        place.as_bytes(),
        0,
    );
    glyph_wait(cx, "the RIS-cleared codepoint is on screen", |cx| {
        fresh.read_with(cx, |view, _| {
            view.cell_geom.is_some() && glyph_presented_row(view, 2).chars().nth(4) == Some('\u{100000}')
        })
    });
    graphics_recovery_destroy(fresh, fresh_window, cx);

    let (after, after_window) = graphics_rooted_consumer(cx, pane, daemon.stream(pane));
    glyph_wait(cx, "the RIS-cleared codepoint replays as text", |cx| {
        after.read_with(cx, |view, _| {
            view.cell_geom.is_some() && glyph_presented_row(view, 2).chars().nth(4) == Some('\u{100000}')
        })
    });
    let cell = glyph_cell_bounds(after_window, &after, cx, 2, 4);
    assert!(
        glyph_paths_in_cell(after_window, &after, cx, cell).is_empty(),
        "RIS clears the glossary at its stream position; no snapshot may restore it"
    );
    graphics_recovery_destroy(after, after_window, cx);
    daemon.panes().kill(pane).unwrap();
}

/// The shell outlives its producer; the producer itself exits. The marker is
/// shell output after that child returned, so no live producer is left to
/// resend a registration the daemon must have retained.
#[cfg(unix)]
#[gpui_kit::test]
fn glyph_state_survives_a_producer_that_truly_exits(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let daemon = GraphicsDaemonProcess::start();
    let mut spawn = daemon
        .panes()
        .spawn(
            None,
            glyph_test_size(),
            Some(crate::daemon::protocol::ShellSpec {
                program: "/bin/sh".into(),
                args: Vec::new(),
                args_are_lyrel_defaults: false,
            }),
            None,
            None,
        )
        .expect("spawn an interactive shell");
    spawn
        .set_recv_timeout(Some(std::time::Duration::from_secs(30)))
        .expect("bound the shell readiness read");
    let pane = spawn.pane_id();
    spawn.detach().expect("detach the spawn link");

    // A plain shell has no fixture protocol; build the rooted consumer without
    // the graphics fixture's initial SEND.
    let (view, window) = graphics_build_rooted_consumer(cx, pane, daemon.stream(pane), Vec::new());
    graphics_assert_production_paint_at(window, &view, cx, TermSize::new(80, 24));

    let place = format!("\x1b[2J\x1b[3;5H{}", char::from_u32(GLYPH_CP).unwrap());
    let marker = format!("GLYPH_PRODUCER_EXITED_{}", std::process::id());
    let exe = std::env::current_exe().unwrap().display().to_string();
    let target = format!(
        "{}::glyph_consumer_fixture_sender",
        module_path!().split_once("::").unwrap().1
    );
    let mut command = format!(
        "{} --ignored --exact {} --nocapture --test-threads=1 <<'LYREL_GLYPH_EOF'\n",
        glyph_shell_quote(&exe),
        glyph_shell_quote(&target)
    );
    command.push_str(&format!(
        "SEND {}\n",
        graphics_pty_fixture::hex(&glyph_register(GLYPH_CP, &glyph_rect(2000, 500), 0))
    ));
    command.push_str("FLOOD 9437184\n");
    command.push_str(&format!(
        "SEND {}\n",
        graphics_pty_fixture::hex(place.as_bytes())
    ));
    command.push_str("LYREL_GLYPH_EOF\n");
    command.push_str(&format!("echo {marker}\n"));
    daemon
        .panes()
        .send_input(pane, command.as_bytes())
        .expect("the shell takes the producer command");

    glyph_wait(cx, "the producer really exited", |cx| {
        view.read_with(cx, |view, _| view.scrollback_text().contains(&marker))
    });
    glyph_wait(cx, "the exited producer's screen is visible", |cx| {
        view.read_with(cx, |view, _| {
            view.cell_geom.is_some()
                && glyph_presented_row(view, 2).chars().nth(4) == Some('\u{100000}')
        })
    });
    let cell = glyph_cell_bounds(window, &view, cx, 2, 4);
    let live = glyph_paths_in_cell(window, &view, cx, cell);
    assert_eq!(live.len(), 1, "the replacement painted before the exit");
    assert!(
        live[0].0.size.width.0 > live[0].0.size.height.0 * 1.5,
        "the replacement, not the first square: {:?}",
        live[0].0
    );

    graphics_recovery_destroy(view, window, cx);
    let (fresh, fresh_window) = graphics_build_rooted_consumer(cx, pane, daemon.stream(pane), Vec::new());
    graphics_assert_production_paint_at(fresh_window, &fresh, cx, TermSize::new(80, 24));
    glyph_wait(cx, "the retained glyph survives the producer exit", |cx| {
        fresh.read_with(cx, |view, _| {
            view.cell_geom.is_some()
                && glyph_presented_row(view, 2).chars().nth(4) == Some('\u{100000}')
        })
    });
    let cell = glyph_cell_bounds(fresh_window, &fresh, cx, 2, 4);
    let recovered = glyph_paths_in_cell(fresh_window, &fresh, cx, cell);
    assert_eq!(
        recovered.len(),
        1,
        "no producer remains to resend the registration"
    );
    assert!(
        recovered[0].0.size.width.0 > recovered[0].0.size.height.0 * 1.5,
        "the retained replacement, not the first square: {:?}",
        recovered[0].0
    );
    graphics_recovery_destroy(fresh, fresh_window, cx);
    daemon.panes().kill(pane).unwrap();
}

#[cfg(unix)]
fn glyph_shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}
