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
