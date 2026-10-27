
use super::test_window::harness_with_pane;
use crate::core::session::{
    RemoteRef, RemoteTarget, WindowView, WindowViews, WorkspaceId, WorkspaceStore,
};
use crate::ui::pane::{Pane, PaneSlot};
use gpui_kit::TestAppContext;
use lyrel_core::core::machine::{LayoutDelta, PaneNode, Tab as TreeTab};

#[gpui_kit::test]
fn a_tree_rebuild_keeps_the_native_ssh_split_a_remote_tab_holds(cx: &mut TestAppContext) {
    let (app, mut vcx, _remote_pane_stream) = harness_with_pane(cx);

    let remote = WindowView::on_remote(RemoteRef::new(
        RemoteTarget::Alias {
            alias: "build-box".into(),
        },
        WorkspaceId::new(),
    ));
    let remote_id = remote.id;
    let _ssh_stream = app.update_in(&mut vcx, |app, window, cx| {
        WorkspaceStore::install_for_test(
            cx,
            WindowViews {
                views: vec![remote],
                active: None,
            },
        );
        app.workspace = remote_id;
        let (ssh_view, stream) = crate::terminal::view::quiet_test_ssh_pane(2, window, cx);
        let existing = std::mem::replace(&mut app.tabs[0].pane, Pane::Empty);
        app.tabs[0].pane = Pane::split_node(
            gpui_kit::Axis::Horizontal,
            0.5,
            existing,
            Pane::leaf(PaneSlot::Ready(ssh_view)),
        );
        stream
    });

    let applied = app.update_in(&mut vcx, |app, window, cx| {
        let tab = TreeTab {
            id: app.tabs[0].id(),
            name: None,
            group: None,
            last_auto: None,
            root: PaneNode::Leaf { pane: 1 },
            hibernated: false,
            presentation: Default::default(),
        };
        app.apply_layout_delta(
            &LayoutDelta::TabRestructured { tab, pane: None },
            window,
            cx,
        )
    });
    assert!(
        applied,
        "the delta must apply without falling back to a resync"
    );

    app.update_in(&mut vcx, |app, _, cx| {
        let leaves = app.tabs[0].pane.leaves();
        assert_eq!(leaves.len(), 2, "the ssh split must survive the rebuild");
        assert!(
            leaves.iter().any(|slot| match slot {
                PaneSlot::Ready(view) => view.read(cx).ssh_spec().is_some(),
                _ => false,
            }),
            "one leaf is still the native-SSH pane"
        );
        assert!(
            leaves.iter().any(|slot| match slot {
                PaneSlot::Ready(view) => {
                    let view = view.read(cx);
                    view.ssh_spec().is_none() && view.pane_id == 1
                }
                _ => false,
            }),
            "the remote pane's existing view is reused, not re-attached"
        );
    });
}

/// Two tabs on pane 1 and pane 2, with the window's sync primed on them the
/// way a landed pull leaves it.
fn primed_two_tab_window(
    cx: &mut TestAppContext,
) -> (
    gpui_kit::Entity<super::LyrelApp>,
    gpui_kit::VisualTestContext,
    Vec<crate::daemon::transport::Stream>,
) {
    let (app, mut vcx, first) = harness_with_pane(cx);
    let second = app.update_in(&mut vcx, |app, window, cx| {
        let (view, stream) = crate::terminal::view::quiet_test_pane(2, window, cx);
        app.tabs
            .push(super::Tab::new(Pane::leaf(PaneSlot::Ready(view))));
        let tabs = app
            .tabs
            .iter()
            .zip([1u64, 2])
            .map(|(tab, pane)| TreeTab {
                id: tab.id(),
                name: None,
                group: None,
                last_auto: None,
                root: PaneNode::Leaf { pane },
                hibernated: false,
                presentation: Default::default(),
            })
            .collect();
        WorkspaceStore::install_for_test(
            cx,
            WindowViews {
                views: vec![WindowView {
                    id: app.workspace,
                    ..WindowView::default()
                }],
                active: None,
            },
        );
        crate::ui::tree_sync::prime_for_test(cx, app.workspace, tabs);
        stream
    });
    (app, vcx, vec![first, second])
}

fn queued_closes(
    app: &super::LyrelApp,
    cx: &mut gpui_kit::App,
) -> Vec<lyrel_core::core::machine::TabId> {
    use lyrel_core::daemon::control::ControlRequest;
    crate::ui::tree_sync::queued_for_test(cx, app.workspace)
        .into_iter()
        .filter_map(|op| match op {
            ControlRequest::TabClose { tab, .. }
            | ControlRequest::TabCloseRemembered { tab, .. } => Some(tab),
            _ => None,
        })
        .collect()
}

/// Restart Server empties the window and waits out the handoff, and every
/// delta that lands meanwhile syncs the emptied window. A window missing
/// its tabs is not a user closing them.
#[gpui_kit::test]
fn emptying_the_window_closes_nothing_on_the_machine(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = primed_two_tab_window(cx);
    let closes = app.update_in(&mut vcx, |app, _, cx| {
        app.tabs.clear();
        app.save_session(cx);
        queued_closes(app, cx)
    });
    assert!(
        closes.is_empty(),
        "this is the regression: Restart Server closed all sixteen tabs of a workspace, \
             unremembered, and left their shells running with no tab to hold them"
    );
}

#[gpui_kit::test]
fn closing_a_tab_closes_it_and_only_it_on_the_machine(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = primed_two_tab_window(cx);
    let (closed, closes) = app.update_in(&mut vcx, |app, window, cx| {
        let closed = app.tabs[1].id();
        app.close_tab_inner(1, true, window, cx);
        (closed, queued_closes(app, cx))
    });
    assert_eq!(closes, vec![closed]);
}

#[gpui_kit::test]
fn a_pure_native_ssh_tab_is_invisible_to_the_tree_not_held(cx: &mut TestAppContext) {
    let (app, mut vcx, _remote_pane_stream) = harness_with_pane(cx);

    let remote = WindowView::on_remote(RemoteRef::new(
        RemoteTarget::Alias {
            alias: "build-box".into(),
        },
        WorkspaceId::new(),
    ));
    let remote_id = remote.id;
    let _ssh_stream = app.update_in(&mut vcx, |app, window, cx| {
        WorkspaceStore::install_for_test(
            cx,
            WindowViews {
                views: vec![remote],
                active: None,
            },
        );
        app.workspace = remote_id;
        let (ssh_view, stream) = crate::terminal::view::quiet_test_ssh_pane(2, window, cx);
        app.tabs
            .push(super::Tab::new(Pane::leaf(PaneSlot::Ready(ssh_view))));
        stream
    });

    let (desired, _active, held) = app.update_in(&mut vcx, |app, _, cx| {
        crate::ui::tree_sync::projection::desired_tabs(
            &app.tabs,
            app.tabs.active_index(),
            app.workspace,
            cx,
        )
    });
    assert_eq!(
        desired.len(),
        1,
        "only the remote-backed tab can be named in the machine's tree"
    );
    assert!(
        held.is_empty(),
        "the pure-SSH tab is permanently invisible, not held — holding it \
             would freeze ordering and active-tab sync for the whole window"
    );
}
