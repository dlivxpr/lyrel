use super::*;
use crate::ui::tree_sync::test_layout::{assert_converged, leaf, seed, split, tab};
#[test]
fn a_pane_id_reused_in_another_tab_is_never_read_as_a_replace() {
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

    let want = vec![tab(a, leaf(2)), tab(b, leaf(2))];
    let ops = diff(ws, &mut mirror, &want, Some(b), SyncScope::Full, &[]);
    assert!(
        !ops.iter()
            .any(|op| matches!(op, ControlRequest::PaneReplace { .. })),
        "got {ops:?}"
    );
}

fn asleep(id: TabId, root: DesiredNode) -> DesiredTab {
    DesiredTab {
        hibernated: true,
        ..tab(id, root)
    }
}

/// Going to sleep changes nothing about the tab's shape — the same panes,
/// by the same ids — so the only thing the machine hears is the mark. A
/// tab that lost its panes would have been a close, and the close would
/// have taken every record the wake needs with it.
#[test]
fn a_tab_going_to_sleep_says_so_and_nothing_else() {
    let ws = WorkspaceId::new();
    let id = TabId::new();
    let mut mirror = WsMirror::default();
    let root = || split(TreeAxis::Vertical, 0.5, leaf(1), leaf(2));
    diff(
        ws,
        &mut mirror,
        &[tab(id, root())],
        Some(id),
        SyncScope::Full,
        &[],
    );

    let ops = diff(
        ws,
        &mut mirror,
        &[asleep(id, root())],
        Some(id),
        SyncScope::Full,
        &[],
    );
    assert_eq!(
        ops,
        vec![ControlRequest::TabSetHibernated {
            workspace: ws,
            tab: id,
            hibernated: true,
        }]
    );
    assert!(mirror.tabs[0].hibernated);
    assert!(
        diff(
            ws,
            &mut mirror,
            &[asleep(id, root())],
            Some(id),
            SyncScope::Full,
            &[]
        )
        .is_empty(),
        "a tab that stays asleep is not told again"
    );
}

/// Waking swaps each stopped pane for its successor. The mark comes off
/// first, while the tab it belongs to is certainly still the one the
/// machine has.
#[test]
fn a_waking_tab_clears_its_mark_before_its_panes_are_replaced() {
    let ws = WorkspaceId::new();
    let id = TabId::new();
    let mut mirror = WsMirror::default();
    diff(
        ws,
        &mut mirror,
        &[asleep(id, leaf(1))],
        Some(id),
        SyncScope::Full,
        &[],
    );

    let want = vec![tab(id, leaf(9))];
    let ops = diff(ws, &mut mirror, &want, Some(id), SyncScope::Full, &[]);
    assert_eq!(
        ops,
        vec![
            ControlRequest::TabSetHibernated {
                workspace: ws,
                tab: id,
                hibernated: false,
            },
            ControlRequest::PaneReplace {
                workspace: ws,
                old: 1,
                new: seed(9),
            },
        ]
    );
    assert!(!mirror.tabs[0].hibernated);
    assert_converged(&mirror, &want);
}

/// A window writing a sleeping tab onto a machine that lost it — a store
/// wiped under a remote workspace — writes it back asleep, not as a tab
/// the next restore would spawn.
#[test]
fn a_tab_created_asleep_is_marked_as_it_is_created() {
    let ws = WorkspaceId::new();
    let id = TabId::new();
    let mut mirror = WsMirror::default();
    let ops = diff(
        ws,
        &mut mirror,
        &[asleep(id, leaf(4))],
        Some(id),
        SyncScope::Full,
        &[],
    );
    assert!(
        matches!(ops.first(), Some(ControlRequest::TabCreate { .. })),
        "{ops:?}"
    );
    assert!(
        ops.contains(&ControlRequest::TabSetHibernated {
            workspace: ws,
            tab: id,
            hibernated: true,
        }),
        "{ops:?}"
    );
    assert!(mirror.tabs[0].hibernated);
}
