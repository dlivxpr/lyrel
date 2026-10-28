use super::{Pane, PaneSlot};
use crate::ui::pending_pane::{PendingPane, PendingSpawn};
use crate::ui::tab::{Tab, TabSet};
use gpui_kit::{
    AppContext as _, Axis, Context, Entity, Focusable as _, IntoElement, Render, Styled as _,
    TestAppContext, Window, div,
};

/// A window has to exist for focus to live in, but nothing this file asks
/// is about what a pane paints.
struct Blank;
impl Render for Blank {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}

/// A leaf that owns a real focus handle without owning a shell. Focus
/// tracking asks the slot, not the terminal behind it, so a connecting
/// pane answers every question here exactly as a running one would — and
/// connecting panes are watched for focus now too, so this is not a
/// stand-in for the case under test but one of its cases.
fn leaf(cx: &mut Context<Blank>) -> Entity<PendingPane> {
    cx.new(|cx| {
        PendingPane::new(
            "test",
            PendingSpawn {
                workspace: None,
                working_directory: None,
                restore_pane: None,
                shell: None,
                agent: None,
                agent_session_id: None,
                agent_launch_argv: None,
                agent_session_scope: None,
                run_on_land: None,
                owner: None,
                font_size: 14.,
            },
            cx,
        )
    })
}

fn two_pane_tab(a: &Entity<PendingPane>, b: &Entity<PendingPane>) -> Tab {
    Tab::new(Pane::split_node(
        Axis::Horizontal,
        0.5,
        Pane::Leaf(PaneSlot::Connecting(a.clone())),
        Pane::Leaf(PaneSlot::Connecting(b.clone())),
    ))
}

fn one_pane_tab(a: &Entity<PendingPane>) -> Tab {
    Tab::new(Pane::Leaf(PaneSlot::Connecting(a.clone())))
}

/// The mechanism behind the report: `remember_active_pane`'s sample asks
/// which leaf holds focus *at that instant*, and a switch begun with focus
/// one handle away — the switcher's own search input, a palette closing,
/// the tab strip — finds no leaf and has nothing to write. This is why the
/// sample cannot be the only writer, and it is pinned here so that a change
/// making `focused_leaf` tolerant would have to argue with a test rather
/// than silently make this fix look unnecessary.
#[gpui_kit::test]
fn a_switch_begun_off_the_panes_samples_nothing(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, _| Blank);
    let (a, b, elsewhere) = window
        .update(cx, |_, _, cx| (leaf(cx), leaf(cx), cx.focus_handle()))
        .unwrap();
    let tab = two_pane_tab(&a, &b);

    window
        .update(cx, |_, window, cx| {
            let on_b = b.read(cx).focus_handle(cx);
            window.focus(&on_b, cx);
            assert_eq!(
                tab.pane.focused_leaf(window, cx).map(|l| l.entity_id()),
                Some(b.entity_id()),
                "with focus in the pane the sample would have found it"
            );

            window.focus(&elsewhere, cx);
            assert!(
                tab.pane.focused_leaf(window, cx).is_none(),
                "one handle off the pane and the switch-away sample has \
                 nothing to write"
            );
        })
        .unwrap();
}

/// So focus-in writes instead, and the tab comes back to the pane focus
/// was last in even though it had wandered off the panes before the switch
/// ever started.
#[gpui_kit::test]
fn a_tab_comes_back_to_the_pane_focus_was_last_in(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, _| Blank);
    let (a, b) = window.update(cx, |_, _, cx| (leaf(cx), leaf(cx))).unwrap();
    let mut tabs = TabSet::new(vec![two_pane_tab(&a, &b)], 0);
    assert_eq!(
        cx.update(|cx| tabs[0].focus_target(cx).map(|l| l.entity_id())),
        Some(a.entity_id()),
        "a tab nobody has worked in yet still opens on its first leaf"
    );

    // The reader clicks into the right-hand pane: focus arrives, and that
    // is the moment the tab is told.
    tabs.remember_leaf(b.entity_id());

    // Focus then leaves the panes — the switcher opens, a palette closes —
    // and the tab is switched away from. `remember_active_pane` finds no
    // focused leaf and writes nothing, which is now harmless.
    assert_eq!(
        cx.update(|cx| tabs[0].focus_target(cx).map(|l| l.entity_id())),
        Some(b.entity_id()),
        "#843: the tab has to come back to the pane the reader was in"
    );
}

/// A pane dragged into another tab is focused where it lands, so the
/// arrival has to change that tab's memory and not the one it left — the
/// reason the tab is found by the leaf rather than taken to be the active
/// one.
#[gpui_kit::test]
fn a_moved_pane_is_remembered_by_the_tab_that_holds_it_now(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, _| Blank);
    let (a, b, c) = window
        .update(cx, |_, _, cx| (leaf(cx), leaf(cx), leaf(cx)))
        .unwrap();
    // Tab 0 is the active one and holds `a`; `b` and `c` live in tab 1.
    let mut tabs = TabSet::new(vec![one_pane_tab(&a), two_pane_tab(&b, &c)], 0);

    tabs.remember_leaf(c.entity_id());

    assert_eq!(
        cx.update(|cx| tabs[1].focus_target(cx).map(|l| l.entity_id())),
        Some(c.entity_id()),
        "the tab holding the focused pane is the one that remembers it"
    );
    assert_eq!(
        cx.update(|cx| tabs[0].focus_target(cx).map(|l| l.entity_id())),
        Some(a.entity_id()),
        "and no other tab's memory is touched"
    );
}

/// A pane that arrives after its slot has gone — a spawn landing on a
/// closed tab, a leaf killed mid-flight — belongs to no tab, and must not
/// leave a memory behind for the first tab that happens to be looked at.
#[gpui_kit::test]
fn a_leaf_no_tab_holds_is_recorded_nowhere(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, _| Blank);
    let (a, b, gone) = window
        .update(cx, |_, _, cx| (leaf(cx), leaf(cx), leaf(cx)))
        .unwrap();
    let mut tabs = TabSet::new(vec![two_pane_tab(&a, &b)], 0);

    tabs.remember_leaf(b.entity_id());
    tabs.remember_leaf(gone.entity_id());

    assert_eq!(
        cx.update(|cx| tabs[0].focus_target(cx).map(|l| l.entity_id())),
        Some(b.entity_id()),
        "a stranger's arrival leaves the tab's own answer alone"
    );
}

/// A pane focused while it was still coming up is remembered under its
/// *pending* slot, and that id dies the moment the pane lands in its
/// place. The memory has to come along with the swap, or the landing is
/// itself what puts the tab back on its first leaf.
///
/// What lands here is another slot rather than a running pane: the swap has
/// to move an id from one slot to another, and which kind of slot arrived
/// is no part of the question.
#[gpui_kit::test]
fn a_landing_pane_inherits_what_its_pending_slot_was_told(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, _| Blank);
    let (a, connecting, landed, other) = window
        .update(cx, |_, _, cx| (leaf(cx), leaf(cx), leaf(cx), leaf(cx)))
        .unwrap();
    let mut tabs = TabSet::new(vec![two_pane_tab(&a, &connecting)], 0);

    // Focus arrives while the pane is still connecting, then the pane it
    // was waiting for lands in that slot.
    tabs.remember_leaf(connecting.entity_id());
    tabs.replace_leaf(connecting.entity_id(), PaneSlot::Connecting(landed.clone()));

    assert_eq!(
        cx.update(|cx| tabs[0].focus_target(cx).map(|l| l.entity_id())),
        Some(landed.entity_id()),
        "#843: the memory follows the pane, not the slot it arrived in"
    );

    // A landing somewhere else in the tab is not an answer to this
    // question and does not touch it.
    tabs.replace_leaf(a.entity_id(), PaneSlot::Connecting(other));
    assert_eq!(
        cx.update(|cx| tabs[0].focus_target(cx).map(|l| l.entity_id())),
        Some(landed.entity_id()),
        "another pane landing leaves the tab's answer alone"
    );
}

/// The wiring, end to end. `watch_pane_focus` is the subscription that
/// writes the record, and a real round trip through `activate` has to come
/// back to the pane focus last arrived in — with focus off the panes well
/// before the switch was made, which is the moment the switch-away sample
/// cannot see (#843).
#[gpui_kit::test]
fn a_tab_switch_returns_to_the_pane_focus_arrived_in(cx: &mut TestAppContext) {
    use super::{LyrelApp, test_window::harness};
    use crate::terminal::view::quiet_test_pane;

    let (app, mut vcx) = harness(cx);
    let (left, right, elsewhere, _held) = app.update_in(&mut vcx, |app, window, cx| {
        let (left, left_stream) = quiet_test_pane(1, window, cx);
        let (right, right_stream) = quiet_test_pane(2, window, cx);
        let (only, only_stream) = quiet_test_pane(3, window, cx);
        app.tabs.push(Tab::new(Pane::split_node(
            Axis::Horizontal,
            0.5,
            Pane::leaf(PaneSlot::Ready(left.clone())),
            Pane::leaf(PaneSlot::Ready(right.clone())),
        )));
        app.tabs.push(Tab::new(Pane::leaf(PaneSlot::Ready(only))));
        app.tabs.select(0);
        // The subscription every spawn path registers for the pane it
        // built.
        for view in [&left, &right] {
            let handle = view.read(cx).focus_handle.clone();
            LyrelApp::watch_pane_focus(&handle, view.entity_id(), window, cx);
        }
        cx.notify();
        (
            left,
            right,
            cx.focus_handle(),
            (left_stream, right_stream, only_stream),
        )
    });
    vcx.background_executor.run_until_parked();

    // The reader clicks into the right-hand pane.
    app.update_in(&mut vcx, |_, window, cx| {
        let handle = right.read(cx).focus_handle.clone();
        handle.focus(window, cx);
    });
    vcx.background_executor.run_until_parked();

    // Focus then leaves the panes altogether — a palette closing, the tab
    // strip, the switcher's own search input — before the tab is left.
    app.update_in(&mut vcx, |_, window, cx| elsewhere.focus(window, cx));
    vcx.background_executor.run_until_parked();

    app.update_in(&mut vcx, |app, window, cx| app.activate(1, window, cx));
    vcx.background_executor.run_until_parked();
    app.update_in(&mut vcx, |app, window, cx| app.activate(0, window, cx));
    vcx.background_executor.run_until_parked();

    app.update_in(&mut vcx, |_, window, cx| {
        assert!(
            right.read(cx).focus_handle.is_focused(window),
            "#843: the tab has to come back to the pane the reader was in"
        );
        assert!(
            !left.read(cx).focus_handle.is_focused(window),
            "and not to the first leaf"
        );
    });
}

/// A split tab on screen with both panes watched the way every spawn path
/// watches them, plus a handle off the panes for focus to wander to.
#[allow(clippy::type_complexity)]
fn watched_split(
    cx: &mut TestAppContext,
) -> (
    Entity<super::LyrelApp>,
    gpui_kit::VisualTestContext,
    Entity<crate::terminal::view::TerminalView>,
    Entity<crate::terminal::view::TerminalView>,
    gpui_kit::FocusHandle,
    impl Sized,
) {
    use super::{LyrelApp, test_window::harness};
    use crate::terminal::view::quiet_test_pane;

    let (app, mut vcx) = harness(cx);
    let (left, right, elsewhere, held) = app.update_in(&mut vcx, |app, window, cx| {
        let (left, left_stream) = quiet_test_pane(1, window, cx);
        let (right, right_stream) = quiet_test_pane(2, window, cx);
        app.tabs.push(Tab::new(Pane::split_node(
            Axis::Horizontal,
            0.5,
            Pane::leaf(PaneSlot::Ready(left.clone())),
            Pane::leaf(PaneSlot::Ready(right.clone())),
        )));
        app.tabs.select(0);
        for view in [&left, &right] {
            let handle = view.read(cx).focus_handle.clone();
            LyrelApp::watch_pane_focus(&handle, view.entity_id(), window, cx);
        }
        cx.notify();
        (left, right, cx.focus_handle(), (left_stream, right_stream))
    });
    vcx.background_executor.run_until_parked();
    (app, vcx, left, right, elsewhere, held)
}

/// Zoom in on a pane, zoom back out, and the cursor is still in that pane
/// (#869). The pane being un-zoomed is the answer outright — it is the one
/// that was on screen to work in — so it is not a question for the tab's
/// memory, which only focus-in writes and which nothing guarantees is
/// current at the instant the toggle lands.
#[gpui_kit::test]
fn unzooming_leaves_focus_in_the_pane_that_was_zoomed(cx: &mut TestAppContext) {
    let (app, mut vcx, left, right, _elsewhere, _held) = watched_split(cx);

    // Into the right-hand pane by keyboard, the road the report took.
    app.update_in(&mut vcx, |app, window, cx| {
        left.read(cx).focus_handle.clone().focus(window, cx);
        app.cycle_pane(true, window, cx);
    });
    vcx.background_executor.run_until_parked();

    app.update_in(&mut vcx, |app, window, cx| {
        app.toggle_maximize(window, cx);
        assert_eq!(
            app.maximized.as_ref().map(|l| l.entity_id()),
            Some(right.entity_id()),
            "the pane focus is in is the one zoomed"
        );
        // Whatever the tab's memory says while zoomed — here the stale
        // answer the report found — must not decide where un-zooming goes.
        app.tabs[0].remember_leaf(left.entity_id());
        app.toggle_maximize(window, cx);
    });
    vcx.background_executor.run_until_parked();

    app.update_in(&mut vcx, |app, window, cx| {
        assert!(app.maximized.is_none());
        assert!(
            right.read(cx).focus_handle.is_focused(window),
            "#869: un-zooming keeps focus in the pane that was zoomed"
        );
        assert_eq!(
            app.tabs[0].remembered_pane(),
            Some(right.entity_id()),
            "and the tab remembers it, so a switch away and back agrees"
        );
    });
}

/// Zooming with focus off the panes — a palette just closed, the tab strip
/// clicked — zooms the pane the tab remembers, not whichever leaf happens
/// to be first: the same pane switching back to the tab would focus.
#[gpui_kit::test]
fn zooming_from_off_the_panes_zooms_the_remembered_pane(cx: &mut TestAppContext) {
    let (app, mut vcx, _left, right, elsewhere, _held) = watched_split(cx);

    app.update_in(&mut vcx, |_, window, cx| {
        right.read(cx).focus_handle.clone().focus(window, cx);
    });
    vcx.background_executor.run_until_parked();
    app.update_in(&mut vcx, |_, window, cx| elsewhere.focus(window, cx));
    vcx.background_executor.run_until_parked();

    app.update_in(&mut vcx, |app, window, cx| {
        app.toggle_maximize(window, cx);
        assert_eq!(
            app.maximized.as_ref().map(|l| l.entity_id()),
            Some(right.entity_id()),
            "the zoom lands on the pane the reader was last in"
        );
        assert!(right.read(cx).focus_handle.is_focused(window));
    });
}

#[gpui_kit::test]
fn reordering_two_split_tabs_keeps_identity_mru_and_each_panes_focus(cx: &mut TestAppContext) {
    use super::LyrelApp;
    use crate::core::actions::{MoveTabRight, SelectNextTab};
    use crate::terminal::view::quiet_test_pane;

    let (app, mut vcx, _left, first_right, _elsewhere, _held) = watched_split(cx);
    let (second_right, ids, _second_streams) = app.update_in(&mut vcx, |app, window, cx| {
        let (left, left_stream) = quiet_test_pane(3, window, cx);
        let (right, right_stream) = quiet_test_pane(4, window, cx);
        for view in [&left, &right] {
            let handle = view.read(cx).focus_handle.clone();
            LyrelApp::watch_pane_focus(&handle, view.entity_id(), window, cx);
        }
        app.tabs.push(Tab::new(Pane::split_node(
            Axis::Horizontal,
            0.5,
            Pane::leaf(PaneSlot::Ready(left)),
            Pane::leaf(PaneSlot::Ready(right.clone())),
        )));
        let ids = [app.tabs[0].id(), app.tabs[1].id()];
        first_right.read(cx).focus_handle.clone().focus(window, cx);
        cx.notify();
        (right, ids, (left_stream, right_stream))
    });
    vcx.run_until_parked();

    vcx.dispatch_action(SelectNextTab);
    vcx.run_until_parked();
    app.update_in(&mut vcx, |_, window, cx| {
        second_right.read(cx).focus_handle.clone().focus(window, cx);
    });
    vcx.run_until_parked();
    vcx.dispatch_action(SelectNextTab);
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, window, cx| {
        assert_eq!(app.tabs[app.tabs.active_index()].id(), ids[0]);
        assert!(first_right.read(cx).focus_handle.is_focused(window));
    });

    vcx.dispatch_action(MoveTabRight);
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, window, cx| {
        assert_eq!(
            app.tabs.iter().map(Tab::id).collect::<Vec<_>>(),
            vec![ids[1], ids[0]]
        );
        assert_eq!(app.tabs[app.tabs.active_index()].id(), ids[0]);
        assert_eq!(
            app.tabs
                .by_mru()
                .iter()
                .map(|&index| app.tabs[index].id())
                .collect::<Vec<_>>(),
            vec![ids[0], ids[1]]
        );
        assert!(first_right.read(cx).focus_handle.is_focused(window));
    });

    vcx.dispatch_action(SelectNextTab);
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, window, cx| {
        assert_eq!(app.tabs[app.tabs.active_index()].id(), ids[1]);
        assert!(second_right.read(cx).focus_handle.is_focused(window));
        assert_eq!(app.tabs[0].pane.leaves().len(), 2);
        assert_eq!(app.tabs[1].pane.leaves().len(), 2);
    });
}
