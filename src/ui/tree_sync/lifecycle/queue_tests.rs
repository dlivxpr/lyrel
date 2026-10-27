use super::*;
use super::super::*;

/// The predecessor's refused operations must not clear work queued for a new
/// hydration or a fresh visit to the same workspace.
#[gpui_kit::test]
fn a_retired_batch_cannot_discard_successor_window_edits(cx: &mut gpui_kit::TestAppContext) {
    let (app, mut vcx, _stream) = crate::ui::app::test_window::harness_with_pane(cx);
    for revisit in [false, true] {
        let (ws, tab, batch) = app.update_in(&mut vcx, |app, _, cx| {
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
            let tab = app.tabs[0].tree_id.get();
            let mut machine_tab = TreeTab::leaf(1);
            machine_tab.id = tab;
            prime_for_test(cx, ws, vec![machine_tab]);
            app.tabs[0].name = Some("predecessor edit".into());
            sync_window(app, cx);
            let state = cx.default_global::<TreeSync>().windows.get_mut(&ws).unwrap();
            state.inflight = false;
            (ws, tab, state.begin_batch().expect("the window queued its edit"))
        });
        app.update_in(&mut vcx, |app, _, cx| {
            if revisit {
                forget(cx, ws);
            } else {
                cx.default_global::<TreeSync>().windows.get_mut(&ws).unwrap()
                    .begin_hydration(Adopt::Replace, vec![tab]);
            }
            let mut machine_tab = TreeTab::leaf(1);
            machine_tab.id = tab;
            prime_for_test(cx, ws, vec![machine_tab]);
            app.tabs[0].name = Some("successor edit".into());
            sync_window(app, cx);
        });
        vcx.update(|_, cx| {
            let refused = batch.ops[0].clone();
            apply_batch_result(
                cx,
                ws,
                batch.epoch,
                batch.retired,
                Err((refused, io::Error::other("retired link"), batch.ops)),
            );
            assert!(queued_for_test(cx, ws).iter().any(|op| {
                matches!(op, ControlRequest::TabRename { tab: id, name, .. }
                    if *id == tab && name.as_deref() == Some("successor edit"))
            }), "a late refusal must leave the successor's queued rename intact");
        });
    }
}
    fn retiring(tab: TabId, panes: Vec<u64>) -> Retiring {
        Retiring {
            tab,
            panes,
            route: crate::terminal::PaneRoute::Local,
        }
    }

    /// Only the close the user asked for is remembered. The diff says
    /// `TabClose` for every tab gone from the window — moved to another window,
    /// never rebuilt — and those stay plain closes.
    #[test]
    fn only_a_retiring_tab_is_closed_into_the_machines_list() {
        let workspace = WorkspaceId::new();
        let (closed, moved) = (TabId::new(), TabId::new());
        let mut ops = vec![
            ControlRequest::TabClose {
                workspace,
                tab: moved,
            },
            ControlRequest::TabClose {
                workspace,
                tab: closed,
            },
        ];
        let mut waiting = vec![retiring(closed, vec![3, 4])];

        let sent = remember_closes(&mut ops, &mut waiting);

        assert_eq!(
            ops,
            vec![
                ControlRequest::TabClose {
                    workspace,
                    tab: moved,
                },
                ControlRequest::TabCloseRemembered {
                    workspace,
                    tab: closed,
                    panes: vec![3, 4],
                },
            ]
        );
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].tab, closed);
        assert!(
            waiting.is_empty(),
            "handed on, so nothing is left to fall back on"
        );
    }

    /// A retirement the sync did not reach stays waiting, for the close to
    /// take back and end the old way.
    #[test]
    fn a_retirement_with_no_close_in_the_sync_stays_waiting() {
        let workspace = WorkspaceId::new();
        let mut ops = vec![ControlRequest::TabMove {
            workspace,
            tab: TabId::new(),
            to: 0,
        }];
        let tab = TabId::new();
        let mut waiting = vec![retiring(tab, vec![3])];

        assert!(remember_closes(&mut ops, &mut waiting).is_empty());
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].tab, tab);
    }
