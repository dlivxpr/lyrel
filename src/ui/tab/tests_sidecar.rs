use super::*;
use crate::core::session::{RemoteRef, RemoteTarget, WindowView, WindowViews};
use gpui_kit::TestAppContext;
use gpui_kit::test::{TestEventExt as _, TestWindowExt as _};
use lyrel_core::core::machine::{LayoutDelta, Tab as TreeTab, Workspace};
use lyrel_core::core::pane_sidecar::{ForeignWorkspace, ScopedPane, SidecarUpdate};

#[gpui_kit::test]
fn delayed_sidecar_snapshots_replay_promote_and_restore_without_replacing_views(
    cx: &mut TestAppContext,
) {
    let (app, mut vcx, _remote_stream) = test_window::harness_with_pane(cx);
    let route = RemoteRef::new(
        RemoteTarget::direct("me", "sidecar-snapshot", 22),
        WorkspaceId::new(),
    );
    let view = WindowView::on_remote(route.clone());
    let window_id = view.id;
    let (remote, native, _native_stream, sidecar) =
        app.update_in(&mut vcx, |app, window, cx| {
            WorkspaceStore::install_for_test(
                cx,
                WindowViews {
                    views: vec![view],
                    active: None,
                },
            );
            app.workspace = window_id;
            let remote = app.tabs[0].pane.first_leaf().unwrap();
            remote.terminal().unwrap().update(cx, |terminal, _| {
                terminal.set_workspace(Some(crate::terminal::PaneWorkspace {
                    workspace: route.workspace,
                    target: route.target.clone(),
                    spec: None,
                    label: None,
                    resize_echo: false,
                }))
            });
            let (native, stream) = crate::terminal::view::quiet_test_ssh_pane(1, window, cx);
            app.tabs[0].pane.split_leaf(
                remote.entity_id(),
                Axis::Horizontal,
                false,
                PaneSlot::Ready(native.clone()),
            );
            let mut saved = TreeTab::leaf(ScopedPane::workspace(1));
            saved.id = app.tabs[0].id();
            saved.root.split_leaf(
                ScopedPane::workspace(1),
                ScopedPane::client_local(1),
                lyrel_core::core::machine::Axis::Horizontal,
                0.5,
                false,
            );
            saved.set_pip(
                Some(ScopedPane::client_local(1)),
                Some(ScopedPane::workspace(1)),
            );
            let mut foreign = ForeignWorkspace::new(route.clone());
            foreign.apply(&[SidecarUpdate::new(None, saved)]).unwrap();
            let sidecar = Workspace {
                foreign_view: Some(foreign),
                ..Workspace::default()
            };
            window.focus(&remote.focus_handle(cx), cx);
            (remote, native, stream, sidecar)
        });
    let weak = app.downgrade();
    vcx.update(|window, cx| {
        if !cx.has_global::<crate::ui::windows::WindowRegistry>() {
            crate::ui::windows::WindowRegistry::init(cx);
        }
        crate::ui::windows::WindowRegistry::register(
            cx,
            window_id,
            window.window_handle(),
            weak,
        );
    });
    vcx.dispatch_action(TogglePanePip);
    let stale = sidecar.clone();
    vcx.update(|_, cx| {
        cx.defer(move |cx| {
            crate::ui::tree_sync::on_layout_delta(
                cx,
                HostId::LOCAL,
                &stale.id.to_string(),
                LayoutDelta::WorkspaceCreated {
                    workspace: stale.clone(),
                },
            )
        })
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .find(("pane-pip", remote.entity_id().as_u64() as usize))
                .visible()
        );
        assert!(
            window
                .try_find(("pane-pip", native.entity_id().as_u64() as usize))
                .is_none()
        );
    });
    vcx.dispatch_action(RestorePanePip);
    vcx.update(|_, cx| {
        cx.defer(move |cx| {
            crate::ui::tree_sync::on_layout_delta(
                cx,
                HostId::LOCAL,
                &sidecar.id.to_string(),
                LayoutDelta::WorkspaceCreated {
                    workspace: sidecar.clone(),
                },
            )
        })
    });
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, _, cx| {
        assert!(app.tabs[0].pip_slot(cx).is_none());
        let leaves = app.tabs[0].pane.leaves();
        assert_eq!(leaves.len(), 2);
        assert!(
            leaves
                .iter()
                .any(|slot| slot.entity_id() == remote.entity_id())
        );
        assert!(
            leaves
                .iter()
                .any(|slot| slot.entity_id() == native.entity_id())
        );
    });
}

fn native_sidecar_window(
    cx: &mut TestAppContext,
    native_only: bool,
) -> (
    Entity<LyrelApp>,
    gpui_kit::VisualTestContext,
    PaneSlot,
    Vec<crate::daemon::transport::Stream>,
    Workspace,
) {
    let (app, mut vcx, remote_stream) = test_window::harness_with_pane(cx);
    let route = RemoteRef::new(
        RemoteTarget::direct("me", "sidecar-presentation", 22),
        WorkspaceId::new(),
    );
    let view = WindowView::on_remote(route.clone());
    let window_id = view.id;
    let weak = app.downgrade();
    let (native, native_stream, sidecar) = app.update_in(&mut vcx, |app, window, cx| {
        WorkspaceStore::install_for_test(
            cx,
            WindowViews {
                views: vec![view],
                active: None,
            },
        );
        app.workspace = window_id;
        let remote = app.tabs[0].pane.first_leaf().unwrap();
        remote.terminal().unwrap().update(cx, |terminal, _| {
            terminal.set_workspace(Some(crate::terminal::PaneWorkspace {
                workspace: route.workspace,
                target: route.target.clone(),
                spec: None,
                label: None,
                resize_echo: false,
            }));
        });
        let (native, stream) = crate::terminal::view::quiet_test_ssh_pane(1, window, cx);
        let native = PaneSlot::Ready(native);
        if native_only {
            app.tabs[0].pane = Pane::Leaf(native.clone());
        } else {
            app.tabs[0].pane.split_leaf(
                remote.entity_id(),
                Axis::Horizontal,
                false,
                native.clone(),
            );
        }
        let saved = crate::ui::pane_sidecar::snapshot(&app.tabs[0], cx).unwrap();
        let mut foreign = ForeignWorkspace::new(route);
        foreign.apply(&[SidecarUpdate::new(None, saved)]).unwrap();
        if !cx.has_global::<crate::ui::windows::WindowRegistry>() {
            crate::ui::windows::WindowRegistry::init(cx);
        }
        crate::ui::windows::WindowRegistry::register(
            cx,
            window_id,
            window.window_handle(),
            weak,
        );
        window.focus(&native.focus_handle(cx), cx);
        (
            native,
            stream,
            Workspace {
                foreign_view: Some(foreign),
                ..Workspace::default()
            },
        )
    });
    deliver_sidecar(&mut vcx, sidecar.clone());
    (
        app,
        vcx,
        native,
        vec![remote_stream, native_stream],
        sidecar,
    )
}

#[gpui_kit::test]
fn sidecar_closed_native_leaf_preserves_other_views_input_and_presentation(
    cx: &mut TestAppContext,
) {
    use crate::daemon::protocol::ClientMsg;
    use lyrel_core::core::machine::PaneRecord;

    let (app, mut vcx, closed, _streams, mut sidecar) = native_sidecar_window(cx, false);
    let (remote, surviving, mut stream) = app.update_in(&mut vcx, |app, window, cx| {
        let remote = app.tabs[0]
            .pane
            .leaves()
            .into_iter()
            .find(|slot| {
                crate::ui::pane_sidecar::scoped_pane(slot, cx) == Some(ScopedPane::workspace(1))
            })
            .unwrap();
        let (native, stream) = crate::terminal::view::quiet_test_ssh_pane(2, window, cx);
        let surviving = PaneSlot::Ready(native);
        app.tabs[0].pane.split_leaf(
            closed.entity_id(),
            Axis::Vertical,
            false,
            surviving.clone(),
        );
        let foreign = sidecar.foreign_view.as_mut().unwrap();
        let before = foreign.tabs()[0].clone();
        let mut after = crate::ui::pane_sidecar::snapshot(&app.tabs[0], cx).unwrap();
        assert!(after.park(ScopedPane::client_local(1)));
        foreign
            .apply(&[SidecarUpdate::new(Some(before), after)])
            .unwrap();
        window.focus(&closed.focus_handle(cx), cx);
        (remote, surviving, stream)
    });
    // A local daemon snapshot is authoritative even if its composition still
    // references the pane another client has already closed.
    deliver_sidecar_records(&mut vcx, sidecar, vec![PaneRecord::new(2)]);
    app.update_in(&mut vcx, |app, window, cx| {
        assert_eq!(app.tabs.len(), 1);
        let leaves = app.tabs[0].pane.leaves();
        assert_eq!(leaves.len(), 2);
        assert!(
            leaves
                .iter()
                .any(|slot| slot.entity_id() == remote.entity_id())
        );
        assert!(
            leaves
                .iter()
                .any(|slot| slot.entity_id() == surviving.entity_id())
        );
        assert!(
            app.tabs[0]
                .scoped_presentation()
                .unwrap()
                .parked()
                .is_empty()
        );
        assert!(
            app.tabs[0]
                .focus_target(cx)
                .unwrap()
                .contains_focused(window, cx)
        );
        window.focus(&surviving.focus_handle(cx), cx);
    });
    vcx.dispatch_action(ParkPane);
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(app.tabs[0].is_parked_slot(&surviving, cx));
        app.restore_parked(surviving.entity_id(), window, cx);
        assert!(surviving.contains_focused(window, cx));
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        for slot in [&surviving, &remote] {
            let handle = ("pane-drag-handle", slot.entity_id().as_u64() as usize);
            window.pointer_move(window.find(handle).bounds().center(), None, cx);
            window.render_frame(cx);
            assert!(window.find(handle).visible());
        }
        assert!(
            window
                .try_find(("pane-drag-handle", closed.entity_id().as_u64() as usize))
                .is_none()
        );
    });
    vcx.simulate_keystrokes("x");
    stream
        .set_read_timeout(Some(std::time::Duration::from_millis(100)))
        .unwrap();
    let mut input = Vec::new();
    while let Ok(message) = ClientMsg::read(&mut stream) {
        if let ClientMsg::Input(bytes) = message {
            input.extend(bytes);
        }
    }
    assert_eq!(
        input, b"x",
        "the surviving native terminal keeps its original attachment"
    );
}

#[gpui_kit::test]
fn sidecar_exited_native_record_is_not_a_closed_session(cx: &mut TestAppContext) {
    let (app, mut vcx, native, _streams, sidecar) = native_sidecar_window(cx, false);
    native.terminal().unwrap().update(&mut vcx, |view, _| {
        view.terminal.exited = true;
    });
    let record = lyrel_core::core::machine::PaneRecord::new(1);
    assert!(!record.live);
    deliver_sidecar_records(&mut vcx, sidecar, vec![record]);
    app.update_in(&mut vcx, |app, window, cx| {
        assert_eq!(app.tabs[0].pane.leaves().len(), 2);
        assert!(
            app.tabs[0]
                .pane
                .leaves()
                .iter()
                .any(|slot| slot.entity_id() == native.entity_id())
        );
        assert!(native.contains_focused(window, cx));
    });
}

#[gpui_kit::test]
fn sidecar_closing_the_last_native_record_removes_its_tab(cx: &mut TestAppContext) {
    let (app, mut vcx, _native, _streams, sidecar) = native_sidecar_window(cx, true);
    deliver_sidecar_records(&mut vcx, sidecar, Vec::new());
    app.read_with(&vcx, |app, _| assert!(app.tabs.is_empty()));
}

fn deliver_sidecar_records(
    vcx: &mut gpui_kit::VisualTestContext,
    sidecar: Workspace,
    records: Vec<lyrel_core::core::machine::PaneRecord>,
) {
    vcx.update(|_, cx| {
        cx.defer(move |cx| {
            crate::ui::pane_sidecar::on_sidecar_snapshot(
                cx,
                lyrel_core::core::pane_sidecar::SidecarSnapshot::new(sidecar, records),
            )
        });
    });
    vcx.run_until_parked();
}

#[gpui_kit::test]
fn sidecar_closed_native_leaf_keeps_a_connecting_remote_view(cx: &mut TestAppContext) {
    use crate::ui::pending_pane::{PendingPane, PendingSpawn};

    let (app, mut vcx, closed, _streams, sidecar) = native_sidecar_window(cx, false);
    let pending = app.update_in(&mut vcx, |app, window, cx| {
        let remote = app.tabs[0]
            .pane
            .leaves()
            .into_iter()
            .find(|slot| {
                crate::ui::pane_sidecar::scoped_pane(slot, cx) == Some(ScopedPane::workspace(1))
            })
            .unwrap();
        let workspace = remote.terminal().unwrap().read(cx).workspace().cloned();
        let pending = cx.new(|cx| {
            PendingPane::new(
                "reconnecting",
                PendingSpawn {
                    workspace,
                    working_directory: None,
                    restore_pane: Some(1),
                    shell: None,
                    agent: None,
                    agent_session_id: None,
                    agent_launch_argv: None,
                    agent_session_scope: None,
                    run_on_land: None,
                    owner: None,
                    font_size: app.font_size,
                },
                cx,
            )
        });
        app.tabs[0].pane = Pane::split_node(
            Axis::Horizontal,
            0.5,
            Pane::Leaf(PaneSlot::Connecting(pending.clone())),
            Pane::Leaf(closed.clone()),
        );
        window.focus(&closed.focus_handle(cx), cx);
        pending
    });
    deliver_sidecar_records(&mut vcx, sidecar, Vec::new());
    app.update_in(&mut vcx, |app, window, cx| {
        assert_eq!(app.tabs.len(), 1);
        let surviving = app.tabs[0].pane.first_leaf().unwrap();
        assert_eq!(app.tabs[0].pane.leaves().len(), 1);
        assert_eq!(surviving.entity_id(), pending.entity_id());
        assert!(surviving.contains_focused(window, cx));
    });
}

#[gpui_kit::test]
fn sidecar_missing_remote_view_keeps_only_authoritatively_live_native_session(
    cx: &mut TestAppContext,
) {
    let (app, mut vcx, native, _streams, sidecar) = native_sidecar_window(cx, false);
    app.update_in(&mut vcx, |app, _, _| {
        app.tabs[0].pane = Pane::Leaf(native.clone());
    });
    deliver_sidecar_records(
        &mut vcx,
        sidecar.clone(),
        vec![lyrel_core::core::machine::PaneRecord::new(1)],
    );
    let remote_entity = app.read_with(&vcx, |app, cx| {
        assert_eq!(app.tabs.len(), 1);
        let leaves = app.tabs[0].pane.leaves();
        assert_eq!(leaves.len(), 2);
        assert!(
            leaves
                .iter()
                .any(|slot| slot.entity_id() == native.entity_id())
        );
        let remote = leaves
            .iter()
            .find(|slot| {
                crate::ui::pane_sidecar::scoped_pane(slot, cx) == Some(ScopedPane::workspace(1))
            })
            .unwrap();
        let PaneSlot::Connecting(pending) = remote else {
            panic!("the unreachable remote pane must retain its retryable placeholder");
        };
        let pending = pending.read(cx);
        assert!(
            pending.is_attach_only(),
            "reconstruction must never spawn a substitute shell"
        );
        assert_eq!(pending.restore_pane(), Some(1));
        assert!(
            pending.workspace().is_some(),
            "remote ownership must not fall back to Local"
        );
        remote.entity_id()
    });
    deliver_sidecar_records(&mut vcx, sidecar, Vec::new());
    app.read_with(&vcx, |app, cx| {
        assert_eq!(app.tabs.len(), 1, "the unresolved remote tab still exists");
        let leaves = app.tabs[0].pane.leaves();
        assert_eq!(leaves.len(), 1);
        assert_eq!(
            crate::ui::pane_sidecar::scoped_pane(&leaves[0], cx),
            Some(ScopedPane::workspace(1)),
        );
        assert_eq!(
            leaves[0].entity_id(),
            remote_entity,
            "reuse the existing remote placeholder"
        );
        assert_eq!(
            crate::ui::pane_sidecar::snapshot(&app.tabs[0], cx)
                .unwrap()
                .root
                .pane_ids(),
            vec![ScopedPane::workspace(1)],
            "saving the reconstructed view must retain the authoritative remote leaf",
        );
        assert_ne!(
            leaves[0].entity_id(),
            native.entity_id(),
            "a missing remote view cannot keep a closed native session mounted"
        );
    });
}

#[gpui_kit::test]
fn sidecar_another_client_closing_a_tab_rejects_pending_and_stale_composition(
    cx: &mut TestAppContext,
) {
    let (app, mut vcx, _native, _streams, mut sidecar) = native_sidecar_window(cx, false);
    let stale = sidecar.clone();
    // Keep a semantic edit queued in this GUI while another client closes
    // the authoritative composition, as on a temporarily disconnected link.
    vcx.dispatch_action(ParkPane);
    vcx.run_until_parked();
    let foreign = sidecar.foreign_view.as_mut().unwrap();
    let tab = foreign.tabs()[0].id;
    foreign.apply(&[SidecarUpdate::close_tab(tab)]).unwrap();
    deliver_sidecar_records(&mut vcx, sidecar, Vec::new());
    app.read_with(&vcx, |app, _| assert!(app.tabs.is_empty()));
    deliver_sidecar(&mut vcx, stale);
    app.read_with(&vcx, |app, _| assert!(app.tabs.is_empty()));
}

#[gpui_kit::test]
fn an_explicit_primary_tab_close_does_not_reappear_from_a_stale_sidecar(
    cx: &mut TestAppContext,
) {
    let (app, mut vcx, _native, _streams, sidecar) = native_sidecar_window(cx, false);
    app.update_in(&mut vcx, |app, window, cx| {
        let tab = app.tabs[0].id();
        assert!(app.apply_layout_delta(
            &LayoutDelta::TabClosed {
                tab,
                reason: lyrel_core::core::machine::TabCloseReason::Explicit,
            },
            window,
            cx
        ));
        assert!(app.tabs.is_empty());
    });
    deliver_sidecar(&mut vcx, sidecar);
    app.read_with(&vcx, |app, _| assert!(app.tabs.is_empty()));
}

#[gpui_kit::test]
fn an_emptied_primary_tab_preserves_its_native_shell_and_rejects_stale_remote_leaves(
    cx: &mut TestAppContext,
) {
    let (app, mut vcx, native, _streams, sidecar) = native_sidecar_window(cx, false);
    vcx.dispatch_action(ParkPane);
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, window, cx| {
        let tab = app.tabs[0].id();
        assert!(app.apply_layout_delta(
            &LayoutDelta::TabClosed {
                tab,
                reason: lyrel_core::core::machine::TabCloseReason::Emptied
            },
            window,
            cx
        ));
        assert!(
            app.tabs
                .iter()
                .any(|candidate| candidate.id() == tab),
            "emptying the remote projection must not close its client-local leaves"
        );
    });
    deliver_sidecar(&mut vcx, sidecar);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.find("parked-empty").visible());
    });
    app.update_in(&mut vcx, |app, window, cx| {
        let restored = crate::ui::pane_sidecar::snapshot(&app.tabs[0], cx).unwrap();
        assert_eq!(restored.root.pane_ids(), vec![ScopedPane::client_local(1)]);
        assert!(app.tabs[0].is_parked_slot(&native, cx));
        app.restore_parked(native.entity_id(), window, cx);
        assert!(native.contains_focused(window, cx));
    });
}

fn deliver_sidecar(vcx: &mut gpui_kit::VisualTestContext, sidecar: Workspace) {
    vcx.update(|_, cx| {
        cx.defer(move |cx| crate::ui::pane_sidecar::on_sidecar_delta(cx, &sidecar));
    });
    vcx.run_until_parked();
}

fn park_native_in_sidecar(sidecar: &mut Workspace) {
    let foreign = sidecar.foreign_view.as_mut().unwrap();
    let before = foreign.tabs()[0].clone();
    let mut after = before.clone();
    assert!(after.park(ScopedPane::client_local(1)));
    foreign
        .apply(&[SidecarUpdate::new(Some(before), after)])
        .unwrap();
}

#[gpui_kit::test]
fn a_native_only_remote_tab_keeps_its_empty_state_and_new_pane_action_when_parked(
    cx: &mut TestAppContext,
) {
    let (app, mut vcx, native, _streams, _) = native_sidecar_window(cx, true);
    vcx.dispatch_action(ParkPane);
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.find("parked-empty").visible());
        assert!(window.find("parked-new-pane").visible());
        assert!(!native.contains_focused(window, cx));
    });
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(app.tabs[0].is_parked_slot(&native, cx));
        app.restore_parked(native.entity_id(), window, cx);
        assert!(!app.tabs[0].is_parked_slot(&native, cx));
        assert!(native.contains_focused(window, cx));
    });
}

#[gpui_kit::test]
fn another_client_parking_native_ssh_moves_focus_to_a_visible_terminal(
    cx: &mut TestAppContext,
) {
    let (app, mut vcx, native, _streams, mut sidecar) = native_sidecar_window(cx, false);
    park_native_in_sidecar(&mut sidecar);
    deliver_sidecar(&mut vcx, sidecar);
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(app.tabs[0].is_parked_slot(&native, cx));
        assert!(!native.contains_focused(window, cx));
        assert!(
            app.tabs[0]
                .focus_target(cx)
                .unwrap()
                .contains_focused(window, cx)
        );
    });
}

#[gpui_kit::test]
fn another_client_parking_a_zoomed_native_pane_unmounts_it_even_with_toolbar_focus(
    cx: &mut TestAppContext,
) {
    let (app, mut vcx, native, _streams, mut sidecar) = native_sidecar_window(cx, false);
    app.update_in(&mut vcx, |app, window, cx| {
        app.maximized = native.terminal().cloned();
        app.tabs[0].park_zoom(native.terminal().cloned());
        window.focus(&app.file_tree.focus_handle, cx);
        cx.notify();
    });
    park_native_in_sidecar(&mut sidecar);
    deliver_sidecar(&mut vcx, sidecar);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .try_find(("pane-drag-handle", native.entity_id().as_u64() as usize))
                .is_none()
        );
    });
    app.update_in(&mut vcx, |app, _, _| {
        assert!(app.maximized.is_none());
        assert!(app.tabs[0].zoomed().is_none());
    });
}

#[gpui_kit::test]
fn native_pip_geometry_survives_primary_updates_without_changing_remote_geometry(
    cx: &mut TestAppContext,
) {
    use gpui_kit::test::TestEventExt as _;
    use lyrel_core::core::machine::PipGeometry;
    let (app, mut vcx, native, _streams, _) = native_sidecar_window(cx, false);
    vcx.dispatch_action(TogglePanePip);
    vcx.run_until_parked();
    let key = native.entity_id().as_u64() as usize;
    let primary_before = app.read_with(&vcx, |app, _| app.tabs[0].presentation().pip_geometry());
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let start = window.find(("pip-move", key)).bounds().center();
        window.pointer_down(start, gpui_kit::MouseButton::Left, cx);
        window.pointer_move(
            start - gpui_kit::point(px(70.), px(55.)),
            Some(gpui_kit::MouseButton::Left),
            cx,
        );
        window.pointer_up(
            start - gpui_kit::point(px(70.), px(55.)),
            gpui_kit::MouseButton::Left,
            cx,
        );
    });
    vcx.run_until_parked();
    let moved = vcx.update(|window, cx| {
        window.render_frame(cx);
        window.find(("pane-pip", key)).bounds()
    });
    app.update_in(&mut vcx, |app, window, cx| {
        assert_eq!(app.tabs[0].presentation().pip_geometry(), primary_before);
        let mut primary = TreeTab::leaf(1);
        primary.id = app.tabs[0].id();
        primary
            .presentation
            .set_pip_geometry(PipGeometry::new(1., 1., 0.5, 0.5));
        assert!(app.apply_layout_delta(
            &LayoutDelta::TabRestructured {
                tab: primary,
                pane: None
            },
            window,
            cx
        ));
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find(("pane-pip", key)).bounds(), moved);
    });
    app.update_in(&mut vcx, |app, _, cx| {
        assert_eq!(
            app.tabs[0].pip_slot(cx).unwrap().entity_id(),
            native.entity_id()
        );
    });
}
