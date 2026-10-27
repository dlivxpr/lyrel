use super::*;
use gpui_kit::test::{TestEventExt as _, TestWindowExt as _};
use gpui_kit::{MouseButton, TestAppContext, point};

#[gpui_kit::test]
fn pip_header_commands_target_the_displayed_pane_not_the_old_focus(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 2);
    let (left, right) = app.update_in(&mut vcx, |app, window, cx| {
        let left = app.tabs[0].pane.first_leaf().unwrap();
        let right = app.tabs.remove(1).pane.first_leaf().unwrap();
        right.terminal().unwrap().update(cx, |view, _| {
            view.title = "PiP scoped terminal".to_owned();
        });
        app.tabs[0]
            .pane
            .split_leaf(left.entity_id(), Axis::Horizontal, false, right.clone());
        window.focus(&right.focus_handle(cx), cx);
        cx.notify();
        (left, right)
    });
    let key = right.entity_id().as_u64() as usize;
    vcx.dispatch_action(TogglePanePip);
    app.update_in(&mut vcx, |_, window, cx| {
        window.focus(&left.focus_handle(cx), cx);
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let header = window.find(("pip-header", key)).bounds();
        assert!((header.size.height - window.rem_size() * (26. / 16.)).abs() <= px(1.));
        assert_eq!(
            window.find(("pip-title", key)).label(),
            Some("PiP scoped terminal")
        );
        assert_eq!(
            window.find(("pip-park", key)).role(),
            Some(gpui_kit::Role::Button)
        );
        assert_eq!(
            window.find(("pip-dock", key)).label(),
            Some(t(L10nKey::PaneRestorePip))
        );
        assert_eq!(window.find_all(("pane-drag-handle", key)).len(), 1);
        window.click(("pip-dock", key), cx);
    });
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(app.tabs[0].pip_slot(cx).is_none());
        assert!(right.contains_focused(window, cx));
        assert!(app.tabs[0].pane.leaves().contains(&right));
    });
    vcx.dispatch_action(TogglePanePip);
    app.update_in(&mut vcx, |_, window, cx| {
        window.focus(&left.focus_handle(cx), cx);
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.click(("pip-park", key), cx);
    });
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(app.tabs[0].pip_slot(cx).is_none());
        assert!(app.tabs[0].is_parked_slot(&right, cx));
        assert!(!app.tabs[0].is_parked_slot(&left, cx));
        assert!(left.contains_focused(window, cx));
        assert!(app.tabs[0].pane.leaves().contains(&right));
    });
}

#[gpui_kit::test]
fn pip_geometry_controls_are_named_buttons_and_keep_arrow_keys_out_of_the_pty(
    cx: &mut TestAppContext,
) {
    use crate::daemon::protocol::ClientMsg;
    use std::time::Duration;
    let (app, mut vcx, mut stream) = test_window::harness_with_pane(cx);
    let pane = app.update_in(&mut vcx, |app, window, cx| {
        let pane = app.tabs[0].pane.first_leaf().unwrap();
        window.focus(&pane.focus_handle(cx), cx);
        pane
    });
    vcx.dispatch_action(TogglePanePip);
    let key = pane.entity_id().as_u64() as usize;
    for (handle, name, arrow) in [
        ("pip-move", L10nKey::PanePipMove, "left"),
        ("pip-resize", L10nKey::PanePipResize, "right"),
    ] {
        vcx.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find((handle, key)).role(),
                Some(gpui_kit::Role::Button)
            );
            assert_eq!(window.find((handle, key)).label(), Some(t(name)));
            window.click((handle, key), cx);
            assert_eq!(window.find((handle, key)).focused(), Some(true));
            let before = window.find(("pane-pip", key)).bounds();
            window.press(arrow, cx);
            let after = window.find(("pane-pip", key)).bounds();
            if handle == "pip-move" {
                assert!(after.left() < before.left());
            } else {
                assert!(after.size.width > before.size.width);
            }
            window.press("escape", cx);
            assert!(pane.contains_focused(window, cx));
        });
    }
    vcx.simulate_keystrokes("x");
    stream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let mut input = Vec::new();
    while let Ok(message) = ClientMsg::read(&mut stream) {
        if let ClientMsg::Input(bytes) = message {
            input.extend(bytes);
        }
    }
    assert_eq!(
        input, b"x",
        "geometry commands must not be sent to the live PTY"
    );
}

#[gpui_kit::test]
fn pip_move_resize_and_zoom_preserve_the_live_terminal_and_focus(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
    let pane = app.update_in(&mut vcx, |app, window, cx| {
        let pane = app.tabs[0].pane.first_leaf().unwrap();
        window.focus(&pane.focus_handle(cx), cx);
        pane
    });
    vcx.dispatch_action(TogglePanePip);
    vcx.run_until_parked();
    let key = pane.entity_id().as_u64() as usize;
    let before = vcx.update(|window, cx| {
        window.render_frame(cx);
        let bounds = window.find(("pane-pip", key)).bounds();
        let start = window.find(("pip-move", key)).bounds().center();
        window.pointer_down(start, MouseButton::Left, cx);
        window.pointer_move(start - point(px(70.), px(55.)), Some(MouseButton::Left), cx);
        window.pointer_up(start - point(px(70.), px(55.)), MouseButton::Left, cx);
        bounds
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let moved = window.find(("pane-pip", key)).bounds();
        assert!(moved.left() < before.left() && moved.top() < before.top());
        assert!(
            pane.contains_focused(window, cx),
            "pointer geometry does not steal terminal focus"
        );
        let start = window.find(("pip-resize", key)).bounds().center();
        window.pointer_down(start, MouseButton::Left, cx);
        window.pointer_move(start + point(px(65.), px(35.)), Some(MouseButton::Left), cx);
        window.pointer_up(start + point(px(65.), px(35.)), MouseButton::Left, cx);
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.find(("pane-pip", key)).bounds().size.width > before.size.width);
        assert!(pane.contains_focused(window, cx));
        assert_eq!(window.find_all(("pane-drag-handle", key)).len(), 1);
    });
    for size in [20., 14.] {
        app.update_in(&mut vcx, |_, _, cx| {
            cx.global_mut::<Config>().ui_font_size = size;
            gpui_kit::component::Theme::update(cx, |theme| theme.font_size = px(size));
            cx.notify();
        });
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            window.render_frame(cx);
            let pip = window.find(("pane-pip", key)).bounds();
            let content = window.find("pane-presentations").bounds();
            assert!(pip.left() >= content.left() && pip.right() <= content.right());
            assert!(pip.top() >= content.top() && pip.bottom() <= content.bottom());
        });
    }
    for width in [620., 1280.] {
        vcx.simulate_resize(size(px(width), px(720.)));
        for left in [true, false] {
            if left {
                vcx.dispatch_action(ToggleLeftPanel);
            } else {
                vcx.dispatch_action(ToggleRightPanel);
            }
            vcx.run_until_parked();
            vcx.update(|window, cx| {
                window.render_frame(cx);
                let pip = window.find(("pane-pip", key)).bounds();
                let content = window.find("pane-presentations").bounds();
                let margin = px((window.rem_size().as_f32() * 2.)
                    .min(content.size.width.as_f32() / 4.)
                    .min(content.size.height.as_f32() / 4.));
                assert!(pip.left() >= content.left() + margin);
                assert!(pip.right() <= content.right() - margin);
                assert!(pip.bottom() < window.find("workbench-status").bounds().top());
            });
        }
    }
    app.update_in(&mut vcx, |app, _, _| {
        assert_eq!(
            app.tabs[0].pane.first_leaf().unwrap().entity_id(),
            pane.entity_id()
        )
    });
}

#[gpui_kit::test]
fn pip_geometry_keeps_output_selection_scrolling_and_input_on_one_attachment(
    cx: &mut TestAppContext,
) {
    use crate::daemon::protocol::{ClientMsg, DaemonMsg};
    use std::time::{Duration, Instant};
    let (app, mut vcx, mut stream) = test_window::harness_with_pane(cx);
    let pane = app.update_in(&mut vcx, |app, window, cx| {
        let pane = app.tabs[0].pane.first_leaf().unwrap();
        window.focus(&pane.focus_handle(cx), cx);
        pane
    });
    DaemonMsg::Output(
        (0..100)
            .map(|n| format!("pip_output_{n:03} abcdefghijklmnopqrstuvwxyz\r\n"))
            .collect::<String>()
            .into_bytes(),
    )
    .encode(&mut stream)
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        vcx.run_until_parked();
        if pane.terminal().unwrap().read_with(&vcx, |view, _| {
            view.scrollback_text().contains("pip_output_099")
        }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "live output reached the original terminal"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    vcx.dispatch_action(TogglePanePip);
    vcx.run_until_parked();
    let key = pane.entity_id().as_u64() as usize;
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let start = window.find(("pip-resize", key)).bounds().center();
        window.pointer_down(start, MouseButton::Left, cx);
        window.pointer_move(start + point(px(40.), px(30.)), Some(MouseButton::Left), cx);
        window.pointer_up(start + point(px(40.), px(30.)), MouseButton::Left, cx);
    });
    vcx.run_until_parked();
    pane.terminal().unwrap().update_in(&mut vcx, |view, _, cx| {
        view.scroll(20, None, &gpui_kit::Modifiers::none(), cx)
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let bounds = window.find(("pane-pip", key)).bounds();
        let start = bounds.origin + point(px(12.), window.rem_size() * 4.);
        window.pointer_down(start, MouseButton::Left, cx);
        window.pointer_move(start + point(px(70.), px(0.)), Some(MouseButton::Left), cx);
        window.pointer_up(start + point(px(70.), px(0.)), MouseButton::Left, cx);
    });
    assert!(pane.terminal().unwrap().read_with(&vcx, |view, _| {
        view.selection_text().is_some_and(|text| !text.is_empty())
    }));
    vcx.simulate_keystrokes("x");
    stream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let mut sizes = Vec::new();
    let mut input = Vec::new();
    while let Ok(message) = ClientMsg::read(&mut stream) {
        match message {
            ClientMsg::Resize(size) => sizes.push((size.cols, size.rows)),
            ClientMsg::Input(bytes) => input.extend(bytes),
            _ => {}
        }
    }
    assert!(
        sizes.len() >= 2 && sizes.first() != sizes.last(),
        "the original attachment receives the changed terminal dimensions"
    );
    assert!(
        input.contains(&b'x'),
        "keyboard input still reaches the same daemon stream"
    );
    assert!(pane.terminal().unwrap().read_with(&vcx, |view, _| {
        view.scrollback_text().contains("pip_output_099")
    }));
}

#[gpui_kit::test]
fn an_incoming_geometry_update_does_not_take_over_a_local_gesture_or_focus(
    cx: &mut TestAppContext,
) {
    use lyrel_core::core::machine::{LayoutDelta, PipGeometry, Tab as TreeTab};
    let (app, mut vcx, _stream) = test_window::harness_with_pane(cx);
    let pane = app.update_in(&mut vcx, |app, window, cx| {
        let pane = app.tabs[0].pane.first_leaf().unwrap();
        window.focus(&pane.focus_handle(cx), cx);
        pane
    });
    vcx.dispatch_action(TogglePanePip);
    vcx.run_until_parked();
    let key = pane.entity_id().as_u64() as usize;
    let end = vcx.update(|window, cx| {
        window.render_frame(cx);
        let start = window.find(("pip-move", key)).bounds().center();
        let end = start - point(px(60.), px(40.));
        window.pointer_down(start, MouseButton::Left, cx);
        window.pointer_move(end, Some(MouseButton::Left), cx);
        end
    });
    vcx.run_until_parked();
    let local = vcx.update(|window, cx| {
        window.render_frame(cx);
        window.find(("pane-pip", key)).bounds()
    });
    app.update_in(&mut vcx, |app, window, cx| {
        let mut remote = TreeTab::leaf(1);
        remote.id = app.tabs[0].id();
        remote.set_pip(Some(1), None);
        remote
            .presentation
            .set_pip_geometry(PipGeometry::new(0., 0., 0.7, 0.7));
        assert!(app.apply_layout_delta(
            &LayoutDelta::TabRestructured {
                tab: remote,
                pane: None
            },
            window,
            cx
        ));
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find(("pane-pip", key)).bounds(), local);
        assert!(pane.contains_focused(window, cx));
        window.pointer_up(end, MouseButton::Left, cx);
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find(("pane-pip", key)).bounds(), local);
        assert!(pane.contains_focused(window, cx));
    });
}
