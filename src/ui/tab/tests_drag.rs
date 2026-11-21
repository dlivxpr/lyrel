//! Dragging a pane across the layout's drop targets.
//!
//! The lit preview is a promise: it shows the patch of layout the pane would
//! fill, and the release that ends the drag must land exactly where the
//! preview last showed. While the pointer is still deciding, nothing under
//! it may move — a preview is paint, and paint never reflows a terminal, so
//! the daemon hears about a pane's geometry only when the pane actually
//! lands somewhere new.

use super::*;
use gpui_kit::TestAppContext;
use gpui_kit::test::{TestEventExt as _, TestWindowExt as _};

use crate::daemon::protocol::ClientMsg;
use crate::ui::pane::{Pane, PaneSlot};

/// Two panes of deliberately uneven widths in the one tab, focused on the
/// left. The uneven split is what makes the swap observable downstream: each
/// pane ends up wearing the size the other had.
fn split_tab(
    app: &gpui_kit::Entity<LyrelApp>,
    vcx: &mut gpui_kit::VisualTestContext,
) -> (PaneSlot, PaneSlot) {
    app.update_in(vcx, |app, window, cx| {
        let right = app
            .tabs
            .remove(1)
            .pane
            .first_leaf()
            .expect("tab holds a pane");
        let left = app.tabs[0].pane.first_leaf().expect("tab holds a pane");
        app.tabs[0].pane = Pane::split_node(
            Axis::Horizontal,
            0.35,
            Pane::leaf(left.clone()),
            Pane::leaf(right.clone()),
        );
        window.focus(&left.focus_handle(cx), cx);
        cx.notify();
        (left, right)
    })
}

/// Where the workbench's panes were drawn, as the window reports it.
fn pane_area(
    app: &gpui_kit::Entity<LyrelApp>,
    vcx: &mut gpui_kit::VisualTestContext,
) -> Bounds<Pixels> {
    app.read_with(vcx, |app, _| app.pane_area.get())
        .expect("the workbench is drawn")
}

fn near(a: Pixels, b: Pixels) -> bool {
    (a - b).abs() < px(1.5)
}

fn rect_near(a: Bounds<Pixels>, b: Bounds<Pixels>) -> bool {
    near(a.origin.x, b.origin.x)
        && near(a.origin.y, b.origin.y)
        && near(a.size.width, b.size.width)
        && near(a.size.height, b.size.height)
}

/// Every resize request a pane has queued for its daemon, consuming whatever
/// else is pending too. Between the phases of one drag this stays zero: a
/// sliding preview must never read as geometry to the terminal behind it.
fn drain_resizes(
    stream: &mut crate::daemon::transport::Stream,
) -> Vec<crate::daemon::protocol::WinSize> {
    let mut resizes = Vec::new();
    loop {
        match ClientMsg::read(stream) {
            Ok(ClientMsg::Resize(win)) => resizes.push(win),
            Ok(_) => {}
            Err(_) => return resizes,
        }
    }
}

fn settle_clock(vcx: &mut gpui_kit::VisualTestContext, ms: u64) {
    vcx.executor()
        .advance_clock(std::time::Duration::from_millis(ms));
    vcx.background_executor.run_until_parked();
}

#[gpui_kit::test]
fn dragging_across_distinct_targets_glides_the_preview_and_the_swap_settles(
    cx: &mut TestAppContext,
) {
    let (app, mut vcx, mut streams) = test_window::harness_with_tabs(cx, 2);
    let (left, right) = split_tab(&app, &mut vcx);
    vcx.run_until_parked();
    vcx.update(|window, cx| window.render_frame(cx));
    for stream in &mut streams {
        stream
            .set_read_timeout(Some(std::time::Duration::from_millis(100)))
            .expect("the test stream takes a read timeout");
    }
    // The first layout is allowed to speak; what the drag must not do is
    // speak while it is only deciding.
    let left_size = *drain_resizes(&mut streams[0])
        .last()
        .expect("the left pane sizes itself once drawn");
    let right_size = *drain_resizes(&mut streams[1])
        .last()
        .expect("the right pane sizes itself once drawn");

    let area = pane_area(&app, &mut vcx);
    let right_middle = point(
        area.origin.x + area.size.width * 0.675,
        area.origin.y + area.size.height / 2.,
    );
    let right_band = point(
        area.origin.x + area.size.width - px(8.),
        area.origin.y + area.size.height / 2.,
    );
    // The middle of the right pane promises a trade — the whole rect — and
    // the band at the tab's own edge promises a third column beside both.
    let swap_target = Bounds {
        origin: point(area.origin.x + area.size.width * 0.35, area.origin.y),
        size: size(area.size.width * 0.65, area.size.height),
    };
    let edge_target = Bounds {
        origin: point(area.origin.x + area.size.width * (2. / 3.), area.origin.y),
        size: size(area.size.width / 3., area.size.height),
    };

    // Pick the left pane up by its handle and hold it over the right pane's
    // middle. The first frame a drag offers a target shows it whole: there is
    // no earlier rectangle to glide from.
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let handle = ("pane-drag-handle", left.entity_id().as_u64() as usize);
        let start = window.find(handle).bounds().center();
        window.pointer_down(start, gpui_kit::MouseButton::Left, cx);
        window.pointer_move(
            start + point(px(4.), px(0.)),
            Some(gpui_kit::MouseButton::Left),
            cx,
        );
        window.pointer_move(right_middle, Some(gpui_kit::MouseButton::Left), cx);
        assert!(
            rect_near(window.find("pane-drop-preview").bounds(), swap_target),
            "the middle of the right pane promises its whole rect"
        );
    });

    // On to the band: the preview glides from the swap rect to the third
    // column instead of jumping — it starts where it was.
    vcx.update(|window, cx| {
        window.pointer_move(right_band, Some(gpui_kit::MouseButton::Left), cx);
        assert!(
            rect_near(window.find("pane-drop-preview").bounds(), swap_target),
            "the glide starts from the rect the preview was painting"
        );
    });
    settle_clock(&mut vcx, 120);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(
            rect_near(window.find("pane-drop-preview").bounds(), edge_target),
            "and settles on the band's target"
        );
    });

    // Back across: the preview follows the pointer again rather than a stale
    // frame.
    vcx.update(|window, cx| {
        window.pointer_move(right_middle, Some(gpui_kit::MouseButton::Left), cx);
        assert!(rect_near(
            window.find("pane-drop-preview").bounds(),
            edge_target
        ));
    });
    settle_clock(&mut vcx, 120);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        assert!(rect_near(
            window.find("pane-drop-preview").bounds(),
            swap_target
        ));
        // Deciding is paint: no pane may have asked its daemon for a new
        // size while the preview slid across targets.
        assert_eq!(
            drain_resizes(&mut streams[0]).len(),
            0,
            "the preview never reflows the left pane"
        );
        assert_eq!(
            drain_resizes(&mut streams[1]).len(),
            0,
            "or the right one"
        );
    });

    // Let go over the middle: the trade the preview promised.
    vcx.update(|window, cx| {
        window.pointer_up(right_middle, gpui_kit::MouseButton::Left, cx);
        assert!(
            window.try_find("pane-drop-preview").is_none(),
            "no stale preview after the drop"
        );
        assert!(!cx.has_active_drag());
    });
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, window, cx| {
        let leaves = app.tabs[0].pane.leaves();
        assert_eq!(
            leaves[0].entity_id(),
            right.entity_id(),
            "the right pane takes the left one's place"
        );
        assert_eq!(
            leaves[1].entity_id(),
            left.entity_id(),
            "and the left pane takes the right one's"
        );
        assert!(
            left.contains_focused(window, cx),
            "the dropped pane is where the keyboard lands"
        );
        assert!(app.pane_drag.borrow().is_none());
    });

    // The swap is one layout truth: each pane tells the daemon about its new
    // geometry once, wearing the size the other pane had — and the settle is
    // not an animation trailing frames behind it.
    let after_left = drain_resizes(&mut streams[0]);
    let after_right = drain_resizes(&mut streams[1]);
    assert_eq!(
        after_left.last(),
        Some(&right_size),
        "the left pane ends up wearing the right pane's size"
    );
    assert_eq!(
        after_right.last(),
        Some(&left_size),
        "and the right pane wears the left pane's"
    );
    settle_clock(&mut vcx, 300);
    assert!(
        drain_resizes(&mut streams[0]).is_empty(),
        "no animation-driven resizes trail the settle"
    );
    assert!(drain_resizes(&mut streams[1]).is_empty());
}

#[gpui_kit::test]
fn reduced_motion_shows_each_target_directly_without_a_glide(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = test_window::harness_with_tabs(cx, 2);
    let (left, _right) = split_tab(&app, &mut vcx);
    vcx.update(|_, cx| cx.set_reduce_motion(true));
    vcx.run_until_parked();
    vcx.update(|window, cx| window.render_frame(cx));

    let area = pane_area(&app, &mut vcx);
    let right_middle = point(
        area.origin.x + area.size.width * 0.675,
        area.origin.y + area.size.height / 2.,
    );
    let right_band = point(
        area.origin.x + area.size.width - px(8.),
        area.origin.y + area.size.height / 2.,
    );
    let swap_target = Bounds {
        origin: point(area.origin.x + area.size.width * 0.35, area.origin.y),
        size: size(area.size.width * 0.65, area.size.height),
    };
    let edge_target = Bounds {
        origin: point(area.origin.x + area.size.width * (2. / 3.), area.origin.y),
        size: size(area.size.width / 3., area.size.height),
    };

    vcx.update(|window, cx| {
        window.render_frame(cx);
        let handle = ("pane-drag-handle", left.entity_id().as_u64() as usize);
        let start = window.find(handle).bounds().center();
        window.pointer_down(start, gpui_kit::MouseButton::Left, cx);
        window.pointer_move(
            start + point(px(4.), px(0.)),
            Some(gpui_kit::MouseButton::Left),
            cx,
        );
        window.pointer_move(right_middle, Some(gpui_kit::MouseButton::Left), cx);
        assert!(rect_near(
            window.find("pane-drop-preview").bounds(),
            swap_target
        ));
        window.pointer_move(right_band, Some(gpui_kit::MouseButton::Left), cx);
        assert!(
            rect_near(window.find("pane-drop-preview").bounds(), edge_target),
            "with motion reduced the preview is drawn at its target, not mid-glide"
        );
        window.pointer_up(right_band, gpui_kit::MouseButton::Left, cx);
    });
    // The band drop is a real landing, not a preview artefact.
    app.update_in(&mut vcx, |app, _, _| {
        assert_eq!(app.tabs[0].pane.leaves().len(), 2, "no pane left its tab");
        assert!(app.pane_drag.borrow().is_none());
    });
}

#[gpui_kit::test]
fn escape_cancels_a_pane_drag_with_the_layout_and_pty_untouched(
    cx: &mut TestAppContext,
) {
    let (app, mut vcx, mut streams) = test_window::harness_with_tabs(cx, 2);
    let (left, _right) = split_tab(&app, &mut vcx);
    vcx.run_until_parked();
    vcx.update(|window, cx| window.render_frame(cx));
    for stream in &mut streams {
        stream
            .set_read_timeout(Some(std::time::Duration::from_millis(100)))
            .expect("the test stream takes a read timeout");
    }
    let _ = drain_resizes(&mut streams[0]);
    let _ = drain_resizes(&mut streams[1]);

    let area = pane_area(&app, &mut vcx);
    let right_middle = point(
        area.origin.x + area.size.width * 0.675,
        area.origin.y + area.size.height / 2.,
    );
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let handle = ("pane-drag-handle", left.entity_id().as_u64() as usize);
        let start = window.find(handle).bounds().center();
        window.pointer_down(start, gpui_kit::MouseButton::Left, cx);
        window.pointer_move(
            start + point(px(4.), px(0.)),
            Some(gpui_kit::MouseButton::Left),
            cx,
        );
        window.pointer_move(right_middle, Some(gpui_kit::MouseButton::Left), cx);
        assert!(window.try_find("pane-drop-preview").is_some());
        assert!(cx.has_active_drag());
        window.press("escape", cx);
        assert!(!cx.has_active_drag(), "escape ends the drag");
        assert!(
            window.try_find("pane-drop-preview").is_none(),
            "and takes the preview with it"
        );
        // A late release where the preview used to be must not drop.
        window.pointer_up(right_middle, gpui_kit::MouseButton::Left, cx);
        assert!(window.try_find("pane-drop-preview").is_none());
    });
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, window, cx| {
        let leaves = app.tabs[0].pane.leaves();
        assert_eq!(
            leaves[0].entity_id(),
            left.entity_id(),
            "cancellation leaves the prior layout intact"
        );
        assert_eq!(leaves.len(), 2);
        assert!(
            left.contains_focused(window, cx),
            "the keyboard stays where it was"
        );
        assert!(app.pane_drag.borrow().is_none());
    });
    assert!(
        drain_resizes(&mut streams[0]).is_empty(),
        "a cancelled drag never reached the daemon"
    );
    assert!(drain_resizes(&mut streams[1]).is_empty());
}
