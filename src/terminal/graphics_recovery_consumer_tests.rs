// These are production-consumer claims: real PTY bytes, retained daemon state,
// decoder output and TerminalElement paint/geometry. The headless GPUI window
// does not certify final GPU pixels or replace the native reopen acceptance.

fn graphics_recovery_wait(
    cx: &mut TestAppContext,
    reason: &str,
    mut ready: impl FnMut(&mut TestAppContext) -> bool,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !ready(cx) {
        assert!(std::time::Instant::now() < deadline, "{reason}");
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(5));
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn graphics_recovery_fresh(
    daemon: &GraphicsDaemonProcess,
    pane: u64,
    cx: &mut TestAppContext,
    serial: usize,
) -> (Entity<TerminalView>, gpui_kit::AnyWindowHandle) {
    graphics_recovery_fresh_at(daemon, pane, cx, serial, TermSize::new(80, 24))
}

fn graphics_recovery_fresh_at(
    daemon: &GraphicsDaemonProcess,
    pane: u64,
    cx: &mut TestAppContext,
    serial: usize,
    size: TermSize,
) -> (Entity<TerminalView>, gpui_kit::AnyWindowHandle) {
    let stream = daemon.stream_with_capacity_at(
        pane,
        64 << 20,
        128 << 20,
        crate::daemon::protocol::WinSize {
            cols: size.cols as u16,
            rows: size.rows as u16,
            cell_w: 10,
            cell_h: 20,
        },
    );
    let source = if cfg!(windows) { PtySource::LocalConpty } else { PtySource::Raw };
    let (view, window) = graphics_build_rooted_consumer_at(cx, pane, stream, Vec::new(), size, source);
    graphics_assert_production_paint_at(window, &view, cx, size);
    // A unique real-text barrier, never a KITTY_DONE_0 left in old replay.
    graphics_recovery_stats(&view, cx, serial);
    (view, window)
}

fn graphics_recovery_destroy(
    view: Entity<TerminalView>,
    window: gpui_kit::AnyWindowHandle,
    cx: &mut TestAppContext,
) {
    let weak_view = view.downgrade();
    let (store_released, pixels) = view.read_with(cx, |view, _| {
        let store = view.terminal.images();
        let pixels: Vec<_> = store
            .snapshot()
            .iter()
            .filter_map(|image| image.data.as_ref().map(std::sync::Arc::downgrade))
            .collect();
        (store.release_probe(), pixels)
    });
    view.update(cx, |view, _| view.terminal.detach_link());
    // The headless platform keeps its active TestWindow (and that window's
    // production IME handler, which owns a strong TerminalView) independently
    // of App's window map. Simulate the native blur before removing the window
    // so the test platform cannot keep the closed consumer alive forever.
    gpui_kit::VisualTestContext::from_window(window, cx).deactivate_window();
    window
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
    drop(view);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        // Dropping the final Entity handle queues its value for release. The
        // normal App::update turn flushes that queue and invokes on_release;
        // running the test dispatcher alone does not drop the entity value.
        cx.update(|_| {});
        let view_alive = weak_view.upgrade().is_some();
        let store_alive = !store_released();
        let textures_alive = pixels
            .iter()
            .filter(|pixel| pixel.upgrade().is_some())
            .count();
        if !view_alive && !store_alive && textures_alive == 0 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "old consumer did not actually die: view_alive={view_alive}, store_alive={store_alive}, textures_alive={textures_alive}"
        );
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(5));
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn graphics_recovery_stats(
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    serial: usize,
) -> String {
    let request = format!("STAT {serial} 0 00 0\n");
    view.read_with(cx, |view, _| view.terminal.write(request.into_bytes()));
    let marker = format!("KITTY_STATS_{serial} ");
    let done = format!("KITTY_DONE_{serial};");
    graphics_recovery_wait(
        cx,
        "real sender statistics and ordered text barrier did not arrive",
        |cx| {
            view.read_with(cx, |view, _| {
                let text = view.scrollback_text();
                text.contains(&marker) && text.contains(&done)
            })
        },
    );
    view.read_with(cx, |view, _| {
        let text = view.scrollback_text();
        text.split_once(&marker)
            .unwrap()
            .1
            .split_whitespace()
            .take(4)
            .collect::<Vec<_>>()
            .join(" ")
    })
}

#[track_caller]
fn graphics_recovery_pixels(
    view: &Entity<TerminalView>,
    cx: &mut TestAppContext,
    id: u32,
    expected: &[u8],
) {
    let caller = std::panic::Location::caller();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let matched = view.read_with(cx, |view, _| {
            graphics_visible_images(view).iter().any(|image| {
                image.placement.image_id == id
                    && !image.evicted
                    && image
                        .data
                        .as_ref()
                        .is_some_and(|data| data.as_bytes(0).unwrap() == expected)
            })
        });
        if matched {
            break;
        }
        if std::time::Instant::now() >= deadline {
            let state = view.read_with(cx, |view, _| {
                let images: Vec<_> = graphics_visible_images(view)
                    .iter()
                    .map(|image| {
                        (
                            image.placement.image_id,
                            image.placement.placement_id,
                            image.placement.screen,
                            image.anchor_row,
                            image.anchor_col,
                            image.evicted,
                            image.data.as_ref().map(|data| {
                                let bytes = data.as_bytes(0).unwrap();
                                (bytes.len(), bytes[..bytes.len().min(32)].to_vec())
                            }),
                        )
                    })
                    .collect();
                let term = view.terminal.term.lock();
                let grid = (
                    term.mode().contains(Mode::ALT_SCREEN),
                    term.columns(),
                    term.screen_lines(),
                    term.display_offset(),
                );
                drop(term);
                (
                    images,
                    grid,
                    view.terminal.exited,
                    view.terminal.child_exited(),
                    view.scrollback_text(),
                    view.terminal.images().recovery_diagnostics(),
                )
            });
            panic!(
                "retained image i={id} did not decode expected BGRA={expected:?}; phase={caller}; actual (placements [image_id, placement_id, screen, row, col, evicted, (pixel bytes, prefix)], grid [alt, cols, rows, offset], exited, child_exited, text, complete cache state)={state:?}"
            );
        }
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(5));
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[gpui_kit::test]
fn graphics_consumer_fresh_recovers_shared_crops_placeholder_history_without_sender_redraw(
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
    let (prior, prior_window) = graphics_recovery_fresh(&daemon, pane, cx, 100);
    graphics_send_fixture(&prior, cx, 101, "\x1b[2J\x1b[3J\x1b[6;1HRECOVERY-HISTORY\x1b[6;5H\x1b_Ga=T,i=7,p=11,c=2,r=1,x=1,y=0,w=2,h=1,C=1,f=32,s=4,v=1,q=2;/wAA/wD/AP8AAP///////w==\x1b\\\x1b_Ga=p,i=7,p=12,U=1,c=4,r=1,q=2\x1b\\\x1b[8;5H\x1b[38;5;7;58;5;12m\u{10eeee}\u{305}\u{305}\u{10eeee}\u{10eeee}\u{10eeee}\x1b[0m\x1b[10;9H\x1b_Ga=p,i=7,p=13,c=2,r=1,x=2,w=2,C=1,q=2\x1b\\".as_bytes());
    graphics_recovery_pixels(
        &prior,
        cx,
        7,
        &[
            0, 0, 255, 255, 0, 255, 0, 255, 255, 0, 0, 255, 255, 255, 255, 255,
        ],
    );
    graphics_assert_production_paint(prior_window, &prior, cx);
    graphics_send_fixture(&prior, cx, 102, &b"\r\n".repeat(60));
    graphics_send_fixture(&prior, cx, 103, b"\x1b[14;1HRECOVERY-SCREEN\x1b[14;20H\x1b_Ga=T,i=8,p=14,c=2,r=1,C=1,f=32,s=1,v=1,q=2;AP8A/w==\x1b\\");
    graphics_recovery_pixels(&prior, cx, 8, &[0, 255, 0, 255]);
    graphics_assert_production_paint(prior_window, &prior, cx);
    let stopped = graphics_recovery_stats(&prior, cx, 104);
    assert!(
        stopped.contains("uploads=2") && stopped.contains("placements=2"),
        "sender must account for the actual upload and independent placement commands: {stopped}"
    );
    graphics_recovery_destroy(prior, prior_window, cx);

    let (view, window) = graphics_recovery_fresh(&daemon, pane, cx, 105);
    graphics_recovery_pixels(&view, cx, 8, &[0, 255, 0, 255]);
    graphics_assert_production_paint(window, &view, cx);
    view.read_with(cx, |view, _| {
        assert_eq!(view.pane_id, pane);
        assert!(view.scrollback_text().contains("RECOVERY-HISTORY"));
        assert!(view.scrollback_text().contains("RECOVERY-SCREEN"));
        let live = graphics_visible_images(view);
        let green = live
            .iter()
            .find(|image| image.placement.image_id == 8)
            .unwrap();
        assert!(green.painted.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(
            green
                .paint_bounds(point(px(0.), px(0.)), px(10.), px(10.), 0, 0, 1.)
                .0,
            Bounds::new(point(px(190.), px(130.)), gpui_kit::size(px(10.), px(10.)))
        );
    });
    view.update(cx, |view, cx| {
        view.scroll(10_000, None, &Modifiers::default(), cx)
    });
    graphics_recovery_pixels(
        &view,
        cx,
        7,
        &[
            0, 0, 255, 255, 0, 255, 0, 255, 255, 0, 0, 255, 255, 255, 255, 255,
        ],
    );
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        let shared: Vec<_> = images
            .iter()
            .filter(|image| image.placement.image_id == 7)
            .collect();
        assert_eq!(
            shared.len(),
            3,
            "two direct crops and one text-derived placeholder run recover once each"
        );
        assert!(shared.iter().all(|image| std::sync::Arc::ptr_eq(
            shared[0].data.as_ref().unwrap(),
            image.data.as_ref().unwrap()
        )));
        let offset = view.terminal.term.lock().display_offset() as i32;
        let first = shared
            .iter()
            .find(|image| image.placement.placement_id == 11)
            .unwrap();
        assert_eq!((first.placement.x, first.placement.w), (1, 2));
        assert_eq!(
            first.paint_bounds(point(px(0.), px(0.)), px(10.), px(10.), 0, offset, 1.),
            (
                Bounds::new(point(px(40.), px(50.)), gpui_kit::size(px(20.), px(10.))),
                Bounds::new(point(px(30.), px(50.)), gpui_kit::size(px(40.), px(10.)))
            )
        );
        let placeholder = shared
            .iter()
            .find(|image| image.placement.placement_id == 12)
            .unwrap();
        assert_eq!(
            placeholder
                .paint_bounds(point(px(0.), px(0.)), px(10.), px(10.), 0, offset, 1.)
                .0,
            Bounds::new(point(px(40.), px(70.)), gpui_kit::size(px(40.), px(10.)))
        );
        let second = shared
            .iter()
            .find(|image| image.placement.placement_id == 13)
            .unwrap();
        assert_eq!((second.placement.x, second.placement.w), (2, 2));
        assert_eq!(
            second
                .paint_bounds(point(px(0.), px(0.)), px(10.), px(10.), 0, offset, 1.)
                .0,
            Bounds::new(point(px(80.), px(90.)), gpui_kit::size(px(20.), px(10.)))
        );
        assert!(
            shared
                .iter()
                .all(|image| image.painted.load(std::sync::atomic::Ordering::Acquire))
        );
    });
    // STAT emits no application bytes, images or placements. The fixture also
    // keeps its PID, proving this was the same living child rather than a restart.
    assert_eq!(graphics_recovery_stats(&view, cx, 106), stopped);
    graphics_recovery_destroy(view, window, cx);
    daemon.panes().kill(pane).unwrap();
}

fn graphics_recovery_detached_request(
    daemon: &GraphicsDaemonProcess,
    pane: u64,
    operation: &str,
    serial: usize,
    bytes: &[u8],
) -> String {
    graphics_recovery_detached_request_with_replies(daemon, pane, operation, serial, bytes, 0)
}

fn graphics_recovery_detached_request_with_replies(
    daemon: &GraphicsDaemonProcess,
    pane: u64,
    operation: &str,
    serial: usize,
    bytes: &[u8],
    replies: usize,
) -> String {
    graphics_recovery_exchange(daemon, pane, operation, serial, bytes, replies).0
}

fn graphics_recovery_exchange(
    daemon: &GraphicsDaemonProcess,
    pane: u64,
    operation: &str,
    serial: usize,
    bytes: &[u8],
    replies: usize,
) -> (String, Vec<DaemonMsg>) {
    let observer = daemon
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
        .set_recv_timeout(Some(std::time::Duration::from_secs(30)))
        .unwrap();
    let hex = graphics_pty_fixture::hex(bytes);
    let request = format!("{operation} {serial} {replies} {hex} 0\n");
    daemon.panes().send_input(pane, request.as_bytes()).unwrap();
    graphics_recovery_collect(observer, serial)
}

fn graphics_recovery_collect(
    mut observer: lyrel_core::client::PaneSession,
    serial: usize,
) -> (String, Vec<DaemonMsg>) {
    let marker = format!("KITTY_DONE_{serial};");
    let mut text = Vec::new();
    let mut trace = Vec::new();
    while !text
        .windows(marker.len())
        .any(|part| part == marker.as_bytes())
    {
        let message = observer.recv().unwrap();
        match &message {
            DaemonMsg::Snapshot(bytes) | DaemonMsg::Output(bytes) => text.extend_from_slice(bytes),
            DaemonMsg::Error(error) => panic!(
                "detached output barrier failed: {error}; actual PTY output={}",
                String::from_utf8_lossy(&text)
            ),
            DaemonMsg::Exited { .. } => panic!(
                "recovery cannot restart or end the real sender; actual PTY output={}",
                String::from_utf8_lossy(&text)
            ),
            _ => {}
        }
        trace.push(message);
    }
    observer.detach().unwrap();
    (String::from_utf8_lossy(&text).into_owned(), trace)
}

fn graphics_recovery_trace(messages: &[DaemonMsg]) -> Vec<String> {
    messages
        .iter()
        .map(|message| match message {
            DaemonMsg::Graphics(frame) => format!(
                "Graphics {:?}",
                lyrel_core::core::kitty_graphics::GraphicsUpdate::decode_frame_owned(
                    frame.clone().into_vec()
                )
            ),
            DaemonMsg::Snapshot(bytes) => format!("Snapshot {:?}", String::from_utf8_lossy(bytes)),
            DaemonMsg::Output(bytes) => format!("Output {:?}", String::from_utf8_lossy(bytes)),
            other => format!("{other:?}"),
        })
        .collect()
}

#[gpui_kit::test]
fn graphics_consumer_detached_changes_and_replay_live_cutover_keep_screen_resize_order(
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
    graphics_send_fixture(&prior, cx, 201, b"\x1b[2J\x1b[3J\x1b[6;5H\x1b_Ga=T,i=21,p=1,c=2,r=1,C=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\\x1b[12;5H\x1b_Ga=T,i=24,p=4,c=2,r=1,C=1,f=32,s=1,v=1,q=2;AAD//w==\x1b\\");
    graphics_recovery_pixels(&prior, cx, 21, &[0, 0, 255, 255]);
    graphics_recovery_pixels(&prior, cx, 24, &[255, 0, 0, 255]);
    graphics_assert_production_paint(prior_window, &prior, cx);
    graphics_recovery_destroy(prior, prior_window, cx);
    // There is no TerminalView, ImageStore or decoder during this production.
    let detached = graphics_recovery_detached_request_with_replies(&daemon, pane, "SEND_OK", 202, "\x1b[6;5H\x1b_Ga=T,i=21,p=1,c=2,r=1,C=1,f=32,s=1,v=1;AP8A/w==\x1b\\\x1b_Ga=T,i=22,p=2,U=1,c=2,r=1,f=32,s=1,v=1;AAD//w==\x1b\\\x1b[9;5H\x1b[38;5;22;58;5;2m\u{10eeee}\u{305}\u{305}\u{10eeee}\x1b[0m\x1b_Ga=d,d=I,i=24\x1b\\\x1b[16;1HDETACHED-MAIN".as_bytes(), 2);
    for reply in [
        b"\x1b_Gi=21,p=1;OK\x1b\\".as_slice(),
        b"\x1b_Gi=22,p=2;OK\x1b\\".as_slice(),
    ] {
        assert!(
            detached.contains(&format!(
                "KITTY_REPLY 202 {}",
                graphics_pty_fixture::hex(reply)
            )),
            "detached uploads must be admitted before they can be claimed as restoration: {detached}"
        );
    }

    let mut stream = daemon.stream(pane);
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(30)))
        .unwrap();
    let mut buffered = Vec::new();
    loop {
        let message = DaemonMsg::read(&mut stream).unwrap();
        let offered = matches!(message, DaemonMsg::GraphicsOffer { .. });
        assert!(
            !matches!(message, DaemonMsg::Error(_)),
            "fresh attach must reach real replay admission: {message:?}"
        );
        message.encode(&mut buffered).unwrap();
        if offered {
            break;
        }
    }
    // The real attach is now awaiting its first desktop grant. Mutate actual
    // PTY output before any replay resource can enter the new ImageStore.
    let (admitted, produced) = graphics_recovery_exchange(&daemon, pane, "SEND_OK", 203, b"\x1b[6;5H\x1b_Ga=T,i=21,p=1,c=2,r=1,C=1,f=32,s=1,v=1;AP///w==\x1b\\\x1b_Ga=d,d=I,i=24\x1b\\\x1b[?1049h\x1b[2J\x1b[4;1HLIVE-ALT\x1b[7;5H\x1b_Ga=T,i=23,p=3,c=2,r=1,C=1,f=32,s=1,v=1;/////w==\x1b\\", 2);
    for reply in [
        b"\x1b_Gi=21,p=1;OK\x1b\\".as_slice(),
        b"\x1b_Gi=23,p=3;OK\x1b\\".as_slice(),
    ] {
        assert!(
            admitted.contains(&format!(
                "KITTY_REPLY 203 {}",
                graphics_pty_fixture::hex(reply)
            )),
            "concurrent replay uploads must be accepted with their own image and placement identities: {admitted}"
        );
    }
    // Keep a production observer across the actual GPUI prepaint/resize, so
    // a lost placement is diagnosed against the daemon's ordered state rather
    // than mistaken for a decoder delay or a desktop screen projection.
    let cutover_observer = daemon
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
    cutover_observer
        .set_recv_timeout(Some(std::time::Duration::from_secs(30)))
        .unwrap();
    stream.set_read_timeout(None).unwrap();
    let (view, window) = graphics_build_rooted_consumer(cx, pane, stream, buffered);
    graphics_assert_production_paint_at(window, &view, cx, TermSize::new(80, 24));
    graphics_recovery_stats(&view, cx, 204);
    daemon
        .panes()
        .send_input(pane, b"STAT 214 0 00 0\n")
        .unwrap();
    let (_, cutover) = graphics_recovery_collect(cutover_observer, 214);
    let updates: Vec<_> = cutover
        .iter()
        .filter_map(|message| match message {
            DaemonMsg::Graphics(frame) => {
                lyrel_core::core::kitty_graphics::GraphicsUpdate::decode_frame_owned(
                    frame.clone().into_vec(),
                )
            }
            _ => None,
        })
        .collect();
    let accepted = updates.iter().find_map(|update| match update {
        lyrel_core::core::kitty_graphics::GraphicsUpdate::Place(placement)
            if placement.image_id == 23 && placement.screen == 1 =>
        {
            Some(placement.key)
        }
        _ => None,
    });
    let removed = accepted.is_some_and(|key| updates.iter().any(|update| matches!(update,
        lyrel_core::core::kitty_graphics::GraphicsUpdate::Remove { placements, .. } if placements.contains(&key))));
    assert!(
        accepted.is_some() && !removed,
        "accepted alternate-screen placement was removed by authoritative output before paint; sender exchange={:?}; replay-to-live={:?}; desktop={}",
        graphics_recovery_trace(&produced),
        graphics_recovery_trace(&cutover),
        view.read_with(cx, |view, _| view.terminal.images().recovery_diagnostics())
    );
    graphics_recovery_pixels(&view, cx, 23, &[255, 255, 255, 255]);
    graphics_assert_production_paint(window, &view, cx);
    view.read_with(cx, |view, _| {
        assert!(view.scrollback_text().contains("LIVE-ALT"));
        let images = graphics_visible_images(view);
        assert_eq!(
            images
                .iter()
                .map(|image| image.placement.image_id)
                .collect::<Vec<_>>(),
            [23],
            "main-screen resources cannot leak into the replayed alternate screen"
        );
        assert_eq!(
            images[0]
                .paint_bounds(point(px(0.), px(0.)), px(10.), px(10.), 0, 0, 1.)
                .0,
            Bounds::new(point(px(40.), px(60.)), gpui_kit::size(px(10.), px(10.)))
        );
    });
    // Real text after the screen operation is the ordering barrier. Merely
    // waiting for a snapshot length could observe the previous Screen frame.
    graphics_send_fixture(&view, cx, 205, b"\x1b[?1049l\x1b[18;1HRETURNED-MAIN");
    graphics_recovery_pixels(&view, cx, 21, &[255, 255, 0, 255]);
    graphics_recovery_pixels(&view, cx, 22, &[255, 0, 0, 255]);
    graphics_assert_production_paint_at(window, &view, cx, TermSize::new(40, 24));
    graphics_send_fixture(&view, cx, 206, b"\x1b[20;1HRESIZED-MAIN");
    graphics_assert_production_paint(window, &view, cx);
    view.read_with(cx, |view, _| {
        let term = view.terminal.term.lock();
        assert_eq!(term.columns(), 40);
        drop(term);
        assert!(view.scrollback_text().contains("DETACHED-MAIN"));
        assert!(view.scrollback_text().contains("RETURNED-MAIN"));
        assert!(view.scrollback_text().contains("RESIZED-MAIN"));
        let images = graphics_visible_images(view);
        assert_eq!(images.len(), 2, "one updated direct placement and the detached placeholder, without duplicate or deleted state");
        assert!(images.iter().all(|image| ![23, 24].contains(&image.placement.image_id)));
        let placeholder = images.iter().find(|image| image.placement.image_id == 22).unwrap();
        assert_eq!(placeholder.paint_bounds(point(px(0.), px(0.)), px(10.), px(10.), 0, 0, 1.).0,
            Bounds::new(point(px(45.), px(80.)), gpui_kit::size(px(10.), px(10.))));
        assert!(images.iter().all(|image| image.painted.load(std::sync::atomic::Ordering::Acquire)));
    });
    let stopped = graphics_recovery_stats(&view, cx, 207);
    graphics_recovery_destroy(view, window, cx);
    let (fresh, fresh_window) =
        graphics_recovery_fresh_at(&daemon, pane, cx, 208, TermSize::new(40, 24));
    graphics_recovery_pixels(&fresh, cx, 21, &[255, 255, 0, 255]);
    graphics_recovery_pixels(&fresh, cx, 22, &[255, 0, 0, 255]);
    graphics_assert_production_paint(fresh_window, &fresh, cx);
    fresh.read_with(cx, |view, _| {
        assert_eq!(view.terminal.term.lock().columns(), 40);
        let images = graphics_visible_images(view);
        let direct = images
            .iter()
            .find(|image| image.placement.image_id == 21)
            .unwrap();
        assert_eq!(
            direct
                .paint_bounds(point(px(0.), px(0.)), px(10.), px(10.), 0, 0, 1.)
                .0,
            Bounds::new(point(px(40.), px(50.)), gpui_kit::size(px(10.), px(10.)))
        );
        assert_eq!(graphics_visible_images(view).len(), 2);
        assert!(
            !graphics_visible_images(view)
                .iter()
                .any(|image| [23, 24].contains(&image.placement.image_id))
        );
        assert!(view.scrollback_text().contains("RESIZED-MAIN"));
    });
    assert_eq!(graphics_recovery_stats(&fresh, cx, 209), stopped);
    graphics_recovery_destroy(fresh, fresh_window, cx);
    daemon.panes().kill(pane).unwrap();
}

#[gpui_kit::test]
fn graphics_consumer_fresh_replays_eviction_notice_but_not_deleted_or_expired_pixels(
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
    let (prior, prior_window) = graphics_build_rooted_consumer(
        cx,
        pane,
        daemon.stream_with_capacity(pane, 1024, 8),
        Vec::new(),
    );
    graphics_assert_production_paint_at(prior_window, &prior, cx, TermSize::new(80, 24));
    graphics_recovery_stats(&prior, cx, 300);
    graphics_send_fixture(&prior, cx, 301, "\x1b[2J\x1b[3J\x1b[6;5H\x1b_Ga=T,i=31,p=1,c=2,r=1,C=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\\x1b_Ga=T,i=32,p=2,U=1,c=2,r=1,f=32,s=1,v=1,q=2;AAD//w==\x1b\\\x1b[8;5H\x1b[38;5;32;58;5;2m\u{10eeee}\u{305}\u{305}\u{10eeee}\x1b[0m".as_bytes());
    graphics_recovery_pixels(&prior, cx, 31, &[0, 0, 255, 255]);
    graphics_recovery_pixels(&prior, cx, 32, &[255, 0, 0, 255]);
    graphics_assert_production_paint(prior_window, &prior, cx);
    let placeholder_area = prior.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        let placeholder = images
            .iter()
            .find(|image| image.placement.image_id == 32)
            .unwrap();
        let area = placeholder.paint_bounds(point(px(0.), px(0.)), px(10.), px(10.), 0, 0, 1.);
        // The square source fits centered inside a two-column placeholder;
        // the notice must keep that real image area, not stretch to its cells.
        assert_eq!(
            area.0,
            Bounds::new(point(px(45.), px(70.)), gpui_kit::size(px(10.), px(10.)))
        );
        area
    });
    graphics_send_fixture(&prior, cx, 302, &b"\r\n".repeat(80));
    graphics_send_fixture(
        &prior,
        cx,
        303,
        b"\x1b[10;5H\x1b_Ga=T,i=33,p=3,c=2,r=1,C=1,f=32,s=1,v=1,q=2;AP8A/w==\x1b\\",
    );
    graphics_recovery_pixels(&prior, cx, 33, &[0, 255, 0, 255]);
    graphics_send_fixture(
        &prior,
        cx,
        304,
        b"\x1b[12;5H\x1b_Ga=T,i=34,p=4,c=2,r=1,C=1,f=32,s=1,v=1,q=2;/////w==\x1b\\",
    );
    graphics_recovery_pixels(&prior, cx, 34, &[255, 255, 255, 255]);
    graphics_assert_production_paint(prior_window, &prior, cx);
    graphics_recovery_destroy(prior, prior_window, cx);
    // Intentional deletion while there is no desktop wins over an eviction
    // notice already retained for that resource; the other notice must survive.
    graphics_recovery_detached_request(&daemon, pane, "SEND", 305, b"\x1b_Ga=d,d=I,i=31\x1b\\");
    let detached = graphics_recovery_detached_request(&daemon, pane, "STAT", 306, &[0]);
    let stopped = detached
        .split_once("KITTY_STATS_306 ")
        .unwrap()
        .1
        .split_whitespace()
        .take(4)
        .collect::<Vec<_>>()
        .join(" ");
    let (view, window) = graphics_recovery_fresh(&daemon, pane, cx, 307);
    graphics_recovery_pixels(&view, cx, 33, &[0, 255, 0, 255]);
    graphics_recovery_pixels(&view, cx, 34, &[255, 255, 255, 255]);
    view.update(cx, |view, cx| {
        view.scroll(10_000, None, &Modifiers::default(), cx)
    });
    graphics_assert_production_paint(window, &view, cx);
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        assert!(
            !images.iter().any(|image| image.placement.image_id == 31),
            "deleted historical data and its notice cannot resurrect"
        );
        let notice = images
            .iter()
            .find(|image| image.placement.image_id == 32)
            .unwrap();
        assert!(
            notice.evicted && notice.data.is_none(),
            "recovery must not find an unaccounted pixel copy for the evicted placeholder"
        );
        assert!(
            notice.painted.load(std::sync::atomic::Ordering::Acquire),
            "production paint must claim the restored notice"
        );
        let offset = view.terminal.term.lock().display_offset() as i32;
        assert_eq!(
            notice.paint_bounds(point(px(0.), px(0.)), px(10.), px(10.), 0, offset, 1.),
            placeholder_area,
            "eviction and fresh recovery must preserve the exact pre-eviction crop and texture area"
        );
        assert!(
            !view.scrollback_text().contains("Image evicted")
                && !view.scrollback_text().contains("图片已淘汰")
        );
    });
    assert_eq!(graphics_recovery_stats(&view, cx, 308), stopped);
    graphics_recovery_destroy(view, window, cx);
    // Exercise actual bounded terminal history, not a synthetic store clear.
    let mut expired = b"\r\n".repeat(10_050);
    expired.extend_from_slice(b"\x1b[16;1HHISTORY-EXPIRED\x1b[16;20H\x1b_Ga=T,i=36,p=6,c=2,r=1,C=1,f=32,s=1,v=1,q=2;AP8A/w==\x1b\\");
    graphics_recovery_detached_request(&daemon, pane, "SEND", 309, &expired);
    let (fresh, fresh_window) = graphics_recovery_fresh(&daemon, pane, cx, 310);
    graphics_recovery_pixels(&fresh, cx, 36, &[0, 255, 0, 255]);
    graphics_assert_production_paint(fresh_window, &fresh, cx);
    fresh.read_with(cx, |view, _| {
        assert!(view.scrollback_text().contains("HISTORY-EXPIRED"));
        let images = view.terminal.images().snapshot();
        assert_eq!(
            images
                .iter()
                .map(|image| image.placement.image_id)
                .collect::<Vec<_>>(),
            [36],
            "expired history cannot recover either pixels or eviction notices"
        );
    });
    graphics_recovery_destroy(fresh, fresh_window, cx);
    daemon.panes().kill(pane).unwrap();
}

#[gpui_kit::test]
fn graphics_consumer_fresh_restore_budget_refusal_disconnects_without_discarding_retained_pixels(
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
    let (prior, prior_window) = graphics_recovery_fresh(&daemon, pane, cx, 400);
    graphics_send_fixture(&prior, cx, 401, "\x1b[2J\x1b[3J\x1b[6;5H\x1b_Ga=T,i=41,p=1,c=2,r=1,C=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\\x1b_Ga=p,i=41,p=2,U=1,c=2,r=1,q=2\x1b\\\x1b[9;5H\x1b[38;5;41;58;5;2m\u{10eeee}\u{305}\u{305}\u{10eeee}\x1b[0m\x1b[16;1HRETAINED-ON-REFUSAL".as_bytes());
    graphics_recovery_pixels(&prior, cx, 41, &[0, 0, 255, 255]);
    graphics_assert_production_paint(prior_window, &prior, cx);
    let stopped = graphics_recovery_stats(&prior, cx, 402);
    graphics_recovery_destroy(prior, prior_window, cx);
    // A one-pixel source still needs an actual minimum-rounded atlas page.
    // A tiny desktop cannot ACK it, and must not appear successfully attached
    // with text but silently missing the session's retained images.
    crate::terminal::images::with_test_cache_limits(1024, 1024, || {
        let (refused, refused_window) =
            graphics_build_rooted_consumer(cx, pane, daemon.stream(pane), Vec::new());
        graphics_assert_production_paint_at(refused_window, &refused, cx, TermSize::new(80, 24));
        graphics_recovery_wait(
            cx,
            "refused restore must report a disconnected consumer, not silent image loss",
            |cx| {
                refused.update(cx, |view, _| {
                    view.terminal.poll_exited();
                    view.terminal.exited
                })
            },
        );
        refused.read_with(cx, |view, _| {
            assert!(
                !view.terminal.child_exited(),
                "desktop admission failure does not end the living pane"
            );
            assert!(
                graphics_visible_images(view).is_empty(),
                "no fabricated eviction notice or partly admitted image"
            );
        });
        graphics_recovery_destroy(refused, refused_window, cx);
    });
    let (fresh, fresh_window) = graphics_recovery_fresh(&daemon, pane, cx, 403);
    graphics_recovery_pixels(&fresh, cx, 41, &[0, 0, 255, 255]);
    graphics_assert_production_paint(fresh_window, &fresh, cx);
    fresh.read_with(cx, |view, _| {
        assert!(view.scrollback_text().contains("RETAINED-ON-REFUSAL"));
        let images = graphics_visible_images(view);
        assert_eq!(
            images.len(),
            2,
            "both direct and placeholder recover when real desktop capacity is available"
        );
        assert!(images.iter().all(|image| !image.evicted
            && image.data.is_some()
            && image.painted.load(std::sync::atomic::Ordering::Acquire)));
    });
    assert_eq!(
        graphics_recovery_stats(&fresh, cx, 404),
        stopped,
        "failed admission cannot require the application to resend"
    );
    graphics_recovery_destroy(fresh, fresh_window, cx);
    daemon.panes().kill(pane).unwrap();
}

#[gpui_kit::test]
fn graphics_consumer_fresh_ring_tail_projects_retained_direct_anchors_without_reviving_missing_text(cx: &mut TestAppContext) {
    let (daemon, pane, prior, prior_window) = graphics_layout_harness(cx);
    graphics_send_fixture(&prior, cx, 1, b"\x1b[2J\x1b[3J\x1b[6;1HLOST_ANCHOR\r\x1b_Ga=T,i=401,p=1,c=1,r=1,C=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\");
    graphics_layout_wait_image(&prior, cx, 401);
    // More than the real 8 MiB ring cap, but fewer than 10,000 text rows.
    // SGR bytes consume replay capacity without wrapping or inventing rows.
    let padding = b"\x1b[0m".repeat(250);
    let mut observer = daemon.panes().observe(pane, crate::daemon::protocol::WinSize {
        cols: 80, rows: 24, cell_w: 10, cell_h: 20,
    }).unwrap();
    observer.set_recv_timeout(Some(std::time::Duration::from_secs(30))).unwrap();
    let monitor = std::thread::spawn(move || {
        let mut bytes_seen = 0usize;
        let mut tail = Vec::new();
        loop {
            match observer.recv().unwrap() {
                DaemonMsg::Snapshot(bytes) | DaemonMsg::Output(bytes) => {
                    bytes_seen += bytes.len();
                    tail.extend(bytes);
                    if tail.windows(b"RING_LOAD_DONE".len()).any(|bytes| bytes == b"RING_LOAD_DONE") { break; }
                    tail.drain(..tail.len().saturating_sub(8192));
                }
                DaemonMsg::Error(error) => panic!("replay load observer failed: {error}; raw_output_tail={:?}", String::from_utf8_lossy(&tail)),
                DaemonMsg::Exited { code } => panic!("replay sender exited: {code:?}; raw_output_tail={:?}", String::from_utf8_lossy(&tail)),
                _ => {}
            }
        }
        observer.detach().unwrap();
        bytes_seen
    });
    for batch in 0..288 {
        let mut output = Vec::new();
        for line in 0..32 {
            let row = batch * 32 + line;
            output.extend_from_slice(&padding);
            output.extend_from_slice(b"\r\n");
            if row == 2048 {
                output.extend_from_slice(b"RECOVERABLE_ANCHOR\r\x1b_Ga=T,i=402,p=2,c=1,r=1,C=1,f=32,s=1,v=1,q=2;AP8A/w==\x1b\\");
            } else {
                output.extend_from_slice(format!("REPLAY_ROW_{row:05}").as_bytes());
            }
        }
        graphics_send_fixture(&prior, cx, batch + 2, &output);
    }
    graphics_send_fixture(&prior, cx, 899, b"RING_LOAD_DONE");
    let pty_bytes = monitor.join().unwrap();
    assert!(pty_bytes > 8 * 1024 * 1024, "actual daemon Snapshot/Output bytes, not producer bytes, must exceed the ring cap: {pty_bytes}");
    let stopped = graphics_recovery_stats(&prior, cx, 900);
    let sent = stopped.split_whitespace().find_map(|field| field.strip_prefix("bytes=")).unwrap().parse::<usize>().unwrap();
    assert!(sent > 8 * 1024 * 1024);
    assert!(stopped.contains("uploads=2"));
    graphics_recovery_destroy(prior, prior_window, cx);
    let (fresh, fresh_window) = graphics_recovery_fresh(&daemon, pane, cx, 901);
    graphics_recovery_pixels(&fresh, cx, 402, &[0, 255, 0, 255]);
    let marker_row = fresh.read_with(cx, |view, _| {
        assert!(!view.scrollback_text().contains("LOST_ANCHOR"));
        let term = view.terminal.term.lock();
        assert!(term.grid.history_size() < 10_000, "the real replay tail is shorter than the daemon's retained grid");
        (-(term.grid.history_size() as i32)..term.screen_lines() as i32).find(|&row| {
            let text: String = term.grid[Line(row)][..].iter().map(|cell| cell.c()).collect();
            text.contains("RECOVERABLE_ANCHOR")
        }).expect("a still-replayable history marker cannot be discarded")
    });
    fresh.update(cx, |view, cx| view.scroll(10_000, None, &Modifiers::default(), cx));
    graphics_assert_production_paint(fresh_window, &fresh, cx);
    fresh.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        let missing = images.iter().find(|image| image.placement.image_id == 401).unwrap();
        let offset = view.terminal.term.lock().display_offset() as i32;
        let (crop, _) = missing.paint_bounds(point(px(0.), px(0.)), px(10.), px(10.), 0, offset, 1.);
        assert!(crop.origin.y + crop.size.height <= px(0.), "missing text cannot acquire a visible direct image at the oldest replayable row");
        assert!(!missing.painted.load(std::sync::atomic::Ordering::Acquire));
    });
    fresh.update(cx, |view, cx| {
        view.scroll(-10_000, None, &Modifiers::default(), cx);
        view.scroll(-marker_row, None, &Modifiers::default(), cx);
    });
    graphics_assert_production_paint(fresh_window, &fresh, cx);
    fresh.read_with(cx, |view, _| {
        let term = view.terminal.term.lock();
        let offset = term.display_offset() as i32;
        let text: String = term.grid[Line(-offset)][..].iter().map(|cell| cell.c()).collect();
        assert!(text.contains("RECOVERABLE_ANCHOR"));
        drop(term);
        let images = graphics_visible_images(view);
        let retained = images.iter().find(|image| image.placement.image_id == 402).unwrap();
        let (crop, _) = retained.paint_bounds(point(px(0.), px(0.)), px(10.), px(10.), 0, offset, 1.);
        assert_eq!(crop.origin, point(px(0.), px(0.)), "global image anchors and local replay rows must project to the same real text row");
        assert!(retained.painted.load(std::sync::atomic::Ordering::Acquire));
    });
    assert_eq!(graphics_recovery_stats(&fresh, cx, 902), stopped, "fresh replay must not ask the real sender to retransmit either image");
    graphics_recovery_destroy(fresh, fresh_window, cx);
    daemon.panes().kill(pane).unwrap();
}
