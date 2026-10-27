use super::*;
use gpui_kit::TestAppContext;
use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt as _};

#[gpui_kit::test]
fn pip_menu_acts_on_its_captured_terminal_not_the_previously_focused_one(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 2);
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
    let right_id = ("pane-pip", right.entity_id().as_u64() as usize);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let handle = ("pane-drag-handle", right.entity_id().as_u64() as usize);
        window.pointer_move(window.find(handle).bounds().center(), None, cx);
        window.click(handle, cx);
    });
    vcx.update(|window, cx| {
        let command = window
            .find_by_label(t(L10nKey::PaneEnterPip))
            .bounds()
            .center();
        window.pointer_down(command, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(command, gpui_kit::MouseButton::Left, cx);
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.find(right_id).visible());
        assert!(
            window
                .try_find(("pane-pip", left.entity_id().as_u64() as usize))
                .is_none()
        );
        assert!(right.contains_focused(window, cx));
    });
    app.update_in(&mut vcx, |_, window, cx| {
        window.focus(&left.focus_handle(cx), cx)
    });
    vcx.dispatch_action(RestorePanePip);
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(right_id).is_none());
        assert!(
            right.contains_focused(window, cx),
            "restore focuses the recovered terminal"
        );
    });
}
