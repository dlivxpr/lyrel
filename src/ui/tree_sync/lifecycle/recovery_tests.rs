use lyrel_core::core::machine::TabPresentation;

use super::super::*;
use super::*;
#[gpui_kit::test]
fn a_parked_folder_outlives_the_pull_that_has_to_be_retried(cx: &mut gpui_kit::TestAppContext) {
    // The folder an Explorer launch asked for is opened by whichever
    // attempt finally lands, so re-entering `hydrate` — which is how every
    // retry gets here — must not reset it along with the rest of the pull
    // state. Losing it would mean the double-clicked folder never opens.
    cx.update(|cx| {
        let ws = WorkspaceId::new();
        let path = std::path::PathBuf::from("/tmp/from-explorer");
        crate::core::session::WorkspaceStore::install_for_test(
            cx,
            crate::core::session::WindowViews::default(),
        );
        crate::ui::windows::WindowRegistry::init(cx);

        let parked = |cx: &mut gpui_kit::App| -> Vec<std::path::PathBuf> {
            cx.default_global::<TreeSync>().windows[&ws]
                .then_open
                .iter()
                .map(|open| match open {
                    ParkedOpen::Folder(cwd) => cwd.clone(),
                    ParkedOpen::Command { cwd, .. } => cwd.clone(),
                    ParkedOpen::Ssh(_) => std::path::PathBuf::from("<ssh>"),
                })
                .collect()
        };

        hydrate_window_then_open(cx, ws, path.clone());
        assert_eq!(
            parked(cx),
            vec![path.clone()],
            "the request must be parked, not opened over a layout still in flight"
        );

        hydrate_with(cx, ws, Adopt::IfEmpty, Vec::new());
        assert_eq!(
            parked(cx),
            vec![path.clone()],
            "a retry must still owe the folder"
        );

        // The command for the folder this window was opened for lands in
        // that parked tab rather than adding a second one beside it.
        assert!(park_command_while_pulling(cx, ws, &path, "./build.sh"));
        assert_eq!(parked(cx), vec![path.clone()]);
        assert!(matches!(
            &cx.default_global::<TreeSync>().windows[&ws].then_open[0],
            ParkedOpen::Command { command, .. } if command == "./build.sh"
        ));

        // A LaunchServices open for somewhere else queues behind it rather
        // than replacing it.
        let script = std::path::PathBuf::from("/tmp/from-finder");
        assert!(park_command_while_pulling(cx, ws, &script, "./deploy.sh"));
        assert_eq!(parked(cx), vec![path.clone(), script]);

        // With no window to put them in there is nothing to open, and the
        // requests must not survive to surface in some unrelated window.
        open_parked_path(cx, ws);
        assert!(parked(cx).is_empty());

        // A window whose layout has settled is opened into directly.
        cx.default_global::<TreeSync>()
            .windows
            .get_mut(&ws)
            .unwrap()
            .sync = SyncPhase::Primed(WsMirror::default());
        assert!(!park_command_while_pulling(cx, ws, &path, "./build.sh"));
    });
}

#[gpui_kit::test]
fn preemption_drops_the_mirror_the_queue_and_the_informed_licence(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(|cx| {
        let ws = WorkspaceId::new();
        {
            let state = cx
                .default_global::<TreeSync>()
                .windows
                .entry(ws)
                .or_default();
            state.sync = SyncPhase::Primed(WsMirror::default());
            state.informed = true;
            state.queue.push_back(ControlRequest::Ping);
        }
        on_preempted(cx, ws);
        let state = &cx.default_global::<TreeSync>().windows[&ws];
        assert!(matches!(
            state.sync,
            SyncPhase::Unprimed {
                dirty: false,
                priming: false,
            }
        ));
        assert!(
            state.queue.is_empty(),
            "queued ops belong to the lost session"
        );
        assert!(
            !state.informed,
            "the licence to prune must not survive a takeover"
        );
    });
}

/// The destructive half of a deletion. It erases state only this client
/// holds — geometry, the label, a remote binding — so the fence in front of
/// it ("no window is showing this workspace") is the whole safety of it.
///
/// The other half needs a live `LyrelApp` in a real window to reach, so it
/// is not tested here; what it does is hydrate, which the hydration tests
/// cover, and it touches neither the store nor the registry.
#[gpui_kit::test]
fn a_deletion_nothing_has_open_forgets_the_workspace_here_too(cx: &mut gpui_kit::TestAppContext) {
    use crate::core::session::{WindowView, WindowViews};

    cx.update(|cx| {
        // Removing a workspace saves the views, and a test has no business
        // writing the real ones.
        let _ = lyrel_core::core::config::set_config_dir(
            std::env::temp_dir().join(format!("lyrel-deleted-test-{}", std::process::id())),
        );
        crate::ui::windows::WindowRegistry::init(cx);

        let deleted = WindowView::default();
        let gone = deleted.id;
        let untouched = WindowView::default();
        let survivor = untouched.id;
        WorkspaceStore::install_for_test(
            cx,
            WindowViews {
                views: vec![deleted, untouched],
                active: Some(gone),
            },
        );
        cx.default_global::<TreeSync>()
            .windows
            .entry(gone)
            .or_default()
            .sync = SyncPhase::Primed(WsMirror::default());

        on_workspace_deleted(cx, gone);

        assert!(
            WorkspaceStore::all(cx).get(gone).is_none(),
            "a row that opens onto nothing is worse than no row at all"
        );
        assert_eq!(
            WorkspaceStore::all(cx).active,
            None,
            "the active workspace cannot be one that no longer exists"
        );
        assert!(
            WorkspaceStore::all(cx).get(survivor).is_some(),
            "a deletion is about one workspace, not about the store"
        );
        assert!(
            !cx.default_global::<TreeSync>().windows.contains_key(&gone),
            "its sync state has nothing left to be about"
        );
    });
}

/// The rule that stops a failed rebuild from being read as "empty".
///
/// A window with no tabs may delete its workspace outright — tree and store
/// both — so the two ways of having no tabs must not look alike. Genuinely
/// empty is a reason; "the panes would not start" is not, and it is what
/// every tab looks like when the pane socket has gone away.
#[gpui_kit::test]
fn only_a_mirror_that_agrees_lets_an_empty_window_delete_its_workspace(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(|cx| {
        let ws = WorkspaceId::new();
        let set = |cx: &mut App, informed: bool, sync: SyncPhase| {
            let state = cx
                .default_global::<TreeSync>()
                .windows
                .entry(ws)
                .or_default();
            state.informed = informed;
            state.sync = sync;
        };
        let unprimed = || SyncPhase::Unprimed {
            dirty: false,
            priming: false,
        };
        let primed_with = |tabs: Vec<TreeTab>| {
            SyncPhase::Primed(WsMirror {
                tabs,
                ..Default::default()
            })
        };
        let a_tab = || TreeTab {
            id: TabId::new(),
            name: None,
            group: None,
            last_auto: None,
            presentation: TabPresentation::default(),
            root: PaneNode::Leaf { pane: 1 },
            hibernated: false,
        };

        assert!(
            !workspace_is_disposable(cx, WorkspaceId::new()),
            "a workspace nothing is tracking is not a workspace to delete"
        );

        set(cx, true, unprimed());
        assert!(
            !workspace_is_disposable(cx, ws),
            "an unpulled mirror knows nothing, and not knowing must never authorize this"
        );

        set(cx, true, primed_with(vec![a_tab()]));
        assert!(
            !workspace_is_disposable(cx, ws),
            "this is the regression: the window came up empty because the rebuild failed, \
                 while the machine still held the tabs. Deleting here destroyed them."
        );

        set(cx, false, primed_with(vec![]));
        assert!(
            !workspace_is_disposable(cx, ws),
            "a window that never put up a layout does not get to say what belongs here"
        );

        set(cx, true, primed_with(vec![]));
        assert!(
            workspace_is_disposable(cx, ws),
            "informed, and the machine agrees it holds nothing — the one case that is"
        );
    });
}

/// #554: Restart Server empties the window *before* it tries the handoff,
/// and a refused handoff leaves the daemon exactly as it was, still serving
/// every pane the window just dropped.
///
/// What makes that recoverable is the window giving up its claim to speak
/// for the machine the moment the failure lands. An emptied window that is
/// still `informed` over a `Primed` mirror is the dangerous shape: the next
/// sync diffs it into "close every tab", and closing the window authorizes
/// a `WorkspaceRemove` outright — the shells stay alive, but their records
/// go, and nothing tree-driven can ever reach them again.
///
/// Tested on `resync_window_from_tree`, which is the per-window step both
/// [`resync_after_local_daemon_change`] and [`resync_local_windows_from_tree`]
/// do their work through — the pair above it only decides whether the link
/// is dropped first and which windows are walked, neither of which is what
/// keeps the emptied window quiet.
#[gpui_kit::test]
fn a_failed_daemon_change_stops_the_emptied_window_speaking_for_the_machine(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(|cx| {
        let ws = WorkspaceId::new();
        let tab = TabId::new();
        {
            let state = cx
                .default_global::<TreeSync>()
                .windows
                .entry(ws)
                .or_default();
            state.informed = true;
            // The mirror the emptied window would have been diffed against,
            // already drained the way `save_session` drains it on the way
            // out of a window that has no tabs left.
            state.sync = SyncPhase::Primed(WsMirror::default());
            state
                .queue
                .push_back(ControlRequest::TabClose { workspace: ws, tab });
        }
        assert!(
            workspace_is_disposable(cx, ws),
            "the shape #554 leaves behind: an emptied window that still speaks for a \
                 machine holding live panes, and may delete the workspace off it"
        );

        resync_window_from_tree(cx, ws);

        assert!(
            !workspace_is_disposable(cx, ws),
            "this is the regression: closing the window after a refused handoff sent \
                 WorkspaceRemove and took the whole workspace, live panes and all"
        );
        let state = &cx.default_global::<TreeSync>().windows[&ws];
        assert!(
            matches!(state.sync, SyncPhase::Unprimed { .. }),
            "the mirror the emptied window would diff into 'close every tab' has to be \
                 dropped, not carried into the next sync"
        );
        assert!(
            state.queue.is_empty(),
            "operations queued from the emptied window speak for a layout that is being \
                 pulled again; sending them would close the tabs it is pulling"
        );
    });
}

/// #579: the other half of an abandoned `Replace`. Dropping the retry is a
/// decision about what to *put up*; it says nothing about the window having
/// learned what belongs in the workspace, and it never did learn — the pull
/// it was told to make failed.
///
/// Left `informed`, that window went on to diff at `SyncScope::Full`
/// against a mirror `start_prime` had just refilled from the machine, so
/// the one tab the user opened over an emptied window emitted `TabClose`
/// for every other tab on it. Those panes' records went with them while
/// their shells kept running, and nothing tree-driven could reach them
/// again — the same damage as #554, through a door #554 did not close.
#[gpui_kit::test]
fn a_resync_takes_back_the_licence_to_prune_until_a_pull_lands(cx: &mut gpui_kit::TestAppContext) {
    cx.update(|cx| {
        let ws = WorkspaceId::new();
        {
            let state = cx
                .default_global::<TreeSync>()
                .windows
                .entry(ws)
                .or_default();
            state.informed = true;
        }

        resync_window_from_tree(cx, ws);
        assert!(
            !cx.default_global::<TreeSync>().windows[&ws].informed,
            "a window told its contents are not the workspace cannot still speak for it"
        );

        // The shape the damage needs: the mirror back from the machine,
        // full of the tabs the window never adopted. Without the licence
        // the sync is additive, which
        // `an_additive_diff_never_closes_tabs_the_window_has_not_seen`
        // pins as closing nothing.
        {
            let state = cx
                .default_global::<TreeSync>()
                .windows
                .get_mut(&ws)
                .unwrap();
            state.rehydrate = None;
            state.sync = SyncPhase::Primed(WsMirror {
                tabs: vec![TreeTab::leaf(1), TreeTab::leaf(2)],
                ..Default::default()
            });
        }
        assert!(
            !workspace_is_disposable(cx, ws),
            "and it cannot delete the workspace out from under those panes either"
        );

        // A pull that lands is the only thing that hands the licence back.
        mark_window_informed(cx, ws);
        assert!(cx.default_global::<TreeSync>().windows[&ws].informed);
    });
}

/// An `IfEmpty` hydration says the opposite of a `Replace`: keep whatever
/// the window has. A window that opened onto a workspace deleted under it
/// is put back by writing its own tabs up, and needs the licence to do it.
#[gpui_kit::test]
fn an_if_empty_pull_leaves_the_licence_where_it_found_it(cx: &mut gpui_kit::TestAppContext) {
    cx.update(|cx| {
        let ws = WorkspaceId::new();
        cx.default_global::<TreeSync>()
            .windows
            .entry(ws)
            .or_default()
            .informed = true;
        hydrate_window_from_tree(cx, ws);
        assert!(cx.default_global::<TreeSync>().windows[&ws].informed);
    });
}

/// #672's residual: a rebuild that put up some of the tabs the tree asked
/// for and dropped the rest (`tabs_from_session` drops a tab none of whose
/// panes would start). It earns the licence — it did put a layout up — and
/// the tabs it dropped are not tabs the user closed, so they must be held
/// out of the diff rather than the licence withheld from the whole window.
#[gpui_kit::test]
fn a_partial_rebuild_holds_the_tabs_it_could_not_put_up(cx: &mut gpui_kit::TestAppContext) {
    cx.update(|cx| {
        let ws = WorkspaceId::new();
        let (put_up, failed) = (TabId::new(), TabId::new());
        let settled = |cx: &mut App| {
            let state = &cx.default_global::<TreeSync>().windows[&ws];
            (state.informed, state.not_rebuilt.clone())
        };

        settle_rebuild(cx, ws, 2, &[put_up, failed], &[put_up]);
        assert_eq!(
            settled(cx),
            (true, vec![failed]),
            "the window speaks for the tab it put up, and holds the one it could not"
        );

        settle_rebuild(cx, ws, 2, &[put_up, failed], &[put_up, failed]);
        assert_eq!(
            settled(cx),
            (true, vec![]),
            "a later rebuild that puts them all up holds nothing back"
        );

        cx.default_global::<TreeSync>()
            .windows
            .get_mut(&ws)
            .unwrap()
            .informed = false;
        settle_rebuild(cx, ws, 2, &[put_up, failed], &[]);
        assert_eq!(
            settled(cx),
            (false, vec![put_up, failed]),
            "a rebuild that produced nothing still does not get to speak for the workspace"
        );

        settle_rebuild(cx, ws, 0, &[], &[]);
        assert_eq!(
            settled(cx),
            (true, vec![]),
            "an empty tree rebuilt into an empty window is the one case that is genuinely empty"
        );

        // A session with tabs but no ids — what a disk-loaded session
        // looks like, since `tree_id` is not serialized — that rebuilt
        // nothing. "No ids" must not read as "no tabs wanted".
        cx.default_global::<TreeSync>()
            .windows
            .get_mut(&ws)
            .unwrap()
            .informed = false;
        settle_rebuild(cx, ws, 2, &[], &[]);
        assert_eq!(
            settled(cx),
            (false, vec![]),
            "the guard counts the tabs asked for, not the ids found"
        );
    });
}

/// The consequence, on the sync that follows: the mirror holds both tabs,
/// the window shows one, and the one it could not put up must not come out
/// of a `Full` diff as `TabClose` — that op deleted from the machine exactly
/// the tabs a restart had failed to bring back, panes and all (#672).
#[gpui_kit::test]
fn the_next_sync_leaves_a_tab_the_rebuild_could_not_put_up_on_the_machine(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (app, mut vcx, _pane_stream) = crate::ui::app::test_window::harness_with_pane(cx);
    let (put_up, failed) = (TabId::new(), TabId::new());
    app.update_in(&mut vcx, |app, _, cx| {
        let view = crate::core::session::WindowView::default();
        let ws = view.id;
        WorkspaceStore::install_for_test(
            cx,
            crate::core::session::WindowViews {
                views: vec![view],
                active: Some(ws),
            },
        );
        app.workspace = ws;
        let pane = std::mem::replace(&mut app.tabs[0].pane, crate::ui::pane::Pane::Empty);
        app.tabs[0] = crate::ui::tab::Tab::from_foreign(put_up, None, pane);
        {
            let state = cx
                .default_global::<TreeSync>()
                .windows
                .entry(ws)
                .or_default();
            state.sync = SyncPhase::Primed(WsMirror {
                tabs: vec![
                    TreeTab {
                        id: put_up,
                        name: None,
                        group: None,
                        last_auto: None,
                        presentation: TabPresentation::default(),
                        root: PaneNode::Leaf { pane: 1 },
                        hibernated: false,
                    },
                    TreeTab {
                        id: failed,
                        name: None,
                        group: None,
                        last_auto: None,
                        presentation: TabPresentation::default(),
                        root: PaneNode::Leaf { pane: 2 },
                        hibernated: false,
                    },
                ],
                active: Some(put_up),
                ..Default::default()
            });
            // Keeps whatever the sync queues where the test can read it:
            // with no link, `pump` would otherwise clear the queue and drop
            // the mirror on its way to a re-pull.
            state.inflight = true;
        }
        settle_rebuild(cx, ws, 2, &[put_up, failed], &[put_up]);
        assert!(
            cx.default_global::<TreeSync>().windows[&ws].informed,
            "the shape the damage needs: a window licensed to prune, over a mirror \
                 holding a tab it is not showing"
        );

        sync_window(app, cx);

        let state = &cx.default_global::<TreeSync>().windows[&ws];
        assert!(
            !state
                .queue
                .iter()
                .any(|op| matches!(op, ControlRequest::TabClose { .. })),
            "the tab that failed to come back is not one the user closed: {:?}",
            state.queue
        );
        match &state.sync {
            SyncPhase::Primed(mirror) => assert_eq!(
                mirror.tabs.iter().map(|t| t.id).collect::<Vec<_>>(),
                vec![put_up, failed],
                "and it stays on the machine for the next rebuild to put up"
            ),
            SyncPhase::Unprimed { .. } => {
                panic!("the sync must not have thrown the mirror away")
            }
        }
    });
}

/// #716: switching into a workspace put its empty session up and saved
/// it — a window showing nothing, syncing at Full scope against the
/// mirror and licence the last visit left behind, which closes every tab
/// on the machine before the pull that would have populated the window
/// has even been ordered. Arriving must speak for nothing.
///
/// The licence is what the assertion holds. The closes it authorises are
/// queued and pumped inside `adopt_workspace`, and the hydrate ordered
/// straight after clears the queue, so by the time a test can look the
/// ops are gone either way — while `informed` outliving the arrival is
/// both durable and the thing that made them possible.
#[gpui_kit::test]
fn arriving_at_a_workspace_does_not_prune_what_is_already_in_it(cx: &mut gpui_kit::TestAppContext) {
    let (app, mut vcx, _pane_stream) = crate::ui::app::test_window::harness_with_pane(cx);
    let theirs = (TabId::new(), TabId::new());
    app.update_in(&mut vcx, |app, window, cx| {
        crate::ui::windows::WindowRegistry::init(cx);
        let here = crate::core::session::WindowView::default();
        let there = crate::core::session::WindowView::default();
        let (here_id, there_id) = (here.id, there.id);
        WorkspaceStore::install_for_test(
            cx,
            crate::core::session::WindowViews {
                views: vec![here, there],
                active: Some(here_id),
            },
        );
        app.workspace = here_id;

        // The workspace being switched to, as an earlier visit left it:
        // primed with the tabs it holds, and licensed to prune them.
        {
            let state = cx
                .default_global::<TreeSync>()
                .windows
                .entry(there_id)
                .or_default();
            state.sync = SyncPhase::Primed(WsMirror {
                tabs: vec![
                    TreeTab {
                        id: theirs.0,
                        name: None,
                        group: None,
                        last_auto: None,
                        presentation: TabPresentation::default(),
                        root: PaneNode::Leaf { pane: 11 },
                        hibernated: false,
                    },
                    TreeTab {
                        id: theirs.1,
                        name: None,
                        group: None,
                        last_auto: None,
                        presentation: TabPresentation::default(),
                        root: PaneNode::Leaf { pane: 12 },
                        hibernated: false,
                    },
                ],
                active: Some(theirs.0),
                ..Default::default()
            });
            state.informed = true;
            // Keeps whatever the switch queues where the test can read it.
            state.inflight = true;
        }

        app.switch_workspace(Some(there_id), window, cx);

        let state = &cx.default_global::<TreeSync>().windows[&there_id];
        assert!(
            !state.informed,
            "a window that has just arrived speaks for nothing in the workspace \
                 until its own pull lands — least of all that it is empty"
        );
        assert!(
            !state
                .queue
                .iter()
                .any(|op| matches!(op, ControlRequest::TabClose { .. })),
            "and it closes nothing it never showed: {:?}",
            state.queue
        );
    });
}
