
use gpui_kit::TestAppContext;

use crate::core::session::SessionPane;
use crate::ui::app::test_window::harness_with_tabs;
use crate::ui::pane::Pane;

fn leaf_pane_id(pane: &SessionPane) -> Option<u64> {
    match pane {
        SessionPane::Leaf { pane_id, .. } => *pane_id,
        SessionPane::Split { .. } => None,
    }
}

/// Asleep, a tab holds nothing running and still holds its place: the
/// pane id the wake restores from, the name it had on screen, and a seat in
/// the sync that tells the machine it is asleep rather than gone.
#[gpui_kit::test]
fn a_sleeping_tab_keeps_its_place_its_pane_and_its_name(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);

    app.update_in(&mut vcx, |app, window, cx| {
        let pane_id = app.tabs[1]
            .pane
            .first_leaf()
            .and_then(|slot| slot.terminal().cloned())
            .expect("tab 1 has a pane")
            .read(cx)
            .pane_id;
        let (before, _) = app.tabs[1].label_view(Some(window), cx);

        app.put_to_sleep(1, window, cx);

        assert_eq!(app.tabs.len(), 2, "sleeping is not closing");
        let tab = &app.tabs[1];
        assert!(tab.is_asleep());
        assert!(matches!(tab.pane, Pane::Empty));
        assert!(tab.pane.terminals().is_empty(), "nothing is left running");
        assert_eq!(tab.asleep_layout().and_then(leaf_pane_id), Some(pane_id));
        let (after, _) = tab.label_view(Some(window), cx);
        assert_eq!(
            format!("{:?}", after.label()),
            format!("{:?}", before.label()),
            "it is called what it was called while awake"
        );
        assert!(!after.live);

        let (desired, _, held) = crate::ui::tree_sync::projection::desired_tabs(
            &app.tabs,
            app.tabs.active_index(),
            app.workspace,
            cx,
        );
        assert!(held.is_empty(), "a sleeping tab is not a tab gone missing");
        assert_eq!(desired.len(), 2);
        assert!(!desired[0].hibernated);
        assert!(desired[1].hibernated);
        match &desired[1].root {
            crate::ui::tree_sync::desired::DesiredNode::Leaf { pane, .. } => {
                assert_eq!(*pane, pane_id)
            }
            other => panic!("expected the one leaf, got {other:?}"),
        }
    });
}

/// Looking at a tab is what wakes it, so the window has to keep one awake
/// to look at; the tab shown in a sleeping one's place is the awake tab
/// used last.
#[gpui_kit::test]
fn the_last_awake_tab_has_nothing_to_hand_the_window_to(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);

    app.update_in(&mut vcx, |app, window, cx| {
        app.activate(1, window, cx);
        app.tabs.touch_active();
        app.activate(2, window, cx);
        app.tabs.touch_active();
        app.activate(0, window, cx);
        app.tabs.touch_active();
        assert_eq!(app.tabs.awake_besides(0), Some(2), "the one used last");

        app.put_to_sleep(2, window, cx);
        assert_eq!(app.tabs.awake_besides(0), Some(1));

        app.put_to_sleep(1, window, cx);
        assert_eq!(app.tabs.awake_besides(0), None);
        assert!(
            !app.can_hibernate_tab(0, cx),
            "the last awake tab stays awake"
        );
        assert!(
            !app.can_hibernate_tab(1, cx),
            "nor is a sleeping tab put to sleep twice"
        );
    });
}

/// Closing a sleeping tab closes it like any other, and what Reopen Closed
/// Tab gets back is the layout it slept with — not the empty stand-in.
#[gpui_kit::test]
fn a_sleeping_tab_closes_into_what_it_would_have_woken_as(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);

    app.update_in(&mut vcx, |app, window, cx| {
        app.put_to_sleep(1, window, cx);
        let layout_id = app.tabs[1].asleep_layout().and_then(leaf_pane_id);
        assert!(layout_id.is_some());

        app.close_tab(1, window, cx);

        assert_eq!(app.tabs.len(), 1);
        let closed = app
            .session
            .newest_closed()
            .expect("the closed tab is remembered");
        assert_eq!(leaf_pane_id(&closed.pane), layout_id);
        assert!(!closed.hibernated, "reopening brings it back awake");
    });
}
