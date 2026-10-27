use super::*;
use crate::core::group_key::GroupId;
use crate::ui::tree_sync::test_layout::{assert_converged, leaf, seed, split, tab};
#[test]
fn opening_the_first_tab_emits_a_create_carrying_the_client_identity() {
    let ws = WorkspaceId::new();
    let id = TabId::new();
    let mut mirror = WsMirror::default();
    let desired = vec![tab(id, leaf(7))];

    let ops = diff(ws, &mut mirror, &desired, Some(id), SyncScope::Full, &[]);
    assert_eq!(
        ops,
        vec![ControlRequest::TabCreate {
            workspace: ws,
            at: Some(0),
            pane: seed(7),
            tab: Some(id),
        }],
        "a created tab is active on the server, so no separate active op"
    );
    assert_converged(&mirror, &desired);
    assert_eq!(mirror.active, Some(id));
}

#[test]
fn a_split_emits_one_pane_split_against_its_sibling() {
    let ws = WorkspaceId::new();
    let id = TabId::new();
    let mut mirror = WsMirror::default();
    let one = vec![tab(id, leaf(1))];
    diff(ws, &mut mirror, &one, Some(id), SyncScope::Full, &[]);

    let two = vec![tab(id, split(TreeAxis::Vertical, 0.5, leaf(1), leaf(2)))];
    let ops = diff(ws, &mut mirror, &two, Some(id), SyncScope::Full, &[]);
    assert_eq!(
        ops,
        vec![ControlRequest::PaneSplit {
            workspace: ws,
            pane: 1,
            axis: TreeAxis::Vertical,
            ratio: 0.5,
            new: seed(2),
            first: false,
        }]
    );
    assert_converged(&mirror, &two);
}

#[test]
fn a_new_pane_on_the_upper_side_splits_with_first_set() {
    let ws = WorkspaceId::new();
    let id = TabId::new();
    let mut mirror = WsMirror::default();
    diff(
        ws,
        &mut mirror,
        &[tab(id, leaf(1))],
        Some(id),
        SyncScope::Full,
        &[],
    );

    let want = vec![tab(id, split(TreeAxis::Horizontal, 0.4, leaf(2), leaf(1)))];
    let ops = diff(ws, &mut mirror, &want, Some(id), SyncScope::Full, &[]);
    assert_eq!(
        ops,
        vec![ControlRequest::PaneSplit {
            workspace: ws,
            pane: 1,
            axis: TreeAxis::Horizontal,
            ratio: 0.4,
            new: seed(2),
            first: true,
        }]
    );
    assert_converged(&mirror, &want);
}

#[test]
fn closing_a_pane_emits_pane_close_and_the_split_collapses() {
    let ws = WorkspaceId::new();
    let id = TabId::new();
    let mut mirror = WsMirror::default();
    diff(
        ws,
        &mut mirror,
        &[tab(id, split(TreeAxis::Vertical, 0.5, leaf(1), leaf(2)))],
        Some(id),
        SyncScope::Full,
        &[],
    );

    let want = vec![tab(id, leaf(1))];
    let ops = diff(ws, &mut mirror, &want, Some(id), SyncScope::Full, &[]);
    assert_eq!(
        ops,
        vec![ControlRequest::PaneClose {
            workspace: ws,
            pane: 2
        }]
    );
    assert_converged(&mirror, &want);
}

#[test]
fn dragging_a_pane_across_the_layout_emits_one_pane_move() {
    let ws = WorkspaceId::new();
    let id = TabId::new();
    let mut mirror = WsMirror::default();
    // 1 | 2
    // ——+——
    //   3
    let grid = |a: DesiredNode, b: DesiredNode| split(TreeAxis::Vertical, 0.5, a, b);
    let before = vec![tab(
        id,
        grid(split(TreeAxis::Horizontal, 0.5, leaf(1), leaf(2)), leaf(3)),
    )];
    diff(ws, &mut mirror, &before, Some(id), SyncScope::Full, &[]);

    // 1 dropped below 3, which leaves 2 holding the top row alone.
    let after = vec![tab(
        id,
        grid(leaf(2), split(TreeAxis::Vertical, 0.5, leaf(3), leaf(1))),
    )];
    let ops = diff(ws, &mut mirror, &after, Some(id), SyncScope::Full, &[]);
    assert_eq!(
        ops,
        vec![ControlRequest::PaneMove {
            workspace: ws,
            pane: 1,
            to: 3,
            axis: TreeAxis::Vertical,
            first: false,
        }],
        "the tab is reshaped in place, not closed and rebuilt"
    );
    assert_converged(&mirror, &after);
}

#[test]
fn merging_a_tab_into_another_moves_its_panes_and_takes_the_tab_with_them() {
    let ws = WorkspaceId::new();
    let (host, guest) = (TabId::new(), TabId::new());
    let mut mirror = WsMirror::default();
    let before = vec![
        tab(host, leaf(1)),
        tab(guest, split(TreeAxis::Horizontal, 0.5, leaf(2), leaf(3))),
    ];
    diff(ws, &mut mirror, &before, Some(host), SyncScope::Full, &[]);

    // The guest tab dropped on the right of pane 1, arriving as the column
    // of two it already was.
    let after = vec![tab(
        host,
        split(
            TreeAxis::Horizontal,
            0.5,
            leaf(1),
            split(TreeAxis::Vertical, 0.5, leaf(2), leaf(3)),
        ),
    )];
    let ops = diff(ws, &mut mirror, &after, Some(host), SyncScope::Full, &[]);
    assert_eq!(
        ops,
        vec![
            ControlRequest::PaneMove {
                workspace: ws,
                pane: 2,
                to: 1,
                axis: TreeAxis::Horizontal,
                first: false,
            },
            ControlRequest::PaneMove {
                workspace: ws,
                pane: 3,
                to: 2,
                axis: TreeAxis::Vertical,
                first: false,
            },
        ],
        "the panes cross one at a time and the emptied tab goes with them, \
             rather than a tab's worth of panes being closed and respawned"
    );
    assert_converged(&mirror, &after);
    assert_eq!(mirror.active, Some(host));
}

#[test]
fn a_tab_grafted_above_a_whole_layout_still_converges() {
    let ws = WorkspaceId::new();
    let (host, guest) = (TabId::new(), TabId::new());
    let mut mirror = WsMirror::default();
    let before = vec![
        tab(host, split(TreeAxis::Horizontal, 0.5, leaf(1), leaf(2))),
        tab(guest, leaf(3)),
    ];
    diff(ws, &mut mirror, &before, Some(host), SyncScope::Full, &[]);

    // Dropped against the host's outer edge, so the newcomer sits above the
    // whole two-pane layout rather than beside one of its panes. No single
    // `PaneMove` can say that, and `migrate_panes` says nothing at all: the
    // passes after it have to land the tab anyway, by the rebuild they have
    // always fallen back to.
    let after = vec![tab(
        host,
        split(
            TreeAxis::Horizontal,
            0.33,
            leaf(3),
            split(TreeAxis::Horizontal, 0.5, leaf(1), leaf(2)),
        ),
    )];
    diff(ws, &mut mirror, &after, Some(host), SyncScope::Full, &[]);
    assert_converged(&mirror, &after);
}

#[test]
fn a_pane_leaving_for_a_tab_of_its_own_gives_it_up_before_it_asks_for_it() {
    let ws = WorkspaceId::new();
    let (held, fresh) = (TabId::new(), TabId::new());
    let mut mirror = WsMirror::default();
    let before = vec![tab(
        held,
        split(TreeAxis::Horizontal, 0.5, leaf(1), leaf(2)),
    )];
    diff(ws, &mut mirror, &before, Some(held), SyncScope::Full, &[]);

    // Pane 2 dropped on the strip ahead of the tab it came from, so the tab
    // it becomes is desired *first*.
    let after = vec![tab(fresh, leaf(2)), tab(held, leaf(1))];
    let ops = diff(ws, &mut mirror, &after, Some(fresh), SyncScope::Full, &[]);
    assert_eq!(
        ops,
        vec![
            ControlRequest::PaneClose {
                workspace: ws,
                pane: 2,
            },
            ControlRequest::TabCreate {
                workspace: ws,
                at: Some(0),
                pane: seed(2),
                tab: Some(fresh),
            },
        ],
        "the machine refuses a pane that is in two tabs at once, so the old \
             tab lets go before the new one is built"
    );
    assert_converged(&mirror, &after);
}

#[test]
fn a_move_that_lands_on_a_new_ratio_settles_it_after_the_move() {
    let ws = WorkspaceId::new();
    let id = TabId::new();
    let mut mirror = WsMirror::default();
    let before = vec![tab(
        id,
        split(
            TreeAxis::Vertical,
            0.5,
            split(TreeAxis::Horizontal, 0.5, leaf(1), leaf(2)),
            leaf(3),
        ),
    )];
    diff(ws, &mut mirror, &before, Some(id), SyncScope::Full, &[]);

    let after = vec![tab(
        id,
        split(
            TreeAxis::Vertical,
            0.5,
            leaf(2),
            split(TreeAxis::Vertical, 0.25, leaf(3), leaf(1)),
        ),
    )];
    let ops = diff(ws, &mut mirror, &after, Some(id), SyncScope::Full, &[]);
    assert_eq!(
        ops,
        vec![
            ControlRequest::PaneMove {
                workspace: ws,
                pane: 1,
                to: 3,
                axis: TreeAxis::Vertical,
                first: false,
            },
            ControlRequest::PaneSetRatio {
                workspace: ws,
                tab: id,
                path: vec![Side::B],
                ratio: 0.25,
            },
        ],
        "a move splits at a half, so a wanted ratio needs its own op"
    );
    assert_converged(&mirror, &after);
}

#[test]
fn a_swap_no_single_op_expresses_rebuilds_the_tab_whole() {
    let ws = WorkspaceId::new();
    let id = TabId::new();
    let mut mirror = WsMirror::default();
    let mut before = vec![tab(
        id,
        split(
            TreeAxis::Vertical,
            0.5,
            split(TreeAxis::Horizontal, 0.5, leaf(1), leaf(2)),
            split(TreeAxis::Horizontal, 0.5, leaf(3), leaf(4)),
        ),
    )];
    let mut original = TreeTab {
        root: before[0].root.to_pane_node(),
        ..TreeTab::leaf(1)
    };
    assert!(original.set_pip(Some(1), Some(2)));
    before[0].presentation = original.presentation.clone();
    diff(ws, &mut mirror, &before, Some(id), SyncScope::Full, &[]);

    // 1 and 4 trade corners: two panes moved, which no one op describes.
    let mut after = vec![tab(
        id,
        split(
            TreeAxis::Vertical,
            0.5,
            split(TreeAxis::Horizontal, 0.5, leaf(4), leaf(2)),
            split(TreeAxis::Horizontal, 0.5, leaf(3), leaf(1)),
        ),
    )];
    after[0].presentation = original.presentation.clone();
    let ops = diff(ws, &mut mirror, &after, Some(id), SyncScope::Full, &[]);
    assert!(
        matches!(ops.first(), Some(ControlRequest::TabClose { .. })),
        "expected the rebuild fallback, got {ops:?}"
    );
    assert!(
        matches!(ops.last(), Some(ControlRequest::PaneSetPresentation {
            pane: 1, pip: true, restore: Some(restore), ..
        }) if Some(restore) == original.presentation.pip())
    );
    assert_eq!(mirror.tabs[0].presentation, original.presentation);
    assert_converged(&mirror, &after);
}

#[test]
fn a_revived_leaf_emits_pane_replace_with_the_successors_seed() {
    let ws = WorkspaceId::new();
    let id = TabId::new();
    let mut mirror = WsMirror::default();
    diff(
        ws,
        &mut mirror,
        &[tab(id, split(TreeAxis::Vertical, 0.5, leaf(1), leaf(2)))],
        Some(id),
        SyncScope::Full,
        &[],
    );

    let want = vec![tab(id, split(TreeAxis::Vertical, 0.5, leaf(1), leaf(9)))];
    let ops = diff(ws, &mut mirror, &want, Some(id), SyncScope::Full, &[]);
    assert_eq!(
        ops,
        vec![ControlRequest::PaneReplace {
            workspace: ws,
            old: 2,
            new: seed(9),
        }]
    );
    assert_converged(&mirror, &want);
}

#[test]
fn a_ratio_drag_emits_set_ratio_with_the_splits_path() {
    let ws = WorkspaceId::new();
    let id = TabId::new();
    let mut mirror = WsMirror::default();
    let nested = |r| {
        split(
            TreeAxis::Vertical,
            0.5,
            leaf(1),
            split(TreeAxis::Horizontal, r, leaf(2), leaf(3)),
        )
    };
    diff(
        ws,
        &mut mirror,
        &[tab(id, nested(0.5))],
        Some(id),
        SyncScope::Full,
        &[],
    );

    let want = vec![tab(id, nested(0.7))];
    let ops = diff(ws, &mut mirror, &want, Some(id), SyncScope::Full, &[]);
    assert_eq!(
        ops,
        vec![ControlRequest::PaneSetRatio {
            workspace: ws,
            tab: id,
            path: vec![Side::B],
            ratio: 0.7,
        }]
    );
    assert_converged(&mirror, &want);
}

#[test]
fn closing_a_tab_emits_tab_close_and_heals_the_active_tab() {
    let ws = WorkspaceId::new();
    let (a, b) = (TabId::new(), TabId::new());
    let mut mirror = WsMirror::default();
    diff(
        ws,
        &mut mirror,
        &[tab(a, leaf(1)), tab(b, leaf(2))],
        Some(b),
        SyncScope::Full,
        &[],
    );

    let want = vec![tab(a, leaf(1))];
    let ops = diff(ws, &mut mirror, &want, None, SyncScope::Full, &[]);
    assert_eq!(
        ops,
        vec![ControlRequest::TabClose {
            workspace: ws,
            tab: b
        }],
        "the heal is the server's own rule, so no active op crosses"
    );
    assert_converged(&mirror, &want);
    assert_eq!(mirror.active, Some(a));
}

#[test]
fn a_tab_reorder_emits_moves_that_land_the_windows_order() {
    let ws = WorkspaceId::new();
    let (a, b, c) = (TabId::new(), TabId::new(), TabId::new());
    let mut mirror = WsMirror::default();
    let before = [tab(a, leaf(1)), tab(b, leaf(2)), tab(c, leaf(3))];
    diff(ws, &mut mirror, &before, Some(c), SyncScope::Full, &[]);

    let want = vec![tab(c, leaf(3)), tab(a, leaf(1)), tab(b, leaf(2))];
    let ops = diff(ws, &mut mirror, &want, Some(c), SyncScope::Full, &[]);
    assert_eq!(
        ops,
        vec![ControlRequest::TabMove {
            workspace: ws,
            tab: c,
            to: 0
        }]
    );
    assert_converged(&mirror, &want);
}

#[test]
fn renaming_and_regrouping_emit_their_label_ops() {
    let ws = WorkspaceId::new();
    let id = TabId::new();
    let mut mirror = WsMirror::default();
    diff(
        ws,
        &mut mirror,
        &[tab(id, leaf(1))],
        Some(id),
        SyncScope::Full,
        &[],
    );

    let mut named = tab(id, leaf(1));
    named.name = Some("build".into());
    let group = GroupId::new();
    named.group = Some(group);
    let want = vec![named];
    let ops = diff(ws, &mut mirror, &want, Some(id), SyncScope::Full, &[]);
    assert_eq!(
        ops,
        vec![
            ControlRequest::TabRename {
                workspace: ws,
                tab: id,
                name: Some("build".into()),
            },
            ControlRequest::TabSetGroup {
                workspace: ws,
                tab: id,
                group: Some(group),
                last_auto: None,
            },
        ]
    );
    assert_converged(&mirror, &want);
}

#[test]
fn switching_tabs_emits_only_set_active_tab() {
    let ws = WorkspaceId::new();
    let (a, b) = (TabId::new(), TabId::new());
    let mut mirror = WsMirror::default();
    let both = [tab(a, leaf(1)), tab(b, leaf(2))];
    diff(ws, &mut mirror, &both, Some(b), SyncScope::Full, &[]);

    let ops = diff(ws, &mut mirror, &both, Some(a), SyncScope::Full, &[]);
    assert_eq!(
        ops,
        vec![ControlRequest::WorkspaceSetActiveTab {
            workspace: ws,
            tab: a
        }]
    );
    assert_eq!(mirror.active, Some(a));
}

#[test]
fn a_deep_tree_materializes_top_split_first_and_converges() {
    let ws = WorkspaceId::new();
    let id = TabId::new();
    let mut mirror = WsMirror::default();
    let want = vec![tab(
        id,
        split(
            TreeAxis::Horizontal,
            0.6,
            split(TreeAxis::Vertical, 0.3, leaf(1), leaf(2)),
            split(TreeAxis::Vertical, 0.7, leaf(3), leaf(4)),
        ),
    )];
    let ops = diff(ws, &mut mirror, &want, Some(id), SyncScope::Full, &[]);
    assert_eq!(
        ops,
        vec![
            ControlRequest::TabCreate {
                workspace: ws,
                at: Some(0),
                pane: seed(1),
                tab: Some(id),
            },
            ControlRequest::PaneSplit {
                workspace: ws,
                pane: 1,
                axis: TreeAxis::Horizontal,
                ratio: 0.6,
                new: seed(3),
                first: false,
            },
            ControlRequest::PaneSplit {
                workspace: ws,
                pane: 1,
                axis: TreeAxis::Vertical,
                ratio: 0.3,
                new: seed(2),
                first: false,
            },
            ControlRequest::PaneSplit {
                workspace: ws,
                pane: 3,
                axis: TreeAxis::Vertical,
                ratio: 0.7,
                new: seed(4),
                first: false,
            },
        ]
    );
    assert_converged(&mirror, &want);
}

#[test]
fn an_unchanged_window_emits_nothing() {
    let ws = WorkspaceId::new();
    let id = TabId::new();
    let mut mirror = WsMirror::default();
    let want = vec![tab(id, split(TreeAxis::Vertical, 0.5, leaf(1), leaf(2)))];
    diff(ws, &mut mirror, &want, Some(id), SyncScope::Full, &[]);
    assert_eq!(
        diff(ws, &mut mirror, &want, Some(id), SyncScope::Full, &[]),
        Vec::new()
    );
}
