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
    // Deliberately retain a nonzero absolute origin: ED3 discards history but
    // does not renumber stable text rows, including on ConPTY.
    let mut setup = graphics_resize_history();
    setup.extend_from_slice(b"\x1b[2J\x1b[3J\x1b[8;5H\x1b_Ga=T,i=71,p=1,c=2,r=2,C=1,f=32,s=2,v=2,q=2;/wAA/wD/AP8AAP///////w==\x1b\\\x1b_Ga=p,i=71,p=2,P=71,Q=1,H=-6,V=1,c=4,r=2,x=1,w=1,X=1,Y=1,z=-2,q=2\x1b\\PARENT-ANCHOR");
    graphics_send_fixture(&view, cx, 681, &setup);
    graphics_recovery_pixels(
        &view,
        cx,
        71,
        &[
            0, 0, 255, 255, 0, 255, 0, 255, 255, 0, 0, 255, 255, 255, 255, 255,
        ],
    );
    let (parent_key, parent_row, child_row) = view.read_with(cx, |view, _| {
        let term = view.terminal.term.lock();
        assert_eq!((term.columns(), term.screen_lines()), (80, 24));
        assert!(term.grid.lines_evicted() > 0);
        drop(term);
        assert_eq!(graphics_resize_text_row(view, "PARENT-ANCHOR"), Some(7));
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
            child.anchor_col, -2,
            "signed geometry must not wrap or clamp at the pane edge"
        );
        assert_eq!(child.placement.z, -2);
        assert_eq!((child.placement.x, child.placement.w), (1, 1));
        let (crop, _) = child.paint_bounds(point(px(0.), px(0.)), px(10.), px(10.), 0, 0, 1.);
        assert_eq!(crop.origin, point(px(-19.), px(81.)));
        let (scaled, _) = child.paint_bounds(point(px(0.), px(0.)), px(15.), px(20.), 0, 0, 2.);
        assert_eq!(scaled.origin, point(px(-29.5), px(160.5)));
        assert_eq!(child.placement.parent, Some(parent.placement.key));
        assert_eq!(child.anchor_row - parent.anchor_row, 1);
        (parent.placement.key, parent.anchor_row, child.anchor_row)
    });
    graphics_assert_production_paint(window, &view, cx);
    graphics_send_fixture(&view, cx, 682, b"\x1b[6;12r\x1b[12;1H\n");
    graphics_assert_production_paint(window, &view, cx);
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
        assert_eq!(graphics_resize_text_row(view, "PARENT-ANCHOR"), Some(6));
        assert_eq!(parent.placement.key, parent_key);
        assert_eq!(parent.anchor_row, parent_row - 1);
        assert_eq!(child.anchor_row, child_row - 1);
        assert_eq!(child.placement.parent, Some(parent_key));
        assert_eq!(child.anchor_row - parent.anchor_row, 1);
        let (crop, _) = child.paint_bounds(point(px(0.), px(0.)), px(10.), px(10.), 0, 0, 1.);
        assert_eq!(
            crop.origin,
            point(px(-19.), px(71.)),
            "regional scroll moves the child to viewport row 7, not absolute row 7"
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

#[gpui_kit::test]
fn graphics_consumer_relative_selected_frames_retire_with_parent_and_history(
    cx: &mut TestAppContext,
) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::component::init(cx);
        cx.set_global(Config::default());
    });
    let daemon = GraphicsDaemonProcess::start();
    for (index, lifecycle) in ["delete", "replace", "expire"].into_iter().enumerate() {
        let serial = 700 + index * 20;
        let pane = daemon.spawn();
        let (view, window) = graphics_recovery_fresh(&daemon, pane, cx, serial);
        graphics_send_fixture_with_replies(
            &view,
            cx,
            serial + 1,
            b"\x1b[2J\x1b[3J\x1b[6;5H\x1b_Ga=T,i=81,p=1,c=2,r=1,C=1,f=32,s=1,v=1;/wAA/w==\x1b\\\x1b_Ga=f,i=81,f=32,s=1,v=1;AP8A/w==\x1b\\\x1b_Ga=T,i=82,p=2,P=81,Q=1,H=3,V=2,c=2,r=1,f=32,s=1,v=1;AAD//w==\x1b\\\x1b_Ga=f,i=82,f=32,s=1,v=1;/////w==\x1b\\ANIMATED-PARENT",
            4,
        );
        graphics_recovery_pixels(&view, cx, 81, &[0, 0, 255, 255]);
        graphics_recovery_pixels(&view, cx, 82, &[255, 0, 0, 255]);
        let (parent_key, parent_root, child_key, child_root) = view.read_with(cx, |view, _| {
            let images = graphics_visible_images(view);
            let parent = images.iter().find(|p| p.placement.image_id == 81).unwrap();
            let child = images.iter().find(|p| p.placement.image_id == 82).unwrap();
            (
                parent.placement.key,
                parent.placement.resource,
                child.placement.key,
                child.placement.resource,
            )
        });
        graphics_send_fixture_with_replies(
            &view,
            cx,
            serial + 2,
            b"\x1b_Ga=a,i=81,s=1,c=2\x1b\\\x1b_Ga=a,i=82,s=1,c=2\x1b\\",
            2,
        );
        graphics_recovery_pixels(&view, cx, 81, &[0, 255, 0, 255]);
        graphics_recovery_pixels(&view, cx, 82, &[255, 255, 255, 255]);
        graphics_assert_production_paint(window, &view, cx);
        view.read_with(cx, |view, _| {
            let images = graphics_visible_images(view);
            assert_eq!(images.len(), 2);
            let parent = images.iter().find(|p| p.placement.image_id == 81).unwrap();
            let child = images.iter().find(|p| p.placement.image_id == 82).unwrap();
            assert_eq!((parent.placement.key, parent.placement.resource), (parent_key, parent_root));
            assert_eq!((child.placement.key, child.placement.resource), (child_key, child_root));
            assert_eq!(child.placement.parent, Some(parent_key));
            assert_eq!(graphics_resize_text_row(view, "ANIMATED-PARENT"), Some(5));
            let (crop, _) = child.paint_bounds(point(px(0.), px(0.)), px(10.), px(10.), 0, 0, 1.);
            assert_eq!(crop.origin, point(px(70.), px(70.)));
        });
        graphics_recovery_destroy(view, window, cx);

        // The publisher changes while every desktop cache is gone. Both
        // pictures had selected non-root frames, but graph identity and the
        // retention group still belong to their original resource roots.
        let mutation = match lifecycle {
            "delete" => b"\x1b_Ga=d,d=i,i=81,p=1\x1b\\".to_vec(),
            "replace" => b"\x1b[10;5H\x1b_Ga=T,i=81,p=9,c=2,r=1,C=1,f=32,s=1,v=1,q=2;/wD//w==\x1b\\\x1b_Ga=f,i=81,f=32,s=1,v=1,q=2;AP///w==\x1b\\\x1b_Ga=a,i=81,s=1,c=2,q=2\x1b\\REPLACED-PARENT".to_vec(),
            "expire" => {
                let mut bytes = b"\x1b[r\x1b[24;1H".to_vec();
                bytes.extend_from_slice(&b"\r\n".repeat(10_100));
                bytes
            }
            _ => unreachable!(),
        };
        graphics_recovery_detached_request(&daemon, pane, "SEND", serial + 3, &mutation);
        let statistics = graphics_recovery_detached_request(&daemon, pane, "STAT", serial + 4, b"\0");
        let (fresh, fresh_window) = graphics_recovery_fresh(&daemon, pane, cx, serial + 5);
        if lifecycle == "replace" {
            graphics_recovery_pixels(&fresh, cx, 81, &[255, 255, 0, 255]);
        }
        graphics_assert_production_paint(fresh_window, &fresh, cx);
        fresh.read_with(cx, |view, _| {
            let images = graphics_visible_images(view);
            if lifecycle == "replace" {
                assert_eq!(images.len(), 1, "retransmission must retire the old parent graph and every selected child frame");
                let parent = &images[0];
                assert_eq!((parent.placement.image_id, parent.placement.placement_id), (81, 9));
                assert_ne!(parent.placement.resource, parent_root);
                assert_ne!(parent.placement.key, parent_key);
                assert_eq!(parent.placement.parent, None);
                assert_eq!(graphics_resize_text_row(view, "REPLACED-PARENT"), Some(9));
                let (crop, _) = parent.paint_bounds(point(px(0.), px(0.)), px(10.), px(10.), 0, 0, 1.);
                assert_eq!(crop.origin, point(px(40.), px(90.)));
            } else {
                assert!(images.is_empty(), "{lifecycle} must not revive either selected frame on fresh attach");
            }
        });
        let recovered_stats = graphics_recovery_stats(&fresh, cx, serial + 6);
        assert!(statistics.contains(&format!("KITTY_STATS_{} {recovered_stats}", serial + 4)),
            "fresh recovery must not request sender retransmission: {statistics}");
        graphics_recovery_destroy(fresh, fresh_window, cx);

        // No remaining child placement means its whole animation is gone,
        // not merely hidden until an explicit frame selection/re-placement.
        let (text, _) = graphics_recovery_exchange(
            &daemon,
            pane,
            "SEND",
            serial + 7,
            b"\x1b_Ga=p,i=82,p=3\x1b\\\x1b_Ga=a,i=82,s=1,c=2\x1b\\",
            2,
        );
        for reply in [
            b"\x1b_Gi=82,p=3;ENOENT:Image not found\x1b\\".as_slice(),
            b"\x1b_Gi=82;ENOENT:Image not found\x1b\\".as_slice(),
        ] {
            assert!(text.contains(&format!("KITTY_REPLY {} {}", serial + 7, graphics_pty_fixture::hex(reply))), "{lifecycle}: {text}");
        }
        daemon.panes().kill(pane).unwrap();
    }
}
