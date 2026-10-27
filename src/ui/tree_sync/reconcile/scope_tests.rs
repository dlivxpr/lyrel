use super::*;
use crate::ui::tree_sync::test_layout::{leaf, seed, tab};
    /// A sync closes only what the user closed here. Restart Server empties
    /// the window and waits out the handoff while deltas keep arriving, each
    /// ending in a `Full` sync of no tabs; reading the absence as a close sent
    /// `TabClose` for all sixteen tabs of a workspace and orphaned their shells.
    #[test]
    fn a_sync_closes_only_the_tabs_the_user_closed() {
        let ws = WorkspaceId::new();
        let tab = |pane| TreeTab {
            id: TabId::new(),
            name: None,
            group: None,
            last_auto: None,
            presentation: TabPresentation::default(),
            root: PaneNode::Leaf { pane },
            hibernated: false,
        };
        let mirror = WsMirror {
            tabs: vec![tab(1), tab(2)],
            ..Default::default()
        };
        let (a, b) = (mirror.tabs[0].id, mirror.tabs[1].id);
        let sync = |closing: &mut Vec<TabId>| {
            let mut mirror = mirror.clone();
            let held = unaccounted(&mirror, &[], closing);
            diff(ws, &mut mirror, &[], None, SyncScope::Full, &held)
        };

        assert!(
            sync(&mut Vec::new()).is_empty(),
            "this is the regression: an emptied window closed every tab on the machine"
        );

        let mut closing = vec![b];
        assert_eq!(
            sync(&mut closing),
            vec![ControlRequest::TabClose {
                workspace: ws,
                tab: b
            }],
            "the tab the user closed goes; the one merely missing stays"
        );
        assert_ne!(a, b);

        let mut closing = vec![TabId::new(), b];
        let _ = sync(&mut closing);
        assert_eq!(
            closing,
            vec![b],
            "an intent the machine has no tab for is spent; one it still has stays until it lands"
        );
    }


    #[test]
    fn a_tab_whose_panes_are_all_still_spawning_is_held_not_closed() {
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

        let ops = diff(ws, &mut mirror, &[], None, SyncScope::Full, &[id]);
        assert_eq!(ops, Vec::new());
        assert_eq!(mirror.tabs.len(), 1, "the daemon tab survives the wait");
    }

    #[test]
    fn an_additive_diff_never_closes_tabs_the_window_has_not_seen() {
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

        let fresh = TabId::new();
        let ops = diff(
            ws,
            &mut mirror,
            &[tab(fresh, leaf(9))],
            Some(fresh),
            SyncScope::Additive,
            &[],
        );
        assert_eq!(
            ops,
            vec![ControlRequest::TabCreate {
                workspace: ws,
                at: Some(2),
                pane: seed(9),
                tab: Some(fresh),
            }],
            "appended after the tabs it has not seen; nothing closed or moved"
        );
        assert_eq!(mirror.tabs.len(), 3);
    }

