fn graphics_playback_wait(
    view: &Entity<TerminalView>, cx: &mut TestAppContext,
    predicate: impl Fn(&lyrel_core::core::graphics_state::Playback) -> bool,
) -> lyrel_core::core::graphics_state::Playback {
    let mut observed = None;
    graphics_recovery_wait(cx, "production consumer did not observe authoritative playback transition", |cx| {
        observed = view.read_with(cx, |view, _| {
            graphics_visible_images(view).into_iter().find_map(|image| image.playback.filter(|p| predicate(p)))
        });
        observed.is_some()
    });
    observed.unwrap()
}

#[gpui_kit::test]
fn graphics_consumer_playback_loading_append_two_consumers_pause_and_fresh_restore(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| { gpui_kit::component::init(cx); cx.set_global(Config::default()); });
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    let (controller, window) = graphics_recovery_fresh(&daemon, pane, cx, 1410);
    graphics_send_fixture_with_replies(&controller, cx, 1411,
        b"\x1b[6;5H\x1b_Ga=T,i=141,p=1,c=2,r=1,C=1,f=32,s=1,v=1;/wAA/w==\x1b\\\x1b_Ga=f,i=141,f=32,s=1,v=1,z=40;AP8A/w==\x1b\\\x1b[8;9H\x1b_Ga=p,i=141,p=2,c=2,r=1,C=1\x1b\\\x1b_Ga=a,i=141,s=2\x1b\\", 4);
    graphics_playback_wait(&controller, cx, |p| p.state == 2 && p.waiting);
    graphics_recovery_pixels(&controller, cx, 141, &[0, 255, 0, 255]);
    graphics_assert_production_paint(window, &controller, cx);
    graphics_send_fixture_with_replies(&controller, cx, 1412,
        b"\x1b_Ga=f,i=141,f=32,s=1,v=1,z=40;AAD//w==\x1b\\", 1);
    graphics_playback_wait(&controller, cx, |p| p.state == 2 && p.waiting);
    graphics_recovery_pixels(&controller, cx, 141, &[255, 0, 0, 255]);
    controller.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        assert_eq!(images.len(), 2);
        assert!(images.iter().all(|image| image.data.as_ref().unwrap().as_bytes(0).unwrap() == [255, 0, 0, 255]));
        assert!(std::sync::Arc::ptr_eq(images[0].data.as_ref().unwrap(), images[1].data.as_ref().unwrap()));
    });
    let mut stream = crate::daemon::transport::connect_endpoint_at(&daemon.endpoint("daemon")).unwrap();
    crate::daemon::protocol::negotiate_pane(&mut stream).unwrap();
    ClientMsg::GraphicsCapacity { encoded_bytes: 64 << 20, decoded_bytes: 128 << 20, offers: true }.encode(&mut stream).unwrap();
    ClientMsg::Observe { pane_id: pane, size: crate::daemon::protocol::WinSize { cols: 80, rows: 24, cell_w: 10, cell_h: 20 } }.encode(&mut stream).unwrap();
    let (observer, observer_window) = graphics_build_rooted_consumer(cx, pane, stream, Vec::new());
    graphics_assert_production_paint(observer_window, &observer, cx);
    graphics_recovery_pixels(&observer, cx, 141, &[255, 0, 0, 255]);
    graphics_playback_wait(&observer, cx, |p| !p.paused);
    controller.read_with(cx, |view, _| view.terminal.viewport_reporter().set_displayed(false));
    graphics_send_fixture_with_replies(&controller, cx, 1413, b"\x1b_Ga=a,i=141,s=2,c=1\x1b\\", 1);
    graphics_playback_wait(&observer, cx, |p| p.state == 2 && p.waiting && !p.paused);
    graphics_recovery_pixels(&observer, cx, 141, &[255, 0, 0, 255]);
    observer.read_with(cx, |view, _| view.terminal.viewport_reporter().set_displayed(false));
    graphics_playback_wait(&controller, cx, |p| p.paused);
    graphics_send_fixture_with_replies(&controller, cx, 1414, b"\x1b_Ga=a,i=141,s=3,c=2,r=2,z=60000,v=4\x1b\\", 1);
    let paused = graphics_playback_wait(&controller, cx, |p| p.state == 3 && p.paused);
    assert_eq!((paused.remaining_ms, paused.loops, paused.completed), (60000, 3, 0));
    graphics_recovery_pixels(&controller, cx, 141, &[0, 255, 0, 255]);
    let stats = graphics_recovery_stats(&controller, cx, 1415);
    graphics_recovery_destroy(observer, observer_window, cx);
    graphics_recovery_destroy(controller, window, cx);
    let (fresh, fresh_window) = graphics_recovery_fresh(&daemon, pane, cx, 1416);
    graphics_recovery_pixels(&fresh, cx, 141, &[0, 255, 0, 255]);
    let restored = graphics_playback_wait(&fresh, cx, |p| p.state == 3 && !p.paused);
    assert_eq!((restored.loops, restored.completed), (3, 0));
    assert!(restored.remaining_ms > 0 && restored.remaining_ms <= 60000);
    assert_eq!(graphics_recovery_stats(&fresh, cx, 1417), stats, "sender remains inactive: restoration and ticks must not re-upload pixels");
    graphics_assert_production_paint(fresh_window, &fresh, cx);
    graphics_recovery_destroy(fresh, fresh_window, cx);
    daemon.panes().kill(pane).unwrap();
}

#[gpui_kit::test]
fn graphics_consumer_mutations_running_loading_hidden_and_fresh(cx: &mut TestAppContext) {
    let (daemon, pane, view, window) = graphics_layout_harness(cx);
    graphics_send_fixture_with_replies(&view, cx, 14330,
        b"\x1b[5;1H\x1b_Ga=T,i=143,p=1,s=1,v=1,c=2,r=1,C=1;/wAA/w==\x1b\\\x1b_Ga=f,i=143,s=1,v=1,z=60000;AP8A/w==\x1b\\\x1b_Ga=f,i=143,s=1,v=1,z=60000;AAD//w==\x1b\\\x1b[8;1H\x1b_Ga=p,i=143,p=2,c=2,r=1,C=1\x1b\\\x1b_Ga=a,i=143,s=3,c=2,v=4\x1b\\", 5);
    graphics_playback_wait(&view, cx, |p| p.state == 3 && !p.paused);
    graphics_recovery_pixels(&view, cx, 143, &[0,255,0,255]);
    graphics_send_fixture_with_replies(&view, cx, 14331, b"\x1b_Ga=c,i=143,r=1,c=2,C=1\x1b\\", 1);
    graphics_recovery_pixels(&view, cx, 143, &[0,0,255,255]);
    graphics_send_fixture_with_replies(&view, cx, 14332, b"\x1b_Ga=f,i=143,r=2,s=1,v=1,X=1;//8A/w==\x1b\\", 1);
    graphics_recovery_pixels(&view, cx, 143, &[0,255,255,255]);
    graphics_send_fixture(&view, cx, 14333, b"\x1b_Ga=d,d=f,i=143,r=2\x1b\\");
    graphics_recovery_pixels(&view, cx, 143, &[255,0,0,255]);
    graphics_playback_wait(&view, cx, |p| p.state == 3 && p.loops == 3 && p.completed == 0);
    graphics_assert_production_paint(window, &view, cx);
    view.read_with(cx, |view, _| view.terminal.viewport_reporter().set_displayed(false));
    graphics_playback_wait(&view, cx, |p| p.paused);
    graphics_send_fixture_with_replies(&view, cx, 14334,
        b"\x1b_Ga=f,i=143,s=1,v=1;/////w==\x1b\\\x1b_Ga=c,i=143,r=1,c=3,C=1\x1b\\", 2);
    graphics_send_fixture(&view, cx, 14335, b"\x1b_Ga=d,d=f,i=143,r=1\x1b\\");
    graphics_recovery_pixels(&view, cx, 143, &[255,0,0,255]);
    let paused = graphics_playback_wait(&view, cx, |p| p.state == 3 && p.paused);
    assert_eq!((paused.loops, paused.completed), (3,0));
    view.read_with(cx, |view, _| view.terminal.viewport_reporter().set_displayed(true));
    graphics_playback_wait(&view, cx, |p| !p.paused);
    graphics_send_fixture_with_replies(&view, cx, 14336, b"\x1b_Ga=a,i=143,s=2,c=2,r=2,z=-1\x1b\\", 1);
    graphics_playback_wait(&view, cx, |p| p.state == 2 && p.waiting);
    graphics_recovery_pixels(&view, cx, 143, &[0,0,255,255]);
    graphics_send_fixture_with_replies(&view, cx, 14337, b"\x1b_Ga=f,i=143,s=1,v=1,z=40;/////w==\x1b\\", 1);
    graphics_playback_wait(&view, cx, |p| p.state == 2 && p.waiting);
    graphics_recovery_pixels(&view, cx, 143, &[255,255,255,255]);
    graphics_send_fixture(&view, cx, 14338, b"\x1b_Ga=d,d=f,i=143,r=2\x1b\\\x1b_Ga=d,d=F,i=143,r=2\x1b\\");
    graphics_recovery_pixels(&view, cx, 143, &[255,0,0,255]);
    let stopped = graphics_recovery_stats(&view, cx, 14339);
    graphics_recovery_destroy(view, window, cx);
    let (fresh, window) = graphics_recovery_fresh(&daemon, pane, cx, 14340);
    graphics_recovery_pixels(&fresh, cx, 143, &[255,0,0,255]);
    graphics_playback_wait(&fresh, cx, |p| p.state == 2 && p.waiting);
    assert_eq!(graphics_recovery_stats(&fresh, cx, 14341), stopped);
    fresh.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        assert_eq!(images.len(), 2);
        assert!(std::sync::Arc::ptr_eq(images[0].data.as_ref().unwrap(), images[1].data.as_ref().unwrap()));
    });
    graphics_assert_production_paint(window, &fresh, cx);
    graphics_recovery_destroy(fresh, window, cx);
    daemon.panes().kill(pane).unwrap();
}
