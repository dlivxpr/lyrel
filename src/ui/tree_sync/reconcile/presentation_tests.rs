use super::*;
use crate::ui::tree_sync::test_layout::{leaf, split, tab};
#[test]
fn pip_sync_keeps_every_logical_pane_and_only_changes_presentation() {
    let workspace = WorkspaceId::new();
    let mut logical = TreeTab::leaf(1);
    logical
        .root
        .split_leaf(1, 2, TreeAxis::Horizontal, 0.4, false);
    let id = logical.id;
    let mut mirror = WsMirror {
        tabs: vec![logical.clone()],
        active: Some(id),
        ..Default::default()
    };
    assert!(logical.set_pip(Some(2), Some(1)));
    let mut desired = tab(id, split(TreeAxis::Horizontal, 0.4, leaf(1), leaf(2)));
    desired.presentation = logical.presentation.clone();
    desired.presentation_fallback = Some(1);
    assert_eq!(
        diff(
            workspace,
            &mut mirror,
            &[desired.clone()],
            Some(id),
            SyncScope::Full,
            &[]
        ),
        vec![ControlRequest::PaneSetPresentation {
            workspace,
            pane: 2,
            pip: true,
            fallback: Some(1),
            restore: logical.presentation.pip().cloned(),
        }],
    );
    assert_eq!(mirror.tabs[0].root.pane_ids(), vec![1, 2]);
    assert_eq!(mirror.tabs[0].presentation, logical.presentation);
    desired.presentation = TabPresentation::default();
    assert_eq!(
        diff(
            workspace,
            &mut mirror,
            &[desired],
            Some(id),
            SyncScope::Full,
            &[]
        ),
        vec![ControlRequest::PaneSetPresentation {
            workspace,
            pane: 2,
            pip: false,
            fallback: Some(1),
            restore: None
        }],
    );
    assert_eq!(mirror.tabs[0].root, logical.root);
    assert!(mirror.tabs[0].presentation.pip().is_none());
}

#[test]
fn an_orphan_pip_restore_syncs_beside_the_actual_desired_fallback() {
    let workspace = WorkspaceId::new();
    let mut logical = TreeTab::leaf(1);
    logical
        .root
        .split_leaf(1, 2, TreeAxis::Horizontal, 0.4, false);
    assert!(logical.set_pip(Some(2), Some(1)));
    logical.root.remove_leaf(1);
    logical
        .root
        .split_leaf(2, 3, TreeAxis::Vertical, 0.6, false);
    logical
        .root
        .split_leaf(3, 4, TreeAxis::Horizontal, 0.7, false);
    let id = logical.id;
    let mut mirror = WsMirror {
        tabs: vec![logical],
        active: Some(id),
        ..Default::default()
    };
    let mut desired = tab(
        id,
        split(
            TreeAxis::Horizontal,
            0.7,
            leaf(3),
            split(TreeAxis::Horizontal, 0.4, leaf(4), leaf(2)),
        ),
    );
    // The restored pane has already received local focus; that must not
    // replace the tiled fallback the restore actually used.
    desired.presentation_fallback = Some(2);
    let ops = diff(
        workspace,
        &mut mirror,
        &[desired.clone()],
        Some(id),
        SyncScope::Full,
        &[],
    );
    assert_eq!(
        ops,
        vec![ControlRequest::PaneSetPresentation {
            workspace,
            pane: 2,
            pip: false,
            fallback: Some(4),
            restore: None,
        }]
    );
    assert_eq!(mirror.tabs[0].root, desired.root.to_pane_node());
    assert!(mirror.tabs[0].presentation.pip().is_none());
}
