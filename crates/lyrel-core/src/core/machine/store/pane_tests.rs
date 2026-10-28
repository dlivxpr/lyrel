use super::super::test_support::*;
use super::*;
use crate::core::machine::{Axis, PaneNode, PaneSeed, Side, TabCloseReason};

#[test]
fn a_seed_for_an_already_dead_pane_registers_as_awaiting_revival() {
    let (store, _dir) = store();
    store.set_liveness_probe(Arc::new(|id| id == 1));
    let ws = store.workspace_create(None, None, None).unwrap();
    store
        .tab_create(ws.id, None, PaneSeed::bare(1), None, None)
        .unwrap();
    store
        .pane_split(
            ws.id,
            1,
            Axis::Vertical,
            0.5,
            PaneSeed::bare(2),
            false,
            None,
        )
        .unwrap();

    assert!(store.pane(1).unwrap().live, "the probe vouched for pane 1");
    assert!(
        !store.pane(2).unwrap().live,
        "pane 2 died before its adopting op; its record must be born revivable"
    );
}

#[test]
fn replacing_a_dead_pane_rebinds_the_leaf_and_spends_the_record() {
    let (store, dir) = store();
    let ws = store.workspace_create(None, None, None).unwrap();
    store
        .tab_create(ws.id, None, seed(7, "/work"), None, None)
        .unwrap();

    let restarted = MachineStore::open(dir.path().join(MACHINE_FILE));
    let (_sub, heard) = recorded(&restarted);
    restarted
        .pane_replace(ws.id, 7, seed(42, "/work"), None)
        .unwrap();

    assert_eq!(
        restarted.workspace(ws.id).unwrap().tabs[0].root.pane_ids(),
        vec![42]
    );
    assert!(restarted.pane(7).is_none(), "the old record is spent");
    assert!(restarted.pane(42).unwrap().live);
    let heard = heard.lock().unwrap();
    assert_eq!(heard.len(), 1);
    match &heard[0].1 {
        LayoutDelta::TabRestructured { tab, pane } => {
            assert_eq!(tab.root.pane_ids(), vec![42]);
            assert_eq!(pane.as_ref().map(|p| p.id), Some(42));
        }
        other => panic!("expected TabRestructured, got {other:?}"),
    }
}

#[test]
fn splitting_and_closing_panes_reshapes_the_tree() {
    let (store, _dir, ws, tab) = store_with_tab();
    store
        .pane_split(ws, 1, Axis::Horizontal, 0.5, seed(2, "/b"), false, None)
        .unwrap();
    store
        .pane_split(ws, 2, Axis::Vertical, 0.5, seed(3, "/c"), true, None)
        .unwrap();
    assert_eq!(
        store.workspace(ws).unwrap().tabs[0].root.pane_ids(),
        vec![1, 3, 2],
        "`first` puts the new pane on the a side"
    );

    let dropped = store.pane_close(ws, 3, None).unwrap();
    assert_eq!(dropped, vec![3]);
    assert_eq!(
        store.workspace(ws).unwrap().tabs[0].root.pane_ids(),
        vec![1, 2]
    );

    store.pane_close(ws, 2, None).unwrap();
    assert!(matches!(
        store.workspace(ws).unwrap().tabs[0].root,
        PaneNode::Leaf { pane: 1 }
    ));

    let (_sub, heard) = recorded(&store);
    store.pane_close(ws, 1, None).unwrap();
    assert!(store.workspace(ws).unwrap().tabs.is_empty());
    assert!(matches!(
        heard.lock().unwrap()[0].1,
        LayoutDelta::TabClosed { tab: id, reason: TabCloseReason::Emptied } if id == tab.id
    ));
}

#[test]
fn a_ratio_change_lands_on_the_split_its_path_names() {
    let (store, _dir, ws, tab) = store_with_tab();
    store
        .pane_split(ws, 1, Axis::Horizontal, 0.5, seed(2, "/b"), false, None)
        .unwrap();
    store
        .pane_split(ws, 2, Axis::Vertical, 0.5, seed(3, "/c"), false, None)
        .unwrap();

    store
        .pane_set_ratio(ws, tab.id, vec![Side::B], 0.7, None)
        .unwrap();
    match &store.workspace(ws).unwrap().tabs[0].root {
        PaneNode::Split { a, b, ratio, .. } => {
            assert!((ratio - 0.5).abs() < 1e-6, "the root ratio is untouched");
            assert!(matches!(&**a, PaneNode::Leaf { pane: 1 }));
            match &**b {
                PaneNode::Split { ratio, .. } => assert!((ratio - 0.7).abs() < 1e-6),
                PaneNode::Leaf { .. } => panic!("the nested split is gone"),
            }
        }
        PaneNode::Leaf { .. } => panic!("the root split is gone"),
    }

    let err = store
        .pane_set_ratio(ws, tab.id, vec![Side::A], 0.6, None)
        .unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);

    store
        .pane_set_ratio(ws, tab.id, vec![], 0.0001, None)
        .unwrap();
    match &store.workspace(ws).unwrap().tabs[0].root {
        PaneNode::Split { ratio, .. } => assert!(*ratio >= 0.05),
        PaneNode::Leaf { .. } => unreachable!(),
    }
}

#[test]
fn moving_a_pane_between_tabs_dissolves_an_emptied_tab() {
    let (store, _dir, ws, first) = store_with_tab();
    let second = store
        .tab_create(ws, None, seed(2, "/b"), None, None)
        .unwrap();

    let (_sub, heard) = recorded(&store);
    store
        .pane_move(ws, 2, 1, Axis::Vertical, false, None)
        .unwrap();
    let workspace = store.workspace(ws).unwrap();
    assert_eq!(workspace.tabs.len(), 1, "the emptied tab dissolved");
    assert_eq!(workspace.tabs[0].id, first.id);
    assert_eq!(workspace.tabs[0].root.pane_ids(), vec![1, 2]);
    assert!(
        !workspace.tabs.iter().any(|t| t.id == second.id),
        "the source tab is gone"
    );
    assert!(store.pane(2).is_some(), "the pane moved; it did not die");
    assert!(
        matches!(heard.lock().unwrap()[0].1, LayoutDelta::TabClosed { tab, reason: TabCloseReason::Emptied } if tab == second.id)
    );

    let err = store
        .pane_move(ws, 2, 2, Axis::Vertical, false, None)
        .unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn a_pane_already_in_the_tree_cannot_be_adopted_again() {
    let (store, _dir, ws, _tab) = store_with_tab();
    store.note_pane_facts(1, |p| p.cwd = Some("/observed".into()));

    let other = store.workspace_create(None, None, None).unwrap();
    let err = store
        .tab_create(other.id, None, seed(1, "/stale"), None, None)
        .unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    assert!(
        store.workspace(other.id).unwrap().tabs.is_empty(),
        "the refused tab must not half-exist"
    );
    assert_eq!(
        store.pane(1).unwrap().cwd.as_deref(),
        Some("/observed"),
        "and the stale seed must not clobber the daemon's own facts"
    );
    let _ = ws;
}
