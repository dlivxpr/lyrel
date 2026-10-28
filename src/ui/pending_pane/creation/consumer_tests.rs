use super::*;
use crate::ui::app::{LyrelApp, test_window};
use crate::ui::pane::{Pane, PaneSlot};
use crate::ui::tab::Tab;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext};

fn pending(
    app: &mut LyrelApp,
    window: &mut Window,
    cx: &mut Context<LyrelApp>,
) -> Entity<PendingPane> {
    let waiting = cx.new(|cx| {
        PendingPane::new(
            "local",
            PendingSpawn {
                workspace: None,
                working_directory: Some(std::path::PathBuf::from("/original")),
                restore_pane: None,
                shell: None,
                agent: None,
                agent_session_id: None,
                agent_launch_argv: None,
                agent_session_scope: None,
                run_on_land: None,
                owner: Some(app.workspace),
                font_size: 14.,
            },
            cx,
        )
    });
    // The same workbench consumer as production; only the daemon reply timing
    // is controlled by this fixture. The terminal still owns a real socket.
    cx.subscribe_in(
        &waiting,
        window,
        |app, pending, _: &SpawnFinished, window, cx| {
            app.land_pane(&pending, window, cx);
        },
    )
    .detach();
    app.tabs
        .push(Tab::new(Pane::leaf(PaneSlot::Connecting(waiting.clone()))));
    app.tabs.select(app.tabs.len() - 1);
    app.focus_active(window, cx);
    cx.notify();
    waiting
}

fn deliver(
    waiting: &Entity<PendingPane>,
    result: Result<ShellParts, String>,
    vcx: &mut VisualTestContext,
) {
    vcx.update(|window, cx| {
        let executor = cx.background_executor().clone();
        waiting.update(cx, |pending, cx| {
            let generation = pending.creation.generation;
            cx.spawn_in(window, async move |this, cx| {
                smol::future::yield_now().await;
                let _ = this.update_in(cx, |pending, window, cx| {
                    pending.receive(
                        generation,
                        result.map(|parts| OwnedTerminal {
                            parts: Some(CreatedTerminal::Shell(parts)),
                            executor,
                        }),
                        window,
                        cx,
                    );
                });
            })
            .detach();
        });
    });
    vcx.run_until_parked();
}

#[gpui_kit::test]
fn a_late_spawn_lands_in_its_own_slot_and_workspace(cx: &mut TestAppContext) {
    let (app, mut vcx, _original_stream) = test_window::harness_with_pane(cx);
    let (waiting, original, owner, _daemon) = app.update_in(&mut vcx, |app, window, cx| {
        let original = app.tabs[0].pane.first_leaf().unwrap().entity_id();
        let waiting = pending(app, window, cx);
        app.tabs.select(0);
        app.focus_active(window, cx);
        let (parts, daemon) =
            crate::terminal::view::quiet_test_shell_parts(91, Some(app.workspace));
        (waiting, original, (app.workspace, parts), daemon)
    });
    deliver(&waiting, Ok(owner.1), &mut vcx);
    app.read_with(&vcx, |app, cx| {
        assert_eq!(app.tabs[0].pane.first_leaf().unwrap().entity_id(), original);
        assert_eq!(
            app.tabs.active_index(),
            0,
            "landing never selects the delayed tab"
        );
        let landed = app.tabs[1].pane.terminals();
        assert_eq!(landed.len(), 1);
        assert_eq!(landed[0].read(cx).pane_id, 91);
        assert_eq!(landed[0].read(cx).owner_workspace(), Some(owner.0));
    });
    vcx.update(|window, cx| {
        assert!(
            app.read(cx).tabs[0]
                .pane
                .first_leaf()
                .unwrap()
                .contains_focused(window, cx)
        );
    });
}

#[gpui_kit::test]
fn a_failed_spawn_keeps_its_failure_and_never_overwrites_the_original(cx: &mut TestAppContext) {
    let (app, mut vcx, mut original_stream) = test_window::harness_with_pane(cx);
    let (waiting, original) = app.update_in(&mut vcx, |app, window, cx| {
        let original = app.tabs[0].pane.first_leaf().unwrap().entity_id();
        let waiting = pending(app, window, cx);
        app.run_in_active_terminal("do-not-send-to-original", window, cx);
        (waiting, original)
    });
    deliver(&waiting, Err("configured shell missing".into()), &mut vcx);
    app.read_with(&vcx, |app, cx| {
        assert!(waiting.read(cx).is_failed());
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(
            app.tabs[1].pane.first_leaf().unwrap().entity_id(),
            waiting.entity_id()
        );
        assert_eq!(app.tabs[0].pane.first_leaf().unwrap().entity_id(), original);
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window.find("pending-pane-retry").visible(),
            "failure has a recovery control"
        );
        waiting.update(cx, |pending, _| {
            assert!(pending.next_auto_retry().is_none());
            assert_eq!(
                pending.spawn.run_on_land.as_deref(),
                Some("do-not-send-to-original")
            );
            assert_eq!(
                pending.spawn.working_directory.as_deref(),
                Some(std::path::Path::new("/original"))
            );
        });
    });
    original_stream
        .set_read_timeout(Some(std::time::Duration::from_millis(100)))
        .unwrap();
    while let Ok(message) = crate::daemon::protocol::ClientMsg::read(&mut original_stream) {
        assert!(
            !matches!(message, crate::daemon::protocol::ClientMsg::Input(_)),
            "failed new-pane command reached the original pane"
        );
    }
}

#[gpui_kit::test]
fn a_closed_or_replaced_slot_cannot_claim_a_late_success(cx: &mut TestAppContext) {
    let (app, mut vcx, _original_stream) = test_window::harness_with_pane(cx);
    let (waiting, original, parts, _daemon) = app.update_in(&mut vcx, |app, window, cx| {
        let original = app.tabs[0].pane.first_leaf().unwrap().entity_id();
        let waiting = pending(app, window, cx);
        app.tabs.remove(1);
        app.tabs.select(0);
        app.focus_active(window, cx);
        let (parts, daemon) =
            crate::terminal::view::quiet_test_shell_parts(92, Some(app.workspace));
        (waiting, original, parts, daemon)
    });
    deliver(&waiting, Ok(parts), &mut vcx);
    app.read_with(&vcx, |app, cx| {
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs[0].pane.first_leaf().unwrap().entity_id(), original);
        assert_eq!(app.tabs[0].pane.terminals()[0].read(cx).pane_id, 1);
    });
}

#[gpui_kit::test]
fn a_workspace_change_rejects_the_original_slots_late_success(cx: &mut TestAppContext) {
    let (app, mut vcx, _original_stream) = test_window::harness_with_pane(cx);
    let (waiting, parts, _daemon) = app.update_in(&mut vcx, |app, window, cx| {
        let waiting = pending(app, window, cx);
        let (parts, daemon) =
            crate::terminal::view::quiet_test_shell_parts(93, Some(app.workspace));
        app.workspace = WorkspaceId::new();
        (waiting, parts, daemon)
    });
    deliver(&waiting, Ok(parts), &mut vcx);
    app.read_with(&vcx, |app, cx| {
        assert_eq!(
            app.tabs[1].pane.first_leaf().unwrap().entity_id(),
            waiting.entity_id()
        );
        assert!(app.tabs[1].pane.terminals().is_empty());
        assert_eq!(app.tabs[0].pane.terminals()[0].read(cx).pane_id, 1);
    });
}

#[gpui_kit::test]
fn a_replacement_terminal_is_not_overwritten_by_the_previous_requests_result(
    cx: &mut TestAppContext,
) {
    let (app, mut vcx, _original_stream) = test_window::harness_with_pane(cx);
    let (waiting, replacement, parts, _replacement_daemon, _late_daemon) =
        app.update_in(&mut vcx, |app, window, cx| {
            let waiting = pending(app, window, cx);
            let (replacement, replacement_daemon) =
                crate::terminal::view::quiet_test_pane(94, window, cx);
            app.tabs
                .replace_leaf(waiting.entity_id(), PaneSlot::Ready(replacement.clone()));
            let (parts, daemon) =
                crate::terminal::view::quiet_test_shell_parts(95, Some(app.workspace));
            (waiting, replacement, parts, replacement_daemon, daemon)
        });
    deliver(&waiting, Ok(parts), &mut vcx);
    app.read_with(&vcx, |app, cx| {
        assert_eq!(app.tabs.len(), 2);
        let landed = app.tabs[1].pane.terminals();
        assert_eq!(landed.len(), 1);
        assert_eq!(landed[0].entity_id(), replacement.entity_id());
        assert_eq!(landed[0].read(cx).pane_id, 94);
    });
}

#[gpui_kit::test]
fn a_focused_split_hands_focus_to_its_landed_terminal(cx: &mut TestAppContext) {
    use gpui_kit::Focusable as _;

    let (app, mut vcx, _original_stream) = test_window::harness_with_pane(cx);
    let (waiting, original, parts, _daemon) = app.update_in(&mut vcx, |app, window, cx| {
        let waiting = pending(app, window, cx);
        app.tabs.remove(1);
        app.tabs.select(0);
        let original = app.tabs[0].pane.first_leaf().unwrap().entity_id();
        assert!(app.tabs.get_mut(0).unwrap().split_pane(
            original,
            gpui_kit::Axis::Horizontal,
            false,
            PaneSlot::Connecting(waiting.clone()),
        ));
        window.focus(&waiting.read(cx).focus_handle(cx), cx);
        let (parts, daemon) =
            crate::terminal::view::quiet_test_shell_parts(96, Some(app.workspace));
        (waiting, original, parts, daemon)
    });
    deliver(&waiting, Ok(parts), &mut vcx);
    app.update_in(&mut vcx, |app, window, cx| {
        assert_eq!(app.tabs.len(), 1);
        let leaves = app.tabs[0].pane.terminals();
        assert_eq!(leaves.len(), 2);
        assert!(leaves.iter().any(|leaf| leaf.entity_id() == original));
        let landed = leaves.iter().find(|leaf| leaf.read(cx).pane_id == 96).unwrap();
        assert!(landed.read(cx).focus_handle.contains_focused(window, cx));
        assert_eq!(
            app.tabs[0].focus_target(cx).unwrap().entity_id(),
            landed.entity_id(),
            "the terminal focus observer remembers the installed entity, not the retired slot"
        );
    });
}

#[gpui_kit::test]
fn a_late_spawn_keeps_the_open_searchs_focus(cx: &mut TestAppContext) {
    let (app, mut vcx, _original_stream) = test_window::harness_with_pane(cx);
    let (waiting, parts, _daemon) = app.update_in(&mut vcx, |app, window, cx| {
        let waiting = pending(app, window, cx);
        let (parts, daemon) =
            crate::terminal::view::quiet_test_shell_parts(97, Some(app.workspace));
        (waiting, parts, daemon)
    });
    vcx.dispatch_action(crate::core::actions::TogglePalette);
    vcx.run_until_parked();
    let search_focus = vcx.update(|window, cx| {
        assert!(app.read(cx).search.is_open());
        window.focused(cx).expect("search owns window focus")
    });
    deliver(&waiting, Ok(parts), &mut vcx);
    vcx.update(|window, cx| {
        let app = app.read(cx);
        assert!(app.search.is_open());
        assert_eq!(app.tabs[1].pane.terminals()[0].read(cx).pane_id, 97);
        assert_eq!(window.focused(cx), Some(search_focus));
    });
}
