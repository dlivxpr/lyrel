use super::*;
use gpui_kit::TestAppContext;
use gpui_kit::test::{TestEventExt as _, TestQueryExt as _, TestWindowExt as _};

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
        assert_eq!(
            app.tabs[0].pane.leaves().len(),
            1,
            "parking cannot create or destroy a session"
        );
        assert!(app.tabs[0].is_parked_slot(&pane, cx));
        assert!(
            !pane.contains_focused(window, cx),
            "cards never route input into their hidden terminal"
        );
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .try_find(("pane-drag-handle", pane.entity_id().as_u64() as usize))
                .is_none()
        );
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
        assert_eq!(
            app.tabs[0].pane.first_leaf().unwrap().entity_id(),
            pane.entity_id()
        );
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
        app.tabs[0]
            .pane
            .split_leaf(left.entity_id(), Axis::Vertical, false, right.clone());
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
        app.tabs[0]
            .pane
            .split_leaf(parked.entity_id(), Axis::Horizontal, false, current.clone());
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
fn another_clients_parking_delta_preserves_view_and_removes_hidden_input_focus(
    cx: &mut TestAppContext,
) {
    use lyrel_core::core::machine::{LayoutDelta, Tab as LogicalTab};
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
    vcx.update(|_, cx| cx.set_reduce_motion(true));
    let pane = app.update_in(&mut vcx, |app, window, cx| {
        let pane = app.tabs[0].pane.first_leaf().unwrap();
        window.focus(&pane.focus_handle(cx), cx);
        let mut shared = LogicalTab::leaf(pane.terminal().unwrap().read(cx).pane_id);
        shared.id = app.tabs[0].tree_id.get();
        shared.park(shared.root.pane_ids()[0]);
        assert!(app.apply_layout_delta(
            &LayoutDelta::TabRestructured {
                tab: shared,
                pane: None
            },
            window,
            cx
        ));
        assert_eq!(
            app.tabs[0].pane.first_leaf().unwrap().entity_id(),
            pane.entity_id()
        );
        assert!(!pane.contains_focused(window, cx));
        pane
    });
    vcx.dispatch_action(RestoreParkedPane);
    vcx.run_until_parked();
    vcx.update(|window, cx| window.press("enter", cx));
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(app.tabs[0].presentation.parked().is_empty());
        assert_eq!(
            app.tabs[0].pane.first_leaf().unwrap().entity_id(),
            pane.entity_id()
        );
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
    let displayed = app.update_in(&mut vcx, |_, _, cx| {
        crate::terminal::view::displayed_for_test(cx, pane.entity_id())
    });
    assert_eq!(displayed, Some(false));
    render_probe::arm(200);
    for _ in 0..16 {
        DaemonMsg::Output(b"parked-background-output\r\n".to_vec())
            .encode(&mut streams[0])
            .unwrap();
        vcx.background_executor.run_until_parked();
    }
    vcx.executor()
        .advance_clock(std::time::Duration::from_secs(3));
    vcx.background_executor.run_until_parked();
    assert!(
        render_probe::draws() < 5,
        "hidden output must not schedule a frame per packet"
    );
    vcx.dispatch_action(RestoreParkedPane);
    vcx.run_until_parked();
    vcx.update(|window, cx| window.press("enter", cx));
    vcx.run_until_parked();
    vcx.update(|window, cx| window.render_frame(cx));
    app.update_in(&mut vcx, |app, _, cx| {
        assert!(app.tabs[0].presentation.parked().is_empty());
        assert_eq!(
            crate::terminal::view::displayed_for_test(cx, pane.entity_id()),
            Some(true)
        );
    });
}

#[gpui_kit::test]
fn park_menu_uses_its_captured_terminal_not_the_previously_focused_one(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 2);
    vcx.update(|_, cx| cx.set_reduce_motion(true));
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
        assert_eq!(
            app.tabs[0].pane.first_leaf().unwrap().entity_id(),
            pane.entity_id()
        );
        assert!(
            pane.terminal().unwrap().read(cx).terminal.exited,
            "Restore cannot start a shell"
        );
    });
}

#[gpui_kit::test]
fn restore_after_an_intervening_split_and_sibling_close_keeps_the_new_terminal(
    cx: &mut TestAppContext,
) {
    use lyrel_core::core::machine::{Axis as LogicalAxis, LayoutDelta, Tab as LogicalTab};
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 3);
    vcx.update(|_, cx| cx.set_reduce_motion(true));
    let (original, parked) = app.update_in(&mut vcx, |app, window, cx| {
        let original = app.tabs[0].pane.first_leaf().unwrap();
        let parked = app.tabs.remove(1).pane.first_leaf().unwrap();
        app.tabs[0].pane.split_leaf(
            original.entity_id(),
            Axis::Horizontal,
            false,
            parked.clone(),
        );
        window.focus(&parked.focus_handle(cx), cx);
        (original, parked)
    });
    vcx.dispatch_action(ParkPane);
    vcx.run_until_parked();
    let current = app.update_in(&mut vcx, |app, window, cx| {
        let current = app.tabs.remove(1).pane.first_leaf().unwrap();
        app.tabs[0]
            .pane
            .split_leaf(original.entity_id(), Axis::Vertical, false, current.clone());
        let current_id = current.terminal().unwrap().read(cx).pane_id;
        let parked_id = parked.terminal().unwrap().read(cx).pane_id;
        let mut shared = LogicalTab::leaf(current_id);
        shared.id = app.tabs[0].tree_id.get();
        shared
            .root
            .split_leaf(current_id, parked_id, LogicalAxis::Vertical, 0.7, false);
        shared.presentation = app.tabs[0].presentation.clone();
        // Another client closes the original sibling after the new split.
        assert!(app.apply_layout_delta(
            &LayoutDelta::TabRestructured {
                tab: shared,
                pane: None
            },
            window,
            cx
        ));
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
        assert!(
            leaves
                .iter()
                .any(|slot| slot.entity_id() == current.entity_id())
        );
        assert!(
            leaves
                .iter()
                .any(|slot| slot.entity_id() == parked.entity_id())
        );
        assert!(
            !leaves
                .iter()
                .any(|slot| slot.entity_id() == original.entity_id())
        );
        assert!(
            matches!(
                &app.tabs[0].pane,
                Pane::Split {
                    axis: Axis::Horizontal,
                    ..
                }
            ),
            "missing original relationship restores beside the current pane using its saved axis"
        );
        assert!(parked.contains_focused(window, cx));
        assert!(app.tabs[0].presentation.parked().is_empty());
    });
}

#[gpui_kit::test]
fn handle_drop_parks_the_last_live_pane_and_keyboard_restores_its_identity(
    cx: &mut TestAppContext,
) {
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
    vcx.update(|_, cx| cx.set_reduce_motion(true));
    let pane = app.update_in(&mut vcx, |app, window, cx| {
        let pane = app.tabs[0].pane.first_leaf().unwrap();
        window.focus(&pane.focus_handle(cx), cx);
        pane
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let start = window
            .find(("pane-drag-handle", pane.entity_id().as_u64() as usize))
            .bounds()
            .center();
        let content = window.find("pane-presentations").bounds();
        let aim = point(content.center().x, content.bottom() - window.rem_size());
        window.pointer_down(start, gpui_kit::MouseButton::Left, cx);
        window.pointer_move(aim, Some(gpui_kit::MouseButton::Left), cx);
        assert!(window.find("park-drop-target").visible());
        let pill = window.find("park-drop-pill").bounds();
        window.pointer_move(pill.center(), Some(gpui_kit::MouseButton::Left), cx);
        window.pointer_up(pill.center(), gpui_kit::MouseButton::Left, cx);
    });
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, window, cx| {
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs[0].pane.leaves().len(), 1);
        assert!(app.tabs[0].is_parked_slot(&pane, cx));
        assert!(!pane.contains_focused(window, cx));
        assert!(app.pane_drag.borrow().is_none());
    });
    vcx.update(|window, cx| {
        assert!(window.find("parked-empty").visible());
        assert!(window.try_find("park-drop-target").is_none());
        assert!(!cx.has_active_drag());
    });
    vcx.dispatch_action(RestoreParkedPane);
    vcx.update(|window, cx| window.press("enter", cx));
    app.update_in(&mut vcx, |app, window, cx| {
        assert_eq!(
            app.tabs[0].pane.first_leaf().unwrap().entity_id(),
            pane.entity_id()
        );
        assert!(app.tabs[0].presentation.parked().is_empty());
        assert!(pane.contains_focused(window, cx));
    });
}

#[gpui_kit::test]
fn escape_cancels_a_valid_parking_drag_without_a_late_drop_or_menu(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
    let pane = app.update_in(&mut vcx, |app, window, cx| {
        let pane = app.tabs[0].pane.first_leaf().unwrap();
        window.focus(&pane.focus_handle(cx), cx);
        pane
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let start = window
            .find(("pane-drag-handle", pane.entity_id().as_u64() as usize))
            .bounds()
            .center();
        let content = window.find("pane-presentations").bounds();
        let aim = point(content.center().x, content.bottom() - window.rem_size());
        window.pointer_down(start, gpui_kit::MouseButton::Left, cx);
        window.pointer_move(aim, Some(gpui_kit::MouseButton::Left), cx);
        assert!(window.find("park-drop-target").visible());
        assert!(cx.has_active_drag());
        window.press("escape", cx);
        assert!(!cx.has_active_drag());
        assert!(window.captured_hitbox().is_none());
        assert!(window.try_find("park-drop-target").is_none());
        window.pointer_up(aim, gpui_kit::MouseButton::Left, cx);
        assert!(window.try_find_by_label(t(L10nKey::PanePark)).is_none());
    });
    app.update_in(&mut vcx, |app, window, cx| {
        assert_eq!(
            app.tabs[0].pane.first_leaf().unwrap().entity_id(),
            pane.entity_id()
        );
        assert!(app.tabs[0].presentation.parked().is_empty());
        assert!(pane.contains_focused(window, cx));
        assert!(app.pane_drag.borrow().is_none());
    });
}

#[gpui_kit::test]
fn cancelled_parking_drag_cannot_rearm_a_split_or_menu_on_late_pointer_events(
    cx: &mut TestAppContext,
) {
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
    for cancellation in ["escape", "mouse-exit", "deactivate"] {
        for release_at_handle in [false, true] {
            vcx.update(|window, _| window.activate_window());
            vcx.run_until_parked();
            let (start, release) = vcx.update(|window, cx| {
                window.render_frame(cx);
                let start = window
                    .find(("pane-drag-handle", left.entity_id().as_u64() as usize))
                    .bounds()
                    .center();
                let content = window.find("pane-presentations").bounds();
                let aim = point(content.center().x, content.bottom() - window.rem_size());
                window.pointer_down(start, gpui_kit::MouseButton::Left, cx);
                window.pointer_move(aim, Some(gpui_kit::MouseButton::Left), cx);
                assert!(cx.has_active_drag());
                assert!(window.find("park-drop-target").visible());
                let release = if release_at_handle {
                    start
                } else {
                    // This would move the left pane to the right of its sibling
                    // if a stale drag were allowed to acquire another landing.
                    point(content.right() - window.rem_size(), content.center().y)
                };
                (start, release)
            });
            match cancellation {
                "escape" => vcx.update(|window, cx| window.press("escape", cx)),
                "mouse-exit" => vcx.simulate_event(gpui_kit::MouseExitEvent {
                    position: start,
                    pressed_button: Some(gpui_kit::MouseButton::Left),
                    ..Default::default()
                }),
                "deactivate" => vcx.deactivate_window(),
                _ => unreachable!(),
            }
            vcx.update(|window, cx| {
                window.render_frame(cx);
                assert!(!cx.has_active_drag(), "{cancellation}");
                assert!(window.captured_hitbox().is_none(), "{cancellation}");
                assert!(
                    window.try_find("park-drop-target").is_none(),
                    "{cancellation}"
                );
                window.pointer_move(release, Some(gpui_kit::MouseButton::Left), cx);
                window.pointer_up(release, gpui_kit::MouseButton::Left, cx);
                assert!(!cx.has_active_drag(), "late move after {cancellation}");
                assert!(
                    window.try_find_by_label(t(L10nKey::PanePark)).is_none(),
                    "late menu after {cancellation}"
                );
            });
            app.update_in(&mut vcx, |app, _, cx| {
                assert_eq!(app.tabs.len(), 1);
                let leaves = app.tabs[0].pane.leaves();
                assert_eq!(leaves.len(), 2);
                assert_eq!(
                    leaves[0].entity_id(),
                    left.entity_id(),
                    "late split after {cancellation}"
                );
                assert_eq!(
                    leaves[1].entity_id(),
                    right.entity_id(),
                    "late split after {cancellation}"
                );
                assert!(app.tabs[0].presentation.parked().is_empty());
                assert!(app.pane_drag.borrow().is_none());
                assert!(app.parked_deck.read(cx).cards().next().is_none());
            });
        }
    }
}

#[gpui_kit::test]
fn leaving_the_target_and_releasing_over_status_does_not_park_or_detach(cx: &mut TestAppContext) {
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
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let start = window
            .find(("pane-drag-handle", right.entity_id().as_u64() as usize))
            .bounds()
            .center();
        let content = window.find("pane-presentations").bounds();
        let aim = point(content.center().x, content.bottom() - window.rem_size());
        window.pointer_down(start, gpui_kit::MouseButton::Left, cx);
        window.pointer_move(aim, Some(gpui_kit::MouseButton::Left), cx);
        assert!(window.find("park-drop-target").visible());
        let status = window.find("workbench-status").bounds().center();
        window.pointer_move(status, Some(gpui_kit::MouseButton::Left), cx);
        assert!(window.try_find("park-drop-target").is_none());
        window.pointer_up(status, gpui_kit::MouseButton::Left, cx);
        assert!(!cx.has_active_drag());
    });
    app.update_in(&mut vcx, |app, _, cx| {
        assert_eq!(app.tabs.len(), 1);
        assert!(app.tabs[0].presentation.parked().is_empty());
        let leaves = app.tabs[0].pane.leaves();
        assert_eq!(leaves[0].entity_id(), left.entity_id());
        assert_eq!(leaves[1].entity_id(), right.entity_id());
        assert!(app.pane_drag.borrow().is_none());
        assert_eq!(app.parked_deck.read(cx).cards().len(), 0);
    });
}

#[gpui_kit::test]
fn dropping_a_nonfocused_handle_parks_only_that_pane_in_its_tab(cx: &mut TestAppContext) {
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
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let start = window
            .find(("pane-drag-handle", right.entity_id().as_u64() as usize))
            .bounds()
            .center();
        let content = window.find("pane-presentations").bounds();
        let aim = point(content.center().x, content.bottom() - window.rem_size());
        window.drag(start, aim, cx);
    });
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(app.tabs[0].is_parked_slot(&right, cx));
        assert!(!app.tabs[0].is_parked_slot(&left, cx));
        assert_eq!(app.tabs[0].pane.leaves().len(), 2);
        assert!(left.contains_focused(window, cx));
        app.activate(1, window, cx);
        assert!(app.tabs[1].presentation.parked().is_empty());
        app.activate(0, window, cx);
        assert!(app.tabs[0].is_parked_slot(&right, cx));
    });
}

#[gpui_kit::test]
fn pip_move_drag_cancels_then_parks_without_committing_geometry_or_replacing_the_session(
    cx: &mut TestAppContext,
) {
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
    vcx.update(|_, cx| cx.set_reduce_motion(true));
    let pane = app.update_in(&mut vcx, |app, window, cx| {
        let pane = app.tabs[0].pane.first_leaf().unwrap();
        window.focus(&pane.focus_handle(cx), cx);
        pane
    });
    vcx.dispatch_action(TogglePanePip);
    let original = app.update_in(&mut vcx, |app, _, _| {
        app.tabs[0].presentation.pip_geometry()
    });
    let key = pane.entity_id().as_u64() as usize;
    for cancel in [true, false] {
        vcx.update(|window, cx| {
            window.render_frame(cx);
            let start = window.find(("pip-move", key)).bounds().center();
            let content = window.find("pane-presentations").bounds();
            let aim = point(content.center().x, content.bottom() - window.rem_size());
            window.pointer_down(start, gpui_kit::MouseButton::Left, cx);
            window.pointer_move(aim, Some(gpui_kit::MouseButton::Left), cx);
            assert!(window.find("park-drop-target").visible());
            if cancel {
                window.press("escape", cx);
            }
            window.pointer_up(aim, gpui_kit::MouseButton::Left, cx);
        });
        vcx.run_until_parked();
        app.update_in(&mut vcx, |app, window, cx| {
            assert_eq!(
                app.tabs[0].pane.first_leaf().unwrap().entity_id(),
                pane.entity_id()
            );
            assert_eq!(app.tabs[0].is_parked_slot(&pane, cx), !cancel);
            assert_eq!(app.tabs[0].presentation.pip().is_some(), cancel);
            assert!(!app.tabs[0].pip_interaction.has_gesture());
            if cancel {
                assert_eq!(app.tabs[0].presentation.pip_geometry(), original);
                assert!(pane.contains_focused(window, cx));
            }
        });
    }
    vcx.dispatch_action(RestoreParkedPane);
    vcx.update(|window, cx| window.press("enter", cx));
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(app.tabs[0].presentation.pip().is_none());
        assert!(app.tabs[0].presentation.parked().is_empty());
        assert_eq!(
            app.tabs[0].pane.first_leaf().unwrap().entity_id(),
            pane.entity_id()
        );
        assert!(pane.contains_focused(window, cx));
    });
}

#[gpui_kit::test]
fn pip_pane_menu_handle_also_drags_to_the_shared_parking_target(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
    let pane = app.update_in(&mut vcx, |app, window, cx| {
        let pane = app.tabs[0].pane.first_leaf().unwrap();
        window.focus(&pane.focus_handle(cx), cx);
        pane
    });
    vcx.dispatch_action(TogglePanePip);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let start = window
            .find(("pane-drag-handle", pane.entity_id().as_u64() as usize))
            .bounds()
            .center();
        let content = window.find("pane-presentations").bounds();
        window.drag(
            start,
            point(content.center().x, content.bottom() - window.rem_size()),
            cx,
        );
    });
    app.update_in(&mut vcx, |app, _, cx| {
        assert!(app.tabs[0].is_parked_slot(&pane, cx));
        assert!(app.tabs[0].presentation.pip().is_none());
        assert_eq!(app.tabs[0].pane.leaves().len(), 1);
    });
}

#[gpui_kit::test]
fn parking_target_matches_painted_ellipse_and_stays_above_status_after_zoom_resize_and_sidebar_changes(
    cx: &mut TestAppContext,
) {
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
    vcx.update(|_, cx| cx.set_reduce_motion(true));
    let pane = app.update_in(&mut vcx, |app, window, cx| {
        let pane = app.tabs[0].pane.first_leaf().unwrap();
        window.focus(&pane.focus_handle(cx), cx);
        pane
    });
    for (width, font, sidebar) in [(1280., 14., false), (620., 20., true), (980., 18., false)] {
        vcx.simulate_resize(size(px(width), px(720.)));
        app.update_in(&mut vcx, |app, _, cx| {
            app.set_ui_font_size(font, cx);
            if app.sidebar_open(cx) != sidebar {
                app.toggle_left_panel(cx);
            }
        });
        vcx.run_until_parked();
        if sidebar {
            vcx.update(|window, cx| {
                window.render_frame(cx);
                let start = window.find("sidebar-resize").bounds().center();
                window.drag(start, start + point(window.rem_size() * 2., px(0.)), cx);
            });
        }
        vcx.update(|window, cx| {
            window.render_frame(cx);
            let start = window
                .find(("pane-drag-handle", pane.entity_id().as_u64() as usize))
                .bounds()
                .center();
            let content = window.find("pane-presentations").bounds();
            let aim = point(content.center().x, content.bottom() - window.rem_size());
            window.pointer_down(start, gpui_kit::MouseButton::Left, cx);
            window.pointer_move(aim, Some(gpui_kit::MouseButton::Left), cx);
            let target = window.find("park-drop-target").bounds();
            let pill = window.find("park-drop-pill").bounds();
            let status = window.find("workbench-status").bounds();
            for bounds in [target, pill] {
                assert!(bounds.left() >= content.left() && bounds.right() <= content.right());
                assert!(bounds.top() >= content.top() && bounds.bottom() <= content.bottom());
                assert!(bounds.bottom() <= status.top());
            }
            // The rectangular semantic envelope's empty top corner is not a drop hit.
            let corner = target.origin + point(px(1.), px(1.));
            window.pointer_move(corner, Some(gpui_kit::MouseButton::Left), cx);
            window.pointer_up(corner, gpui_kit::MouseButton::Left, cx);
        });
        app.update_in(&mut vcx, |app, _, _| {
            assert!(app.tabs[0].presentation.parked().is_empty())
        });
        vcx.update(|window, cx| {
            let start = window
                .find(("pane-drag-handle", pane.entity_id().as_u64() as usize))
                .bounds()
                .center();
            let content = window.find("pane-presentations").bounds();
            window.pointer_down(start, gpui_kit::MouseButton::Left, cx);
            window.pointer_move(
                point(content.center().x, content.bottom() - window.rem_size()),
                Some(gpui_kit::MouseButton::Left),
                cx,
            );
            let pill = window.find("park-drop-pill").bounds();
            window.pointer_move(pill.center(), Some(gpui_kit::MouseButton::Left), cx);
            window.pointer_up(pill.center(), gpui_kit::MouseButton::Left, cx);
        });
        app.update_in(&mut vcx, |app, _, cx| {
            assert!(app.tabs[0].is_parked_slot(&pane, cx))
        });
        vcx.dispatch_action(RestoreParkedPane);
        vcx.update(|window, cx| window.press("enter", cx));
        app.update_in(&mut vcx, |app, window, cx| {
            assert!(app.tabs[0].presentation.parked().is_empty());
            assert!(pane.contains_focused(window, cx));
        });
    }
}

#[gpui_kit::test]
fn drag_park_restore_keeps_output_and_input_on_the_original_daemon_attachment(
    cx: &mut TestAppContext,
) {
    use crate::daemon::protocol::{ClientMsg, DaemonMsg};
    use std::time::{Duration, Instant};
    let (app, mut vcx, mut stream) = test_window::harness_with_pane(cx);
    vcx.update(|_, cx| cx.set_reduce_motion(true));
    let pane = app.update_in(&mut vcx, |app, window, cx| {
        let pane = app.tabs[0].pane.first_leaf().unwrap();
        window.focus(&pane.focus_handle(cx), cx);
        pane
    });
    DaemonMsg::Output(b"drag_park_original_attachment\r\n".to_vec())
        .encode(&mut stream)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        vcx.run_until_parked();
        if pane.terminal().unwrap().read_with(&vcx, |view, _| {
            view.scrollback_text()
                .contains("drag_park_original_attachment")
        }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "output reached the original terminal"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let start = window
            .find(("pane-drag-handle", pane.entity_id().as_u64() as usize))
            .bounds()
            .center();
        let content = window.find("pane-presentations").bounds();
        window.drag(
            start,
            point(content.center().x, content.bottom() - window.rem_size()),
            cx,
        );
    });
    app.update_in(&mut vcx, |app, _, cx| {
        assert!(app.tabs[0].is_parked_slot(&pane, cx))
    });
    DaemonMsg::Output(b"drag_park_background_output\r\n".to_vec())
        .encode(&mut stream)
        .unwrap();
    vcx.dispatch_action(RestoreParkedPane);
    vcx.update(|window, cx| window.press("enter", cx));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        vcx.run_until_parked();
        if pane.terminal().unwrap().read_with(&vcx, |view, _| {
            view.scrollback_text()
                .contains("drag_park_background_output")
        }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "parked output remains attached after restoration"
        );
        std::thread::sleep(Duration::from_millis(2));
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
    assert!(
        input.contains(&b'x'),
        "restored input uses the same daemon attachment"
    );
    app.update_in(&mut vcx, |app, window, cx| {
        assert_eq!(
            app.tabs[0].pane.first_leaf().unwrap().entity_id(),
            pane.entity_id()
        );
        assert!(pane.contains_focused(window, cx));
    });
}

#[gpui_kit::test]
fn twenty_parked_panes_remain_reachable_by_keyboard_beyond_the_visible_deck(
    cx: &mut TestAppContext,
) {
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 22);
    vcx.update(|_, cx| cx.set_reduce_motion(true));
    let (parked, current) = app.update_in(&mut vcx, |app, window, cx| {
        let current = app.tabs[0].pane.first_leaf().unwrap();
        let mut parked = Vec::new();
        while app.tabs.len() > 2 {
            let pane = app.tabs.remove(1).pane.first_leaf().unwrap();
            pane.terminal().unwrap().update(cx, |view, cx| {
                view.title = format!("构建终端与长标题 · shell {}", parked.len() + 1).into();
                cx.notify();
            });
            app.tabs[0]
                .pane
                .split_leaf(current.entity_id(), Axis::Horizontal, false, pane.clone());
            app.park_pane(pane.entity_id(), window, cx);
            parked.push(pane);
        }
        window.focus(&current.focus_handle(cx), cx);
        (parked, current)
    });
    // The harness starts maximized on the test display, where all twenty
    // cards may fit. Pin the consumer viewport that must need overflow.
    vcx.simulate_resize(size(px(900.), px(600.)));
    vcx.run_until_parked();
    let last = parked.last().unwrap();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .try_find(("parked-card", last.entity_id().as_u64() as usize))
                .is_none_or(|card| !card.visible()),
            "the last pane needs the overflow path at this viewport"
        );
        assert!(window.find("parked-more").visible());
        window.click("parked-more", cx);
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.press("home", cx);
        for (ix, pane) in parked.iter().enumerate() {
            if ix > 0 {
                window.press("down", cx);
            }
            window.render_frame(cx);
            assert!(
                window
                    .find(("parked-overflow-row", pane.entity_id().as_u64() as usize))
                    .visible(),
                "every original pane remains reachable, including virtualized rows"
            );
        }
    });
    vcx.update(|window, cx| {
        window.press("end", cx);
        window.render_frame(cx);
        assert!(
            window
                .find(("parked-overflow-row", last.entity_id().as_u64() as usize))
                .visible()
        );
        window.press("enter", cx);
    });
    vcx.run_until_parked();
    let replacement = app.update_in(&mut vcx, |app, window, cx| {
        use lyrel_core::core::machine::{Axis as LogicalAxis, LayoutDelta, Tab as LogicalTab};
        let replacement = app.tabs.remove(1).pane.first_leaf().unwrap();
        let replacement_id = replacement.terminal().unwrap().read(cx).pane_id;
        // Another client replaces the original sibling and changes the split
        // while this client's overflow preview remains selected.
        app.tabs[0].pane.split_leaf(
            current.entity_id(),
            Axis::Vertical,
            false,
            replacement.clone(),
        );
        let mut shared = LogicalTab::leaf(replacement_id);
        shared.id = app.tabs[0].tree_id.get();
        for pane in &parked {
            let id = pane.terminal().unwrap().read(cx).pane_id;
            shared
                .root
                .split_leaf(replacement_id, id, LogicalAxis::Vertical, 0.7, false);
        }
        shared.presentation = app.tabs[0].presentation.clone();
        assert!(app.apply_layout_delta(
            &LayoutDelta::TabRestructured {
                tab: shared,
                pane: None
            },
            window,
            cx
        ));
        replacement
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.find("parked-preview").visible());
        window.press("enter", cx);
    });
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, window, cx| {
        assert_eq!(app.tabs[0].pane.leaves().len(), 21);
        assert_eq!(app.tabs[0].presentation.parked().len(), 19);
        assert!(!app.tabs[0].is_parked_slot(last, cx));
        assert!(
            parked[..19]
                .iter()
                .all(|pane| app.tabs[0].is_parked_slot(pane, cx))
        );
        assert!(last.contains_focused(window, cx));
        assert!(!current.contains_focused(window, cx));
        assert!(
            app.tabs[0]
                .pane
                .leaves()
                .iter()
                .any(|pane| pane.entity_id() == replacement.entity_id())
        );
        assert!(
            !app.tabs[0]
                .pane
                .leaves()
                .iter()
                .any(|pane| pane.entity_id() == current.entity_id())
        );
    });
}

#[gpui_kit::test]
fn tab_switch_and_lost_pressed_button_cancel_pending_parking(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 2);
    let pane = app.update_in(&mut vcx, |app, window, cx| {
        let pane = app.tabs[0].pane.first_leaf().unwrap();
        window.focus(&pane.focus_handle(cx), cx);
        pane
    });
    for switch in [false, true] {
        let aim = vcx.update(|window, cx| {
            window.render_frame(cx);
            let start = window
                .find(("pane-drag-handle", pane.entity_id().as_u64() as usize))
                .bounds()
                .center();
            let content = window.find("pane-presentations").bounds();
            let aim = point(content.center().x, content.bottom() - window.rem_size());
            window.pointer_down(start, gpui_kit::MouseButton::Left, cx);
            window.pointer_move(aim, Some(gpui_kit::MouseButton::Left), cx);
            assert!(window.find("park-drop-target").visible());
            aim
        });
        if switch {
            app.update_in(&mut vcx, |app, window, cx| app.activate(1, window, cx));
        } else {
            vcx.update(|window, cx| window.pointer_move(aim, None, cx));
        }
        vcx.update(|window, cx| {
            window.render_frame(cx);
            assert!(!cx.has_active_drag());
            assert!(window.captured_hitbox().is_none());
            assert!(window.try_find("park-drop-target").is_none());
            window.pointer_up(aim, gpui_kit::MouseButton::Left, cx);
        });
        app.update_in(&mut vcx, |app, _, _| {
            assert!(
                app.tabs
                    .iter()
                    .all(|tab| tab.presentation.parked().is_empty())
            );
            assert_eq!(
                app.tabs[0].pane.first_leaf().unwrap().entity_id(),
                pane.entity_id()
            );
            assert!(app.pane_drag.borrow().is_none());
        });
    }
}

#[gpui_kit::test]
fn a_parking_target_empty_corner_cannot_become_an_unintended_split_drop(cx: &mut TestAppContext) {
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
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let start = window
            .find(("pane-drag-handle", right.entity_id().as_u64() as usize))
            .bounds()
            .center();
        let content = window.find("pane-presentations").bounds();
        window.pointer_down(start, gpui_kit::MouseButton::Left, cx);
        window.pointer_move(
            point(content.center().x, content.bottom() - window.rem_size()),
            Some(gpui_kit::MouseButton::Left),
            cx,
        );
        let corner = window.find("park-drop-target").bounds().origin + point(px(1.), px(1.));
        window.pointer_move(corner, Some(gpui_kit::MouseButton::Left), cx);
        window.pointer_up(corner, gpui_kit::MouseButton::Left, cx);
    });
    app.update_in(&mut vcx, |app, _, _| {
        assert_eq!(app.tabs.len(), 1);
        assert!(app.tabs[0].presentation.parked().is_empty());
        let leaves = app.tabs[0].pane.leaves();
        assert_eq!(leaves[0].entity_id(), left.entity_id());
        assert_eq!(leaves[1].entity_id(), right.entity_id());
        assert!(matches!(
            &app.tabs[0].pane,
            Pane::Split {
                axis: Axis::Horizontal,
                ..
            }
        ));
    });
}

#[gpui_kit::test]
fn overflow_uses_content_width_and_zoom_without_shrinking_targets_or_losing_entries(
    cx: &mut TestAppContext,
) {
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 4);
    vcx.update(|_, cx| cx.set_reduce_motion(true));
    let parked = app.update_in(&mut vcx, |app, window, cx| {
        let current = app.tabs[0].pane.first_leaf().unwrap();
        let mut panes = Vec::new();
        while app.tabs.len() > 1 {
            let pane = app.tabs.remove(1).pane.first_leaf().unwrap();
            app.tabs[0]
                .pane
                .split_leaf(current.entity_id(), Axis::Horizontal, false, pane.clone());
            app.park_pane(pane.entity_id(), window, cx);
            panes.push(pane);
        }
        panes
    });
    vcx.simulate_resize(size(px(1440.), px(720.)));
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("parked-more").is_none(),
            "a comfortable small fan has no overflow"
        );
    });
    vcx.simulate_resize(size(px(620.), px(540.)));
    app.update_in(&mut vcx, |app, _, cx| app.set_ui_font_size(24., cx));
    vcx.run_until_parked();
    vcx.update(|window, cx| window.click("parked-more", cx));
    vcx.run_until_parked();
    for step in 0..3 {
        match step {
            1 => vcx.dispatch_action(ToggleLeftPanel),
            2 => vcx.dispatch_action(ToggleRightPanel),
            _ => {}
        }
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            window.render_frame(cx);
            let list = window.find("parked-overflow").bounds();
            let content = window.find("workbench-content").bounds();
            let status = window.find("workbench-status").bounds();
            assert!(list.left() >= content.left() && list.right() <= content.right());
            assert!(list.top() >= content.top() && list.bottom() <= content.bottom());
            assert!(list.bottom() <= status.top());
            window.press("end", cx);
            window.render_frame(cx);
            let row = window
                .find((
                    "parked-overflow-row",
                    parked[2].entity_id().as_u64() as usize,
                ))
                .bounds();
            assert!(row.size.height >= window.rem_size() * 3.);
        });
    }
    vcx.update(|window, cx| window.press("enter", cx));
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(
            window.find("parked-overflow").visible(),
            "Escape first returns from preview to list"
        );
    });
    vcx.simulate_resize(size(px(1440.), px(720.)));
    app.update_in(&mut vcx, |app, _, cx| app.set_ui_font_size(16., cx));
    vcx.run_until_parked();
    vcx.update(|window, cx| window.press("escape", cx));
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("parked-overflow").is_none());
        assert!(window.try_find("parked-more").is_none());
    });
    app.update_in(&mut vcx, |app, window, cx| {
        assert_eq!(app.tabs[0].presentation.parked().len(), 3);
        assert!(
            parked
                .iter()
                .all(|pane| app.tabs[0].is_parked_slot(pane, cx))
        );
        assert!(
            app.parked_deck
                .read(cx)
                .focus_handle(cx)
                .contains_focused(window, cx),
            "a vanished More returns to the still-valid deck"
        );
    });
}

#[gpui_kit::test]
fn overflow_tabbed_rows_activate_their_own_pane_before_restore(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 4);
    vcx.update(|_, cx| cx.set_reduce_motion(true));
    let parked = app.update_in(&mut vcx, |app, window, cx| {
        let current = app.tabs[0].pane.first_leaf().unwrap();
        let mut parked = Vec::new();
        while app.tabs.len() > 1 {
            let pane = app.tabs.remove(1).pane.first_leaf().unwrap();
            app.tabs[0]
                .pane
                .split_leaf(current.entity_id(), Axis::Horizontal, false, pane.clone());
            app.park_pane(pane.entity_id(), window, cx);
            parked.push(pane);
        }
        parked
    });
    // Leave room for all three Tab stops; virtualization is covered separately.
    vcx.simulate_resize(size(px(620.), px(900.)));
    app.update_in(&mut vcx, |app, _, cx| app.set_ui_font_size(24., cx));
    vcx.run_until_parked();
    vcx.update(|window, cx| window.click("parked-more", cx));
    vcx.run_until_parked();
    for (pane, activation) in [(&parked[1], "enter"), (&parked[2], "space")] {
        vcx.update(|window, cx| {
            // Home deliberately leaves the arrow-key target on the first pane.
            // Tab moves native focus, not that retained active selection.
            window.press("home", cx);
            let row = ("parked-overflow-row", pane.entity_id().as_u64() as usize);
            for _ in 0..8 {
                if window.find(row).focused() == Some(true) {
                    break;
                }
                window.press("tab", cx);
            }
            assert_eq!(window.find(row).focused(), Some(true));
            window.press(activation, cx);
        });
        vcx.run_until_parked();
        app.update_in(&mut vcx, |app, _, cx| {
            assert_eq!(
                app.parked_deck.read(cx).selected_pane(),
                Some(pane.entity_id().as_u64()),
                "Enter and Space must preview the focused row, not the old active pane"
            );
            assert_eq!(app.tabs[0].presentation.parked().len(), 3);
        });
        vcx.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.find("parked-preview").visible());
            if activation == "enter" {
                window.press("escape", cx);
            } else {
                window.press("enter", cx);
            }
        });
        vcx.run_until_parked();
    }
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(parked[2].contains_focused(window, cx));
        assert!(!app.tabs[0].is_parked_slot(&parked[2], cx));
        assert!(app.tabs[0].is_parked_slot(&parked[0], cx));
        assert!(app.tabs[0].is_parked_slot(&parked[1], cx));
        assert!(
            app.tabs[0]
                .pane
                .leaves()
                .iter()
                .any(|pane| { pane.entity_id() == parked[2].entity_id() })
        );
    });
}

#[gpui_kit::test]
fn overflow_reconciles_remote_parking_and_deleted_preview_without_retargeting_restore(
    cx: &mut TestAppContext,
) {
    use lyrel_core::core::machine::{Axis as LogicalAxis, LayoutDelta, Tab as LogicalTab};
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 5);
    vcx.update(|_, cx| cx.set_reduce_motion(true));
    let (current, parked) = app.update_in(&mut vcx, |app, window, cx| {
        let current = app.tabs[0].pane.first_leaf().unwrap();
        let mut parked = Vec::new();
        while app.tabs.len() > 1 {
            let pane = app.tabs.remove(1).pane.first_leaf().unwrap();
            app.tabs[0]
                .pane
                .split_leaf(current.entity_id(), Axis::Horizontal, false, pane.clone());
            app.park_pane(pane.entity_id(), window, cx);
            parked.push(pane);
        }
        (current, parked)
    });
    vcx.simulate_resize(size(px(620.), px(540.)));
    app.update_in(&mut vcx, |app, _, cx| app.set_ui_font_size(20., cx));
    vcx.run_until_parked();
    vcx.update(|window, cx| window.click("parked-more", cx));
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.press("end", cx);
        window.press("enter", cx);
    });
    vcx.run_until_parked();
    let mut shared = app.update_in(&mut vcx, |app, window, cx| {
        let current_id = current.terminal().unwrap().read(cx).pane_id;
        let mut shared = LogicalTab::leaf(current_id);
        shared.id = app.tabs[0].tree_id.get();
        for pane in &parked {
            shared.root.split_leaf(
                current_id,
                pane.terminal().unwrap().read(cx).pane_id,
                LogicalAxis::Vertical,
                0.6,
                false,
            );
        }
        shared.presentation = app.tabs[0].presentation.clone();
        assert!(
            shared.park(current_id),
            "a second client adds another parked pane"
        );
        assert!(app.apply_layout_delta(
            &LayoutDelta::TabRestructured {
                tab: shared.clone(),
                pane: None
            },
            window,
            cx
        ));
        shared
    });
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, window, cx| {
        assert_eq!(
            app.parked_deck.read(cx).selected_pane(),
            Some(parked[3].entity_id().as_u64())
        );
        let deleted = parked[3].terminal().unwrap().read(cx).pane_id;
        assert!(shared.root.remove_leaf(deleted).is_some());
        shared
            .presentation
            .remap_panes(|id| (id != deleted).then_some(id));
        assert!(app.apply_layout_delta(
            &LayoutDelta::TabRestructured {
                tab: shared,
                pane: None
            },
            window,
            cx
        ));
        app.restore_parked(parked[3].entity_id(), window, cx);
        assert_eq!(
            app.tabs[0].pane.leaves().len(),
            4,
            "a stale request cannot recreate the closed session"
        );
        assert_eq!(app.tabs[0].presentation.parked().len(), 4);
    });
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, _, cx| {
        assert_eq!(app.parked_deck.read(cx).selected_pane(), None);
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("parked-preview").is_none());
        assert!(window.try_find("parked-overflow").is_none());
        window.click("parked-more", cx);
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.press("end", cx);
        window.press("enter", cx);
        window.press("enter", cx);
    });
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(!app.tabs[0].is_parked_slot(&current, cx));
        assert!(current.contains_focused(window, cx));
        assert!(
            parked[..3]
                .iter()
                .all(|pane| app.tabs[0].is_parked_slot(pane, cx))
        );
        assert!(
            !app.tabs[0]
                .pane
                .leaves()
                .iter()
                .any(|pane| pane.entity_id() == parked[3].entity_id())
        );
    });
}
