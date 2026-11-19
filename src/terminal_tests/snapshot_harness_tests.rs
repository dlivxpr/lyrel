#[gpui_kit::test]
fn reused_viewport_discards_erased_text_and_outside_grid_padding(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (app, mut vcx, _stream) = crate::ui::app::test_window::harness_with_pane(cx);
    let view = app.update_in(&mut vcx, |app, _, _| match &app.tabs[0].pane.leaves()[0] {
        crate::ui::pane::PaneSlot::Ready(view) => view.clone(),
        _ => panic!("the fixture must have a terminal"),
    });
    let terminal = cx.update(|cx| view.read(cx).terminal.term.clone());
    let (rows, cols) = {
        let mut term = terminal.lock();
        rio_vt::performer::handler::Processor::default().advance(
            &mut *term,
            "\x1b[H\x1b[31mA\u{301}\x1b[4;58;5;9m\u{10eeee}\u{305}\u{305}".as_bytes(),
        );
        (term.screen_lines(), term.columns())
    };
    let element = TerminalElement::new(view);
    let mut buf = Vec::new();
    let build = |buf: &mut Vec<RenderCell>, cx: &mut gpui_kit::TestAppContext| {
        cx.update(|cx| {
            let colors = PaintColors::resolve(cx.theme(), cx);
            assert!(
                element
                    .build_grid(
                        &colors,
                        buf,
                        rows + 1,
                        cols + 1,
                        false,
                        cx,
                        1.,
                        Rgba::default(),
                        true,
                        false,
                    )
                    .is_ok()
            );
        });
    };
    build(&mut buf, cx);
    assert_eq!(buf[0].c, 'A');
    assert_eq!(buf[0].marks.as_deref(), Some(&['\u{301}'][..]));
    assert_eq!(
        buf[1].c, ' ',
        "unresolved image placeholders never render PUA glyphs"
    );
    assert!(buf[1].marks.is_none());
    assert_eq!(buf[1].underline, UnderlineKind::None);
    // Simulate content left by a formerly wider/taller source grid.
    let old = buf[0].clone();
    buf[cols] = old.clone();
    buf[rows * (cols + 1)] = old;
    rio_vt::performer::handler::Processor::default()
        .advance(&mut *terminal.lock(), b"\x1b[H\x1b[0m\x1b[2K");
    build(&mut buf, cx);
    for cell in [&buf[0], &buf[cols], &buf[rows * (cols + 1)]] {
        assert!(
            is_blank(cell),
            "erased text or old padding must not reappear"
        );
        assert!(cell.marks.is_none());
        assert!(!cell.selected && !cell.link_hover && !cell.match_hit && !cell.match_current);
    }
}

#[gpui_kit::test]
fn first_snapshot_never_overrides_an_open_presentation_hold(cx: &mut gpui_kit::TestAppContext) {
    let (app, mut vcx, _stream) = crate::ui::app::test_window::harness_with_pane(cx);
    let view = app.update_in(&mut vcx, |app, _, _| match &app.tabs[0].pane.leaves()[0] {
        crate::ui::pane::PaneSlot::Ready(view) => view.clone(),
        _ => panic!("the fixture must have a terminal"),
    });
    let terminal = cx.update(|cx| view.read(cx).terminal.term.clone());
    let (rows, cols) = {
        let mut term = terminal.lock();
        rio_vt::performer::handler::Processor::default()
            .advance(&mut *term, b"\x1b[?2026h\x1b[Hfirst-hidden");
        (term.screen_lines(), term.columns())
    };
    let element = TerminalElement::new(view);
    let mut cells = Vec::new();
    cx.update(|cx| {
        let colors = PaintColors::resolve(cx.theme(), cx);
        assert!(matches!(
            element.build_grid(
                &colors,
                &mut cells,
                rows,
                cols,
                false,
                cx,
                1.,
                Rgba::default(),
                true,
                false,
            ),
            Err(SnapshotUnavailable::Synchronized)
        ));
        assert!(
            cells.is_empty(),
            "must_block is not permission to publish a partial first frame"
        );
        terminal.lock().end_sync_update();
        let frame = element
            .build_grid(
                &colors,
                &mut cells,
                rows,
                cols,
                false,
                cx,
                1.,
                Rgba::default(),
                true,
                false,
            )
            .unwrap();
        assert!(
            cells
                .iter()
                .take(12)
                .map(|cell| cell.c)
                .collect::<String>()
                .starts_with("first-hidden")
        );
        assert!(frame.cursor.is_some());
    });
}

fn glyph_test_outline(side: i16) -> rio_graphics::glyph::glyf_decode::Outline {
    rio_graphics::glyph::glyf_decode::Outline {
        contours: Vec::new(),
        x_min: 0,
        y_min: 0,
        x_max: i32::from(side),
        y_max: i32::from(side),
    }
}

#[gpui_kit::test]
fn glyph_contended_snapshot_never_rewrites_the_captured_picture(
    cx: &mut gpui_kit::TestAppContext,
) {
    use rio_graphics::glyph_layout::GlyphLayout;
    let cp = 0x100000u32;
    let ch = char::from_u32(cp).unwrap();
    let (app, mut vcx, _stream) = crate::ui::app::test_window::harness_with_pane(cx);
    let view = app.update_in(&mut vcx, |app, _, _| match &app.tabs[0].pane.leaves()[0] {
        crate::ui::pane::PaneSlot::Ready(view) => view.clone(),
        _ => panic!("the fixture must have a terminal"),
    });
    let terminal = cx.update(|cx| view.read(cx).terminal.term.clone());
    let (rows, cols) = {
        let mut term = terminal.lock();
        rio_vt::performer::handler::Processor::default()
            .advance(&mut *term, format!("\x1b[H{ch}").as_bytes());
        (term.screen_lines(), term.columns())
    };
    cx.update(|cx| {
        view.read(cx).terminal.glyphs().apply(
            cp,
            glyph_test_outline(10),
            1000,
            1,
            GlyphLayout::default(),
        );
    });
    let element = TerminalElement::new(view.clone());
    let colors = cx.update(|cx| PaintColors::resolve(cx.theme(), cx));
    let mut buf = Vec::new();
    cx.update(|cx| {
        assert!(
            element
                .build_grid(
                    &colors,
                    &mut buf,
                    rows,
                    cols,
                    false,
                    cx,
                    1.,
                    Rgba::default(),
                    true,
                    false,
                )
                .is_ok()
        );
    });
    let captured = buf[0]
        .glyph
        .as_ref()
        .expect("the snapshot captured its registration")
        .revision;
    assert_eq!(buf[0].glyph.as_ref().unwrap().outline.x_max, 10);

    cx.update(|cx| {
        view.read(cx).terminal.glyphs().apply(
            cp,
            glyph_test_outline(20),
            1000,
            1,
            GlyphLayout::default(),
        );
    });
    let guard = terminal.lock();
    cx.update(|cx| {
        assert!(matches!(
            element.build_grid(
                &colors,
                &mut buf,
                rows,
                cols,
                false,
                cx,
                1.,
                Rgba::default(),
                false,
                true,
            ),
            Err(SnapshotUnavailable::Contended)
        ));
    });
    drop(guard);
    assert_eq!(
        buf[0].glyph.as_ref().unwrap().revision,
        captured,
        "a contended snapshot leaves the retained picture untouched"
    );
    assert_eq!(buf[0].glyph.as_ref().unwrap().outline.x_max, 10);

    cx.update(|cx| {
        assert!(
            element
                .build_grid(
                    &colors,
                    &mut buf,
                    rows,
                    cols,
                    false,
                    cx,
                    1.,
                    Rgba::default(),
                    true,
                    true,
                )
                .is_ok()
        );
    });
    assert_ne!(
        buf[0].glyph.as_ref().unwrap().revision,
        captured,
        "the next successful snapshot adopts the replacement"
    );
    assert_eq!(buf[0].glyph.as_ref().unwrap().outline.x_max, 20);
}
