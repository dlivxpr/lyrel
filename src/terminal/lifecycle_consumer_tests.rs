// Consumer regressions for the retained terminal's event and resource lifetime.
    #[gpui_kit::test]
    fn graphics_consumer_hidden_decode_keeps_pixels_without_notifying_window(
        cx: &mut TestAppContext,
    ) {
        use lyrel_core::core::kitty_graphics::{GraphicsUpdate, Image, WireFormat};
        use std::{cell::Cell, rc::Rc, sync::Arc};

        let (window, mut daemon) = harness_on(cx, PtySource::Raw);
        let images = window
            .update(cx, |view, _, _| view.terminal.images())
            .unwrap();
        let pane = window
            .update(cx, |_, _, cx| cx.entity())
            .unwrap();
        cx.update(|cx| declare_displayed(cx, [(pane.entity_id(), false)]));
        cx.run_until_parked();
        let notifications = Rc::new(Cell::new(0));
        let count = notifications.clone();
        let _subscription = cx.update(|cx| {
            cx.observe(&pane, move |_, _| {
                count.set(count.get() + 1);
            })
        });
        send_admitted_graphics(
            &images,
            &mut daemon,
            GraphicsUpdate::Resource {
                key: 41,
                image: Arc::new(Image {
                    width: 1,
                    height: 1,
                    format: WireFormat::Rgba,
                    compressed: false,
                    data: vec![1, 2, 3, 64],
                }),
            },
        );
        let mut placement = graphics_placement(51, 41, 0, 0);
        placement.x = 0;
        send_admitted_graphics(&images, &mut daemon, GraphicsUpdate::Place(placement));
        for _ in 0..200 {
            cx.run_until_parked();
            if pane.read_with(cx, |view, _| !view.terminal.images().snapshot().is_empty()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        cx.run_until_parked();
        pane.read_with(cx, |view, _| {
            let images = view.terminal.images().snapshot();
            assert_eq!(
                images[0].data.as_ref().unwrap().as_bytes(0).unwrap(),
                &[3, 2, 1, 64]
            );
            assert!(!images[0].painted.load(std::sync::atomic::Ordering::Acquire));
        });
        assert_eq!(
            notifications.get(),
            0,
            "decoded hidden output must not dirty the window"
        );
        cx.update(|cx| declare_displayed(cx, [(pane.entity_id(), true)]));
        pane.read_with(cx, |view, _| {
            let images = view.terminal.images().snapshot();
            assert_eq!(
                images.len(),
                1,
                "returning to the pane retains its current image without a resend"
            );
            assert_eq!(
                images[0].data.as_ref().unwrap().as_bytes(0).unwrap(),
                &[3, 2, 1, 64]
            );
        });
        send_graphics(
            &mut daemon,
            GraphicsUpdate::Remove {
                placements: vec![51],
                resources: vec![41],
            },
        );
        for _ in 0..200 {
            cx.run_until_parked();
            if notifications.get() > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(notifications.get() > 0, "a visible removal must repaint");
        pane.read_with(cx, |view, _| {
            assert!(view.terminal.images().snapshot().is_empty())
        });
    }

    #[gpui_kit::test]
    fn daemon_output_reaches_the_grid_through_the_event_pump(cx: &mut TestAppContext) {
        let (window, mut daemon) = harness(cx);

        let read_row = |cx: &mut TestAppContext, len: usize| -> String {
            window
                .update(cx, |view, _, _| {
                    let term = view.terminal.term.clone();
                    let term = term.lock();
                    let grid = &term.grid;
                    (0..len).map(|c| grid[Line(0)][Column(c)].c()).collect()
                })
                .unwrap()
        };
        let wait_for = |cx: &mut TestAppContext, want: &str| {
            let mut got = String::new();
            for _ in 0..400 {
                cx.run_until_parked();
                got = read_row(cx, want.chars().count());
                if got == want {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            got
        };

        DaemonMsg::Output(b"hello".to_vec())
            .encode(&mut daemon)
            .unwrap();
        assert_eq!(wait_for(cx, "hello"), "hello");

        DaemonMsg::Output(b" again".to_vec())
            .encode(&mut daemon)
            .unwrap();
        assert_eq!(wait_for(cx, "hello again"), "hello again");
    }
struct GraphicsWindowRenderProbe {
    renders: std::rc::Rc<std::cell::Cell<usize>>,
    terminal: Option<Entity<TerminalView>>,
}

impl Render for GraphicsWindowRenderProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        gpui_kit::div().size_full().children(self.terminal.clone())
    }
}

#[gpui_kit::test]
fn graphics_consumer_hidden_retired_atlas_does_not_wake_window_and_returns_current_pixels(
    cx: &mut TestAppContext,
) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        let mut config = Config::default();
        config.cursor_blink = false;
        cx.set_global(config);
    });
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    let (view, window) = graphics_rooted_consumer(cx, pane, daemon.stream(pane));
    graphics_send_fixture(&view, cx, 1, b"\x1b[2J\x1b[3J\x1b[6;5H\x1b_Ga=T,i=7,p=1,c=2,r=1,C=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\\x1b[8;5H\x1b_Ga=T,i=8,p=2,c=2,r=1,C=1,f=32,s=1,v=1,q=2;AAD//w==\x1b\\");
    graphics_layout_wait_image(&view, cx, 7);
    graphics_layout_wait_image(&view, cx, 8);
    graphics_assert_production_paint(window, &view, cx);
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
    // Do not use graphics_send_fixture here: its global refresh is deliberate
    // paint-driver machinery and would conceal a hidden-output invalidation.
    let request = format!(
        "SEND 2 0 {} 0\n",
        graphics_pty_fixture::hex(
            b"\x1b[6;5H\x1b_Ga=T,i=7,p=1,c=2,r=1,C=1,f=32,s=1,v=1,q=2;AP8A/w==\x1b\\"
        )
    );
    view.read_with(cx, |view, _| view.terminal.write(request.into_bytes()));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        cx.run_until_parked();
        let ready = view.read_with(cx, |view, _| {
            view.scrollback_text().contains("KITTY_DONE_2;")
                && graphics_visible_images(view).iter().any(|image| {
                    image.placement.image_id == 7
                        && image
                            .data
                            .as_ref()
                            .is_some_and(|data| data.as_bytes(0).unwrap() == [0, 255, 0, 255])
                })
        });
        if ready {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "hidden replacement did not complete"
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
        "retiring previously painted hidden images must not invalidate an unrelated window surface"
    );
    cx.update(|cx| declare_displayed(cx, [(view.entity_id(), true)]));
    probe.update(cx, |probe, cx| {
        probe.terminal = Some(view.clone());
        cx.notify();
    });
    graphics_assert_production_paint(window, &view, cx);
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        assert_eq!(images.len(), 2);
        for (id, pixels) in [(7, [0, 255, 0, 255]), (8, [255, 0, 0, 255])] {
            let image = images
                .iter()
                .find(|image| image.placement.image_id == id)
                .unwrap();
            assert_eq!(image.data.as_ref().unwrap().as_bytes(0).unwrap(), pixels);
            assert!(image.painted.load(std::sync::atomic::Ordering::Acquire));
        }
    });
    cx.run_until_parked();
    let baseline = renders.get();
    let request = format!(
        "SEND 3 0 {} 0\n",
        graphics_pty_fixture::hex(b"\x1b_Ga=d,d=I,i=8\x1b\\")
    );
    view.read_with(cx, |view, _| view.terminal.write(request.into_bytes()));
    while view.read_with(cx, |view, _| {
        !view.scrollback_text().contains("KITTY_DONE_3;")
    }) {
        assert!(
            std::time::Instant::now() < deadline,
            "visible removal did not complete"
        );
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    cx.run_until_parked();
    assert!(
        renders.get() > baseline,
        "visible atlas invalidation must still repaint"
    );
    daemon.panes().kill(pane).unwrap();
}

#[gpui_kit::test]
fn graphics_consumer_window_closed_before_release_returns_images_and_registration(
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
    graphics_send_fixture(&view, cx, 1, b"\x1b[2J\x1b[3J\x1b[6;5H\x1b_Ga=T,i=7,p=1,c=2,r=1,C=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\");
    graphics_layout_wait_image(&view, cx, 7);
    graphics_assert_production_paint(window, &view, cx);
    let id = view.entity_id();
    let weak = view.downgrade();
    // Retain the store independently: relying on its final Drop would hide a
    // release hook that was skipped because its window can no longer update.
    let (images, pixels) = view.read_with(cx, |view, _| {
        let images = view.terminal.images();
        let pixels: Vec<_> = images.snapshot().iter()
            .filter_map(|image| image.data.as_ref().map(std::sync::Arc::downgrade))
            .collect();
        (images, pixels)
    });
    assert!(!pixels.is_empty(), "the closed consumer must own real decoded pixels");
    cx.update(|cx| {
        assert_eq!(crate::terminal::lifecycle::displayed_for_test(cx, id), Some(true));
    });
    // The headless platform retains the active IME handler independently of
    // App's window map. Native deactivation releases that strong view handle.
    gpui_kit::VisualTestContext::from_window(window, cx).deactivate_window();
    window.update(cx, |_, window, _| window.remove_window()).unwrap();
    assert!(window.update(cx, |_, _, _| ()).is_err());
    // The existing foreground clock detects an unavailable window even while
    // another owner retains the pane entity. Cleanup must not await its Drop.
    cx.executor().advance_clock(std::time::Duration::from_millis(300));
    cx.run_until_parked();
    assert!(images.snapshot().is_empty(), "closed-window cleanup cannot wait for entity release");
    cx.update(|cx| {
        assert_eq!(crate::terminal::lifecycle::displayed_for_test(cx, id), None);
    });
    drop(view);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        cx.update(|_| {});
        if weak.upgrade().is_none() && pixels.iter().all(|pixel| pixel.upgrade().is_none()) {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "closed consumer retained its entity or pixels");
        cx.executor().advance_clock(std::time::Duration::from_millis(5));
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(images.snapshot().is_empty(), "release must clear even an independently retained store");
    cx.update(|cx| {
        assert_eq!(crate::terminal::lifecycle::displayed_for_test(cx, id), None);
    });
    // Closing the GUI consumer detached it; the daemon still owns the live
    // pane and can supply a new consumer without restarting its PTY.
    let (reopened, reopened_window) = graphics_recovery_fresh(&daemon, pane, cx, 2);
    graphics_recovery_pixels(&reopened, cx, 7, &[0, 0, 255, 255]);
    graphics_recovery_destroy(reopened, reopened_window, cx);
    daemon.panes().kill(pane).unwrap();
}
