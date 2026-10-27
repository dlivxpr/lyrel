use super::*;
use crate::core::session::{RemoteRef, RemoteTarget, WindowView, WindowViews};
use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt as _;

#[gpui_kit::test]
fn a_native_ssh_pane_with_the_same_daemon_id_does_not_block_remote_pip(
    cx: &mut TestAppContext,
) {
    let (app, mut vcx, _remote_stream) = test_window::harness_with_pane(cx);
    let remote = WindowView::on_remote(RemoteRef::new(
        RemoteTarget::Alias {
            alias: "pip-remote".into(),
        },
        WorkspaceId::new(),
    ));
    let remote_id = remote.id;
    let (target, ssh, _ssh_stream) = app.update_in(&mut vcx, |app, window, cx| {
        WorkspaceStore::install_for_test(
            cx,
            WindowViews {
                views: vec![remote],
                active: None,
            },
        );
        app.workspace = remote_id;
        let target = app.tabs[0].pane.first_leaf().unwrap();
        target.terminal().unwrap().update(cx, |view, _| {
            view.set_workspace(Some(crate::terminal::PaneWorkspace {
                workspace: WorkspaceId::new(),
                target: RemoteTarget::direct("me", "pip-remote", 22),
                spec: None,
                label: None,
                resize_echo: false,
            }));
        });
        let (ssh, stream) = crate::terminal::view::quiet_test_ssh_pane(1, window, cx);
        app.tabs[0].pane.split_leaf(
            target.entity_id(),
            Axis::Horizontal,
            false,
            PaneSlot::Ready(ssh.clone()),
        );
        window.focus(&target.focus_handle(cx), cx);
        cx.notify();
        (target, ssh, stream)
    });
    vcx.dispatch_action(TogglePanePip);
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .find(("pane-pip", target.entity_id().as_u64() as usize))
                .visible()
        );
        assert!(
            window
                .try_find(("pane-pip", ssh.entity_id().as_u64() as usize))
                .is_none()
        );
    });
    vcx.dispatch_action(RestorePanePip);
    app.update_in(&mut vcx, |app, _, _| {
        let leaves = app.tabs[0].pane.leaves();
        assert_eq!(leaves.len(), 2);
        assert!(
            leaves
                .iter()
                .any(|slot| slot.entity_id() == ssh.entity_id())
        );
        assert!(
            leaves
                .iter()
                .any(|slot| slot.entity_id() == target.entity_id())
        );
    });
    app.update_in(&mut vcx, |_, window, cx| {
        let focus = ssh.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
    });
    vcx.dispatch_action(TogglePanePip);
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .try_find(("pane-pip", target.entity_id().as_u64() as usize))
                .is_none()
        );
        assert!(
            window
                .find(("pane-pip", ssh.entity_id().as_u64() as usize))
                .visible()
        );
    });
    app.update_in(&mut vcx, |app, window, cx| {
        assert_eq!(
            app.tabs[0].pane.leaves().len(),
            2,
            "a foreign-owned command must never target a colliding remote ID"
        );
        assert!(ssh.read(cx).focus_handle.contains_focused(window, cx));
    });
    vcx.dispatch_action(RestorePanePip);
    app.update_in(&mut vcx, |app, _, cx| {
        assert_eq!(app.tabs[0].pane.leaves().len(), 2);
        assert!(app.tabs[0].pip_slot(cx).is_none());
    });
}
