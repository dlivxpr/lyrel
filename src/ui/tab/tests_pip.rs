use super::*;
use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt as _;

#[gpui_kit::test]
fn single_pane_pip_keeps_one_terminal_and_restores_keyboard_focus(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
    let pane = app.update_in(&mut vcx, |app, window, cx| {
        let pane = app.tabs[0].pane.first_leaf().unwrap();
        window.focus(&pane.focus_handle(cx), cx);
        pane
    });
    vcx.dispatch_action(TogglePanePip);
    vcx.run_until_parked();
    let id = ("pane-pip", pane.entity_id().as_u64() as usize);
    let handle = ("pane-drag-handle", pane.entity_id().as_u64() as usize);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let pip = window.find(id).bounds();
        let content = window.find("pane-presentations").bounds();
        assert!(pip.size.width > px(0.) && pip.size.height > px(0.));
        assert!(pip.left() >= content.left() && pip.right() <= content.right());
        assert!(pip.top() >= content.top() && pip.bottom() <= content.bottom());
        assert_eq!(
            window.find_all(handle).len(),
            1,
            "the live terminal is mounted only once"
        );
        assert!(pane.contains_focused(window, cx));
    });
    app.update_in(&mut vcx, |app, _, _| {
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(
            app.tabs[0].pane.leaves().len(),
            1,
            "empty tiled space creates no shell"
        );
    });
    vcx.dispatch_action(RestorePanePip);
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(id).is_none());
        assert_eq!(window.find_all(handle).len(), 1);
        assert!(pane.contains_focused(window, cx));
    });
}

#[gpui_kit::test]
fn replacing_pip_restores_the_previous_terminal_and_switches_with_its_tab(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 3);
    let (left, right) = app.update_in(&mut vcx, |app, window, cx| {
        let left = app.tabs[0].pane.first_leaf().unwrap();
        let right = app.tabs.remove(1).pane.first_leaf().unwrap();
        app.tabs[0]
            .pane
            .split_leaf(left.entity_id(), Axis::Horizontal, false, right.clone());
        window.focus(&left.focus_handle(cx), cx);
        cx.notify();
        (left, right)
    });
    vcx.dispatch_action(TogglePanePip);
    app.update_in(&mut vcx, |_, window, cx| {
        window.focus(&right.focus_handle(cx), cx)
    });
    vcx.dispatch_action(TogglePanePip);
    vcx.run_until_parked();
    let right_id = ("pane-pip", right.entity_id().as_u64() as usize);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .try_find(("pane-pip", left.entity_id().as_u64() as usize))
                .is_none()
        );
        assert!(window.find(right_id).visible());
        for pane in [&left, &right] {
            assert_eq!(
                window
                    .find_all(("pane-drag-handle", pane.entity_id().as_u64() as usize))
                    .len(),
                1
            );
        }
    });
    app.update_in(&mut vcx, |app, window, cx| app.activate(1, window, cx));
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find(right_id).is_none(),
            "another tab owns another presentation"
        );
    });
    app.update_in(&mut vcx, |app, window, cx| app.activate(0, window, cx));
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.find(right_id).visible());
    });
    vcx.dispatch_action(RestorePanePip);
    app.update_in(&mut vcx, |app, window, cx| {
        assert_eq!(app.tabs[0].pane.leaves().len(), 2);
        assert!(right.contains_focused(window, cx));
    });
}
