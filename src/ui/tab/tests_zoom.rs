use gpui_kit::TestAppContext;

use crate::ui::app::test_window::harness_with_tabs;

#[gpui_kit::test]
fn a_tabs_zoom_survives_a_round_trip_to_another_tab(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);

    app.update_in(&mut vcx, |app, window, cx| {
        let leaf = app.tabs[0]
            .pane
            .first_leaf()
            .and_then(|slot| slot.terminal().cloned())
            .expect("tab 0 has a pane");
        app.maximized = Some(leaf.clone());

        app.activate(1, window, cx);
        assert!(app.maximized.is_none(), "tab 1 never zoomed anything");

        app.activate(0, window, cx);
        assert_eq!(
            app.maximized.as_ref().map(|l| l.entity_id()),
            Some(leaf.entity_id()),
            "tab 0's zoom is still where it was left (#599)"
        );

        // A zoom stashed for a pane that is no longer in the tab does not
        // come back — it exited (or was closed) while the tab was away.
        app.maximized = Some(leaf.clone());
        app.activate(1, window, cx);
        // activate already stashed this zoom.
        app.tabs[0].pane =
            crate::ui::pane::Pane::leaf(app.tabs[1].pane.first_leaf().expect("a donor leaf"));
        app.activate(0, window, cx);
        assert!(
            app.maximized.is_none(),
            "a stashed zoom whose pane is gone stays gone"
        );
    });
}

/// The mark the chrome wears (#752) has to go out again by every road the
/// zoom itself leaves by, and it has to name the right tab while several
/// tabs are each holding one.
#[gpui_kit::test]
fn the_zoom_mark_is_on_whichever_tabs_are_hiding_panes(cx: &mut TestAppContext) {
    use crate::terminal::view::quiet_test_pane;
    use crate::ui::pane::{Pane, PaneSlot};

    let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);

    app.update_in(&mut vcx, |app, window, cx| {
        // A zoom only hides something where there is a sibling to hide, so
        // tabs 0 and 1 get a second pane and tab 2 stays single.
        let mut held = Vec::new();
        for tab in 0..2 {
            let (view, stream) = quiet_test_pane(90 + tab as u64, window, cx);
            held.push(stream);
            let first = app.tabs[tab].pane.first_leaf().expect("tab has a pane");
            app.tabs[tab].pane = Pane::split_node(
                gpui_kit::Axis::Horizontal,
                0.5,
                Pane::leaf(first),
                Pane::leaf(PaneSlot::Ready(view)),
            );
        }

        for i in 0..app.tabs.len() {
            assert!(!app.tab_is_zoomed(i), "nothing is zoomed yet");
        }

        // Zooming marks the tab it happened in, and only that one.
        app.toggle_maximize(window, cx);
        assert!(app.tab_is_zoomed(0), "the zoomed tab wears the mark");
        assert!(!app.tab_is_zoomed(1));
        assert!(!app.tab_is_zoomed(2));

        // And un-zooming takes it away again.
        app.toggle_maximize(window, cx);
        assert!(!app.tab_is_zoomed(0), "un-zooming clears the mark");

        // The mark rides with its tab across a switch (#599) — an inactive
        // tab holding a zoom still wears it — and two tabs can wear one at
        // the same time, each reading its own handle.
        app.toggle_maximize(window, cx);
        app.activate(1, window, cx);
        assert!(app.tab_is_zoomed(0), "the parked zoom is still a zoom");
        assert!(!app.tab_is_zoomed(1));
        app.toggle_maximize(window, cx);
        assert!(app.tab_is_zoomed(0) && app.tab_is_zoomed(1));
        assert!(!app.tab_is_zoomed(2), "a single-pane tab hides nothing");

        // A parked zoom naming a pane that has since left the tab is no
        // zoom: it would not come back on a switch, so it is not marked.
        let parked = app.tabs[0].zoomed().cloned().expect("tab 0 parked a zoom");
        let elsewhere = app.tabs[2]
            .pane
            .first_leaf()
            .and_then(|slot| slot.terminal().cloned());
        app.tabs[0].park_zoom(elsewhere);
        assert!(
            !app.tab_is_zoomed(0),
            "a zoom over a pane this tab does not hold is not marked"
        );
        app.tabs[0].park_zoom(Some(parked.clone()));
        assert!(app.tab_is_zoomed(0));

        // Nor is a zoom over the last pane standing: its siblings closed
        // while the tab was away, and the tab now looks like — and draws
        // as — an ordinary single pane.
        app.tabs[0].pane = Pane::leaf(PaneSlot::Ready(parked));
        assert!(
            !app.tab_is_zoomed(0),
            "a zoom that covers nothing stops being marked"
        );

        assert!(!app.tab_is_zoomed(9), "there is no tab 9 to mark");
        drop(held);
    });
}
