// Real PTY -> daemon/ordered transport -> production TerminalView -> painted
// picture for the monochrome Glyph Protocol slice. The producer is the same
// fixture child the graphics consumers use; every byte here is real output and
// every reply is read back from the actual PTY input through the request's
// gate file. The glyph producer/paint helpers live in
// `graphics_pty_consumer_support.rs` so the native-font consumer reuses them.

/// The scene rectangles a registered glyph occupies on the declared span.
fn glyph_painted(
    window: gpui_kit::AnyWindowHandle,
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    region: Bounds<gpui_kit::ScaledPixels>,
) -> Vec<(
    Bounds<gpui_kit::ScaledPixels>,
    Bounds<gpui_kit::ScaledPixels>,
    gpui_kit::Background,
)> {
    glyph_paths_with_clip_in_region(window, view, cx, region)
}

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

/// The §8.5 sizing/placement model, through the real PTY and the production
/// terminal element: declared extents, every size mode, horizontal/vertical
/// alignment, fractional padding, degenerate padding, clipping and the text
/// baseline. Expected geometry is read from the actual cell metrics and the
/// font's own baseline, not from a copy of the placement formula.
#[gpui_kit::test]
fn glyph_consumer_declared_extents_modes_alignment_padding_and_baseline(
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
    let reply_path = |serial: usize| replies.path().join(format!("r{serial}"));
    let cp = GLYPH_CP;
    let mut serial = 0usize;

    let glyph_at = |cx: &mut TestAppContext, serial: &mut usize, row: usize, col: usize, at: u32| {
        let at_char = char::from_u32(at).unwrap();
        let text = at_char.to_string();
        *serial += 1;
        glyph_show(&view, cx, *serial, row, col, &text);
        glyph_wait(cx, "declared glyph cell painted", |cx| {
            view.read_with(cx, |view, _| {
                view.cell_geom.is_some() && glyph_presented_row(view, row).chars().nth(col) == Some(at_char)
            })
        });
    };

    // A declared extent is the placement frame. Stretch + start/start fills
    // the cell exactly, and deleting the outline's own bounds from the
    // equation is what keeps a coordinated set from drifting.
    serial += 1;
    glyph_register_ok(
        &view,
        cx,
        serial,
        &reply_path(serial),
        cp,
        &glyph_square(1000),
        1000,
        "aw=1000;lh=1000;ymin=0;size=stretch;align=start,start;pad=0,0,0,0",
    );
    glyph_at(cx, &mut serial, 2, 4, cp);
    let cell = glyph_region_bounds(window, &view, cx, 2, 4, 1);
    let painted = glyph_painted(window, &view, cx, cell);
    assert_eq!(painted.len(), 1, "one declared glyph path: {painted:?}");
    assert!(
        glyph_bounds_close(painted[0].0, cell),
        "stretch fills the declared cell: {:?} vs {cell:?}",
        painted[0].0
    );
    assert!(
        glyph_bounds_close(painted[0].1, cell),
        "the clip is the unpadded span: {:?} vs {cell:?}",
        painted[0].1
    );

    // A smaller outline under the same declared extent stays anchored to the
    // declared bottom-left instead of being centred on its own bbox.
    serial += 1;
    glyph_register_ok(
        &view,
        cx,
        serial,
        &reply_path(serial),
        cp,
        &glyph_rect(500, 500),
        1000,
        "aw=1000;lh=1000;ymin=0;size=stretch;align=start,start;pad=0,0,0,0",
    );
    glyph_wait(cx, "declared quarter repainted", |cx| {
        let painted = glyph_painted(window, &view, cx, cell);
        painted.first().is_some_and(|path| {
            (path.0.size.width.0 - cell.size.width.0 / 2.).abs() <= 1.5
                && (path.0.size.height.0 - cell.size.height.0 / 2.).abs() <= 1.5
        })
    });
    let painted = glyph_painted(window, &view, cx, cell);
    assert_eq!(painted.len(), 1, "one quarter glyph path: {painted:?}");
    let quarter = painted[0].0;
    assert!(
        (quarter.origin.x.0 - cell.origin.x.0).abs() <= 1.5,
        "declared start keeps the left edge: {quarter:?} vs {cell:?}"
    );
    assert!(
        (quarter.origin.y.0 - (cell.origin.y.0 + cell.size.height.0 / 2.)).abs() <= 1.5,
        "declared start keeps the bottom edge: {quarter:?} vs {cell:?}"
    );
    assert!(
        (quarter.size.width.0 - cell.size.width.0 / 2.).abs() <= 1.5
            && (quarter.size.height.0 - cell.size.height.0 / 2.).abs() <= 1.5,
        "the quarter scales by the declared extent, not by its own bounds: {quarter:?}"
    );

    // baseline: authored y=0 sits on the primary text baseline, so a
    // descender (-200) crosses below it while the ascender (800) rises above.
    serial += 1;
    glyph_register_ok(
        &view,
        cx,
        serial,
        &reply_path(serial),
        cp,
        &glyph_polygon(&[(0, -200), (1000, -200), (1000, 800), (0, 800)]),
        1000,
        "aw=1000;lh=1000;ymin=-200;size=stretch;align=center,baseline;pad=0,0,0,0",
    );
    glyph_at(cx, &mut serial, 3, 4, cp);
    let baseline_cell = glyph_region_bounds(window, &view, cx, 3, 4, 1);
    let (font, font_size, line_height) = view.read_with(cx, |view, _| {
        (view.font.clone(), view.font_size, view.line_height)
    });
    let scale = window
        .update(cx, |_, window, _| window.scale_factor())
        .unwrap();
    let baseline = window
        .update(cx, |_, window, _| {
            let font_id = window.text_system().resolve_font(&font);
            window
                .text_system()
                .baseline_offset(font_id, font_size, line_height)
        })
        .unwrap();
    let baseline_px = baseline.as_f32() * scale;
    let painted = glyph_painted(window, &view, cx, baseline_cell);
    assert_eq!(painted.len(), 1, "one baseline glyph path: {painted:?}");
    let authored = painted[0].0;
    let top = authored.origin.y.0;
    let bottom = authored.origin.y.0 + authored.size.height.0;
    assert!(
        (bottom - (baseline_cell.origin.y.0 + baseline_px)).abs() <= 2.,
        "authored y=0 lands on the text baseline: bottom={bottom:?}, baseline={:?}",
        baseline_cell.origin.y.0 + baseline_px
    );
    assert!(
        top < baseline_cell.origin.y.0 + baseline_px,
        "the ascender rises above the baseline: {authored:?}"
    );
    assert!(
        bottom > baseline_cell.origin.y.0 + baseline_px,
        "the descender sits below the baseline: {authored:?}"
    );
    assert!(
        glyph_bounds_close(painted[0].1, baseline_cell),
        "descender pixels clip to the cell: {:?} vs {baseline_cell:?}",
        painted[0].1
    );

    // Every size mode, on a square outline with a declared 1000x1000 extent.
    let modes = [
        ("height", 2),
        ("advance", 4),
        ("contain", 6),
        ("cover", 8),
        ("stretch", 10),
    ];
    for (mode, row) in modes {
        let mode_cp = cp + mode_index(mode);
        serial += 1;
        glyph_register_ok(
            &view,
            cx,
            serial,
            &reply_path(serial),
            mode_cp,
            &glyph_square(1000),
            1000,
            &format!(
                "aw=1000;lh=1000;ymin=0;size={mode};align=start,start;pad=0,0,0,0"
            ),
        );
        let text = char::from_u32(mode_cp).unwrap().to_string();
        serial += 1;
        glyph_show(&view, cx, serial, row, 4, &text);
        glyph_wait(cx, "mode glyph painted", |cx| {
            view.read_with(cx, |view, _| {
                view.cell_geom.is_some()
                    && glyph_presented_row(view, row).chars().nth(4)
                        == char::from_u32(mode_cp)
            })
        });
        let cell = glyph_region_bounds(window, &view, cx, row, 4, 1);
        let painted = glyph_painted(window, &view, cx, cell);
        assert_eq!(painted.len(), 1, "{mode} has one path: {painted:?}");
        let size = painted[0].0.size;
        match mode {
            // Line-height drives both axes for a square outline.
            "height" => assert!(
                (size.width.0 - cell.size.height.0).abs() <= 1.5
                    && (size.height.0 - cell.size.height.0).abs() <= 1.5,
                "height = the cell height: {size:?} vs {cell:?}"
            ),
            // Advance drives both axes for a square outline.
            "advance" => assert!(
                (size.width.0 - cell.size.width.0).abs() <= 1.5
                    && (size.height.0 - cell.size.width.0).abs() <= 1.5,
                "advance = the cell width: {size:?} vs {cell:?}"
            ),
            "contain" => {
                let smallest = cell.size.width.0.min(cell.size.height.0);
                assert!(
                    (size.width.0 - smallest).abs() <= 1.5
                        && (size.height.0 - smallest).abs() <= 1.5,
                    "contain fits the smaller span axis: {size:?} vs {cell:?}"
                );
                assert!(
                    painted[0].0.origin.x.0 >= cell.origin.x.0 - 1.5
                        && painted[0].0.origin.y.0 >= cell.origin.y.0 - 1.5,
                    "contain stays inside the span: {:?}",
                    painted[0].0
                );
            }
            "cover" => {
                let largest = cell.size.width.0.max(cell.size.height.0);
                assert!(
                    (size.width.0 - largest).abs() <= 1.5
                        && (size.height.0 - largest).abs() <= 1.5,
                    "cover fills the larger span axis: {size:?} vs {cell:?}"
                );
                assert!(
                    size.width.0 > cell.size.width.0 + 1.5
                        || size.height.0 > cell.size.height.0 + 1.5,
                    "cover overflows at least one axis: {size:?} vs {cell:?}"
                );
            }
            "stretch" => assert!(
                glyph_bounds_close(painted[0].0, cell),
                "stretch has each axis independently: {:?} vs {cell:?}",
                painted[0].0
            ),
            other => panic!("unexpected mode {other}"),
        }
        assert!(
            glyph_bounds_close(painted[0].1, cell),
            "{mode} clips to the unpadded render span: {:?} vs {cell:?}",
            painted[0].1
        );
    }

    // Fractional padding and non-default alignment. With stretch, the
    // effective span is exactly the padded rectangle.
    serial += 1;
    glyph_register_ok(
        &view,
        cx,
        serial,
        &reply_path(serial),
        cp + 20,
        &glyph_square(1000),
        1000,
        "aw=1000;lh=1000;ymin=0;size=stretch;align=end,start;pad=0.1,0.2,0,0.3",
    );
    glyph_at(cx, &mut serial, 12, 4, cp + 20);
    let padded = glyph_region_bounds(window, &view, cx, 12, 4, 1);
    let painted = glyph_painted(window, &view, cx, padded);
    assert_eq!(painted.len(), 1, "one padded glyph path: {painted:?}");
    let expected = Bounds::new(
        point(
            gpui_kit::ScaledPixels(padded.origin.x.0 + padded.size.width.0 * 0.3),
            gpui_kit::ScaledPixels(padded.origin.y.0 + padded.size.height.0 * 0.1),
        ),
        gpui_kit::size(
            gpui_kit::ScaledPixels(padded.size.width.0 * 0.5),
            gpui_kit::ScaledPixels(padded.size.height.0 * 0.9),
        ),
    );
    assert!(
        glyph_bounds_close(painted[0].0, expected),
        "fractional padding + end/start: {:?} vs {expected:?}",
        painted[0].0
    );

    serial += 1;
    glyph_register_ok(
        &view,
        cx,
        serial,
        &reply_path(serial),
        cp + 21,
        &glyph_square(1000),
        1000,
        "aw=1000;lh=1000;ymin=0;size=stretch;align=center,center;pad=0.2,0.2,0.2,0.2",
    );
    glyph_at(cx, &mut serial, 13, 4, cp + 21);
    let centred = glyph_region_bounds(window, &view, cx, 13, 4, 1);
    let painted = glyph_painted(window, &view, cx, centred);
    assert_eq!(painted.len(), 1, "one centred glyph path: {painted:?}");
    let expected = Bounds::new(
        point(
            gpui_kit::ScaledPixels(centred.origin.x.0 + centred.size.width.0 * 0.2),
            gpui_kit::ScaledPixels(centred.origin.y.0 + centred.size.height.0 * 0.2),
        ),
        gpui_kit::size(
            gpui_kit::ScaledPixels(centred.size.width.0 * 0.6),
            gpui_kit::ScaledPixels(centred.size.height.0 * 0.6),
        ),
    );
    assert!(
        glyph_bounds_close(painted[0].0, expected),
        "centre alignment is measured on the padded span: {:?} vs {expected:?}",
        painted[0].0
    );

    // Degenerate padding is all-or-nothing: one saturated axis zeroes all
    // four insets, not just the axis that overflowed.
    for (offset, params, row) in [
        (
            30,
            "aw=1000;lh=1000;ymin=0;size=stretch;align=start,start;pad=0.4,0.2,0.6,0.1",
            14,
        ),
        (
            31,
            "aw=1000;lh=1000;ymin=0;size=stretch;align=start,start;pad=0.1,0.5,0.2,0.5",
            15,
        ),
    ] {
        let degenerate_cp = cp + offset;
        serial += 1;
        glyph_register_ok(
            &view,
            cx,
            serial,
            &reply_path(serial),
            degenerate_cp,
            &glyph_square(1000),
            1000,
            params,
        );
        let text = char::from_u32(degenerate_cp).unwrap().to_string();
        serial += 1;
        glyph_show(&view, cx, serial, row, 4, &text);
        glyph_wait(cx, "degenerate-padding glyph painted", |cx| {
            view.read_with(cx, |view, _| {
                view.cell_geom.is_some()
                    && glyph_presented_row(view, row).chars().nth(4)
                        == char::from_u32(degenerate_cp)
            })
        });
        let cell = glyph_region_bounds(window, &view, cx, row, 4, 1);
        let painted = glyph_painted(window, &view, cx, cell);
        assert_eq!(painted.len(), 1, "one unpadded glyph path: {painted:?}");
        assert!(
            glyph_bounds_close(painted[0].0, cell),
            "degenerate padding is no padding at all: {:?} vs {cell:?}",
            painted[0].0
        );
    }
}

fn mode_index(mode: &str) -> u32 {
    match mode {
        "height" => 2,
        "advance" => 4,
        "contain" => 6,
        "cover" => 8,
        "stretch" => 10,
        other => panic!("unexpected mode {other}"),
    }
}

/// `width=2` is a pixel render span only. The production consumer must use it
/// on a blank following cell, fall back to one cell at the row end or before
/// anything the next cell already owns, clip to the chosen span, and leave
/// the codepoint's logical one-cell advance, wrapping, selection and copy
/// untouched.
#[gpui_kit::test]
fn glyph_consumer_width_two_span_fallback_clip_and_logical_cells(
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
    let reply_path = |serial: usize| replies.path().join(format!("r{serial}"));
    let cp = GLYPH_CP;
    let glyph = char::from_u32(cp).unwrap();
    let mut serial = 0usize;

    // A blank following cell lets stretch fill both cells.
    serial += 1;
    glyph_register_ok(
        &view,
        cx,
        serial,
        &reply_path(serial),
        cp,
        &glyph_square(1000),
        1000,
        "width=2;aw=1000;lh=1000;ymin=0;size=stretch;align=start,start;pad=0,0,0,0",
    );
    serial += 1;
    glyph_show(&view, cx, serial, 2, 4, &format!("{glyph} "));
    glyph_wait(cx, "width-two glyph painted", |cx| {
        view.read_with(cx, |view, _| {
            view.cell_geom.is_some() && glyph_presented_row(view, 2).chars().nth(4) == Some(glyph)
        })
    });
    let span = glyph_region_bounds(window, &view, cx, 2, 4, 2);
    let painted = glyph_painted(window, &view, cx, span);
    assert_eq!(painted.len(), 1, "one wide glyph path: {painted:?}");
    assert!(
        glyph_bounds_close(painted[0].0, span),
        "a blank neighbour lends the second cell: {:?} vs {span:?}",
        painted[0].0
    );
    assert!(
        glyph_bounds_close(painted[0].1, span),
        "the render span is the two-cell clip: {:?} vs {span:?}",
        painted[0].1
    );

    // The final column has nowhere for the overflow: one-cell span.
    serial += 1;
    glyph_show(&view, cx, serial, 3, 79, &glyph.to_string());
    glyph_wait(cx, "final-column glyph painted", |cx| {
        view.read_with(cx, |view, _| {
            glyph_presented_row(view, 3).chars().nth(79) == Some(glyph)
        })
    });
    let last = glyph_region_bounds(window, &view, cx, 3, 79, 1);
    let painted = glyph_painted(window, &view, cx, last);
    assert_eq!(painted.len(), 1, "one final-column glyph path: {painted:?}");
    assert!(
        glyph_bounds_close(painted[0].0, last),
        "the final column falls back to one cell: {:?} vs {last:?}",
        painted[0].0
    );
    assert!(
        glyph_bounds_close(painted[0].1, last),
        "the fallback clip is one cell: {:?} vs {last:?}",
        painted[0].1
    );

    // A nonblank following cell cannot receive the overflow, and the cursor
    // still advanced one logical cell before it.
    serial += 1;
    glyph_show(&view, cx, serial, 4, 4, &format!("{glyph}X"));
    glyph_wait(cx, "nonblank-neighbour glyph painted", |cx| {
        view.read_with(cx, |view, _| {
            glyph_presented_row(view, 4).chars().nth(4) == Some(glyph)
        })
    });
    let row = view.read_with(cx, |view, _| glyph_presented_row(view, 4));
    assert_eq!(
        row.chars().nth(5),
        Some('X'),
        "the codepoint still advances exactly one logical cell"
    );
    let one = glyph_region_bounds(window, &view, cx, 4, 4, 1);
    let painted = glyph_painted(window, &view, cx, one);
    assert_eq!(painted.len(), 1, "one fallback glyph path: {painted:?}");
    assert!(
        glyph_bounds_close(painted[0].0, one),
        "a nonblank neighbour falls back to one cell: {:?} vs {one:?}",
        painted[0].0
    );
    assert!(glyph_bounds_close(painted[0].1, one));

    // A blank cell that already paints a background is not free either. Move
    // the cursor past it before the fixture marker so it stays background.
    serial += 1;
    glyph_show(&view, cx, serial, 5, 5, "\x1b[44m \x1b[0m");
    serial += 1;
    glyph_show(&view, cx, serial, 5, 4, &format!("{glyph}\x1b[7G"));
    glyph_wait(cx, "owned-blank glyph painted", |cx| {
        view.read_with(cx, |view, _| {
            glyph_presented_row(view, 5).chars().nth(4) == Some(glyph)
        })
    });
    let owned = glyph_region_bounds(window, &view, cx, 5, 4, 1);
    let painted = glyph_painted(window, &view, cx, owned);
    assert_eq!(painted.len(), 1, "one owned-blank glyph path: {painted:?}");
    assert!(
        glyph_bounds_close(painted[0].0, owned),
        "a background-painting blank keeps its cell: {:?} vs {owned:?}",
        painted[0].0
    );

    // Cover overflows both axes; the renderer must clip the visible pixels to
    // the unpadded two-cell span, not to the outline's own bounds.
    serial += 1;
    glyph_register_ok(
        &view,
        cx,
        serial,
        &reply_path(serial),
        cp,
        &glyph_square(1000),
        1000,
        "width=2;aw=2000;lh=200;ymin=0;size=cover;align=start,start;pad=0,0,0,0",
    );
    serial += 1;
    glyph_show(&view, cx, serial, 7, 4, &format!("{glyph} "));
    glyph_wait(cx, "clipped wide glyph painted", |cx| {
        view.read_with(cx, |view, _| {
            glyph_presented_row(view, 7).chars().nth(4) == Some(glyph)
        })
    });
    let cover_span = glyph_region_bounds(window, &view, cx, 7, 4, 2);
    let painted = glyph_painted(window, &view, cx, cover_span);
    assert_eq!(painted.len(), 1, "one clipped glyph path: {painted:?}");
    assert!(
        painted[0].0.size.width.0 > cover_span.size.width.0 + 1.5
            && painted[0].0.size.height.0 > cover_span.size.height.0 + 1.5,
        "cover deliberately overflows the span: {:?} vs {cover_span:?}",
        painted[0].0
    );
    assert!(
        glyph_bounds_close(painted[0].0.intersect(&cover_span), cover_span),
        "visible pixels stop at the span: {:?} vs {cover_span:?}",
        painted[0].0.intersect(&cover_span)
    );
    assert!(
        glyph_bounds_close(painted[0].1, cover_span),
        "clip is the span before padding: {:?} vs {cover_span:?}",
        painted[0].1
    );

    // The foreground is a paint input, not part of the registration.
    serial += 1;
    glyph_register_ok(
        &view,
        cx,
        serial,
        &reply_path(serial),
        cp,
        &glyph_square(1000),
        1000,
        "",
    );
    serial += 1;
    glyph_show(&view, cx, serial, 8, 4, &glyph.to_string());
    glyph_wait(cx, "foreground baseline painted", |cx| {
        view.read_with(cx, |view, _| {
            glyph_presented_row(view, 8).chars().nth(4) == Some(glyph)
        })
    });
    let cell = glyph_region_bounds(window, &view, cx, 8, 4, 1);
    let before_fg = view.read_with(cx, |view, _| {
        let cols = view.cell_geom.unwrap().cols;
        view.grid_buf[8 * cols + 4].fg
    });
    let before = glyph_painted(window, &view, cx, cell);
    assert_eq!(before.len(), 1, "one default-foreground path: {before:?}");
    assert_eq!(before[0].2.as_solid(), Some(before_fg));
    serial += 1;
    glyph_show(&view, cx, serial, 8, 4, &format!("\x1b[31m{glyph}"));
    glyph_wait(cx, "foreground repainted", |cx| {
        let fg = view.read_with(cx, |view, _| {
            let cols = view.cell_geom.unwrap().cols;
            view.grid_buf[8 * cols + 4].fg
        });
        fg != before_fg
            && glyph_painted(window, &view, cx, cell)
                .first()
                .is_some_and(|path| path.2.as_solid() == Some(fg))
    });
    let after_fg = view.read_with(cx, |view, _| {
        let cols = view.cell_geom.unwrap().cols;
        view.grid_buf[8 * cols + 4].fg
    });
    let after = glyph_painted(window, &view, cx, cell);
    assert_eq!(after.len(), 1, "one recoloured glyph path: {after:?}");
    assert_eq!(
        after[0].2.as_solid(),
        Some(after_fg),
        "the glyph follows the cell's effective foreground"
    );

    // The raw codepoint remains the selectable/copyable text under the wide
    // render span.
    assert!(
        view.read_with(cx, |view, _| view.scrollback_text().contains(glyph)),
        "raw codepoint survives a width-two registration"
    );
    view.update(cx, |view, cx| {
        view.on_select_start(4, 2, false, 1, false, cx);
        view.on_select_update(6, 2, true, cx);
        view.copy_selection(cx);
    });
    let copied = cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert!(
        copied.as_deref().is_some_and(|text| text.contains(glyph)),
        "the copied selection keeps the raw codepoint: {copied:?}"
    );

    // The logical advance still wraps at the last column.
    serial += 1;
    glyph_show(&view, cx, serial, 9, 79, &format!("{glyph}Z"));
    glyph_wait(cx, "wide-glyph wrap", |cx| {
        view.read_with(cx, |view, _| {
            glyph_presented_row(view, 10).chars().next() == Some('Z')
        })
    });
    let last_row = view.read_with(cx, |view, _| glyph_presented_row(view, 9));
    assert_eq!(last_row.chars().nth(79), Some(glyph));
}

/// Derived rendering is computed from the live inputs, so registration
/// identity, cell geometry, display scale and foreground changes cannot serve
/// a stale picture; invalid sizing is rejected by the daemon and paints
/// nothing.
#[gpui_kit::test]
fn glyph_consumer_derived_rendering_follows_geometry_scale_and_revision(
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
    let reply_path = |serial: usize| replies.path().join(format!("r{serial}"));
    let cp = GLYPH_CP;
    let glyph = char::from_u32(cp).unwrap();
    let mut serial = 0usize;

    // Advance + baseline is already a non-default layout, so the very first
    // picture proves the placement slice rather than the old default path.
    serial += 1;
    glyph_register_ok(
        &view,
        cx,
        serial,
        &reply_path(serial),
        cp,
        &glyph_square(1000),
        1000,
        "aw=1000;lh=1000;ymin=0;size=advance;align=center,baseline",
    );
    serial += 1;
    glyph_show(&view, cx, serial, 2, 4, &glyph.to_string());
    glyph_wait(cx, "advance glyph painted", |cx| {
        view.read_with(cx, |view, _| {
            view.cell_geom.is_some() && glyph_presented_row(view, 2).chars().nth(4) == Some(glyph)
        })
    });
    let cell = glyph_region_bounds(window, &view, cx, 2, 4, 1);
    let first = glyph_painted(window, &view, cx, cell);
    assert_eq!(first.len(), 1, "one advance glyph path: {first:?}");
    assert!(
        (first[0].0.size.width.0 - cell.size.width.0).abs() <= 1.5
            && (first[0].0.size.height.0 - cell.size.width.0).abs() <= 1.5,
        "advance drives a square outline by the cell width: {:?} vs {cell:?}",
        first[0].0
    );
    let first_revision = view.read_with(cx, |view, _| {
        view.terminal.glyphs().get(cp).expect("registration").revision
    });

    // Many replacements, ending in a deliberately small outline. A cache
    // keyed by insertion slot or codepoint alone would still show an earlier
    // extent here.
    let params = "aw=1000;lh=1000;ymin=0;size=stretch;align=start,start;pad=0,0,0,0";
    for side in [1000, 400, 800, 200, 600, 300, 900, 250, 1000, 400, 700, 350] {
        serial += 1;
        glyph_register_ok(
            &view,
            cx,
            serial,
            &reply_path(serial),
            cp,
            &glyph_square(side),
            1000,
            params,
        );
    }
    glyph_wait(cx, "final replacement repainted", |cx| {
        let cell = glyph_region_bounds(window, &view, cx, 2, 4, 1);
        glyph_painted(window, &view, cx, cell).first().is_some_and(|path| {
            (path.0.size.width.0 - cell.size.width.0 * 0.35).abs() <= 1.5
                && (path.0.size.height.0 - cell.size.height.0 * 0.35).abs() <= 1.5
        })
    });
    let replaced_revision = view.read_with(cx, |view, _| {
        view.terminal.glyphs().get(cp).expect("registration").revision
    });
    assert!(
        replaced_revision > first_revision,
        "re-registration takes a new revision"
    );

    // A font-size change reflows the cell: the same registration must paint
    // at the new metrics without re-registering.
    let old_cell = glyph_region_bounds(window, &view, cx, 2, 4, 1);
    view.update(cx, |view, _| view.font_size = view.font_size * 2.);
    let _ = glyph_painted(window, &view, cx, old_cell);
    let grown_cell = glyph_region_bounds(window, &view, cx, 2, 4, 1);
    assert!(
        grown_cell.size.width.0 > old_cell.size.width.0 + 1.5,
        "the font-size change grew the cell: {grown_cell:?} vs {old_cell:?}"
    );
    let painted = glyph_painted(window, &view, cx, grown_cell);
    assert_eq!(painted.len(), 1, "one grown glyph path: {painted:?}");
    assert!(
        (painted[0].0.size.width.0 - grown_cell.size.width.0 * 0.35).abs() <= 1.5
            && (painted[0].0.size.height.0 - grown_cell.size.height.0 * 0.35).abs() <= 1.5,
        "the declaration scales with the new metrics: {:?} vs {grown_cell:?}",
        painted[0].0
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.terminal.glyphs().get(cp).expect("registration").revision),
        replaced_revision,
        "a font-size change does not re-register the outline"
    );

    // A display-scale change scales the same logical geometry.
    let before_scale = glyph_region_bounds(window, &view, cx, 2, 4, 1);
    cx.simulate_window_scale_factor_change(window, 2.);
    let _ = glyph_painted(window, &view, cx, before_scale);
    let after_scale = glyph_region_bounds(window, &view, cx, 2, 4, 1);
    assert!(
        after_scale.size.width.0 > before_scale.size.width.0 + 1.5,
        "the display scale grew the painted cell: {after_scale:?} vs {before_scale:?}"
    );
    let painted = glyph_painted(window, &view, cx, after_scale);
    assert_eq!(painted.len(), 1, "one rescaled glyph path: {painted:?}");
    assert!(
        (painted[0].0.size.width.0 - after_scale.size.width.0 * 0.35).abs() <= 1.5
            && (painted[0].0.size.height.0 - after_scale.size.height.0 * 0.35).abs() <= 1.5,
        "placement follows the device scale: {:?} vs {after_scale:?}",
        painted[0].0
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.terminal.glyphs().get(cp).expect("registration").revision),
        replaced_revision,
        "a display-scale change does not re-register the outline"
    );

    // Invalid sizing is rejected by the pinned parser and paints nothing.
    let invalid_cp = GLYPH_CP + 1;
    for (offset, params) in [
        (0, "size=huge"),
        (1, "pad=0,0,0,2"),
        (2, "width=3"),
    ] {
        serial += 1;
        let reply = glyph_send(
            &view,
            cx,
            serial,
            &reply_path(serial),
            &glyph_register_with(invalid_cp, &glyph_square(1000), 1000, params, 1),
            1,
        );
        assert_eq!(
            reply,
            format!(
                "\x1b_25a1;r;cp={invalid_cp:x};status=1;reason=malformed_payload\x1b\\"
            )
            .into_bytes(),
            "invalid sizing is rejected: {params} (case {offset})"
        );
    }
    serial += 1;
    let reply = glyph_send(
        &view,
        cx,
        serial,
        &reply_path(serial),
        &glyph_query(invalid_cp),
        1,
    );
    assert_eq!(
        reply,
        format!("\x1b_25a1;q;cp={invalid_cp:x};status=system\x1b\\").into_bytes(),
        "a rejected registration never enters the glossary"
    );
    let invalid_text = char::from_u32(invalid_cp).unwrap().to_string();
    serial += 1;
    glyph_show(&view, cx, serial, 6, 4, &invalid_text);
    glyph_wait(cx, "invalid glyph cell visible", |cx| {
        view.read_with(cx, |view, _| {
            view.cell_geom.is_some()
                && glyph_presented_row(view, 6).chars().nth(4)
                    == char::from_u32(invalid_cp)
        })
    });
    let rejected_cell = glyph_region_bounds(window, &view, cx, 6, 4, 1);
    assert!(
        glyph_painted(window, &view, cx, rejected_cell).is_empty(),
        "an invalid layout leaves the cell to ordinary text"
    );
}
