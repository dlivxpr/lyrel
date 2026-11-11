// Production reader/TerminalElement observations; no private reading-state assertions.
#[gpui_kit::test]
fn reading_lifecycle_clear_reset_and_same_batch_alt_release_normal_rows(cx: &mut TestAppContext) {
    use gpui_kit::test::TestWindowExt as _;
    let (daemon, pane, view, window) = graphics_layout_harness(cx);
    for (serial, transition, marker) in [
        (200, "\x1b[3J", "AFTER-3J"),
        (210, "\x1bc", "AFTER-RIS"),
        (220, "\x1b[?1049hALT-TRANSCRIPT\x1b[?1049l", "AFTER-ALT"),
    ] {
        graphics_send_fixture(&view, cx, serial, &b"OLD-READING\r\n".repeat(72));
        graphics_send_fixture(&view, cx, serial + 1,
            "\x1b[8;1H\x1b[2KFROZEN-ROW\x1b[10;5H\x1b_Ga=T,i=7,c=1,r=1,C=1,f=32,s=1,v=1,q=2;AAD//w==\x1b\\\x1b[12;5H\x1b_Ga=T,i=8,p=2,U=1,c=1,r=1,f=32,s=1,v=1,q=2;AAD//w==\x1b\\\x1b[38;5;8;58;5;2m\u{10eeee}\u{305}\u{305}\x1b[0m".as_bytes());
        graphics_layout_wait_image(&view, cx, 7);
        graphics_layout_wait_image(&view, cx, 8);
        view.update(cx, |view, cx| {
            view.smooth_scroll(1., cx);
        });
        graphics_assert_production_paint(window, &view, cx);
        assert!(reading_painted_rows(&view, cx)
            .iter()
            .any(|row| row == "FROZEN-ROW"));
        view.read_with(cx, |view, _| {
            let images = &view.grid_snap.as_ref().unwrap().images;
            assert_eq!(images.len(), 2);
            assert!(images
                .iter()
                .all(|image| reading_rgba_pixel(image) == [0, 0, 255, 255]
                    && image.painted.load(std::sync::atomic::Ordering::Acquire)));
        });
        // CSI 3J preserves lines_evicted + history_size. Only the explicit
        // parse transition can distinguish it from ordinary history eviction.
        graphics_send_fixture(
            &view,
            cx,
            serial + 2,
            format!("{transition}\x1b[H\x1b[2J\x1b[8;1H{marker}").as_bytes(),
        );
        graphics_assert_production_paint(window, &view, cx);
        let shown = reading_painted_rows(&view, cx);
        assert_eq!(shown[7], marker);
        assert!(!shown
            .iter()
            .any(|row| row.contains("FROZEN-ROW") || row.contains("ALT-TRANSCRIPT")));
        view.read_with(cx, |view, _| {
            assert!(
                view.grid_snap.as_ref().unwrap().images.is_empty(),
                "clear/reset/alt cannot paint retired normal direct or placeholder leases"
            )
        });
        window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                assert!(window.try_find("terminal-return-live").is_none());
            })
            .unwrap();
        assert!(
            view.read_with(cx, |view, _| view.scrollback_text().contains(marker)),
            "live parsing advances across the transition"
        );
        graphics_send_fixture(&view, cx, serial + 3, b"\x1b[24;1H\r\nFRESH-HISTORY\r\n");
        view.update(cx, |view, cx| {
            view.smooth_scroll(1., cx);
        });
        graphics_assert_production_paint(window, &view, cx);
        assert!(
            !reading_painted_rows(&view, cx)
                .iter()
                .any(|row| row.contains("FROZEN-ROW")),
            "fresh entry cannot reuse the retired generation"
        );
        view.update(cx, |view, _| view.jump_to_prompt());
    }
    daemon.panes().kill(pane).unwrap();
    graphics_recovery_destroy(view, window, cx);
}

#[gpui_kit::test]
fn reading_lifecycle_replay_and_relink_publish_fresh_content(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let (client, mut sender) = test_stream_pair();
    let (view, window) = graphics_build_rooted_consumer_at(
        cx,
        254,
        client,
        Vec::new(),
        TermSize::new(80, 24),
        PtySource::Raw,
    );
    DaemonMsg::Output(b"OLD-BASELINE\r\n".repeat(72))
        .encode(&mut sender)
        .unwrap();
    graphics_recovery_wait(cx, "initial baseline was not parsed", |cx| {
        view.read_with(cx, |view, _| {
            view.scrollback_text().contains("OLD-BASELINE")
        })
    });
    view.update(cx, |view, cx| {
        view.smooth_scroll(1., cx);
    });
    graphics_assert_production_paint(window, &view, cx);
    assert!(reading_painted_rows(&view, cx)
        .iter()
        .any(|row| row.contains("OLD-BASELINE")));
    // A snapshot is a discontinuous baseline even when absolute origin sums
    // happen to agree. It is not ordinary append output.
    DaemonMsg::Snapshot(b"\x1b[H\x1b[2JNEW-SNAPSHOT".to_vec())
        .encode(&mut sender)
        .unwrap();
    graphics_recovery_wait(cx, "snapshot baseline was not parsed", |cx| {
        view.read_with(cx, |view, _| {
            view.scrollback_text().contains("NEW-SNAPSHOT")
        })
    });
    graphics_assert_production_paint(window, &view, cx);
    assert_eq!(reading_painted_rows(&view, cx)[0], "NEW-SNAPSHOT");
    view.update(cx, |view, cx| {
        view.smooth_scroll(1., cx);
    });
    graphics_assert_production_paint(window, &view, cx);
    let (client, mut fresh_sender) = test_stream_pair();
    view.update(cx, |view, cx| {
        view.adopt_relink(
            client,
            Vec::new(),
            &crate::terminal::PaneRoute::Local,
            TermSize::new(80, 24),
            10,
            20,
            cx,
        )
        .unwrap();
    });
    DaemonMsg::Snapshot(b"\x1b[HRELINKED-BASELINE".to_vec())
        .encode(&mut fresh_sender)
        .unwrap();
    graphics_recovery_wait(cx, "relink baseline was not parsed", |cx| {
        view.read_with(cx, |view, _| {
            view.scrollback_text().contains("RELINKED-BASELINE")
        })
    });
    graphics_assert_production_paint(window, &view, cx);
    let shown = reading_painted_rows(&view, cx);
    assert_eq!(shown[0], "RELINKED-BASELINE");
    assert!(!shown
        .iter()
        .any(|row| row.contains("OLD-BASELINE") || row.contains("NEW-SNAPSHOT")));
    assert!(
        view.read_with(cx, |view, _| presentation_allocation_observation(view))["reading"]
            .is_null()
    );
    DaemonMsg::Output(b"\x1b[24;1H\r\nRETAINED-AFTER-EOF\r\n".repeat(48))
        .encode(&mut fresh_sender)
        .unwrap();
    graphics_recovery_wait(cx, "retained baseline did not advance", |cx| {
        view.read_with(cx, |view, _| {
            view.scrollback_text().contains("RETAINED-AFTER-EOF")
        })
    });
    view.update(cx, |view, cx| {
        view.smooth_scroll(1., cx);
    });
    graphics_assert_production_paint(window, &view, cx);
    assert!(reading_painted_rows(&view, cx)
        .iter()
        .any(|row| row.contains("RETAINED-AFTER-EOF")));
    drop(fresh_sender);
    graphics_recovery_wait(cx, "retained pane did not observe EOF", |cx| {
        view.read_with(cx, |view, _| view.terminal.exited)
    });
    view.update(cx, |view, cx| view.clear_scrollback(cx));
    graphics_assert_production_paint(window, &view, cx);
    assert!(
        reading_painted_rows(&view, cx).iter().all(String::is_empty),
        "explicit clear of a disconnected pane cannot restore frozen rows"
    );
    assert!(
        view.read_with(cx, |view, _| presentation_allocation_observation(view))["reading"]
            .is_null()
    );
    graphics_recovery_destroy(view, window, cx);
}

#[gpui_kit::test]
fn reading_lifecycle_history_pressure_clamps_rows_and_releases_expired_images(
    cx: &mut TestAppContext,
) {
    use std::sync::Arc;
    let (daemon, pane, view, window) = graphics_layout_harness(cx);
    graphics_send_fixture(&view, cx, 240, &b"BEFORE-EVICTION\r\n".repeat(72));
    graphics_send_fixture(&view, cx, 241,
        b"\x1b[8;1H\x1b[2KEXPIRED-TEXT\x1b[8;5H\x1b_Ga=T,i=7,p=1,c=2,r=1,C=1,f=32,s=1,v=1,q=2;AAD//w==\x1b\\");
    graphics_layout_wait_image(&view, cx, 7);
    view.update(cx, |view, cx| {
        view.smooth_scroll(1., cx);
    });
    graphics_assert_production_paint(window, &view, cx);
    let old = view.read_with(cx, |view, _| {
        let frame = view.grid_snap.as_ref().unwrap();
        assert!(frame
            .images
            .iter()
            .any(|image| image.placement.image_id == 7
                && image.painted.load(std::sync::atomic::Ordering::Acquire)));
        Arc::downgrade(frame.images[0].data.as_ref().unwrap())
    });
    // Both daemon and mirror retain 10,000 history rows. Pressure exceeds that
    // limit, not merely the current viewport or a GUI-only history setting.
    for serial in 242..253 {
        graphics_send_fixture(&view, cx, serial, &b"\x1b[24;1H\r\nPRESSURE".repeat(1_000));
        graphics_assert_production_paint(window, &view, cx);
        let allocations = view.read_with(cx, |view, _| presentation_allocation_observation(view));
        assert!(
            allocations["reading"]["rows"].as_u64().unwrap() <= 49,
            "{allocations}"
        );
        assert!(
            allocations["reading"]["imageSlices"].as_u64().unwrap() <= 1,
            "{allocations}"
        );
    }
    let shown = reading_painted_rows(&view, cx);
    assert!(shown
        .iter()
        .all(|row| !row.contains("EXPIRED-TEXT") && !row.contains("BEFORE-EVICTION")));
    assert!(shown.iter().any(|row| row == "PRESSURE"));
    view.read_with(cx, |view, _| {
        assert!(view.grid_snap.as_ref().unwrap().images.is_empty())
    });
    // Named uploads may remain cached after their last placement expires.
    // Remove the upload too, so weak-pixel lifetime measures only display leases.
    graphics_send_fixture(&view, cx, 253, b"\x1b_Ga=d,d=I,i=7\x1b\\");
    window
        .update(cx, |_, window, cx| {
            view.read(cx).terminal.images().release_retired(window, cx)
        })
        .unwrap();
    assert!(
        old.upgrade().is_none(),
        "expired reading rows cannot pin a deleted upload"
    );
    // Ordered reflow cannot restore a row/image that has already expired.
    graphics_assert_production_paint_at(window, &view, cx, TermSize::new(40, 24));
    graphics_send_fixture(&view, cx, 254, b"\x1b[0m");
    view.update(cx, |view, _| view.jump_to_prompt());
    graphics_assert_production_paint_at(window, &view, cx, TermSize::new(40, 24));
    assert!(!reading_painted_rows(&view, cx)
        .iter()
        .any(|row| row.contains("EXPIRED-TEXT")));
    view.read_with(cx, |view, _| {
        assert!(view.grid_snap.as_ref().unwrap().images.is_empty())
    });
    graphics_send_fixture(
        &view,
        cx,
        255,
        b"\x1b[6;3H\x1b_Ga=T,i=7,p=1,c=1,r=1,C=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\",
    );
    graphics_layout_wait_image(&view, cx, 7);
    graphics_assert_production_paint_at(window, &view, cx, TermSize::new(40, 24));
    view.read_with(cx, |view, _| {
        let images = &view.grid_snap.as_ref().unwrap().images;
        assert_eq!(images.len(), 1);
        assert_eq!(reading_rgba_pixel(&images[0]), [255, 0, 0, 255]);
        assert!(images[0].painted.load(std::sync::atomic::Ordering::Acquire));
    });
    assert!(
        old.upgrade().is_none(),
        "reusing a protocol image ID never revives expired pixels"
    );
    daemon.panes().kill(pane).unwrap();
    graphics_recovery_destroy(view, window, cx);
}

#[gpui_kit::test]
fn reading_lifecycle_hidden_output_releases_pixels_without_window_notifications(
    cx: &mut TestAppContext,
) {
    use std::sync::Arc;
    let (daemon, pane, view, window) = graphics_layout_harness(cx);
    cx.update(|cx| cx.global_mut::<Config>().cursor_blink = false);
    graphics_send_fixture(&view, cx, 260, &b"HIDDEN-HISTORY\r\n".repeat(72));
    graphics_send_fixture(&view, cx, 261,
        b"\x1b[8;1H\x1b[2KREAD-BEFORE-HIDE\x1b[8;30H\x1b_Ga=T,i=7,c=2,r=1,C=1,f=32,s=1,v=1,q=2;AAD//w==\x1b\\");
    graphics_layout_wait_image(&view, cx, 7);
    view.update(cx, |view, cx| {
        view.smooth_scroll(1., cx);
    });
    graphics_assert_production_paint(window, &view, cx);
    let before = reading_painted_rows(&view, cx);
    let frozen = view.read_with(cx, |view, _| {
        Arc::downgrade(
            view.grid_snap.as_ref().unwrap().images[0]
                .data
                .as_ref()
                .unwrap(),
        )
    });
    cx.update(|cx| declare_displayed(cx, [(view.entity_id(), false)]));
    let renders = std::rc::Rc::new(std::cell::Cell::new(0));
    let probe = window
        .update(cx, |_, window, cx| {
            let probe = cx.new(|_| GraphicsWindowRenderProbe {
                renders: renders.clone(),
                terminal: None,
            });
            window.replace_root(cx, |window, cx| {
                gpui_kit::component::Root::new(probe.clone(), window, cx)
            });
            window.blur(cx);
            probe
        })
        .unwrap();
    cx.run_until_parked();
    let baseline = renders.get();
    let bytes = b"\x1b_Ga=d,d=I,i=7\x1b\\\x1b[8;1H\x1b[2KLIVE-WHILE-HIDDEN\x1b[8;30H\x1b_Ga=T,i=7,c=2,r=1,C=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\\x1b[24;1H\r\nHIDDEN-PROGRESS";
    let request = format!("SEND 262 0 {} 0\n", graphics_pty_fixture::hex(bytes));
    view.read_with(cx, |view, _| view.terminal.write(request.into_bytes()));
    // No refresh_windows, graphics_send_fixture or paint helper while hidden:
    // those intentionally invalidate windows and would mask the gate.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        cx.run_until_parked();
        let ready = view.read_with(cx, |view, _| {
            view.title == "KITTY_DONE_262;"
                && view.scrollback_text().contains("HIDDEN-PROGRESS")
                && presentation_allocation_observation(view)["reading"]["imageSlices"] == 0
        });
        if ready && frozen.upgrade().is_none() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "hidden parser / lease retirement did not advance"
        );
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(5));
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(350));
    cx.run_until_parked();
    assert_eq!(
        renders.get(),
        baseline,
        "hidden reading distance and image retirement cannot repaint the window"
    );
    cx.update(|cx| declare_displayed(cx, [(view.entity_id(), true)]));
    probe.update(cx, |probe, cx| {
        probe.terminal = Some(view.clone());
        cx.notify();
    });
    graphics_assert_production_paint(window, &view, cx);
    assert_eq!(
        reading_painted_rows(&view, cx),
        before,
        "surviving semantic reading rows remain stable"
    );
    view.read_with(cx, |view, _| {
        assert!(
            view.grid_snap.as_ref().unwrap().images.is_empty(),
            "released reading pixels cannot be recaptured under retained row identities"
        )
    });
    view.update(cx, |view, _| view.jump_to_prompt());
    graphics_assert_production_paint(window, &view, cx);
    assert!(reading_painted_rows(&view, cx)
        .iter()
        .any(|row| row.contains("LIVE-WHILE-HIDDEN")));
    view.read_with(cx, |view, _| {
        let image = &view.grid_snap.as_ref().unwrap().images[0];
        assert_eq!(reading_rgba_pixel(image), [255, 0, 0, 255]);
        assert!(image.painted.load(std::sync::atomic::Ordering::Acquire));
    });
    assert!(frozen.upgrade().is_none());
    probe.update(cx, |probe, _| probe.terminal = None);
    daemon.panes().kill(pane).unwrap();
    graphics_recovery_destroy(view, window, cx);
}

fn reading_wheel_checkpoint(
    cx: &mut TestAppContext,
    view: &Entity<TerminalView>,
    dir: &std::path::Path,
    serial: usize,
) -> Vec<u8> {
    view.read_with(cx, |view, _| view.terminal.write(vec![4]));
    let path = dir.join(format!("mouse-{serial}.bin"));
    graphics_recovery_wait(cx, "wheel agent input checkpoint missing", |_| {
        path.exists()
    });
    std::fs::read(path).unwrap()
}

#[gpui_kit::test]
fn reading_lifecycle_wheel_uses_live_agent_modes_and_shift_local_escape(cx: &mut TestAppContext) {
    use gpui_kit::test::TestWindowExt as _;
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        let mut config = Config::default();
        config.smooth_scroll = false;
        config.mouse_scroll_multiplier = 1.;
        cx.set_global(config);
    });
    let daemon = GraphicsDaemonProcess::start();
    let (pane, dir) = daemon.spawn_mouse_recorder("reading-wheel");
    let source = if cfg!(windows) {
        PtySource::LocalConpty
    } else {
        PtySource::Raw
    };
    let (view, window) = graphics_build_rooted_consumer_at(
        cx,
        pane,
        daemon.stream(pane),
        Vec::new(),
        TermSize::new(80, 24),
        source,
    );
    view.read_with(cx, |view, _| view.terminal.write(vec![7]));
    graphics_recovery_wait(cx, "wheel agent history missing", |cx| {
        view.read_with(cx, |view, _| {
            view.scrollback_text().contains("MOUSE-HISTORY-READY") && view.mouse_mode()
        })
    });
    graphics_assert_production_paint(window, &view, cx);
    let wheel = |cx: &mut TestAppContext, shift, up| {
        let geom = view.read_with(cx, |view, _| view.cell_geom.unwrap());
        mouse_reporting_event(
            cx,
            window,
            gpui_kit::PlatformInput::Scroll(ScrollEvent {
                position: point(
                    geom.origin.x + geom.cell_width * 2.5,
                    geom.origin.y + geom.line_height * 3.5,
                ),
                delta: gpui_kit::ScrollDelta::Lines(point(0., if up { 1. } else { -1. })),
                modifiers: Modifiers {
                    shift,
                    ..Default::default()
                },
                ..Default::default()
            }),
        );
    };
    let live = reading_painted_rows(&view, cx);
    wheel(cx, false, true);
    wheel(cx, false, false);
    assert_eq!(
        reading_wheel_checkpoint(cx, &view, &dir, 1),
        b"\x1b[<64;3;4M\x1b[<65;3;4M"
    );
    graphics_assert_production_paint(window, &view, cx);
    assert_eq!(reading_painted_rows(&view, cx), live);
    wheel(cx, true, true);
    assert!(reading_wheel_checkpoint(cx, &view, &dir, 2).is_empty());
    graphics_assert_production_paint(window, &view, cx);
    let frozen = reading_painted_rows(&view, cx);
    assert_ne!(
        frozen, live,
        "Shift+wheel enters local history despite reporting"
    );
    wheel(cx, false, true);
    assert_eq!(
        reading_wheel_checkpoint(cx, &view, &dir, 3),
        b"\x1b[<64;3;4M"
    );
    graphics_assert_production_paint(window, &view, cx);
    assert_eq!(reading_painted_rows(&view, cx), frozen);
    view.read_with(cx, |view, _| view.terminal.write(vec![8]));
    graphics_recovery_wait(cx, "wheel agent alternate screen missing", |cx| {
        view.read_with(cx, |view, _| {
            view.scrollback_text().contains("ALT-WHEEL-READY") && !view.mouse_mode()
        })
    });
    graphics_assert_production_paint(window, &view, cx);
    assert_eq!(reading_painted_rows(&view, cx)[0], "ALT-WHEEL-READY");
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("terminal-return-live").is_none());
        })
        .unwrap();
    wheel(cx, false, true);
    wheel(cx, false, false);
    assert_eq!(
        reading_wheel_checkpoint(cx, &view, &dir, 4),
        b"\x1bOA\x1bOB"
    );
    wheel(cx, true, true);
    assert!(reading_wheel_checkpoint(cx, &view, &dir, 5).is_empty());
    graphics_assert_production_paint(window, &view, cx);
    assert_eq!(
        reading_painted_rows(&view, cx)[0],
        "ALT-WHEEL-READY",
        "local override cannot expose the old normal snapshot in an agent transcript"
    );
    view.read_with(cx, |view, _| view.terminal.write(vec![9]));
    graphics_recovery_wait(cx, "normal screen return missing", |cx| {
        view.read_with(cx, |view, _| {
            view.scrollback_text().contains("NORMAL-WHEEL-READY")
        })
    });
    graphics_assert_production_paint(window, &view, cx);
    wheel(cx, false, true);
    assert!(reading_wheel_checkpoint(cx, &view, &dir, 6).is_empty());
    graphics_assert_production_paint(window, &view, cx);
    assert!(reading_painted_rows(&view, cx)
        .iter()
        .any(|row| row.contains("NORMAL-WHEEL-READY")));
    assert!(!reading_painted_rows(&view, cx)
        .iter()
        .any(|row| row.contains("ALT-WHEEL-READY")));
    daemon.panes().kill(pane).unwrap();
    graphics_recovery_destroy(view, window, cx);
}

#[gpui_kit::test]
fn reading_lifecycle_closed_window_releases_retired_reading_pixels_before_view_drop(
    cx: &mut TestAppContext,
) {
    let (daemon, pane, view, window) = graphics_layout_harness(cx);
    graphics_send_fixture(&view, cx, 280, &b"CLOSE-READING\r\n".repeat(72));
    graphics_send_fixture(
        &view,
        cx,
        281,
        b"\x1b[8;5H\x1b_Ga=T,i=7,c=2,r=1,C=1,f=32,s=1,v=1,q=2;AAD//w==\x1b\\",
    );
    graphics_layout_wait_image(&view, cx, 7);
    view.update(cx, |view, cx| {
        view.smooth_scroll(1., cx);
    });
    graphics_assert_production_paint(window, &view, cx);
    let (images, pixels) = view.read_with(cx, |view, _| {
        (
            view.terminal.images(),
            std::sync::Arc::downgrade(
                view.grid_snap.as_ref().unwrap().images[0]
                    .data
                    .as_ref()
                    .unwrap(),
            ),
        )
    });
    graphics_send_fixture(&view, cx, 282, b"\x1b_Ga=d,d=I,i=7\x1b\\");
    graphics_assert_production_paint(window, &view, cx);
    assert!(
        pixels.upgrade().is_some(),
        "visible reading still owns its captured resource"
    );
    gpui_kit::VisualTestContext::from_window(window, cx).deactivate_window();
    window
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
    assert!(window.update(cx, |_, _, _| ()).is_err());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(300));
        cx.run_until_parked();
        if pixels.upgrade().is_none() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "closed-window reading pixels await entity Drop"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(images.snapshot().is_empty());
    assert!(
        view.read_with(cx, |view, _| presentation_allocation_observation(view))["reading"]
            .is_null()
    );
    drop(view);
    cx.run_until_parked();
    daemon.panes().kill(pane).unwrap();
}
