fn graphics_resize_text_row(view: &TerminalView, needle: &str) -> Option<usize> {
    use rio_vt::crosswords::pos::{Column, Pos};
    let term = view.terminal.term.lock();
    (0..term.screen_lines()).find(|&row| {
        let text: String = (0..term.columns())
            .flat_map(|col| term.grid.cell_text(Pos::new(Line(row as i32), Column(col))))
            .collect();
        text.contains(needle)
    })
}

fn graphics_resize_assert_geometry(view: &TerminalView, image_id: u32, text: &str, row: usize) {
    assert_eq!(graphics_resize_text_row(view, text), Some(row));
    let images = graphics_visible_images(view);
    let image = images.iter().find(|image| image.placement.image_id == image_id)
        .expect("production consumer decoded the retained image");
    let (crop, full) = image.paint_bounds(point(px(0.), px(0.)), px(10.), px(20.), 0, 0, 1.);
    assert_eq!(crop.origin.y, px(row as f32 * 20.), "image and actual text row must share geometry");
    assert_eq!(full.origin.y, crop.origin.y);
    assert_eq!(crop.size, gpui_kit::size(px(10.), px(10.)));
}

fn graphics_resize_history() -> Vec<u8> {
    let mut bytes = b"\x1b[2J\x1b[3J\x1b[H".to_vec();
    for row in 0..30 {
        if row > 0 { bytes.extend_from_slice(b"\r\n"); }
        bytes.extend_from_slice(format!("HISTORY_{row:02}").as_bytes());
    }
    bytes
}

fn graphics_resize_real_daemon(cx: &mut TestAppContext, grow: bool) {
    use std::io::Write as _;
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    let stream = daemon.stream(pane);
    // This writer uses the real attached pane socket, not RemoteTerminal::resize:
    // only the daemon's ordered DaemonMsg::Size can resize the consumer grid.
    let mut controller = stream.try_clone().unwrap();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let built = std::rc::Rc::new(std::cell::RefCell::new(None));
    let out = built.clone();
    let _window = cx.add_window(move |window, cx| {
        let terminal = RemoteTerminal::from_stream_with(
            stream, TermSize::new(80, 24), Vec::new(),
            if cfg!(windows) { PtySource::LocalConpty } else { PtySource::Raw },
        ).unwrap();
        let view = cx.new(|cx| TerminalView::with_terminal(terminal, pane, window, cx));
        *out.borrow_mut() = Some(view.clone());
        gpui_kit::component::Root::new(view, window, cx)
    });
    let view = built.borrow_mut().take().unwrap();
    let mut bytes = graphics_resize_history();
    let (anchor, label, image_id, rows, expected) = if grow {
        (6, "GROW_ANCHOR", 421, 30, if cfg!(windows) { 5 } else { 11 })
    } else {
        (10, "SHRINK_ANCHOR", 422, 18, if cfg!(windows) { 3 } else { 9 })
    };
    bytes.extend_from_slice(format!(
        "\x1b[{anchor};1H\x1b_Ga=T,i={image_id},f=32,s=1,v=1,c=1,r=1,C=1,q=2;/wAA/w==\x1b\\{label}"
    ).as_bytes());
    // For shrink the last written row is below the cursor. The generic Rio
    // resize drops that content; ConPTY must retain it at the new bottom.
    if !grow { bytes.extend_from_slice(b"\x1b[24;1HLAST_WRITTEN\x1b[10;14H"); }
    graphics_send_fixture(&view, cx, 421, &bytes);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if view.read_with(cx, |view, _| graphics_visible_images(view).iter().any(|image| image.placement.image_id == image_id)) { break; }
        assert!(std::time::Instant::now() < deadline, "initial image decode did not finish");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    view.read_with(cx, |view, _| graphics_resize_assert_geometry(view, image_id, label, anchor - 1));
    graphics_assert_production_paint(_window.into(), &view, cx);
    ClientMsg::Resize(crate::daemon::protocol::WinSize { cols: 80, rows, cell_w: 10, cell_h: 20 })
        .encode(&mut controller).unwrap();
    controller.flush().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if view.read_with(cx, |view, _| view.terminal.term.lock().screen_lines() == usize::from(rows)) { break; }
        assert!(std::time::Instant::now() < deadline, "real daemon Size was not consumed");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // The sender's APC round-trip is also an ordered graphics/layout barrier.
    graphics_send_fixture(&view, cx, 422, b"\x1b[0m");
    view.read_with(cx, |view, _| {
        graphics_resize_assert_geometry(view, image_id, label, expected);
        if !grow && cfg!(windows) {
            assert_eq!(graphics_resize_text_row(view, "LAST_WRITTEN"), Some(17));
        }
    });
    graphics_assert_production_paint(_window.into(), &view, cx);
    if grow {
        // A newly placed image must agree too: an origin mismatch affects both
        // retained and subsequent placements, not just the original anchor.
        graphics_send_fixture(&view, cx, 423, b"\x1b[8;1H\x1b_Ga=T,i=423,f=32,s=1,v=1,c=1,r=1,C=1,q=2;/wAA/w==\x1b\\NEW_ANCHOR");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            if view.read_with(cx, |view, _| graphics_visible_images(view).iter().any(|image| image.placement.image_id == 423)) { break; }
            assert!(std::time::Instant::now() < deadline, "post-resize image decode did not finish");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        view.read_with(cx, |view, _| graphics_resize_assert_geometry(view, 423, "NEW_ANCHOR", 7));
        graphics_assert_production_paint(_window.into(), &view, cx);
    }
    daemon.panes().kill(pane).unwrap();
}

#[gpui_kit::test]
fn graphics_consumer_real_daemon_grow_preserves_conpty_history_row_geometry(cx: &mut TestAppContext) {
    graphics_resize_real_daemon(cx, true);
}

#[gpui_kit::test]
fn graphics_consumer_real_daemon_shrink_tracks_last_written_below_cursor(cx: &mut TestAppContext) {
    graphics_resize_real_daemon(cx, false);
}

#[gpui_kit::test]
fn graphics_consumer_remote_raw_size_keeps_standard_pty_history_growth(cx: &mut TestAppContext) {
    use std::io::Write as _;
    // A Linux PTY routed to a Windows desktop is Raw even on Windows. This
    // production reader regression deliberately does not infer mode from cfg!.
    let (window, mut daemon) = harness_on(cx, PtySource::Raw);
    let mut bytes = graphics_resize_history();
    bytes.extend_from_slice(b"\x1b[6;1HREMOTE_RAW_ANCHOR");
    DaemonMsg::Output(bytes).encode(&mut daemon).unwrap();
    DaemonMsg::Size(crate::daemon::protocol::WinSize { cols: 80, rows: 30, cell_w: 10, cell_h: 20 })
        .encode(&mut daemon).unwrap();
    daemon.flush().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let ready = window.update(cx, |view, _, _| {
            let rows = view.terminal.term.lock().screen_lines();
            rows == 30 && graphics_resize_text_row(view, "REMOTE_RAW_ANCHOR") == Some(11)
        }).unwrap();
        if ready { break; }
        assert!(std::time::Instant::now() < deadline, "Raw Size must pull six history rows, unlike ConPTY");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
