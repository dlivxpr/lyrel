// #275: static Sixel discovery, restoration and refusal through the production
// seam — real PTY producer -> daemon authority -> ordered pane stream -> rooted
// TerminalView / TerminalElement paint. Decoder and paint contracts here are
// not native-window, GPU or SSH evidence.

#[track_caller]
fn sixel_reply_bytes(text: &str, serial: usize) -> Option<Vec<u8>> {
    let prefix = format!("KITTY_REPLY {serial} ");
    text.lines().find_map(|line| {
        line.trim()
            .strip_prefix(prefix.as_str())
            .map(graphics_pty_fixture::unhex)
    })
}

fn sixel_exchange(
    view: &Entity<TerminalView>,
    observer: &mut lyrel_core::client::PaneSession,
    cx: &mut TestAppContext,
    serial: usize,
    bytes: &[u8],
) -> Vec<u8> {
    let request = format!("SEND {serial} 1 {} 0\n", graphics_pty_fixture::hex(bytes));
    let input_bytes = request.len();
    view.read_with(cx, |view, _| view.terminal.write(request.into_bytes()));
    // The fixture cannot finish until the real reply reaches its stdin, so the
    // view must be pumped first; only then is the echoed reply observable.
    graphics_wait_fixture(view, cx, serial, input_bytes);
    let marker = format!("KITTY_DONE_{serial};");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut output = Vec::new();
    loop {
        match observer.recv() {
            Ok(DaemonMsg::Output(bytes) | DaemonMsg::Snapshot(bytes)) => output.extend(bytes),
            Ok(DaemonMsg::Exited { code }) => panic!("sixel application exited: {code:?}"),
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => panic!("sixel observer: {error}"),
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
    sixel_reply_bytes(&String::from_utf8_lossy(&output), serial)
        .unwrap_or_else(|| panic!("serial {serial} produced no application reply"))
}

fn sixel_screen_text(view: &TerminalView) -> String {
    use rio_vt::crosswords::pos::{Column, Pos};
    let term = view.terminal.term.lock();
    (0..term.screen_lines())
        .map(|row| {
            (0..term.columns())
                .flat_map(|col| term.grid.cell_text(Pos::new(Line(row as i32), Column(col))))
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn sixel_screen_row(view: &TerminalView, needle: &str) -> Option<usize> {
    use rio_vt::crosswords::pos::{Column, Pos};
    let term = view.terminal.term.lock();
    (0..term.screen_lines()).find(|&row| {
        let text: String = (0..term.columns())
            .flat_map(|col| term.grid.cell_text(Pos::new(Line(row as i32), Column(col))))
            .collect();
        text.contains(needle)
    })
}

fn sixel_send_file(
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    serial: usize,
    path: &std::path::Path,
) {
    let request = format!(
        "FILE_SEND {serial} 0 {} 65536\n",
        graphics_pty_fixture::hex(path.to_str().unwrap().as_bytes())
    );
    let input_bytes = request.len();
    view.read_with(cx, |view, _| view.terminal.write(request.into_bytes()));
    graphics_wait_fixture(view, cx, serial, input_bytes);
}

/// 250 SGR resets per line consume replay-ring bytes without occupying cells,
/// so more than the production 8 MiB ring is trimmed while fewer than the
/// daemon's 10,000 retained text rows are used.
fn sixel_ring_line() -> Vec<u8> {
    let mut line = b"\x1b[0m".repeat(250);
    line.extend_from_slice(b"\r\n");
    line
}

#[gpui_kit::test]
fn sixel_discovery_reports_the_negotiated_endpoint_and_never_a_mirror_guess(
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
    // XTSMGRAPHICS is answered by the daemon that owns pixel geometry and
    // admission bounds, in request order, before the following `CSI 14t`.
    // The current geometry must be exactly the text area that same pane
    // reports, so the assertion derives it from the `14t` reply instead of
    // assuming the window's final cell grid.
    let replies = sixel_exchange(
        &view,
        &mut observer,
        cx,
        1,
        b"\x1b[?1;1;0S\x1b[?2;1;0S\x1b[?2;4S\x1b[?3;1;0S\x1b[14t",
    );
    let at = replies
        .windows(4)
        .position(|part| part == b"\x1b[4;")
        .expect("the pane answered its text-area query");
    let text_area = &replies[at..];
    let fields: Vec<&[u8]> = text_area[4..text_area.len() - 1]
        .split(|byte| *byte == b';')
        .collect();
    let (height, width) = (
        std::str::from_utf8(fields[0]).unwrap(),
        std::str::from_utf8(fields[1]).unwrap(),
    );
    let mut expected = format!(
        "\x1b[?1;0;1024S\x1b[?2;0;{width};{height}S\x1b[?2;3S\x1b[?3;1S"
    )
    .into_bytes();
    expected.extend_from_slice(text_area);
    assert_eq!(
        replies, expected,
        "registers, current geometry from this pane, denied maximum/set, ReGIS, then 14t"
    );
    graphics_assert_production_paint(window, &view, cx);
    // Windows ConPTY consumes an application's DA1 query before the pane sees
    // it (the same limitation `theme_query_consumer_tests` records, which is
    // why its DA1 case is Unix-only). The attached client's answer is therefore
    // observed on a raw Unix pty; the endpoint gating itself is deterministically
    // pinned by the `ParseBridge` test in `bridge.rs`.
    #[cfg(unix)]
    assert_eq!(
        sixel_exchange(&view, &mut observer, cx, 2, b"\x1b[c"),
        b"\x1b[?4;6c"
    );
    graphics_recovery_destroy(view, window, cx);
    observer.detach().unwrap();
    daemon.panes().kill(pane).unwrap();
}

#[gpui_kit::test]
fn sixel_fresh_consumer_restores_the_accepted_image_after_the_ring_trims(
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
    let (prior, prior_window) = graphics_recovery_fresh(&daemon, pane, cx, 200);
    let mut bytes = b"\x1b[2J\x1b[3J\x1b[6;1HEARLY_SIXEL\r".to_vec();
    bytes.extend_from_slice(SIXEL_PRODUCER_OUTPUT);
    bytes.extend_from_slice(b"AFTER_EARLY");
    graphics_send_fixture(&prior, cx, 201, &bytes);
    graphics_recovery_wait(cx, "early Sixel decoded by the production consumer", |cx| {
        prior.read_with(cx, |view, _| {
            graphics_visible_images(view)
                .iter()
                .any(|image| image.placement.z == -1 && image.data.is_some())
        })
    });
    // The producer's transmission is extracted before the text ring, so the
    // ring never holds the DCS at all; this load then trims the early text row
    // out of the replay the way any long output trims older bytes.
    let path = std::env::temp_dir().join(format!("lyrel-sixel-ring-{}.txt", std::process::id()));
    let line = sixel_ring_line();
    let mut file = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
    use std::io::Write as _;
    for _ in 0..9_200 {
        file.write_all(&line).unwrap();
    }
    file.flush().unwrap();
    drop(file);
    sixel_send_file(&prior, cx, 202, &path);
    // A second, still-replayable image placed after the trim: its text row is
    // inside the retained tail, so the fresh consumer must show it.
    let mut late = b"\x1b[8;1HLATE_SIXEL\r\n".to_vec();
    late.extend_from_slice(SIXEL_PRODUCER_OUTPUT);
    late.extend_from_slice(b"AFTER_LATE");
    graphics_send_fixture(&prior, cx, 204, &late);
    graphics_recovery_wait(cx, "late Sixel decoded before detach", |cx| {
        prior.read_with(cx, |view, _| {
            graphics_visible_images(view)
                .iter()
                .filter(|image| image.placement.z == -1)
                .count()
                == 2
        })
    });
    // The producer now waits for its next request: it produced its bytes once.
    let stopped = graphics_recovery_stats(&prior, cx, 203);
    assert!(
        stopped
            .split_whitespace()
            .find_map(|field| field.strip_prefix("bytes="))
            .unwrap()
            .parse::<usize>()
            .unwrap()
            > 8 * 1024 * 1024,
        "the real sender must have written more than the production ring: {stopped}"
    );
    assert!(
        !prior.read_with(cx, |view, _| sixel_screen_text(view).contains("EARLY_SIXEL")),
        "the ring trimmed the early text row"
    );
    graphics_recovery_destroy(prior, prior_window, cx);
    // Observe the actual replay bytes: the transmission must not ride the ring,
    // and the tail must be the real >8 MiB ring, not a synthetic snapshot.
    let mut replay = daemon
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
    replay
        .set_recv_timeout(Some(std::time::Duration::from_secs(30)))
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut raw = Vec::new();
    loop {
        match replay.recv() {
            Ok(DaemonMsg::Snapshot(bytes) | DaemonMsg::Output(bytes)) => raw.extend(bytes),
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => panic!("replay observer: {error}"),
        }
        if raw.windows(10).any(|part| part == b"AFTER_LATE") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "replay tail never reached the late marker"
        );
    }
    replay.detach().unwrap();
    assert!(
        raw.len() > 8 * 1024 * 1024,
        "the replay is the real ring tail: {} bytes",
        raw.len()
    );
    assert!(
        !raw.windows(3).any(|part| part == b"\x1bPq"),
        "the producer's DCS never rides the text replay"
    );
    let (fresh, fresh_window) = graphics_recovery_fresh(&daemon, pane, cx, 205);
    fresh.read_with(cx, |view, _| {
        assert_eq!(view.pane_id, pane);
        let text = sixel_screen_text(view);
        assert!(!text.contains("EARLY_SIXEL"), "trimmed text cannot return");
        assert!(text.contains("LATE_SIXEL"), "retained text must replay");
        assert!(text.contains("AFTER_LATE"));
    });
    graphics_recovery_wait(cx, "restored Sixel paints without retransmission", |cx| {
        fresh.read_with(cx, |view, _| {
            graphics_visible_images(view).iter().any(|image| {
                image.placement.z == -1 && image.painted.load(std::sync::atomic::Ordering::Acquire)
            })
        })
    });
    graphics_assert_production_paint(fresh_window, &fresh, cx);
    fresh.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        let painted: Vec<_> = images
            .iter()
            .filter(|image| {
                image.placement.z == -1 && image.painted.load(std::sync::atomic::Ordering::Acquire)
            })
            .collect();
        assert_eq!(
            painted.len(),
            1,
            "the trimmed early placement cannot paint; the retained one does"
        );
        let restored = painted[0];
        let rgba = restored.data.as_ref().unwrap().as_bytes(0).unwrap();
        assert_eq!(&rgba[..4], &[61, 51, 184, 255], "actual producer pixels");
        let row = sixel_screen_row(view, "LATE_SIXEL").expect("late text row");
        let (crop, _) = restored.paint_bounds(point(px(0.), px(0.)), px(10.), px(20.), 0, 0, 1.);
        assert_eq!(
            crop.origin.y,
            px((row + 1) as f32 * 20.),
            "the restored image shares the text row below LATE_SIXEL"
        );
    });
    // STAT emits no image bytes: the same sender counters prove the fresh
    // consumer restored authority instead of re-running the producer.
    assert_eq!(graphics_recovery_stats(&fresh, cx, 206), stopped);
    graphics_recovery_destroy(fresh, fresh_window, cx);
    std::fs::remove_file(&path).unwrap();
    daemon.panes().kill(pane).unwrap();
}

#[gpui_kit::test]
fn sixel_refusal_keeps_the_accepted_image_and_following_text_coherent(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    let (view, window) = graphics_rooted_consumer(cx, pane, daemon.stream(pane));
    let mut bytes = b"\x1b[2J\x1b[H\x1b[6;5H".to_vec();
    bytes.extend_from_slice(SIXEL_PRODUCER_OUTPUT);
    bytes.extend_from_slice(b"AFTER_ACCEPTED");
    graphics_send_fixture(&view, cx, 301, &bytes);
    graphics_recovery_wait(cx, "accepted Sixel decoded", |cx| {
        view.read_with(cx, |view, _| {
            graphics_visible_images(view)
                .iter()
                .any(|image| image.data.is_some())
        })
    });
    let accepted = view.read_with(cx, |view, _| {
        graphics_visible_images(view)
            .iter()
            .find(|image| image.placement.z == -1)
            .unwrap()
            .data
            .as_ref()
            .unwrap()
            .clone()
    });
    // Oversized raster, malformed palette component and an unterminated body
    // are all refused before admission. None may move the cursor, replace the
    // accepted image, or lose the following ordinary text.
    let mut refused = Vec::new();
    for payload in [
        b"\x1bPq\"1;1;65535;65535#0;2;100;0;0~".as_slice(),
        b"\x1bPq#0;2;101;0;0~".as_slice(),
        b"\x1bPq\"1;1;40;40#0;2;100;0;0!40~\x18".as_slice(),
    ] {
        refused.extend_from_slice(payload);
        refused.extend_from_slice(b"AFTER_REFUSED\r\n");
    }
    graphics_send_fixture(&view, cx, 302, &refused);
    graphics_assert_production_paint(window, &view, cx);
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        assert_eq!(images.len(), 1, "refusals create no placement");
        assert!(std::sync::Arc::ptr_eq(
            images[0].data.as_ref().unwrap(),
            &accepted
        ));
        let text = sixel_screen_text(view);
        // The first two payloads have no terminator, so their following bytes
        // are part of the refused DCS and must never surface as text. The
        // CAN-cancelled third one ends the DCS and its text does print.
        assert_eq!(
            text.matches("AFTER_REFUSED").count(),
            1,
            "an unterminated DCS owns its body; a cancelled one ends it: {text}"
        );
        assert!(
            text.contains("AFTER_ACCEPTED"),
            "the accepted image's text survives refusals: {text}"
        );
    });
    // The test's own clone must go before the consumer retires, or the
    // teardown leak assertion sees a texture the view no longer owns.
    drop(accepted);
    graphics_recovery_destroy(view, window, cx);
    daemon.panes().kill(pane).unwrap();
}

#[gpui_kit::test]
fn sixel_resize_keeps_the_image_on_its_text_row(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    let stream = daemon.stream(pane);
    let mut controller = stream.try_clone().unwrap();
    use std::io::Write as _;
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let (view, window) = graphics_rooted_consumer(cx, pane, stream);
    let mut bytes = b"\x1b[2J\x1b[H\x1b[6;1HRESIZE_SIXEL".to_vec();
    bytes.extend_from_slice(SIXEL_PRODUCER_OUTPUT);
    bytes.extend_from_slice(b"\x1b[20;1HBELOW_SIXEL");
    graphics_send_fixture(&view, cx, 401, &bytes);
    graphics_recovery_wait(cx, "resize setup decoded", |cx| {
        view.read_with(cx, |view, _| {
            graphics_visible_images(view)
                .iter()
                .any(|image| image.data.is_some())
        })
    });
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        let image = images.iter().find(|image| image.placement.z == -1).unwrap();
        let (crop, _) = image.paint_bounds(point(px(0.), px(0.)), px(10.), px(20.), 0, 0, 1.);
        assert_eq!(crop.origin.y, px(5. * 20.), "anchored below RESIZE_SIXEL");
    });
    graphics_assert_production_paint(window, &view, cx);
    let target = graphics_window_geometry(window, &view, cx, TermSize::new(80, 30)).0;
    let authoritative_term = view.read_with(cx, |view, _| view.terminal.term.clone());
    ClientMsg::Resize(crate::daemon::protocol::WinSize {
        cols: 80,
        rows: 30,
        cell_w: 10,
        cell_h: 20,
    })
    .encode(&mut controller)
    .unwrap();
    controller.flush().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if authoritative_term.lock().screen_lines() == 30 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "real daemon Size was not consumed"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    cx.simulate_window_resize(window, target);
    graphics_send_fixture(&view, cx, 402, b"\x1b[0m");
    view.read_with(cx, |view, _| {
        let text = sixel_screen_text(view);
        assert!(text.contains("RESIZE_SIXEL") && text.contains("BELOW_SIXEL"));
        let images = graphics_visible_images(view);
        let image = images.iter().find(|image| image.placement.z == -1).unwrap();
        let (crop, _) = image.paint_bounds(point(px(0.), px(0.)), px(10.), px(20.), 0, 0, 1.);
        assert_eq!(
            crop.origin.y,
            px(5. * 20.),
            "the image still shares its text row after resize"
        );
    });
    graphics_assert_production_paint(window, &view, cx);
    graphics_recovery_destroy(view, window, cx);
    daemon.panes().kill(pane).unwrap();
}
