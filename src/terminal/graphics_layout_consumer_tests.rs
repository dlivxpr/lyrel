fn graphics_layout_harness(
    cx: &mut TestAppContext,
) -> (
    GraphicsDaemonProcess,
    u64,
    Entity<TerminalView>,
    gpui_kit::AnyWindowHandle,
) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    let stream = daemon.stream(pane);
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let built = std::rc::Rc::new(std::cell::RefCell::new(None));
    let out = built.clone();
    let window = cx.add_window(move |window, cx| {
        let source = if cfg!(windows) {
            PtySource::LocalConpty
        } else {
            PtySource::Raw
        };
        let terminal =
            RemoteTerminal::from_stream_with(stream, TermSize::new(80, 24), Vec::new(), source)
                .unwrap();
        let view = cx.new(|cx| TerminalView::with_terminal(terminal, pane, window, cx));
        *out.borrow_mut() = Some(view.clone());
        gpui_kit::component::Root::new(view, window, cx)
    });
    let view = built.borrow_mut().take().unwrap();
    graphics_assert_production_paint_at(window.into(), &view, cx, TermSize::new(80, 24));
    graphics_send_fixture(&view, cx, 0, b"\x1b[0m");
    (daemon, pane, view, window.into())
}

fn graphics_layout_wait_image(view: &Entity<TerminalView>, cx: &mut TestAppContext, id: u32) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if view.read_with(cx, |view, _| {
            graphics_visible_images(view)
                .iter()
                .any(|image| image.placement.image_id == id)
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "production image decoder did not finish for {id}"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn graphics_assert_production_paint(
    window: gpui_kit::AnyWindowHandle,
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
) {
    let size = view.read_with(cx, |view, _| {
        let term = view.terminal.term.lock();
        TermSize::new(term.columns(), term.screen_lines())
    });
    graphics_assert_production_paint_at(window, view, cx, size);
}

fn graphics_assert_production_paint_at(
    window: gpui_kit::AnyWindowHandle,
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    size: TermSize,
) {
    use gpui_kit::{Hsla, SharedString, TextRun};
    use std::sync::atomic::Ordering;
    let (target, cols, rows, scale) = window
        .update(cx, |_, window, cx| {
            let view = view.read(cx);
            let sample = window.text_system().shape_line(
                SharedString::new_static("M"),
                view.font_size,
                &[TextRun {
                    len: 1,
                    font: view.font.clone(),
                    color: Hsla::default(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            );
            let width = sample.width.max(px(1.));
            let height = px((view.font_size.as_f32() * view.line_height_mul).round()).max(px(1.));
            (
                gpui_kit::size(
                    width * size.cols as f32 + px(2. * GRID_PAD_X + 0.5),
                    height * size.rows as f32 + px(2. * GRID_PAD_Y + 0.5),
                ),
                size.cols,
                size.rows,
                window.scale_factor(),
            )
        })
        .unwrap();
    // Preserve the requested grid through the actual prepaint sizing callback;
    // otherwise a default test window would silently resize the real daemon.
    cx.simulate_window_resize(window, target);
    let previously_painted = view.update(cx, |view, _| {
        let images = graphics_visible_images(view);
        let previous: Vec<_> = images
            .iter()
            .map(|image| (image.painted.clone(), image.painted.load(Ordering::Acquire)))
            .collect();
        for image in images {
            image.painted.store(false, Ordering::Release);
        }
        view.cell_geom = None;
        previous
    });
    window
        .update(cx, |_, window, _| {
            window.activate_window();
            window.refresh();
        })
        .unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(400));
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        let geom = view.cell_geom.expect("rooted production TerminalElement must paint a fresh frame");
        assert_eq!((geom.cols, geom.rows), (cols, rows));
        let viewport = Bounds::new(geom.origin, gpui_kit::size(geom.cell_width * cols as f32, geom.line_height * rows as f32));
        let term = view.terminal.term.lock();
        let offset = term.display_offset() as i32;
        drop(term);
        let images = graphics_visible_images(view);
        for image in &images {
            let expected = images.iter().filter(|other| std::sync::Arc::ptr_eq(&image.painted, &other.painted)).any(|other| {
                let (crop, _) = other.paint_bounds(geom.origin, geom.cell_width, geom.line_height, 0, offset, scale);
                let visible = crop.intersect(&viewport);
                visible.size.width > px(0.) && visible.size.height > px(0.)
            });
            assert_eq!(image.painted.load(Ordering::Acquire), expected, "fresh production paint claim must match the current visible resource geometry; not GPU pixels");
        }
    });
    // The flag also tracks atlas ownership. An offscreen texture painted in a
    // previous frame must still be retired when a later user action removes it.
    for (painted, previous) in previously_painted {
        if previous {
            painted.store(true, Ordering::Release);
        }
    }
}

#[gpui_kit::test]
fn graphics_consumer_real_daemon_resize_keeps_clipped_texture_slice(cx: &mut TestAppContext) {
    let (daemon, pane, view, window) = graphics_layout_harness(cx);
    graphics_send_fixture(&view, cx, 1, b"\x1b[?1049h\x1b[2J\x1b[1;1H\x1b_Ga=T,i=1,c=1,r=5,C=1,f=32,s=1,v=10,q=2;/wAA//8AAP//AAD//wAA//8AAP//AAD//wAA//8AAP//AAD//wAA/w==\x1b\\\x1b[24;1H\n");
    graphics_layout_wait_image(&view, cx, 1);
    view.update(cx, |view, _| {
        view.terminal.resize(TermSize::new(40, 24), 10, 20)
    });
    graphics_send_fixture(&view, cx, 2, b"\x1b[0m");
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        assert_eq!(
            images.len(),
            1,
            "resizing a partially clipped placement must not expire its retained rows"
        );
        assert_eq!(
            (
                images[0].placement.clip_top,
                images[0].placement.clip_bottom
            ),
            (1, 5)
        );
        let (crop, full) =
            images[0].paint_bounds(point(px(0.), px(0.)), px(10.), px(20.), 0, 0, 1.);
        assert_eq!(
            crop,
            Bounds::new(point(px(0.), px(0.)), gpui_kit::size(px(10.), px(80.)))
        );
        assert_eq!(
            full,
            Bounds::new(point(px(0.), px(-20.)), gpui_kit::size(px(10.), px(100.)))
        );
        assert_eq!(&images[0].data.as_ref().unwrap().as_bytes(0).unwrap()[..4], [0, 0, 255, 255]);
    });
    graphics_assert_production_paint(window, &view, cx);
    graphics_send_fixture(&view, cx, 3, b"\x1b[24;1H\n\n\n\n");
    assert!(view.read_with(cx, |view, _| graphics_visible_images(view).is_empty()));
    graphics_assert_production_paint(window, &view, cx);
    view.update(cx, |view, _| {
        view.terminal.resize(TermSize::new(40, 24), 1, 2)
    });
    graphics_send_fixture(&view, cx, 4, b"\x1b[1;1H\x1b_Ga=T,i=2,C=1,f=32,s=1,v=10,q=2;/wAA//8AAP//AAD//wAA//8AAP//AAD//wAA//8AAP//AAD//wAA/w==\x1b\\\x1b[24;1H\n");
    graphics_layout_wait_image(&view, cx, 2);
    view.read_with(cx, |view, _| {
        let image = graphics_visible_images(view).remove(0);
        assert_eq!(
            (image.placement.clip_top, image.placement.clip_bottom),
            (1, 5)
        );
    });
    view.update(cx, |view, _| {
        view.terminal.resize(TermSize::new(40, 24), 10, 20)
    });
    graphics_send_fixture(&view, cx, 5, b"\x1b[0m");
    assert!(
        view.read_with(cx, |view, _| graphics_visible_images(view).is_empty()),
        "a font change cannot retain an empty or inverted slice"
    );
    graphics_assert_production_paint(window, &view, cx);
    view.update(cx, |view, _| {
        view.terminal.resize(TermSize::new(30, 24), 10, 20)
    });
    graphics_send_fixture(&view, cx, 6, b"\x1b[0m");
    assert!(view.read_with(cx, |view, _| graphics_visible_images(view).is_empty()));
    graphics_assert_production_paint(window, &view, cx);
    daemon.panes().kill(pane).unwrap();
}

#[gpui_kit::test]
fn graphics_consumer_real_daemon_history_expiration_clips_before_resize(cx: &mut TestAppContext) {
    let (daemon, pane, view, window) = graphics_layout_harness(cx);
    graphics_send_fixture(&view, cx, 1, b"\x1b[2J\x1b[3J\x1b[1;1H\x1b_Ga=T,i=1,c=1,r=5,C=1,f=32,s=1,v=10,q=2;/wAA//8AAP//AAD//wAA//8AAP//AAD//wAA//8AAP//AAD//wAA/w==\x1b\\\x1b[24;1H");
    graphics_layout_wait_image(&view, cx, 1);
    graphics_send_fixture(&view, cx, 2, &b"\r\n".repeat(10_002));
    view.update(cx, |view, _| {
        view.terminal.resize(TermSize::new(40, 24), 10, 20)
    });
    graphics_send_fixture(&view, cx, 3, b"\x1b[0m");
    view.update(cx, |view, _| {
        view.terminal.term.lock().scroll_display(Scroll::Top)
    });
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        assert_eq!(images.len(), 1);
        assert_eq!(
            (
                images[0].placement.clip_top,
                images[0].placement.clip_bottom
            ),
            (2, 5)
        );
        let offset = view.terminal.term.lock().display_offset() as i32;
        let (crop, full) =
            images[0].paint_bounds(point(px(0.), px(0.)), px(10.), px(20.), 0, offset, 1.);
        assert_eq!(
            crop.origin.y,
            px(0.),
            "the oldest retained slice starts at the history viewport edge"
        );
        assert_eq!(crop.size.height, px(60.));
        assert_eq!(
            full.origin.y,
            px(-40.),
            "expired source rows cannot return after reflow"
        );
    });
    graphics_assert_production_paint(window, &view, cx);
    daemon.panes().kill(pane).unwrap();
}

#[gpui_kit::test]
fn graphics_consumer_real_daemon_anonymous_expiration_restores_admission(cx: &mut TestAppContext) {
    let (daemon, pane, view, window) = graphics_layout_harness(cx);
    graphics_send_fixture(&view, cx, 1, b"\x1b[?1049h\x1b[2J\x1b[H");
    for serial in 2..18 {
        graphics_send_fixture(
            &view,
            cx,
            serial,
            &b"\x1b_Ga=T,f=32,s=1,v=1,C=1,q=2;/wAA/w==\x1b\\".repeat(64),
        );
    }
    graphics_send_fixture(&view, cx, 18, b"\x1b[24;1H\n");
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        let term = view.terminal.term.lock();
        assert!(images.is_empty(), "anonymous placements after expiration: grid={}x{}, history={}, evicted={}, remaining={:?}", term.columns(), term.screen_lines(), term.grid.history_size(), term.grid.lines_evicted(), images.iter().take(3).map(|image| &image.placement).collect::<Vec<_>>());
    });
    graphics_assert_production_paint(window, &view, cx);
    graphics_send_fixture(
        &view,
        cx,
        19,
        b"\x1b[6;2H\x1b_Ga=T,i=2000,f=32,s=1,v=1,c=1,r=1,C=1,q=2;/wAA/w==\x1b\\",
    );
    graphics_layout_wait_image(&view, cx, 2000);
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        assert_eq!(
            images.len(),
            1,
            "all 1,024 expired anonymous placements release their inaccessible resource identities"
        );
        assert_eq!(images[0].placement.image_id, 2000);
        let (crop, _) = images[0].paint_bounds(point(px(0.), px(0.)), px(10.), px(20.), 0, 0, 1.);
        assert_eq!(crop.origin, point(px(10.), px(100.)));
        assert_eq!(images[0].data.as_ref().unwrap().as_bytes(0).unwrap(), [0, 0, 255, 255]);
    });
    graphics_assert_production_paint(window, &view, cx);
    daemon.panes().kill(pane).unwrap();
}

#[gpui_kit::test]
fn graphics_consumer_real_daemon_deccolm_scrolls_match_text(cx: &mut TestAppContext) {
    let (daemon, pane, view, window) = graphics_layout_harness(cx);
    let mut serial = 0;
    for mode in [b'h', b'l'] {
        for (operation, expected_row) in [
            ("\x1b[10;1H\n", 5),
            ("\x1b[3;1H\x1bM", 5),
            ("\x1b[S", 4),
            ("\x1b[T", 6),
        ] {
            serial += 1;
            graphics_send_fixture(&view, cx, serial, b"\x1b[?1049h\x1b[2J\x1b[3;10r\x1b[6;2H\x1b_Ga=T,i=1,c=1,r=1,C=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\");
            graphics_layout_wait_image(&view, cx, 1);
            let mut output = vec![b'\x1b', b'[', b'?', b'3', mode];
            output.extend_from_slice(b"\x1b[6;2HX");
            output.extend_from_slice(operation.as_bytes());
            serial += 1;
            graphics_send_fixture(&view, cx, serial, &output);
            view.read_with(cx, |view, _| {
                let images = graphics_visible_images(view);
                assert_eq!(images.len(), 1);
                let (crop, _) =
                    images[0].paint_bounds(point(px(0.), px(0.)), px(10.), px(20.), 0, 0, 1.);
                assert_eq!(crop.origin, point(px(10.), px(expected_row as f32 * 20.)));
                let term = view.terminal.term.lock();
                assert_eq!(
                    term.grid[Line(expected_row)][Column(1)].c(),
                    'X',
                    "text and image use the full-screen region after DECCOLM {mode}"
                );
            });
            graphics_assert_production_paint(window, &view, cx);
            serial += 1;
            graphics_send_fixture(&view, cx, serial, b"\x1b[?1049l");
            graphics_assert_production_paint(window, &view, cx);
        }
    }
    daemon.panes().kill(pane).unwrap();
}
