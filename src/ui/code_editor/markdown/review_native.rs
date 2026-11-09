use super::SourcePresentation;
use super::review_support::{bytes, fixture, select, settle, until};
use crate::ui::app::test_window::harness_with_tabs;
use crate::ui::code_editor::markdown::Selection;
use gpui_kit::test::{TestEventExt as _, TestWindowExt as _};
use gpui_kit::{
    AppContext as _, Focusable as _, TestAppContext, VisualContext as _, VisualTestContext,
};
pub(super) fn native_cases() -> [crate::native_markdown::Case; 7] {
    use crate::native_markdown::Case;
    [
        Case::new(
            "ui::code_editor::markdown::source::review_tests::native_down_visits_each_soft_wrapped_row_without_skipping",
            native_down_visits_each_soft_wrapped_row_without_skipping,
        ),
        Case::new(
            "ui::code_editor::markdown::source::review_tests::first_soft_wrapped_link_grapheme_hovers_edits_and_opens",
            first_soft_wrapped_link_grapheme_hovers_edits_and_opens,
        ),
        Case::new(
            "ui::code_editor::markdown::source::review_tests::fragment_tail_link_grapheme_hovers_edits_and_opens",
            fragment_tail_link_grapheme_hovers_edits_and_opens,
        ),
        Case::new(
            "ui::code_editor::markdown::source::review_tests::native_soft_wrap_caret_keeps_clicked_side_across_navigation_and_input",
            native_soft_wrap_caret_keeps_clicked_side_across_navigation_and_input,
        ),
        Case::new(
            "ui::code_editor::markdown::source::review_tests::native_modified_link_drag_retains_upper_wrap_anchor",
            native_modified_link_drag_retains_upper_wrap_anchor,
        ),
        Case::new(
            "ui::code_editor::markdown::source::review_tests::native_extreme_nowrap_line_keeps_pointer_selection_and_source_exact",
            native_extreme_nowrap_line_keeps_pointer_selection_and_source_exact,
        ),
        Case::new(
            "ui::code_editor::markdown::source::review_tests::native_aligned_table_pointer_and_selection_follow_content",
            native_aligned_table_pointer_and_selection_follow_content,
        ),
    ]
}
fn native_down_visits_each_soft_wrapped_row_without_skipping(cx: &mut TestAppContext) {
    use unicode_segmentation::UnicodeSegmentation as _;

    let original = format!("{}\n\nkeep\n", "W ".repeat(140));
    let (_directory, _path, _app, mut context, _streams, view) = fixture!(cx, &original, true);
    view.update_in(&mut context, |view, _, cx| view.set_soft_wrap(true, cx));
    cx.simulate_window_resize(
        context.window_handle(),
        gpui_kit::size(gpui_kit::px(1000.), gpui_kit::px(1200.)),
    );
    select(&view, &mut context, 0, 0);
    until(&mut context, |context| {
        view.read_with(context, |view, cx| view.geometry_is_current(cx))
    });
    let rows = view.read_with(&context, |view, _| {
        let leaf = view.projection.leaf_at(0).unwrap();
        let geometry = &view.geometry.as_ref().unwrap().1[&leaf.id];
        // The source W at each independently shaped continuation-row start is
        // the expected endpoint, not the next glyph's trailing caret.
        let rows = leaf
            .text
            .grapheme_indices(true)
            .filter_map(|(ix, text)| {
                let a = geometry
                    .position_for_caret(gpui_kit::base::text::editor_host::Caret::upstream(ix));
                let b =
                    geometry.position_for_caret(gpui_kit::base::text::editor_host::Caret::new(ix));
                (text == "W" && a.y < b.y).then_some((leaf.source_offset(ix), b.y))
            })
            .take(2)
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 2, "fixture must contain two continuation rows");
        rows
    });
    for (expected, top) in rows {
        context.update(|window, cx| window.press("down", cx));
        settle(&view, &mut context);
        view.read_with(&context, |view, cx| {
            assert_eq!(view.selection(), Selection::caret(expected));
            assert_eq!(
                view.range_to_bounds(&(expected..expected), cx)
                    .unwrap()
                    .top(),
                top
            );
            assert_eq!(view.document().read(cx).revision(), 0);
            assert_eq!(
                view.document().read(cx).encode().unwrap(),
                original.as_bytes()
            );
        });
    }
}
fn native_soft_wrap_caret_keeps_clicked_side_across_navigation_and_input(cx: &mut TestAppContext) {
    use gpui_kit::{EntityInputHandler as _, MouseButton, point, px, size};

    let original = format!("{}\n\nkeep\n", "界".repeat(240));
    let (_directory, _path, _app, mut context, _streams, view) = fixture!(cx, &original, true);
    view.update_in(&mut context, |view, _, cx| view.set_soft_wrap(true, cx));
    cx.simulate_window_resize(context.window_handle(), size(px(1000.), px(1200.)));
    select(&view, &mut context, 0, 0);
    settle(&view, &mut context);
    // Equal-advance CJK glyphs wrap at every character. Derive the expected
    // source boundary from available width and one ordinary, non-wrap advance;
    // pointer positions and expected row tops come from the paragraph box.
    // Neither hit testing nor position_for_caret supplies the expected result.
    let (at, upper, lower, height) = view.read_with(&context, |view, _| {
        let leaf = view.projection.leaf_at(0).unwrap();
        let geometry = &view.geometry.as_ref().unwrap().1[&leaf.id];
        let bounds = geometry.bounds();
        let advance = geometry.position_for_index("界".len()).x - bounds.left();
        assert!(advance > px(0.));
        let columns = (bounds.size.width / advance).floor() as usize;
        assert!(columns > 1 && columns * 2 < 240);
        let byte = columns * "界".len();
        assert_eq!(&original[byte..byte + "界".len()], "界");
        (
            byte,
            point(bounds.left() + advance * columns as f32, bounds.top()),
            point(bounds.left(), bounds.top() + geometry.line_height()),
            geometry.caret_height(),
        )
    });
    let assert_bounds = |bounds: gpui_kit::Bounds<gpui_kit::Pixels>,
                         expected: gpui_kit::Point<gpui_kit::Pixels>| {
        assert_eq!(
            bounds.top(),
            expected.y,
            "caret must stay on the independently located visual row"
        );
        assert!(
            (bounds.left() - expected.x).abs() < px(0.5),
            "caret must retain the row's expected glyph column"
        );
    };
    let click = |position: gpui_kit::Point<gpui_kit::Pixels>, context: &mut VisualTestContext| {
        context.update(|window, cx| {
            let position = point(position.x, position.y + height / 2.);
            window.pointer_down(position, MouseButton::Left, cx);
            window.pointer_up(position, MouseButton::Left, cx);
        });
        settle(&view, context);
    };
    let assert_anchor = |expected, context: &mut VisualTestContext| {
        view.update_in(context, |view, window, cx| {
            assert_eq!(view.selection(), Selection::caret(at));
            assert_bounds(view.range_to_bounds(&(at..at), cx).unwrap(), expected);
            let utf16 = view.document().read(cx).utf16_offset(at);
            let bounds = view
                .bounds_for_range(utf16..utf16, view.scroll.bounds(), window, cx)
                .unwrap();
            assert_bounds(bounds, expected);
            // An OS query for another UTF-16 position must not be replaced by head.
            let requested = view
                .bounds_for_range(0..0, view.scroll.bounds(), window, cx)
                .unwrap();
            assert_eq!(
                requested.origin,
                view.range_to_bounds(&(0..0), cx).unwrap().origin
            );
            assert_ne!(requested.origin, expected);
        });
    };
    click(upper, &mut context);
    assert_anchor(upper, &mut context);
    context.update(|window, cx| window.press("right", cx));
    settle(&view, &mut context);
    assert_anchor(lower, &mut context);
    context.update(|window, cx| window.press("left", cx));
    settle(&view, &mut context);
    assert_anchor(upper, &mut context);
    context.dispatch_action(gpui_kit::component::input::SelectToNextWordEnd);
    context.dispatch_action(gpui_kit::component::input::MoveLeft);
    settle(&view, &mut context);
    assert_anchor(upper, &mut context);
    context.dispatch_action(gpui_kit::component::input::SelectToStart);
    context.dispatch_action(gpui_kit::component::input::MoveRight);
    settle(&view, &mut context);
    assert_anchor(upper, &mut context);
    context.dispatch_action(gpui_kit::component::input::SelectToEnd);
    context.dispatch_action(gpui_kit::component::input::MoveLeft);
    settle(&view, &mut context);
    assert_anchor(upper, &mut context);
    click(lower, &mut context);
    assert_anchor(lower, &mut context);
    context.update(|window, cx| window.press("up", cx));
    settle(&view, &mut context);
    assert_eq!(
        view.read_with(&context, |view, _| view.selection()),
        Selection::caret(0)
    );
    context.update(|window, cx| window.press("shift-down", cx));
    settle(&view, &mut context);
    assert_eq!(
        view.read_with(&context, |view, _| view.selection()),
        Selection::new(0, at)
    );
    view.update_in(&mut context, |view, window, cx| {
        let utf16 = view.document().read(cx).utf16_offset(at);
        assert_bounds(
            view.bounds_for_range(utf16..utf16, view.scroll.bounds(), window, cx)
                .unwrap(),
            lower,
        );
    });
    context.update(|window, cx| window.press("shift-up", cx));
    settle(&view, &mut context);
    assert_eq!(
        view.read_with(&context, |view, _| view.selection()),
        Selection::caret(0)
    );
    // A real drag keeps the same source endpoint but changes its painted row.
    context.update(|window, cx| {
        window.pointer_down(point(lower.x, lower.y + height / 2.), MouseButton::Left, cx);
        window.pointer_move(
            point(upper.x, upper.y + height / 2.),
            Some(MouseButton::Left),
            cx,
        );
        window.pointer_up(point(upper.x, upper.y + height / 2.), MouseButton::Left, cx);
    });
    settle(&view, &mut context);
    assert_anchor(upper, &mut context);
    context.update(|window, cx| window.input("X", cx));
    settle(&view, &mut context);
    let mut edited = original.clone();
    edited.insert(at, 'X');
    assert_eq!(bytes(&view, &context), edited.as_bytes());
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&view, &mut context);
    assert_eq!(bytes(&view, &context), original.as_bytes());
    assert_anchor(lower, &mut context);
    click(upper, &mut context);
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    settle(&view, &mut context);
    assert_anchor(lower, &mut context);
    click(upper, &mut context);
    let sibling = context.update(|_, cx| {
        let document = view.read(cx).document().clone();
        cx.new(|cx| SourcePresentation::new(document, true, cx))
    });
    sibling.update_in(&mut context, |sibling, window, cx| {
        let start = original.find("keep").unwrap();
        sibling.command(
            super::Command::Replace {
                range: start..start + 4,
                text: "else".into(),
                kind: super::EditKind::Command,
            },
            window,
            cx,
        );
    });
    settle(&view, &mut context);
    assert_anchor(lower, &mut context);
    assert_eq!(
        bytes(&view, &context),
        original.replacen("keep", "else", 1).as_bytes()
    );
    let next_at = at * 2;
    let next_upper = point(upper.x, lower.y);
    click(next_upper, &mut context);
    context.dispatch_action(gpui_kit::base::input::AddCursorAbove);
    settle(&view, &mut context);
    view.read_with(&context, |view, cx| {
        assert_eq!(view.selection(), Selection::caret(next_at));
        assert_eq!(view.extra_selections, vec![Selection::caret(at)]);
        assert_bounds(view.range_to_bounds(&(at..at), cx).unwrap(), upper);
    });
    sibling.update_in(&mut context, |sibling, window, cx| {
        let start = original.find("keep").unwrap();
        sibling.command(
            super::Command::Replace {
                range: start..start + 4,
                text: "keep".into(),
                kind: super::EditKind::Command,
            },
            window,
            cx,
        );
    });
    settle(&view, &mut context);
    view.read_with(&context, |view, cx| {
        assert_eq!(view.extra_selections, vec![Selection::caret(at)]);
        assert_bounds(view.range_to_bounds(&(at..at), cx).unwrap(), lower);
    });
    context.update(|window, cx| window.press("right", cx));
    settle(&view, &mut context);
    view.read_with(&context, |view, cx| {
        assert!(view.extra_selections.is_empty());
        assert_bounds(view.range_to_bounds(&(at..at), cx).unwrap(), lower);
    });
    click(next_upper, &mut context);
    context.dispatch_action(gpui_kit::base::input::AddCursorAbove);
    settle(&view, &mut context);
    view.read_with(&context, |view, cx| {
        assert_bounds(view.range_to_bounds(&(at..at), cx).unwrap(), upper);
    });
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    settle(&view, &mut context);
    view.read_with(&context, |view, cx| {
        assert_bounds(view.range_to_bounds(&(at..at), cx).unwrap(), lower);
    });
    assert_eq!(bytes(&view, &context), original.as_bytes());
}

fn native_modified_link_drag_retains_upper_wrap_anchor(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, MouseButton, point, px, size};
    let source = format!("[{}](target.md)\n\nkeep\n", "界".repeat(240));
    let (_directory, path, app, mut context, _streams, view) = fixture!(cx, &source, true);
    view.update_in(&mut context, |view, _, cx| view.set_soft_wrap(true, cx));
    cx.simulate_window_resize(context.window_handle(), size(px(1000.), px(1200.)));
    select(&view, &mut context, source.len(), source.len());
    settle(&view, &mut context);
    let (at, upper, target, height) = view.read_with(&context, |view, _| {
        let leaf = view.projection.leaf_at(1).unwrap();
        let geometry = &view.geometry.as_ref().unwrap().1[&leaf.id];
        let bounds = geometry.bounds();
        let advance = geometry.position_for_index("界".len()).x - bounds.left();
        assert!(advance > px(0.));
        let columns = (bounds.size.width / advance).floor() as usize;
        assert!(columns > 2 && columns * 2 < 240);
        (
            1 + columns * "界".len(),
            point(bounds.left() + advance * columns as f32, bounds.top()),
            point(
                bounds.left() + advance * 2.,
                bounds.top() + geometry.line_height(),
            ),
            geometry.caret_height(),
        )
    });
    let modifiers = if cfg!(target_os = "macos") {
        Modifiers {
            platform: true,
            ..Modifiers::none()
        }
    } else {
        Modifiers::control()
    };
    context.simulate_mouse_down(
        point(upper.x - px(1.), upper.y + height / 2.),
        MouseButton::Left,
        modifiers,
    );
    context.update(|window, cx| {
        window.pointer_move(
            point(target.x, target.y + height / 2.),
            Some(MouseButton::Left),
            cx,
        );
    });
    context.simulate_mouse_up(
        point(target.x, target.y + height / 2.),
        MouseButton::Left,
        modifiers,
    );
    settle(&view, &mut context);
    assert_eq!(
        view.read_with(&context, |view, _| view.selection()),
        Selection::new(at, at + 2 * "界".len())
    );
    context.dispatch_action(gpui_kit::component::input::MoveLeft);
    settle(&view, &mut context);
    view.read_with(&context, |view, cx| {
        assert_eq!(view.selection(), Selection::caret(at));
        let caret = view.range_to_bounds(&(at..at), cx).unwrap();
        // Entering the link adds its contextual toolbar above the document.
        // Use the current paragraph box, not the pre-gesture window origin;
        // Downstream would still be exactly one visual row below this oracle.
        let leaf = view.projection.leaf_at(1).unwrap();
        let geometry = &view.geometry.as_ref().unwrap().1[&leaf.id];
        let row = geometry.bounds();
        let advance = geometry.position_for_index("界".len()).x - row.left();
        assert_eq!(caret.top(), row.top());
        let expected_x = row.left() + advance * ((at - 1) / "界".len()) as f32;
        assert!((caret.left() - expected_x).abs() < px(0.5));
    });
    assert_eq!(
        app.read_with(&context, |app, _| app.active_buffer().unwrap().path.clone()),
        path
    );
    assert_eq!(bytes(&view, &context), source.as_bytes());
}

fn first_soft_wrapped_link_grapheme_hovers_edits_and_opens(cx: &mut gpui_kit::TestAppContext) {
    native_link_grapheme_hovers_edits_and_opens(cx, false);
}

fn fragment_tail_link_grapheme_hovers_edits_and_opens(cx: &mut TestAppContext) {
    native_link_grapheme_hovers_edits_and_opens(cx, true);
}

fn native_link_grapheme_hovers_edits_and_opens(cx: &mut TestAppContext, fragment_tail: bool) {
    // Both boundary cases use the same real workbench, LocalHost and native text system.
    use gpui_kit::{
        Focusable as _, Modifiers, MouseButton, VisualContext as _, point, px, size,
        test::{TestEventExt as _, TestWindowExt as _},
    };
    use std::time::{Duration, Instant};
    use unicode_segmentation::UnicodeSegmentation as _;

    let glyph = if fragment_tail { "界" } else { "W" };
    let original = if fragment_tail {
        // CJK supplies visible, non-whitespace wrap boundaries. The embedded
        // image makes this a real InlineFlow, not a whole TextLayout paragraph.
        format!(
            "[{}![alt](missing.png) after](target.md#target-title)\n\nkeep\n",
            glyph.repeat(100)
        )
    } else {
        format!("[{}](target.md#target-title)\n\nkeep\n", "W ".repeat(100))
    };
    let (directory, path, app, mut context, _streams, view) = fixture!(cx, &original, true);
    let identity = view.read_with(&context, |view, _| view.document().entity_id());
    let target = directory.path().join("target.md");
    std::fs::write(&target, "# Target title\n\nFirst paragraph\n").unwrap();
    view.update_in(&mut context, |view, _, cx| {
        view.set_soft_wrap(true, cx);
        view.select_range(original.len()..original.len(), cx);
    });
    cx.simulate_window_resize(context.window_handle(), size(px(1000.), px(1800.)));
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        settle(&view, &mut context);
        if view.read_with(&context, |view, cx| {
            view.geometry_is_current(cx) && !view.projection.links.links.is_empty()
        }) {
            break;
        }
        assert!(Instant::now() < deadline, "link projection did not settle");
    }
    // Locate the pointer only through independent native caret geometry, not
    // the link-region helper whose fragment-tail recovery is under test.
    let locate = |context: &VisualTestContext| {
        view.read_with(context, |view, cx| {
            assert!(view.geometry_is_current(cx));
            let link = &view.projection.links.links[0];
            let leaf = view.projection.leaf_at(link.label.start).unwrap();
            if fragment_tail {
                assert!(
                    leaf.image_urls().next().is_some(),
                    "link must contain an image atom"
                );
            }
            let geometry = &view.geometry.as_ref().unwrap().1[&leaf.id];
            let reference = leaf
                .text
                .grapheme_indices(true)
                .find_map(|(ix, text)| {
                    let a = geometry.position_for_index(ix);
                    let b = geometry.position_for_index(ix + text.len());
                    (text == glyph && a.y == b.y && b.x > a.x).then_some(b.x - a.x)
                })
                .unwrap();
            let origin = geometry.position_for_index(0).x;
            let viewport = view.viewport.bounds.unwrap();
            let (ix, a, b) = leaf
                .text
                .grapheme_indices(true)
                .find_map(|(ix, text)| {
                    let a = geometry.position_for_index(ix);
                    let b = geometry.position_for_index(ix + text.len());
                    let position = if fragment_tail {
                        point(a.x + reference * 0.25, a.y + geometry.caret_height() / 2.)
                    } else {
                        point(
                            origin + reference * 0.75,
                            b.y + geometry.caret_height() / 2.,
                        )
                    };
                    (text == glyph
                        && a.y != b.y
                        && leaf.atom_source(ix).is_none()
                        && viewport.contains(&position))
                    .then_some((ix, a, b))
                })
                .expect("native link must exercise a visible grapheme across a caret row boundary");
            let next_start =
                geometry.index_for_position(point(origin, b.y + geometry.caret_height() / 2.));
            let (position, hit) = if fragment_tail {
                assert_ne!(
                    next_start, ix,
                    "must be a fragment tail, not a whole-layout wrapped start"
                );
                assert!(a.y < b.y);
                let position = point(a.x + reference * 0.25, a.y + geometry.caret_height() / 2.);
                // The native hit and its caret both belong to the preceding piece's
                // final visible glyph. Ordinary same-row glyphs cannot satisfy this.
                assert_eq!(geometry.index_for_position(position), ix);
                assert_eq!(geometry.position_for_index(ix).y, a.y);
                assert_eq!(
                    geometry.index_for_position(point(a.x + reference * 0.75, position.y)),
                    ix + glyph.len()
                );
                (position, ix)
            } else {
                assert_eq!(next_start, ix);
                let position = point(
                    origin + reference * 0.75,
                    b.y + geometry.caret_height() / 2.,
                );
                assert_eq!(geometry.index_for_position(position), ix + glyph.len());
                (position, ix + glyph.len())
            };
            (position, leaf.source_offset(hit))
        })
    };
    let (position, at) = locate(&context);
    context.update(|window, cx| window.pointer_move(position, None, cx));
    context.update(|window, cx| window.pointer_move(position, None, cx));
    context.run_until_parked();
    context.executor().advance_clock(Duration::from_millis(400));
    context.run_until_parked();
    context.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("markdown-link-preview").is_none());
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        context.executor().advance_clock(Duration::from_millis(100));
        context.run_until_parked();
        if context.update(|window, cx| {
            window.render_frame(cx);
            window
                .try_find("markdown-link-preview")
                .is_some_and(|preview| {
                    preview.visible()
                        && preview.label().is_some_and(|label| {
                            label.contains("Target title") && label.contains("First paragraph")
                        })
                })
        }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "native boundary glyph did not open its preview"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    context.update(|window, cx| {
        window.pointer_down(position, MouseButton::Left, cx);
        window.pointer_up(position, MouseButton::Left, cx);
        assert_eq!(view.read(cx).selection(), Selection::caret(at));
        assert_eq!(view.read(cx).document().entity_id(), identity);
        assert_eq!(app.read(cx).active_buffer().unwrap().path, path);
        window.input("X", cx);
    });
    settle(&view, &mut context);
    view.read_with(&context, |view, cx| {
        let mut edited = original.clone();
        edited.insert(at, 'X');
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            edited.as_bytes()
        );
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&view, &mut context);
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    settle(&view, &mut context);
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().entity_id(), identity);
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        );
    });
    // Editing, mode switches and image settlement can replace the InlineFlow
    // pieces. Re-find a guarded native boundary in the current painted frame.
    until(&mut context, |context| {
        view.read_with(context, |view, cx| view.geometry_is_current(cx))
    });
    let (position, _) = locate(&context);
    let modifiers = if cfg!(target_os = "macos") {
        Modifiers {
            platform: true,
            ..Modifiers::none()
        }
    } else {
        Modifiers::control()
    };
    context.simulate_mouse_down(position, MouseButton::Left, modifiers);
    context.simulate_mouse_up(position, MouseButton::Left, modifiers);
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        context.run_until_parked();
        if app.read_with(&context, |app, _| {
            app.active_buffer()
                .is_some_and(|buffer| buffer.path == target)
        }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "modifier click did not open the owning LocalHost file"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(std::fs::read(&path).unwrap(), original.as_bytes());
}
fn native_extreme_nowrap_line_keeps_pointer_selection_and_source_exact(cx: &mut TestAppContext) {
    use gpui_kit::{MouseButton, point, px};
    let ordinary = "ordinary ".repeat(256);
    let source = format!("{ordinary}\n\n{} e\u{301} 👩‍💻 END", "w".repeat(200_000));
    let (_directory, _path, _app, mut context, _streams, view) = fixture!(cx, &source);
    for live in [true, false] {
        view.update_in(&mut context, |view, window, cx| {
            if view.is_live() != live {
                view.toggle_mode(window, cx);
            }
            view.set_soft_wrap(false, cx);
        });
        select(&view, &mut context, ordinary.len(), ordinary.len());
        until(&mut context, |context| {
            view.read_with(context, |view, cx| {
                view.geometry_is_current(cx)
                    && !view.reveal_caret
                    && view.scroll_offset().x < px(0.)
            })
        });
        let end = source.len();
        select(&view, &mut context, end, end);
        until(&mut context, |context| {
            view.read_with(context, |view, cx| {
                view.geometry_is_current(cx) && !view.reveal_caret
            })
        });
        let (start, finish) = view.read_with(&context, |view, cx| {
            assert!(
                !view.is_soft_wrap(),
                "safety wrapping must not change the preference"
            );
            let start = view.range_to_bounds(&(end - 1..end - 1), cx).unwrap();
            let finish = view.range_to_bounds(&(end..end), cx).unwrap();
            let viewport = view.scroll.bounds();
            assert!(viewport.contains(&start.center()) && viewport.contains(&finish.center()));
            assert_eq!(start.top(), finish.top());
            assert!(finish.left() > start.left());
            (start, finish)
        });
        context.update(|window, cx| {
            let at = point(
                start.left() + (finish.left() - start.left()) * 0.2,
                start.center().y,
            );
            window.pointer_down(at, MouseButton::Left, cx);
            window.pointer_up(at, MouseButton::Left, cx);
            assert_eq!(
                view.read(cx).selection(),
                Selection::caret(end - 1),
                "live={live}, pointer={at:?}, start={start:?}, finish={finish:?}",
            );
            window.press("shift-right", cx);
            assert_eq!(view.read(cx).selection(), Selection::new(end - 1, end));
            window.input("Q", cx);
        });
        let mut edited = source.clone();
        edited.replace_range(end - 1..end, "Q");
        assert_eq!(bytes(&view, &context), edited.as_bytes());
        context.dispatch_action(gpui_kit::component::input::Undo);
        until(&mut context, |context| {
            bytes(&view, context) == source.as_bytes()
        });
        assert_eq!(bytes(&view, &context), source.as_bytes());
    }
}

fn native_aligned_table_pointer_and_selection_follow_content(cx: &mut TestAppContext) {
    use gpui_kit::{MouseButton, point, px};
    let source =
        "<!-- table:grid cols=160,160 -->\n| **甲** | 乙 |\n| :---: | ---: |\n| 中文 | 日本 |";
    let (_directory, _path, _app, mut context, _streams, view) = fixture!(cx, source);
    for (text, centered) in [("甲", true), ("乙", false)] {
        let at = source.find(text).unwrap();
        let (start, end) = view.read_with(&context, |view, cx| {
            let start = view.range_to_bounds(&(at..at), cx).unwrap();
            let end = view.range_to_bounds(&(at..at + text.len()), cx).unwrap();
            let cell = view.table_geometry[&view.projection.leaf_at(at).unwrap().id];
            if centered {
                assert!(((start.left() + end.left()) / 2. - cell.center().x).abs() < px(1.));
            } else {
                assert!(start.left() > cell.center().x && end.left() <= cell.right());
            }
            (start, end)
        });
        context.update(|window, cx| {
            let position = point(
                start.left() + (end.left() - start.left()) * 0.2,
                start.center().y,
            );
            window.pointer_down(position, MouseButton::Left, cx);
            window.pointer_up(position, MouseButton::Left, cx);
            assert_eq!(view.read(cx).selection(), Selection::caret(at));
            window.press("shift-right", cx);
            assert_eq!(
                view.read(cx).selection(),
                Selection::new(at, at + text.len())
            );
            window.input("Q", cx);
        });
        assert_eq!(
            bytes(&view, &context),
            source.replacen(text, "Q", 1).as_bytes()
        );
        context.dispatch_action(gpui_kit::component::input::Undo);
        settle(&view, &mut context);
        assert_eq!(bytes(&view, &context), source.as_bytes());
    }
}
