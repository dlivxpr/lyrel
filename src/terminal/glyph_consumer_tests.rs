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

// A held or reading presentation keeps the registration it captured. These
// helpers observe the cells and paths the production consumer paints, not a
// private glyph cache.

fn glyph_presentation_cell(view: &TerminalView, cp: char) -> Option<(usize, usize)> {
    let geom = view.cell_geom?;
    view.grid_buf
        .iter()
        .take(geom.rows * geom.cols)
        .position(|cell| !cell.spacer && cell.c == cp)
        .map(|index| (index / geom.cols, index % geom.cols))
}

fn glyph_presentation_paths(
    window: gpui_kit::AnyWindowHandle,
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    row: usize,
    col: usize,
) -> Vec<(
    gpui_kit::Bounds<gpui_kit::ScaledPixels>,
    gpui_kit::Background,
)> {
    let cell = glyph_cell_bounds(window, view, cx, row, col);
    glyph_paths_in_cell(window, view, cx, cell)
}

fn glyph_presentation_aspect(bounds: &gpui_kit::Bounds<gpui_kit::ScaledPixels>) -> f32 {
    bounds.size.width.0 / bounds.size.height.0.max(1.)
}

fn glyph_presentation_version(view: &TerminalView, cp: u32) -> Option<u64> {
    view.terminal.glyphs().get(cp).map(|glyph| glyph.revision)
}

/// Wait until the reader has parsed a registration change. This is only a
/// synchronisation point: the assertions below still read painted pictures.
fn glyph_presentation_wait_version(
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    cp: u32,
    previous: Option<u64>,
) {
    glyph_wait(cx, "the consumer did not parse the glyph change", |cx| {
        view.read_with(cx, |view, _| {
            glyph_presentation_version(view, cp) != previous
        })
    });
}

fn glyph_presentation_redraw(window: gpui_kit::AnyWindowHandle, cx: &mut TestAppContext) {
    window
        .update(cx, |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        })
        .unwrap();
}

/// Forces the next frame through the element's real prepaint/paint stages.
/// `window.draw` alone can reuse an existing frame after output-only changes.
fn glyph_presentation_force_frame(
    window: gpui_kit::AnyWindowHandle,
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
) {
    if view.read_with(cx, |view, _| view.cell_geom.is_some()) {
        glyph_presentation_redraw(window, cx);
        let cell = glyph_cell_bounds(window, view, cx, 0, 0);
        let _ = glyph_paths_in_cell(window, view, cx, cell);
    }
}

/// Releases a helper thread's grid guard even if the painted assertion panics,
/// so a failing contention test cannot strand the pane lock.
struct GlyphGridRelease(std::sync::mpsc::Sender<()>);

impl Drop for GlyphGridRelease {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

#[gpui_kit::test]
fn glyph_reading_rows_keep_their_captured_definition_across_replace_and_clear(
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
    let cp = char::from_u32(GLYPH_CP).unwrap();

    // A square registration before the row that displays its codepoint exists.
    let reply = glyph_send(
        &view,
        cx,
        1,
        &replies.path().join("r1"),
        &glyph_register(GLYPH_CP, &glyph_square(1000), 1),
        1,
    );
    assert_eq!(
        reply,
        format!("\x1b_25a1;r;cp={GLYPH_CP:x};status=0\x1b\\").into_bytes()
    );

    // Put the glyph row at the top of the scrollback, then enough output after
    // it that it is immutable history before the user starts reading.
    let mut scene = b"\x1b[2J\x1b[HGLYPH-LINE ".to_vec();
    scene.extend_from_slice(cp.to_string().as_bytes());
    scene.extend_from_slice(b"\x1b[2;1H");
    for row in 0..48 {
        scene.extend_from_slice(format!("HISTORY-{row:02}\r\n").as_bytes());
    }
    graphics_send_fixture(&view, cx, 2, &scene);
    glyph_wait(cx, "the live glyph frame", |cx| {
        view.read_with(cx, |view, _| view.cell_geom.is_some())
    });
    graphics_assert_production_paint(window, &view, cx);

    // Enter reading and scroll to the oldest retained row.
    view.update(cx, |view, cx| view.smooth_scroll(1., cx));
    glyph_presentation_redraw(window, cx);
    view.update(cx, |view, cx| view.smooth_scroll(200., cx));
    glyph_presentation_redraw(window, cx);
    assert!(
        view.read_with(cx, |view, _| view.selection_scroll.reading.is_some()),
        "the test needs a genuine reading presentation"
    );
    let (row, col) = view
        .read_with(cx, |view, _| glyph_presentation_cell(view, cp))
        .expect("the captured glyph row is visible in reading");
    assert!(
        reading_painted_rows(&view, cx)
            .iter()
            .any(|line| line.contains("GLYPH-LINE")),
        "the raw codepoint row is still the row the user reads"
    );
    let square = glyph_presentation_paths(window, &view, cx, row, col);
    assert_eq!(square.len(), 1, "one captured glyph path: {square:?}");
    let captured = square[0].0;
    assert!(
        glyph_presentation_aspect(&captured) < 2.,
        "the captured square must be the square registration"
    );

    // Replace A with a wide rectangle while the user is reading.
    let before = view.read_with(cx, |view, _| glyph_presentation_version(view, GLYPH_CP));
    let reply = glyph_send(
        &view,
        cx,
        3,
        &replies.path().join("r3"),
        &glyph_register(GLYPH_CP, &glyph_rect(2000, 500), 1),
        1,
    );
    assert_eq!(
        reply,
        format!("\x1b_25a1;r;cp={GLYPH_CP:x};status=0\x1b\\").into_bytes()
    );
    glyph_presentation_wait_version(&view, cx, GLYPH_CP, before);
    let held = glyph_presentation_paths(window, &view, cx, row, col);
    assert_eq!(held.len(), 1, "the reading row still paints: {held:?}");
    assert_eq!(
        held[0].0, captured,
        "a replacement must not rewrite the picture the user is reading"
    );

    // The next live picture does adopt the replacement.
    view.update(cx, |view, _| view.jump_to_prompt());
    graphics_send_fixture(&view, cx, 4, format!("\r\n\x1b[2K{cp}").as_bytes());
    glyph_wait(cx, "the live row adopts the replacement", |cx| {
        glyph_presentation_redraw(window, cx);
        view.read_with(cx, |view, _| {
            view.selection_scroll.reading.is_none() && glyph_presentation_cell(view, cp).is_some()
        })
    });
    let (row, col) = view
        .read_with(cx, |view, _| glyph_presentation_cell(view, cp))
        .expect("the live glyph row is painted");
    let live = glyph_presentation_paths(window, &view, cx, row, col);
    assert_eq!(live.len(), 1);
    assert!(
        glyph_presentation_aspect(&live[0].0) > 2.,
        "live cells adopt the wide replacement"
    );

    // A new reading capture of the original history row uses the current
    // definition; that is the version it will now keep.
    view.update(cx, |view, cx| view.smooth_scroll(1., cx));
    glyph_presentation_redraw(window, cx);
    view.update(cx, |view, cx| view.smooth_scroll(200., cx));
    glyph_presentation_redraw(window, cx);
    let (row, col) = view
        .read_with(cx, |view, _| glyph_presentation_cell(view, cp))
        .expect("the history row is visible again");
    let recaptured = glyph_presentation_paths(window, &view, cx, row, col);
    assert_eq!(recaptured.len(), 1);
    assert!(
        glyph_presentation_aspect(&recaptured[0].0) > 2.,
        "a new capture keeps the live definition"
    );

    // Clearing while reading leaves the captured B picture alone.
    let reply = glyph_send(
        &view,
        cx,
        5,
        &replies.path().join("r5"),
        &glyph_clear(Some(GLYPH_CP)),
        1,
    );
    assert_eq!(
        reply,
        format!("\x1b_25a1;c;cp={GLYPH_CP:x};status=0\x1b\\").into_bytes()
    );
    glyph_wait(cx, "the consumer applied the clear", |cx| {
        view.read_with(cx, |view, _| {
            glyph_presentation_version(view, GLYPH_CP).is_none()
        })
    });
    let cleared = glyph_presentation_paths(window, &view, cx, row, col);
    assert_eq!(
        cleared.len(),
        1,
        "the captured picture survives its live registration: {cleared:?}"
    );
    assert!(
        glyph_presentation_aspect(&cleared[0].0) > 2.,
        "the captured picture is still B"
    );

    // A cleared live cell falls back to ordinary font rendering, and the raw
    // codepoint the user copies or searches for is unchanged.
    view.update(cx, |view, _| view.jump_to_prompt());
    graphics_send_fixture(&view, cx, 6, format!("\r\n\x1b[2K{cp}").as_bytes());
    glyph_wait(cx, "the cleared live row is painted", |cx| {
        glyph_presentation_redraw(window, cx);
        view.read_with(cx, |view, _| {
            view.selection_scroll.reading.is_none() && glyph_presentation_cell(view, cp).is_some()
        })
    });
    let (row, col) = view
        .read_with(cx, |view, _| glyph_presentation_cell(view, cp))
        .expect("the cleared live row is painted");
    assert!(
        glyph_presentation_paths(window, &view, cx, row, col).is_empty(),
        "a cleared registration falls back to the font"
    );
    assert!(
        view.read_with(cx, |view, _| view.scrollback_text().contains(cp)),
        "selection and search keep the original codepoint"
    );
}

#[gpui_kit::test]
fn glyph_synchronized_picture_keeps_its_committed_version_until_esu(cx: &mut TestAppContext) {
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
    let cp = char::from_u32(GLYPH_CP).unwrap();

    let reply = glyph_send(
        &view,
        cx,
        1,
        &replies.path().join("r1"),
        &glyph_register(GLYPH_CP, &glyph_square(1000), 1),
        1,
    );
    assert_eq!(
        reply,
        format!("\x1b_25a1;r;cp={GLYPH_CP:x};status=0\x1b\\").into_bytes()
    );
    let display = format!("\x1b[2J\x1b[H\x1b[2;5H{cp}");
    graphics_send_fixture(&view, cx, 2, display.as_bytes());
    glyph_wait(cx, "the committed glyph frame", |cx| {
        glyph_presentation_force_frame(window, &view, cx);
        view.read_with(cx, |view, _| {
            view.cell_geom.is_some() && glyph_presentation_cell(view, cp).is_some()
        })
    });
    let (row, col) = view
        .read_with(cx, |view, _| glyph_presentation_cell(view, cp))
        .expect("the committed glyph row is painted");
    let committed = glyph_presentation_paths(window, &view, cx, row, col);
    assert_eq!(committed.len(), 1);
    let square = committed[0].0;

    // Begin a synchronized update. The established 150 ms fail-safe belongs
    // to the separate test below, so this atomicity test owns its clock origin.
    graphics_send_fixture(&view, cx, 3, b"\x1b[?2026h");
    glyph_wait(cx, "the synchronized update was parsed", |cx| {
        view.read_with(cx, |view, _| {
            view.terminal.term.lock().synchronized_output()
        })
    });
    view.read_with(cx, |view, _| {
        view.terminal.term.lock().sync_update_started =
            Some(rio_vt::time::Instant::now() + std::time::Duration::from_secs(60));
    });

    // Replace the registration while the frame is unfinished.
    let before = view.read_with(cx, |view, _| glyph_presentation_version(view, GLYPH_CP));
    let reply = glyph_send(
        &view,
        cx,
        4,
        &replies.path().join("r4"),
        &glyph_register(GLYPH_CP, &glyph_rect(2000, 500), 1),
        1,
    );
    assert_eq!(
        reply,
        format!("\x1b_25a1;r;cp={GLYPH_CP:x};status=0\x1b\\").into_bytes()
    );
    glyph_presentation_wait_version(&view, cx, GLYPH_CP, before);

    for paint in 0..2 {
        let held = glyph_presentation_paths(window, &view, cx, row, col);
        assert_eq!(held.len(), 1, "repaint {paint}: {held:?}");
        assert_eq!(
            held[0].0, square,
            "repaint {paint} must reuse the committed picture"
        );
    }
    assert!(
        view.read_with(cx, |view, _| {
            view.terminal.term.lock().synchronized_output()
        }),
        "the repaint must not end the synchronized update"
    );

    graphics_send_fixture(&view, cx, 5, b"\x1b[?2026l");
    glyph_wait(cx, "ESU publishes the replacement", |cx| {
        let paths = glyph_presentation_paths(window, &view, cx, row, col);
        paths.len() == 1 && paths[0].0 != square
    });
    let published = glyph_presentation_paths(window, &view, cx, row, col);
    assert!(
        glyph_presentation_aspect(&published[0].0) > 2.,
        "the published picture is the replacement"
    );
}

#[gpui_kit::test]
fn glyph_synchronized_picture_keeps_its_committed_version_until_the_fail_safe(
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
    let cp = char::from_u32(GLYPH_CP).unwrap();

    let reply = glyph_send(
        &view,
        cx,
        1,
        &replies.path().join("r1"),
        &glyph_register(GLYPH_CP, &glyph_square(1000), 1),
        1,
    );
    assert_eq!(
        reply,
        format!("\x1b_25a1;r;cp={GLYPH_CP:x};status=0\x1b\\").into_bytes()
    );
    let display = format!("\x1b[2J\x1b[H\x1b[2;5H{cp}");
    graphics_send_fixture(&view, cx, 2, display.as_bytes());
    glyph_wait(cx, "the committed glyph frame", |cx| {
        glyph_presentation_force_frame(window, &view, cx);
        view.read_with(cx, |view, _| {
            view.cell_geom.is_some() && glyph_presentation_cell(view, cp).is_some()
        })
    });
    let (row, col) = view
        .read_with(cx, |view, _| glyph_presentation_cell(view, cp))
        .expect("the committed glyph row is painted");
    let committed = glyph_presentation_paths(window, &view, cx, row, col);
    assert_eq!(committed.len(), 1);
    let square = committed[0].0;

    graphics_send_fixture(&view, cx, 3, b"\x1b[?2026h");
    glyph_wait(cx, "the synchronized update was parsed", |cx| {
        view.read_with(cx, |view, _| {
            view.terminal.term.lock().synchronized_output()
        })
    });
    view.read_with(cx, |view, _| {
        view.terminal.term.lock().sync_update_started =
            Some(rio_vt::time::Instant::now() + std::time::Duration::from_secs(60));
    });

    let before = view.read_with(cx, |view, _| glyph_presentation_version(view, GLYPH_CP));
    let reply = glyph_send(
        &view,
        cx,
        4,
        &replies.path().join("r4"),
        &glyph_register(GLYPH_CP, &glyph_rect(2000, 500), 1),
        1,
    );
    assert_eq!(
        reply,
        format!("\x1b_25a1;r;cp={GLYPH_CP:x};status=0\x1b\\").into_bytes()
    );
    glyph_presentation_wait_version(&view, cx, GLYPH_CP, before);
    let held = glyph_presentation_paths(window, &view, cx, row, col);
    assert_eq!(held.len(), 1);
    assert_eq!(
        held[0].0, square,
        "an unfinished frame must reuse its committed picture"
    );

    // No ESU arrives: the established 150 ms fail-safe releases the frame.
    view.read_with(cx, |view, _| {
        view.terminal.term.lock().sync_update_started =
            Some(rio_vt::time::Instant::now() - std::time::Duration::from_secs(1));
    });
    glyph_wait(cx, "the fail-safe publishes the replacement", |cx| {
        let paths = glyph_presentation_paths(window, &view, cx, row, col);
        paths.len() == 1 && paths[0].0 != square
    });
    assert!(
        view.read_with(cx, |view, _| {
            !view.terminal.term.lock().synchronized_output()
        }),
        "the fail-safe ends the synchronized update"
    );
    let published = glyph_presentation_paths(window, &view, cx, row, col);
    assert!(
        glyph_presentation_aspect(&published[0].0) > 2.,
        "the fail-safe publishes the replacement, not a stale registration"
    );
}

#[gpui_kit::test]
fn glyph_held_picture_survives_equal_area_resize_contention_and_ordered_size(
    cx: &mut TestAppContext,
) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    // A resize-echo link so the equal-area resize stays a committed old-width
    // picture until the ordered Size arrives, as on a routed pane.
    let (client, _initial_sender) = test_stream_pair();
    let (view, window) = graphics_build_rooted_consumer_at(
        cx,
        176,
        client,
        Vec::new(),
        TermSize::new(80, 24),
        PtySource::Raw,
    );
    let (client, mut sender) = test_stream_pair();
    view.update(cx, |view, cx| {
        view.adopt_relink_with_version(
            client,
            Vec::new(),
            &crate::terminal::PaneRoute::Remote {
                header: Box::new(crate::daemon::router::RouteHeader::wsl(
                    "glyph-resize-fixture",
                )),
                resize_echo: true,
            },
            TermSize::new(80, 24),
            10,
            20,
            Some(crate::daemon::protocol::DaemonVersion::current()),
            cx,
        )
        .unwrap();
    });
    let cp = char::from_u32(GLYPH_CP).unwrap();

    let mut scene = glyph_register(GLYPH_CP, &glyph_square(1000), 1);
    scene.extend_from_slice(b"\x1b[2J\x1b[H\x1b[2;5H");
    scene.extend_from_slice(cp.to_string().as_bytes());
    DaemonMsg::Output(scene).encode(&mut sender).unwrap();
    glyph_wait(cx, "the committed glyph frame", |cx| {
        glyph_presentation_force_frame(window, &view, cx);
        view.read_with(cx, |view, _| {
            view.cell_geom.is_some() && glyph_presentation_cell(view, cp).is_some()
        })
    });
    let (row, col) = view
        .read_with(cx, |view, _| glyph_presentation_cell(view, cp))
        .expect("the committed glyph row is painted");
    let committed = glyph_presentation_paths(window, &view, cx, row, col);
    assert_eq!(committed.len(), 1);
    let square = committed[0].0;

    DaemonMsg::Output(b"\x1b[?2026h".to_vec())
        .encode(&mut sender)
        .unwrap();
    glyph_wait(cx, "the synchronized update was parsed", |cx| {
        view.read_with(cx, |view, _| {
            view.terminal.term.lock().synchronized_output()
        })
    });
    view.read_with(cx, |view, _| {
        view.terminal.term.lock().sync_update_started =
            Some(rio_vt::time::Instant::now() + std::time::Duration::from_secs(60));
    });
    let before = view.read_with(cx, |view, _| glyph_presentation_version(view, GLYPH_CP));
    DaemonMsg::Output(glyph_register(GLYPH_CP, &glyph_rect(2000, 500), 1))
        .encode(&mut sender)
        .unwrap();
    glyph_presentation_wait_version(&view, cx, GLYPH_CP, before);

    // Grid contention happens when the reader holds the grid while the UI
    // paints. Hold it on a helper thread (never on the painting thread) so the
    // production `try_lock` yields `Contended`; the committed picture and its
    // captured registration must survive and still be what the next paint
    // shows.
    let terminal = view.read_with(cx, |view, _| view.terminal.term.clone());
    let (locked, wait_locked) = std::sync::mpsc::channel();
    let (release, wait_release) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let guard = terminal.lock();
        locked.send(()).unwrap();
        let _ = wait_release.recv_timeout(std::time::Duration::from_secs(3));
        drop(guard);
    });
    wait_locked.recv().unwrap();
    let release = GlyphGridRelease(release);
    let contended = glyph_presentation_paths(window, &view, cx, row, col);
    drop(release);
    holder.join().unwrap();
    assert_eq!(contended.len(), 1, "{contended:?}");
    assert_eq!(
        contended[0].0, square,
        "a contended repaint reuses the committed registration"
    );

    // Equal-area resize while the frame is unfinished: the committed cells keep
    // their old addressing until the ordered Size.
    let target = graphics_window_geometry(window, &view, cx, TermSize::new(96, 20)).0;
    cx.simulate_window_resize(window, target);
    glyph_presentation_redraw(window, cx);
    view.read_with(cx, |view, _| {
        assert_eq!(
            view.cell_geom.map(|geom| (geom.cols, geom.rows)),
            Some((80, 24)),
            "an unfinished frame keeps its committed addressing"
        );
    });
    let resized_hold = glyph_presentation_paths(window, &view, cx, row, col);
    assert_eq!(resized_hold.len(), 1, "{resized_hold:?}");
    assert_eq!(
        resized_hold[0].0, square,
        "an equal-area resize must not repaint the captured registration"
    );

    DaemonMsg::Size(crate::daemon::protocol::WinSize {
        cols: 96,
        rows: 20,
        cell_w: 10,
        cell_h: 20,
    })
    .encode(&mut sender)
    .unwrap();
    glyph_wait(cx, "the ordered Size was applied", |cx| {
        view.read_with(cx, |view, _| {
            let term = view.terminal.term.lock();
            (term.columns(), term.screen_lines()) == (96, 20)
        })
    });
    let ordered_hold = glyph_presentation_paths(window, &view, cx, row, col);
    assert_eq!(ordered_hold.len(), 1, "{ordered_hold:?}");
    assert_eq!(
        ordered_hold[0].0, square,
        "the ordered Size does not publish the unfinished frame"
    );

    DaemonMsg::Output(b"\x1b[?2026l".to_vec())
        .encode(&mut sender)
        .unwrap();
    glyph_wait(cx, "ESU publishes at the ordered geometry", |cx| {
        glyph_presentation_redraw(window, cx);
        let paths = {
            let (row, col) = view
                .read_with(cx, |view, _| glyph_presentation_cell(view, cp))
                .unwrap_or((row, col));
            glyph_presentation_paths(window, &view, cx, row, col)
        };
        !paths.is_empty()
            && paths[0].0 != square
            && view.read_with(cx, |view, _| {
                view.cell_geom
                    .is_some_and(|geom| (geom.cols, geom.rows) == (96, 20))
                    && !view.terminal.term.lock().synchronized_output()
            })
    });
    let (row, col) = view
        .read_with(cx, |view, _| glyph_presentation_cell(view, cp))
        .expect("the resized glyph row is painted");
    let published = glyph_presentation_paths(window, &view, cx, row, col);
    assert_eq!(published.len(), 1);
    assert!(
        glyph_presentation_aspect(&published[0].0) > 2.,
        "the published picture is the replacement at the ordered geometry"
    );
}
