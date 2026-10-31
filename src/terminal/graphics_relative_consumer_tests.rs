#[gpui_kit::test]
fn graphics_consumer_relative_graph_moves_and_recovers_without_retransmit(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    let (view, window) = graphics_recovery_fresh(&daemon, pane, cx, 600);
    graphics_send_fixture(&view, cx, 601, b"\x1b[2J\x1b[3J\x1b[6;5H\x1b_Ga=T,i=41,p=1,c=2,r=1,C=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\\x1b_Ga=T,i=42,p=2,P=41,Q=1,H=-2,V=2,c=1,r=1,f=32,s=1,v=1,q=2;AAD//w==\x1b\\CURSOR-UNCHANGED");
    graphics_recovery_pixels(&view, cx, 42, &[255, 0, 0, 255]);
    graphics_assert_production_paint(window, &view, cx);
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        let child = images.iter().find(|p| p.placement.image_id == 42).unwrap();
        assert_eq!(
            child
                .paint_bounds(point(px(0.), px(0.)), px(10.), px(10.), 0, 0, 1.)
                .0
                .origin,
            point(px(20.), px(70.))
        );
        let term = view.terminal.term.lock();
        assert_eq!(term.grid[Line(5)][Column(4)].c(), 'C');
    });
    graphics_send_fixture(
        &view,
        cx,
        602,
        b"\x1b[8;8H\x1b_Ga=p,i=41,p=1,c=2,r=1,C=1,q=2\x1b\\",
    );
    let stopped = graphics_recovery_stats(&view, cx, 603);
    graphics_recovery_destroy(view, window, cx);
    let (fresh, fresh_window) = graphics_recovery_fresh(&daemon, pane, cx, 604);
    graphics_recovery_pixels(&fresh, cx, 42, &[255, 0, 0, 255]);
    graphics_assert_production_paint(fresh_window, &fresh, cx);
    fresh.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        let child = images.iter().find(|p| p.placement.image_id == 42).unwrap();
        assert_eq!(
            child
                .paint_bounds(point(px(0.), px(0.)), px(10.), px(10.), 0, 0, 1.)
                .0
                .origin,
            point(px(50.), px(90.))
        );
        assert!(child.painted.load(std::sync::atomic::Ordering::Acquire));
    });
    assert_eq!(graphics_recovery_stats(&fresh, cx, 605), stopped);
    graphics_send_fixture(&fresh, cx, 606, b"\x1b_Ga=d,d=i,i=41,p=1\x1b\\");
    assert!(fresh.read_with(cx, |view, _| graphics_visible_images(view).is_empty()));
    graphics_assert_production_paint(fresh_window, &fresh, cx);
    graphics_recovery_destroy(fresh, fresh_window, cx);
    let (last, last_window) = graphics_recovery_fresh(&daemon, pane, cx, 607);
    assert!(last.read_with(cx, |view, _| graphics_visible_images(view).is_empty()));
    graphics_recovery_destroy(last, last_window, cx);
    daemon.panes().kill(pane).unwrap();
}

#[gpui_kit::test]
fn graphics_consumer_relative_validation_is_correlated_quiet_and_atomic(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    let (view, window) = graphics_recovery_fresh(&daemon, pane, cx, 620);
    graphics_send_fixture(
        &view,
        cx,
        621,
        b"\x1b[2J\x1b[3J\x1b[6;5H\x1b_Ga=T,i=51,p=1,c=1,r=1,C=1,f=32,s=1,v=1,q=2;/wAA/w==\x1b\\",
    );
    let mut chain = String::new();
    for link in 1..=8 {
        chain.push_str(&format!(
            "\x1b_Ga=p,i=51,p={},P=51,Q={},H=1,V=1,c=1,r=1,q=2\x1b\\",
            link + 1,
            link
        ));
    }
    graphics_send_fixture(&view, cx, 622, chain.as_bytes());
    graphics_recovery_wait(cx, "eight-link graph must decode", |cx| {
        view.read_with(cx, |view, _| graphics_visible_images(view).len() == 9)
    });
    graphics_assert_production_paint(window, &view, cx);
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        let last = images
            .iter()
            .find(|p| p.placement.placement_id == 9)
            .unwrap();
        assert_eq!(
            last.paint_bounds(point(px(0.), px(0.)), px(10.), px(10.), 0, 0, 1.)
                .0
                .origin,
            point(px(120.), px(130.))
        );
        assert_eq!(
            last.data.as_ref().unwrap().as_bytes(0).unwrap(),
            &[0, 0, 255, 255]
        );
    });
    for (index, controls, status) in [
        (
            0,
            "p=10,P=51,Q=9",
            "ETOODEEP:Relative placement chain exceeds eight links",
        ),
        (1, "p=1,P=51,Q=9", "ECYCLE:Placement cycle"),
        (2, "p=11,P=99,Q=1", "ENOPARENT:Parent placement not found"),
        (
            3,
            "p=12,P=51,Q=1,U=1",
            "EINVAL:Virtual placement cannot be relative",
        ),
    ] {
        for quiet in 0..=2 {
            let serial = 630 + index * 3 + quiet;
            let request = format!("\x1b_Ga=p,i=51,{controls},q={quiet}\x1b\\");
            let (text, _) = graphics_recovery_exchange(
                &daemon,
                pane,
                "SEND",
                serial,
                request.as_bytes(),
                usize::from(quiet != 2),
            );
            if quiet != 2 {
                let placement = [10, 1, 11, 12][index];
                let reply = format!("\x1b_Gi=51,p={placement};{status}\x1b\\");
                assert!(
                    text.contains(&format!(
                        "KITTY_REPLY {serial} {}",
                        graphics_pty_fixture::hex(reply.as_bytes())
                    )),
                    "{text}"
                );
            }
        }
    }
    graphics_send_fixture(
        &view,
        cx,
        650,
        b"\x1b[?1049h\x1b_Ga=T,i=52,p=1,P=51,Q=1,f=32,s=1,v=1,q=2;AAD//w==\x1b\\\x1b[?1049l",
    );
    assert_eq!(
        view.read_with(cx, |view, _| graphics_visible_images(view).len()),
        9
    );
    graphics_send_fixture(&view, cx, 651, b"\x1b_Ga=d,d=i,i=51,p=1\x1b\\");
    assert!(view.read_with(cx, |view, _| graphics_visible_images(view).is_empty()));
    graphics_recovery_destroy(view, window, cx);
    daemon.panes().kill(pane).unwrap();
}

#[gpui_kit::test]
fn graphics_consumer_relative_virtual_parent_follows_real_text_and_detached_deletion(
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
    let (view, window) = graphics_recovery_fresh(&daemon, pane, cx, 660);
    graphics_send_fixture(&view, cx, 661, "\x1b[2J\x1b[3J\x1b_Ga=T,i=61,p=1,U=1,c=2,r=1,f=32,s=1,v=1,q=2;AAAAAA==\x1b\\\x1b_Ga=T,i=62,p=2,P=61,Q=1,H=-1,V=1,c=2,r=1,f=32,s=1,v=1,q=2;AP8A/w==\x1b\\\x1b[8;9H\x1b[38;5;61;58;5;1m\u{10eeee}\u{305}\u{305}\x1b[6;12H\u{10eeee}\u{305}\u{305}\x1b[0m".as_bytes());
    graphics_recovery_pixels(&view, cx, 62, &[0, 255, 0, 255]);
    graphics_assert_production_paint(window, &view, cx);
    let assert_child = |view: &TerminalView, x, y| {
        let images = graphics_visible_images(view);
        let child = images.iter().find(|p| p.placement.image_id == 62).unwrap();
        assert_eq!(
            child
                .paint_bounds(point(px(0.), px(0.)), px(10.), px(10.), 0, 0, 1.)
                .0
                .origin,
            point(px(x), px(y))
        );
    };
    // Independent minima, not the first run or the prototype's cursor.
    view.read_with(cx, |view, _| assert_child(view, 70., 60.));
    graphics_send_fixture(&view, cx, 662, b"\x1b[6;1H\x1b[L");
    view.read_with(cx, |view, _| assert_child(view, 70., 70.));
    graphics_send_fixture(&view, cx, 663, b"\x1b[7;12H\x1b[X");
    view.read_with(cx, |view, _| assert_child(view, 70., 90.));
    graphics_send_fixture(&view, cx, 664, b"\x1b[9;9H\x1b[X");
    assert!(view.read_with(cx, |view, _| graphics_visible_images(view).is_empty()));
    graphics_send_fixture(
        &view,
        cx,
        665,
        "\x1b[8;9H\x1b[38;5;61;58;5;1m\u{10eeee}\u{305}\u{305}\x1b[0m".as_bytes(),
    );
    graphics_recovery_pixels(&view, cx, 62, &[0, 255, 0, 255]);
    graphics_recovery_destroy(view, window, cx);
    let (fresh, fresh_window) = graphics_recovery_fresh(&daemon, pane, cx, 666);
    graphics_recovery_pixels(&fresh, cx, 62, &[0, 255, 0, 255]);
    fresh.read_with(cx, |view, _| assert_child(view, 70., 80.));
    graphics_recovery_destroy(fresh, fresh_window, cx);
    graphics_recovery_exchange(
        &daemon,
        pane,
        "SEND",
        667,
        b"\x1b_Ga=d,d=i,i=61,p=1\x1b\\",
        0,
    );
    let (last, last_window) = graphics_recovery_fresh(&daemon, pane, cx, 668);
    assert!(last.read_with(cx, |view, _| graphics_visible_images(view).is_empty()));
    graphics_recovery_destroy(last, last_window, cx);
    daemon.panes().kill(pane).unwrap();
}

#[gpui_kit::test]
fn graphics_consumer_relative_layout_clips_and_expires_as_one_group(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let daemon = GraphicsDaemonProcess::start();
    let pane = daemon.spawn();
    let (view, window) = graphics_recovery_fresh(&daemon, pane, cx, 680);
    graphics_send_fixture(&view, cx, 681, b"\x1b[2J\x1b[3J\x1b[8;5H\x1b_Ga=T,i=71,p=1,c=2,r=2,C=1,f=32,s=2,v=2,q=2;/wAA/wD/AP8AAP///////w==\x1b\\\x1b_Ga=p,i=71,p=2,P=71,Q=1,H=-6,V=1,c=4,r=2,x=1,w=1,X=1,Y=1,z=-2,q=2\x1b\\");
    graphics_recovery_pixels(
        &view,
        cx,
        71,
        &[
            0, 0, 255, 255, 0, 255, 0, 255, 255, 0, 0, 255, 255, 255, 255, 255,
        ],
    );
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        let child = images
            .iter()
            .find(|p| p.placement.placement_id == 2)
            .unwrap();
        assert_eq!(
            child.anchor_col, -2,
            "signed geometry must not wrap or clamp at the pane edge"
        );
        assert_eq!(child.placement.z, -2);
        assert_eq!((child.placement.x, child.placement.w), (1, 1));
        let (crop, _) = child.paint_bounds(point(px(0.), px(0.)), px(10.), px(10.), 0, 0, 1.);
        assert_eq!(crop.origin, point(px(-19.), px(81.)));
        let (scaled, _) = child.paint_bounds(point(px(0.), px(0.)), px(15.), px(20.), 0, 0, 2.);
        assert_eq!(scaled.origin, point(px(-29.5), px(160.5)));
    });
    graphics_assert_production_paint(window, &view, cx);
    graphics_send_fixture(&view, cx, 682, b"\x1b[6;12r\x1b[12;1H\n");
    graphics_assert_production_paint(window, &view, cx);
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        let child = images
            .iter()
            .find(|p| p.placement.placement_id == 2)
            .unwrap();
        assert_eq!(
            child.anchor_row, 7,
            "regional scroll moves the parent once, not parent plus child"
        );
    });
    graphics_assert_production_paint_at(window, &view, cx, TermSize::new(40, 24));
    graphics_send_fixture(&view, cx, 683, b"\x1b[r\x1b[20;1HLAYOUT-BARRIER");
    view.read_with(cx, |view, _| {
        let images = graphics_visible_images(view);
        let parent = images
            .iter()
            .find(|p| p.placement.placement_id == 1)
            .unwrap();
        let child = images
            .iter()
            .find(|p| p.placement.placement_id == 2)
            .unwrap();
        assert_eq!(
            (
                child.anchor_row - parent.anchor_row,
                child.anchor_col - parent.anchor_col
            ),
            (1, -6)
        );
    });
    graphics_send_fixture(&view, cx, 684, &b"\r\n".repeat(10_100));
    graphics_assert_production_paint(window, &view, cx);
    assert!(view.read_with(cx, |view, _| graphics_visible_images(view).is_empty()));
    graphics_recovery_destroy(view, window, cx);
    let (fresh, fresh_window) =
        graphics_recovery_fresh_at(&daemon, pane, cx, 685, TermSize::new(40, 24));
    assert!(fresh.read_with(cx, |view, _| graphics_visible_images(view).is_empty()));
    graphics_send_fixture(&fresh, cx, 686, b"\x1bc");
    graphics_assert_production_paint(fresh_window, &fresh, cx);
    graphics_recovery_destroy(fresh, fresh_window, cx);
    daemon.panes().kill(pane).unwrap();
}
