use super::*;
use gpui_kit::TestAppContext;
use gpui_kit::test::{TestEventExt as _, TestQueryExt as _, TestWindowExt as _};

#[gpui_kit::test]
fn a_single_pane_handle_opens_by_keyboard_and_escape_returns_to_its_pane(
    cx: &mut TestAppContext,
) {
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 1);
    let pane = app.update_in(&mut vcx, |app, _, _| app.tabs[0].pane.first_leaf().unwrap());
    let id = ("pane-drag-handle", pane.entity_id().as_u64() as usize);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(!window.find(id).visible());
        window.pointer_move(window.find(id).bounds().center(), None, cx);
        assert!(window.find(id).visible());
        window.click(id, cx);
        assert!(
            !pane.contains_focused(window, cx),
            "click transfers focus to the menu"
        );
        window.pointer_move(point(px(10.), px(10.)), None, cx);
        assert!(
            window.find(id).visible(),
            "an open menu keeps its trigger visible"
        );
        window.press("escape", cx);
    });
    app.update_in(&mut vcx, |_, window, cx| {
        assert!(pane.contains_focused(window, cx));
    });
    // Start in window navigation: a focused terminal owns Tab for the shell.
    vcx.update(|window, cx| {
        window.blur(cx);
        window.focus_next(cx);
        window.render_frame(cx);
        for _ in 0..32 {
            window.press("tab", cx);
            if window.find(id).focused() == Some(true) {
                break;
            }
        }
        assert_eq!(window.find(id).focused(), Some(true));
        assert!(
            window.find(id).visible(),
            "keyboard focus reveals the handle"
        );
        window.press("enter", cx);
        assert!(
            !pane.contains_focused(window, cx),
            "Enter opens the menu without hover"
        );
        window.press("escape", cx);
    });
    app.update_in(&mut vcx, |_, window, cx| {
        assert!(pane.contains_focused(window, cx))
    });
}

#[gpui_kit::test]
fn move_to_tab_keeps_the_selected_remote_transport_and_the_other_pane(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 2);
    let (left, right) = app.update_in(&mut vcx, |app, window, cx| {
        let left = app.tabs[0].pane.first_leaf().unwrap();
        let right = app.tabs.remove(1).pane.first_leaf().unwrap();
        right.terminal().unwrap().update(cx, |view, _| {
            view.set_workspace(Some(crate::terminal::PaneWorkspace {
                workspace: WorkspaceId::new(),
                target: crate::core::session::RemoteTarget::direct("me", "remote-build", 22),
                spec: None,
                label: None,
                resize_echo: false,
            }));
        });
        app.tabs[0]
            .pane
            .split_leaf(left.entity_id(), Axis::Horizontal, false, right.clone());
        window.focus(&right.focus_handle(cx), cx);
        cx.notify();
        (left, right)
    });
    vcx.dispatch_action(MovePaneToTab);
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, window, cx| {
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(
            app.tabs[0].pane.first_leaf().unwrap().entity_id(),
            left.entity_id()
        );
        assert_eq!(
            app.tabs[1].pane.first_leaf().unwrap().entity_id(),
            right.entity_id()
        );
        assert_eq!(app.tabs.active_index(), 1);
        assert!(right.contains_focused(window, cx));
        assert!(!right.terminal().unwrap().read(cx).pane_route().is_local());
    });
}

#[gpui_kit::test]
fn menu_zoom_targets_the_clicked_pane_without_changing_fonts(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 2);
    let (left, right, font) = app.update_in(&mut vcx, |app, window, cx| {
        let left = app.tabs[0].pane.first_leaf().unwrap();
        let right = app.tabs.remove(1).pane.first_leaf().unwrap();
        app.tabs[0]
            .pane
            .split_leaf(left.entity_id(), Axis::Horizontal, false, right.clone());
        window.focus(&left.focus_handle(cx), cx);
        cx.notify();
        (left, right, app.font_size)
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let id = ("pane-drag-handle", right.entity_id().as_u64() as usize);
        window.pointer_move(window.find(id).bounds().center(), None, cx);
        window.click(
            ("pane-drag-handle", right.entity_id().as_u64() as usize),
            cx,
        );
    });
    vcx.update(|window, cx| {
        let zoom = window
            .find_by_label(t(L10nKey::CmdZoomPane))
            .bounds()
            .center();
        window.pointer_down(zoom, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(zoom, gpui_kit::MouseButton::Left, cx);
    });

    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, window, cx| {
        assert_eq!(
            app.maximized.as_ref().unwrap().entity_id(),
            right.entity_id()
        );
        assert!(right.contains_focused(window, cx));
        assert_eq!(app.font_size, font);
        assert_eq!(app.tabs[0].pane.leaves().len(), 2);
        assert_ne!(left.entity_id(), right.entity_id());
    });
    vcx.dispatch_action(ToggleMaximizePane);
    app.update_in(&mut vcx, |app, _, _| assert!(app.maximized.is_none()));
}

#[gpui_kit::test]
fn restarting_an_unreachable_remote_never_replaces_it_with_a_local_shell(
    cx: &mut TestAppContext,
) {
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 2);
    let remote = app.update_in(&mut vcx, |app, window, cx| {
        let remote = app.tabs[0].pane.terminals()[0].clone();
        remote.update(cx, |view, _| {
            view.set_workspace(Some(crate::terminal::PaneWorkspace {
                workspace: WorkspaceId::new(),
                target: crate::core::session::RemoteTarget::direct("me", "offline-build", 22),
                spec: None,
                label: None,
                resize_echo: false,
            }))
        });
        let focus = remote.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
        remote
    });
    vcx.dispatch_action(RestartPaneShell);
    vcx.run_until_parked();
    assert!(vcx.has_pending_prompt());
    // Focus changes while a confirmation is up must not retarget it.
    app.update_in(&mut vcx, |app, window, cx| {
        app.tabs.select(1);
        app.focus_active(window, cx);
    });
    vcx.simulate_prompt_answer(t(L10nKey::AppRestart));
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, _, cx| {
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.tabs[0].pane.terminals()[0], remote);
        assert!(!remote.read(cx).pane_route().is_local());
        assert_eq!(app.tabs.active_index(), 1);
    });
}

#[gpui_kit::test]
fn dragging_the_handle_moves_the_same_pane_without_opening_its_menu(cx: &mut TestAppContext) {
    use gpui_kit::{MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PlatformInput};
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
    let start = vcx.update(|window, cx| {
        window.render_frame(cx);
        window
            .find(("pane-drag-handle", right.entity_id().as_u64() as usize))
            .bounds()
            .center()
    });
    let end = app.update_in(&mut vcx, |app, _, _| {
        crate::ui::pane_drag::leaf_bounds(&app.tabs[0].pane, app.pane_area.get().unwrap())[0]
            .center()
    });
    vcx.update(|window, cx| {
        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: start,
                pressed_button: None,
                modifiers: gpui_kit::Modifiers::none(),
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position: start,
                modifiers: gpui_kit::Modifiers::none(),
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: end,
                pressed_button: Some(MouseButton::Left),
                modifiers: gpui_kit::Modifiers::none(),
            }),
            cx,
        );
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        assert!(cx.has_active_drag());
        window.render_frame(cx);
        window.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Left,
                position: end,
                modifiers: gpui_kit::Modifiers::none(),
                click_count: 1,
            }),
            cx,
        );
    });
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, window, cx| {
        let leaves = app.tabs[0].pane.leaves();
        assert_eq!(leaves[0].entity_id(), right.entity_id());
        assert_eq!(leaves[1].entity_id(), left.entity_id());
        assert!(
            right.contains_focused(window, cx),
            "drag release must not open a menu"
        );
    });
}
