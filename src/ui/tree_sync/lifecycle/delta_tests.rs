use super::*;
use super::super::*;
use super::super::test_layout::{leaf, split, tab};
    #[test]
    fn a_tab_created_delta_that_straddled_a_repull_lands_once_in_the_window_mirror() {
        let mut mirror = WsMirror::default();
        let delta = LayoutDelta::TabCreated {
            at: 0,
            tab: TreeTab::leaf(1),
        };
        assert!(apply_to_mirror(&mut mirror, &delta));
        assert!(apply_to_mirror(&mut mirror, &delta));
        assert_eq!(mirror.tabs.len(), 1);
    }


    #[test]
    fn deltas_advance_the_mirror_exactly_as_the_writers_operations_did() {
        let ws = WorkspaceId::new();
        let id = TabId::new();
        let mut watcher = WsMirror::default();

        let tree_tab = TreeTab {
            id,
            name: None,
            group: None,
            last_auto: None,
            presentation: TabPresentation::default(),
            root: PaneNode::Leaf { pane: 1 },
            hibernated: false,
        };
        assert!(apply_to_mirror(
            &mut watcher,
            &LayoutDelta::TabCreated {
                at: 0,
                tab: tree_tab,
            },
        ));
        assert!(apply_to_mirror(
            &mut watcher,
            &LayoutDelta::ActiveTabChanged { tab: id },
        ));
        assert!(apply_to_mirror(
            &mut watcher,
            &LayoutDelta::TabRestructured {
                tab: TreeTab {
                    id,
                    name: None,
                    group: None,
                    last_auto: None,
                    presentation: TabPresentation::default(),
                    root: PaneNode::Split {
                        axis: TreeAxis::Vertical,
                        ratio: 0.5,
                        a: Box::new(PaneNode::Leaf { pane: 1 }),
                        b: Box::new(PaneNode::Leaf { pane: 2 }),
                    },
                    hibernated: false,
                },
                pane: None,
            },
        ));
        assert!(apply_to_mirror(
            &mut watcher,
            &LayoutDelta::RatioChanged {
                tab: id,
                path: Vec::new(),
                ratio: 0.7,
            },
        ));

        let mut writer = WsMirror::default();
        diff(
            ws,
            &mut writer,
            &[tab(id, leaf(1))],
            Some(id),
            SyncScope::Full,
            &[],
        );
        let final_state = vec![tab(id, split(TreeAxis::Vertical, 0.7, leaf(1), leaf(2)))];
        diff(
            ws,
            &mut writer,
            &final_state,
            Some(id),
            SyncScope::Full,
            &[],
        );

        assert_eq!(watcher, writer);
    }

    #[test]
    fn a_delta_about_a_tab_the_mirror_does_not_hold_reports_itself() {
        let mut mirror = WsMirror::default();
        assert!(
            !apply_to_mirror(
                &mut mirror,
                &LayoutDelta::TabRenamed {
                    tab: TabId::new(),
                    name: Some("x".into()),
                },
            ),
            "an unappliable delta must say so, so the caller re-pulls"
        );
        assert!(!apply_to_mirror(
            &mut mirror,
            &LayoutDelta::TabClosed {
                tab: TabId::new(),
                reason: lyrel_core::core::machine::TabCloseReason::Explicit
            },
        ),);
    }
