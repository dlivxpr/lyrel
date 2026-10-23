use super::*;
use gpui_kit::test::{TestWindowExt as _, TestQueryExt as _, TestEventExt as _};
use gpui_kit::TestAppContext;

#[gpui_kit::test]
fn park_last_pane_keeps_tab_session_and_keyboard_recovery(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
    vcx.update(|_, cx| cx.set_reduce_motion(true));
    let pane = app.update_in(&mut vcx, |app, window, cx| {
        let pane = app.tabs[0].pane.first_leaf().unwrap();
        window.focus(&pane.focus_handle(cx), cx);
        pane
    });
    vcx.dispatch_action(ParkPane);
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, window, cx| {
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs[0].pane.leaves().len(), 1, "parking cannot create or destroy a session");
        assert!(app.tabs[0].is_parked_slot(&pane, cx));
        assert!(!pane.contains_focused(window, cx), "cards never route input into their hidden terminal");
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(("pane-drag-handle", pane.entity_id().as_u64() as usize)).is_none());
        assert!(window.find("parked-empty").visible());
        let deck = window.find("parked-deck").bounds();
        let content = window.find("workbench-content").bounds();
        assert!(deck.bottom() <= content.bottom());
    });
    vcx.dispatch_action(RestoreParkedPane);
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let preview = window.find("parked-preview").bounds();
        let content = window.find("workbench-content").bounds();
        assert!(preview.left() >= content.left() && preview.right() <= content.right());
        assert!(preview.top() >= content.top() && preview.bottom() <= content.bottom());
    });
    vcx.update(|window, cx| window.press("enter", cx));
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(app.tabs[0].presentation.parked().is_empty());
        assert_eq!(app.tabs[0].pane.first_leaf().unwrap().entity_id(), pane.entity_id());
        assert!(pane.contains_focused(window, cx));
    });
}

#[gpui_kit::test]
fn parking_pip_restores_only_selected_pane_and_preserves_other_tabs(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 3);
    vcx.update(|_, cx| cx.set_reduce_motion(true));
    let (left, right) = app.update_in(&mut vcx, |app, window, cx| {
        let left = app.tabs[0].pane.first_leaf().unwrap();
        let right = app.tabs.remove(1).pane.first_leaf().unwrap();
        app.tabs[0].pane.split_leaf(left.entity_id(), Axis::Vertical, false, right.clone());
        window.focus(&left.focus_handle(cx), cx);
        cx.notify();
        (left, right)
    });
    vcx.dispatch_action(TogglePanePip);
    vcx.dispatch_action(ParkPane);
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(app.tabs[0].presentation.pip().is_none());
        window.focus(&right.focus_handle(cx), cx);
    });
    vcx.dispatch_action(ParkPane);
    app.update_in(&mut vcx, |app, window, cx| {
        assert_eq!(app.tabs[0].presentation.parked().len(), 2);
        app.activate(1, window, cx);
        assert!(app.tabs[1].presentation.parked().is_empty());
        app.activate(0, window, cx);
    });
    vcx.dispatch_action(RestoreParkedPane);
    vcx.run_until_parked();
    vcx.update(|window, cx| window.press("enter", cx));
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(!app.tabs[0].is_parked_slot(&left, cx));
        assert!(app.tabs[0].is_parked_slot(&right, cx));
        assert!(app.tabs[0].presentation.pip().is_none());
        assert!(left.contains_focused(window, cx));
        assert_eq!(app.tabs[0].pane.leaves().len(), 2);
    });
}

#[gpui_kit::test]
fn preview_escape_and_outside_dismissal_leave_parked_shell_untouched(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 2);
    vcx.update(|_, cx| cx.set_reduce_motion(true));
    let (parked, current) = app.update_in(&mut vcx, |app, window, cx| {
        let parked = app.tabs[0].pane.first_leaf().unwrap();
        let current = app.tabs.remove(1).pane.first_leaf().unwrap();
        app.tabs[0].pane.split_leaf(parked.entity_id(), Axis::Horizontal, false, current.clone());
        window.focus(&parked.focus_handle(cx), cx);
        (parked, current)
    });
    vcx.dispatch_action(ParkPane);
    vcx.dispatch_action(RestoreParkedPane);
    vcx.run_until_parked();
    vcx.update(|window, cx| window.press("escape", cx));
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(app.tabs[0].is_parked_slot(&parked, cx));
        assert!(current.contains_focused(window, cx));
    });
    vcx.dispatch_action(RestoreParkedPane);
    vcx.run_until_parked();
    vcx.update(|window, cx| window.click_at("pane-presentations", point(px(8.), px(8.)), cx));
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(app.tabs[0].is_parked_slot(&parked, cx));
        assert!(current.contains_focused(window, cx));
    });
}

#[gpui_kit::test]
fn another_clients_parking_delta_preserves_view_and_removes_hidden_input_focus(cx: &mut TestAppContext) {
    use lyrel_core::core::machine::{LayoutDelta, Tab as LogicalTab};
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
    vcx.update(|_, cx| cx.set_reduce_motion(true));
    let pane = app.update_in(&mut vcx, |app, window, cx| {
        let pane = app.tabs[0].pane.first_leaf().unwrap();
        window.focus(&pane.focus_handle(cx), cx);
        let mut shared = LogicalTab::leaf(pane.terminal().unwrap().read(cx).pane_id);
        shared.id = app.tabs[0].tree_id.get();
        shared.park(shared.root.pane_ids()[0]);
        assert!(app.apply_layout_delta(&LayoutDelta::TabRestructured { tab: shared, pane: None }, window, cx));
        assert_eq!(app.tabs[0].pane.first_leaf().unwrap().entity_id(), pane.entity_id());
        assert!(!pane.contains_focused(window, cx));
        pane
    });
    vcx.dispatch_action(RestoreParkedPane);
    vcx.run_until_parked();
    vcx.update(|window, cx| window.press("enter", cx));
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(app.tabs[0].presentation.parked().is_empty());
        assert_eq!(app.tabs[0].pane.first_leaf().unwrap().entity_id(), pane.entity_id());
        assert!(pane.contains_focused(window, cx));
    });
}

#[gpui_kit::test]
fn parked_output_does_not_cause_continuous_window_repaint(cx: &mut TestAppContext) {
    use crate::daemon::protocol::DaemonMsg;
    let (app, mut vcx, mut streams) = test_window::harness_with_tabs(cx, 1);
    vcx.update(|_, cx| cx.set_reduce_motion(true));
    let pane = app.update_in(&mut vcx, |app, window, cx| {
        let pane = app.tabs[0].pane.first_leaf().unwrap();
        window.focus(&pane.focus_handle(cx), cx);
        pane
    });
    vcx.dispatch_action(ParkPane);
    vcx.run_until_parked();
    test_window::quiesce(&mut vcx, None);
    let displayed = app.update_in(&mut vcx, |_, _, cx| crate::terminal::view::displayed_for_test(cx, pane.entity_id()));
    assert_eq!(displayed, Some(false));
    render_probe::arm(200);
    for _ in 0..16 {
        DaemonMsg::Output(b"parked-background-output\r\n".to_vec()).encode(&mut streams[0]).unwrap();
        vcx.background_executor.run_until_parked();
    }
    vcx.executor().advance_clock(std::time::Duration::from_secs(3));
    vcx.background_executor.run_until_parked();
    assert!(render_probe::draws() < 5, "hidden output must not schedule a frame per packet");
    vcx.dispatch_action(RestoreParkedPane);
    vcx.run_until_parked();
    vcx.update(|window, cx| window.press("enter", cx));
    vcx.run_until_parked();
    vcx.update(|window, cx| window.render_frame(cx));
    app.update_in(&mut vcx, |app, _, cx| {
        assert!(app.tabs[0].presentation.parked().is_empty());
        assert_eq!(crate::terminal::view::displayed_for_test(cx, pane.entity_id()), Some(true));
    });
}

#[gpui_kit::test]
fn park_menu_uses_its_captured_terminal_not_the_previously_focused_one(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 2);
    vcx.update(|_, cx| cx.set_reduce_motion(true));
    let (left, right) = app.update_in(&mut vcx, |app, window, cx| {
        let left = app.tabs[0].pane.first_leaf().unwrap();
        let right = app.tabs.remove(1).pane.first_leaf().unwrap();
        app.tabs[0].pane.split_leaf(left.entity_id(), Axis::Horizontal, false, right.clone());
        window.focus(&left.focus_handle(cx), cx);
        cx.notify();
        (left, right)
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let handle = ("pane-drag-handle", right.entity_id().as_u64() as usize);
        window.pointer_move(window.find(handle).bounds().center(), None, cx);
        window.click(handle, cx);
    });
    vcx.update(|window, cx| {
        let command = window.find_by_label(t(L10nKey::PanePark)).bounds().center();
        window.pointer_down(command, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(command, gpui_kit::MouseButton::Left, cx);
    });
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(app.tabs[0].is_parked_slot(&right, cx));
        assert!(!app.tabs[0].is_parked_slot(&left, cx));
        assert!(left.contains_focused(window, cx));
    });
}

#[gpui_kit::test]
fn restore_of_an_exited_card_keeps_the_same_exited_terminal(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
    vcx.update(|_, cx| cx.set_reduce_motion(true));
    let pane = app.update_in(&mut vcx, |app, window, cx| {
        let pane = app.tabs[0].pane.first_leaf().unwrap();
        pane.terminal().unwrap().update(cx, |view, cx| {
            view.title = "已退出 · 构建日志".into();
            view.terminal.exited = true;
            cx.notify();
        });
        window.focus(&pane.focus_handle(cx), cx);
        pane
    });
    vcx.dispatch_action(ParkPane);
    vcx.dispatch_action(RestoreParkedPane);
    vcx.run_until_parked();
    vcx.update(|window, cx| window.press("enter", cx));
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, _, cx| {
        assert!(app.tabs[0].presentation.parked().is_empty());
        assert_eq!(app.tabs[0].pane.first_leaf().unwrap().entity_id(), pane.entity_id());
        assert!(pane.terminal().unwrap().read(cx).terminal.exited, "Restore cannot start a shell");
    });
}

#[gpui_kit::test]
fn restore_after_an_intervening_split_and_sibling_close_keeps_the_new_terminal(cx: &mut TestAppContext) {
    use lyrel_core::core::machine::{Axis as LogicalAxis, LayoutDelta, Tab as LogicalTab};
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 3);
    vcx.update(|_, cx| cx.set_reduce_motion(true));
    let (original, parked) = app.update_in(&mut vcx, |app, window, cx| {
        let original = app.tabs[0].pane.first_leaf().unwrap();
        let parked = app.tabs.remove(1).pane.first_leaf().unwrap();
        app.tabs[0].pane.split_leaf(original.entity_id(), Axis::Horizontal, false, parked.clone());
        window.focus(&parked.focus_handle(cx), cx);
        (original, parked)
    });
    vcx.dispatch_action(ParkPane);
    vcx.run_until_parked();
    let current = app.update_in(&mut vcx, |app, window, cx| {
        let current = app.tabs.remove(1).pane.first_leaf().unwrap();
        app.tabs[0].pane.split_leaf(original.entity_id(), Axis::Vertical, false, current.clone());
        let current_id = current.terminal().unwrap().read(cx).pane_id;
        let parked_id = parked.terminal().unwrap().read(cx).pane_id;
        let mut shared = LogicalTab::leaf(current_id);
        shared.id = app.tabs[0].tree_id.get();
        shared.root.split_leaf(current_id, parked_id, LogicalAxis::Vertical, 0.7, false);
        shared.presentation = app.tabs[0].presentation.clone();
        // Another client closes the original sibling after the new split.
        assert!(app.apply_layout_delta(&LayoutDelta::TabRestructured { tab: shared, pane: None }, window, cx));
        window.focus(&current.focus_handle(cx), cx);
        current
    });
    vcx.dispatch_action(RestoreParkedPane);
    vcx.run_until_parked();
    vcx.update(|window, cx| window.press("enter", cx));
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, window, cx| {
        let leaves = app.tabs[0].pane.leaves();
        assert_eq!(leaves.len(), 2);
        assert!(leaves.iter().any(|slot| slot.entity_id() == current.entity_id()));
        assert!(leaves.iter().any(|slot| slot.entity_id() == parked.entity_id()));
        assert!(!leaves.iter().any(|slot| slot.entity_id() == original.entity_id()));
        assert!(matches!(&app.tabs[0].pane, Pane::Split { axis: Axis::Horizontal, .. }),
            "missing original relationship restores beside the current pane using its saved axis");
        assert!(parked.contains_focused(window, cx));
        assert!(app.tabs[0].presentation.parked().is_empty());
    });
}
