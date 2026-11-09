use super::support::{settle_geometry, spelling_fixture};
use super::{Diagnostic, grapheme_line_box};
use crate::ui::app::test_window::harness_with_tabs;
use crate::ui::code_editor::markdown::projection::Kind;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Pixels, TestAppContext, VisualContext as _, point, px, size};
use unicode_segmentation::UnicodeSegmentation as _;
pub(super) fn native_cases() -> [crate::native_markdown::Case; 4] {
    use crate::native_markdown::Case;
    [
        Case::new(
            "ui::code_editor::markdown::source::spelling::tests::spelling_long_code_remains_editable_with_offscreen_diagnostics",
            spelling_long_code_remains_editable_with_offscreen_diagnostics,
        ),
        Case::new(
            "ui::code_editor::markdown::source::spelling::tests::spelling_full_mixed_paragraph_keeps_visible_text_editable_around_tall_atoms",
            spelling_full_mixed_paragraph_keeps_visible_text_editable_around_tall_atoms,
        ),
        Case::new(
            "ui::code_editor::markdown::source::spelling::tests::spelling_wrapped_starts_follow_native_prose_heading_list_quote_and_zoom",
            spelling_wrapped_starts_follow_native_prose_heading_list_quote_and_zoom,
        ),
        Case::new(
            "ui::code_editor::markdown::source::spelling::tests::spelling_native_hard_breaks_and_image_formula_fragments_do_not_bridge_rows",
            spelling_native_hard_breaks_and_image_formula_fragments_do_not_bridge_rows,
        ),
    ]
}
fn spelling_long_code_remains_editable_with_offscreen_diagnostics(cx: &mut TestAppContext) {
    use gpui_kit::test::TestEventExt as _;
    let original = format!("# Huge code\n\n```text\n{}\n```\n", "wide ".repeat(80_000));
    let body = original.find("wide").unwrap();
    let (_directory, path, _app, mut context, _streams, view) =
        spelling_fixture!(cx, original.as_bytes());
    view.update_in(&mut context, |view, _, cx| {
        view.set_soft_wrap(false, cx);
        view.select_range(body..body, cx);
    });
    settle_geometry(&view, &mut context);
    view.update_in(&mut context, |view, _, cx| {
        view.spelling.invalidate();
        view.set_spelling_diagnostics(
            vec![Diagnostic {
                ranges: vec![body..body + 400_000],
                word: "deterministic OS diagnostic".into(),
            }],
            cx,
        );
    });
    context.update(|window, cx| window.render_frame(cx));
    let position = view.read_with(&context, |view, _| {
        let leaf = view.projection.leaf_at(body).unwrap();
        let geometry = &view.geometry.as_ref().unwrap().1[&leaf.id];
        geometry.position_for_index(0) + point(px(0.), geometry.caret_height() / 2.)
    });
    context.update(|window, cx| {
        window.pointer_down(position, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(position, gpui_kit::MouseButton::Left, cx);
        window.input("X", cx);
    });
    settle_geometry(&view, &mut context);
    let mut expected = original.clone();
    expected.insert(body, 'X');
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document.read(cx).encode().unwrap(),
            expected.as_bytes()
        );
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle_geometry(&view, &mut context);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document.read(cx).encode().unwrap(),
            original.as_bytes()
        );
    });
    assert_eq!(std::fs::read(path).unwrap(), original.as_bytes());
}

fn spelling_full_mixed_paragraph_keeps_visible_text_editable_around_tall_atoms(
    cx: &mut TestAppContext,
) {
    use gpui_kit::test::TestEventExt as _;
    let original = format!(
        "A {}![tall](tall.png) $\\frac{{a}}{{b}}$ {}",
        "a ".repeat(100_000),
        "b ".repeat(100_000),
    );
    assert_eq!(original.len(), 400_034);
    let body = original.find("$ ").unwrap() + 2;
    let (_directory, path, _app, mut context, _streams, view) =
        spelling_fixture!(cx, original.as_bytes());
    image::RgbaImage::from_pixel(12, 96, image::Rgba([0, 0, 255, 255]))
        .save(path.parent().unwrap().join("tall.png"))
        .unwrap();
    settle_geometry(&view, &mut context);
    context.update(|window, cx| window.click("markdown-load-images", cx));
    for wrap in [true, false] {
        view.update_in(&mut context, |view, window, cx| {
            view.set_soft_wrap(wrap, cx);
            view.select_range(body..body, cx);
            // Loading is a command surface with its own focus. A source
            // selection is revealed only by its focused presentation.
            view.focus.focus(window, cx);
        });
        settle_geometry(&view, &mut context);
        view.update_in(&mut context, |view, _, cx| {
            view.spelling.invalidate();
            view.set_spelling_diagnostics(
                vec![Diagnostic {
                    ranges: vec![0..original.len()],
                    word: "deterministic OS diagnostic".into(),
                }],
                cx,
            );
        });
        context.update(|window, cx| window.render_frame(cx));
        let position = view.read_with(&context, |view, cx| {
                let leaf = view.projection.leaf_at(body).unwrap();
                let geometry = &view.geometry.as_ref().unwrap().1[&leaf.id];
                let display = leaf.display_offset(body);
                let caret = view.caret_bounds(body, cx).unwrap();
                let viewport = view.scroll.bounds();
                let visible = super::visible_spelling_text(leaf, geometry, viewport);
                if !wrap {
                    assert!(
                        view.scroll.max_offset().x <= viewport.size.width,
                        "the complete extreme line must stay in bounded visual rows"
                    );
                }
                assert!(
                    caret.top() >= viewport.top()
                        && caret.bottom() <= viewport.bottom()
                        && caret.left() >= viewport.left()
                        && caret.right() <= viewport.right(),
                    "post-atom caret must actually be visible: wrap={wrap}, caret={caret:?}, viewport={viewport:?}, display={display}, visible={visible:?}"
                );
                assert!(
                    visible.iter().any(|range| range.contains(&display)),
                    "native visible text after the high atoms retains its diagnostic: wrap={wrap}, caret={caret:?}, viewport={viewport:?}, display={display}, visible={visible:?}"
                );
                caret.center()
            });
        context.update(|window, cx| {
            window.pointer_down(position, gpui_kit::MouseButton::Left, cx);
            window.pointer_up(position, gpui_kit::MouseButton::Left, cx);
            window.input("X", cx);
        });
        let mut expected = original.clone();
        expected.insert(body, 'X');
        view.read_with(&context, |view, cx| {
            assert_eq!(
                view.document.read(cx).encode().unwrap(),
                expected.as_bytes()
            );
        });
        context.dispatch_action(gpui_kit::component::input::Undo);
        settle_geometry(&view, &mut context);
        view.read_with(&context, |view, cx| {
            assert_eq!(
                view.document.read(cx).encode().unwrap(),
                original.as_bytes()
            );
        });
    }
    assert_eq!(std::fs::read(path).unwrap(), original.as_bytes());
}

fn spelling_wrapped_starts_follow_native_prose_heading_list_quote_and_zoom(
    cx: &mut TestAppContext,
) {
    // Native font shaping, still in the real workbench's test window; this
    // is not a GPU screenshot or an operating-system window acceptance.
    let original = format!(
        "\u{feff}{}\r\n\r\n# {}\n\n- {}\r\n\r\n> {}\n\nkeep\r\n",
        "W ".repeat(80),
        "W ".repeat(80),
        "田".repeat(140),
        "e\u{301} ".repeat(100),
    );
    let (_directory, path, _app, mut context, _streams, view) =
        spelling_fixture!(cx, original.as_bytes());
    cx.simulate_window_resize(context.window_handle(), size(px(1000.), px(1800.)));
    let revision = view.read_with(&context, |view, cx| view.document.read(cx).revision());
    let mut previous_heights: Option<Vec<Pixels>> = None;
    for rem in [16., 22.] {
        // Match the production typography setting consumed by the root;
        // setting Window rem alone is overwritten by the next root render.
        context.update(|_, cx| {
            cx.global_mut::<crate::core::config::Config>().ui_font_size = rem;
            gpui_kit::component::Theme::update(cx, |theme| theme.font_size = px(rem));
        });
        settle_geometry(&view, &mut context);
        let heights = view.read_with(&context, |view, cx| {
            assert!(view.geometry_is_current(cx));
            let (_, geometries) = view.geometry.as_ref().unwrap();
            let mut heights = Vec::new();
            for kind in [Kind::Prose, Kind::Heading(1)] {
                let leaf = view
                    .projection
                    .leaves
                    .iter()
                    .find(|leaf| leaf.kind == kind)
                    .unwrap();
                heights.push(geometries[&leaf.id].caret_height());
            }
            let mut covered = [false; 4];
            for leaf in &view.projection.leaves {
                let slot = match leaf.kind {
                    Kind::Prose if leaf.text.starts_with('W') => 0,
                    Kind::Heading(1) => 1,
                    Kind::List(_) => 2,
                    Kind::Quote(_) => 3,
                    _ => continue,
                };
                let geometry = &geometries[&leaf.id];
                let origin = geometry.bounds().left();
                assert_eq!(geometry.position_for_index(0).x, origin);
                for (ix, grapheme) in leaf.text.grapheme_indices(true) {
                    if grapheme.chars().all(char::is_whitespace) {
                        continue;
                    }
                    let a = geometry.position_for_index(ix);
                    let b = geometry.position_for_index(ix + grapheme.len());
                    if a.y == b.y {
                        continue;
                    }
                    let segment = grapheme_line_box(geometry, ix, grapheme)
                        .expect("a whole paragraph's continuation starts with a spelling segment");
                    // Independent caret hit tests prove that the emitted
                    // segment belongs to this grapheme on the next row.
                    assert_eq!(segment.left(), origin);
                    assert_eq!(segment.top(), b.y);
                    let leading = point(
                        segment.left() + segment.size.width * 0.25,
                        segment.center().y,
                    );
                    let raw_leading = geometry.index_for_position(leading);
                    assert!(leaf.text.is_char_boundary(raw_leading));
                    assert!(ix <= raw_leading && raw_leading < ix + grapheme.len());
                    assert_eq!(
                        super::super::navigation::hit_test_caret(&leaf.text, geometry, leading,)
                            .index,
                        ix
                    );
                    let trailing = geometry.index_for_position(point(
                        segment.right() - segment.size.width * 0.25,
                        segment.center().y,
                    ));
                    // Native shaping can return a raw caret inside a
                    // combining grapheme, whose physical point need not be
                    // its end. The editor must normalize the right-hand hit
                    // to the complete grapheme boundary below.
                    assert!(leaf.text.is_char_boundary(trailing));
                    assert!(ix < trailing && trailing <= ix + grapheme.len());
                    assert_eq!(
                        super::super::navigation::hit_test_caret(
                            &leaf.text,
                            geometry,
                            point(
                                segment.right() - segment.size.width * 0.25,
                                segment.center().y,
                            ),
                        )
                        .index,
                        ix + grapheme.len()
                    );
                    // The same repeated grapheme on the first row is an
                    // independent, native-shaped full-advance reference.
                    let reference = leaf
                        .text
                        .grapheme_indices(true)
                        .find_map(|(start, text)| {
                            let a = geometry.position_for_index(start);
                            let b = geometry.position_for_index(start + text.len());
                            (text == grapheme && a.y == b.y && b.x > a.x).then_some(b.x - a.x)
                        })
                        .unwrap();
                    assert!((segment.size.width - reference).abs() < px(0.01));
                    covered[slot] = true;
                }
            }
            assert_eq!(
                covered, [true; 4],
                "each real layout must exercise soft wrap"
            );
            assert_eq!(
                view.document.read(cx).encode().unwrap(),
                original.as_bytes()
            );
            assert_eq!(view.document.read(cx).revision(), revision);
            heights
        });
        if let Some(previous) = previous_heights {
            for (before, after) in previous.into_iter().zip(&heights) {
                assert!(*after > before, "zoom must produce fresh native geometry");
            }
        }
        previous_heights = Some(heights);
    }
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.dispatch_action(gpui_kit::component::input::Redo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document.read(cx).encode().unwrap(),
            original.as_bytes()
        );
        assert_eq!(view.document.read(cx).revision(), revision);
    });
    assert_eq!(std::fs::read(path).unwrap(), original.as_bytes());
}

fn spelling_native_hard_breaks_and_image_formula_fragments_do_not_bridge_rows(
    cx: &mut TestAppContext,
) {
    let original = format!(
        "\u{feff}hard first  \r\nhard second\n\n{}![alt](missing.png){}\r\n\r\n{}$x${}\n\nkeep\r\n",
        "image words ".repeat(25),
        " after words".repeat(25),
        "formula words ".repeat(25),
        " after words".repeat(25),
    );
    let (_directory, path, _app, mut context, _streams, view) =
        spelling_fixture!(cx, original.as_bytes());
    cx.simulate_window_resize(context.window_handle(), size(px(1000.), px(1800.)));
    settle_geometry(&view, &mut context);
    let revision = view.read_with(&context, |view, cx| {
        assert!(view.geometry_is_current(cx));
        let (_, geometries) = view.geometry.as_ref().unwrap();
        let mut hard_breaks = 0;
        let mut fragmented = [false; 2];
        let mut local_segments = [0; 2];
        for leaf in &view.projection.leaves {
            let Some(geometry) = geometries.get(&leaf.id) else {
                continue;
            };
            let slot = if leaf.image_urls().next().is_some() {
                Some(0)
            } else if leaf.formulas().next().is_some() {
                Some(1)
            } else {
                None
            };
            for (ix, grapheme) in leaf.text.grapheme_indices(true) {
                if grapheme.contains(['\r', '\n']) {
                    assert!(grapheme_line_box(geometry, ix, grapheme).is_none());
                    hard_breaks += 1;
                    continue;
                }
                let Some(slot) = slot else {
                    continue;
                };
                // Atoms retain their own native bounds; this regression is
                // about the surrounding InlineFlow text fragments.
                if leaf.atom_source(ix).is_some() {
                    continue;
                }
                let a = geometry.position_for_index(ix);
                let b = geometry.position_for_index(ix + grapheme.len());
                let segment = grapheme_line_box(geometry, ix, grapheme);
                if a.y != b.y {
                    assert_ne!(
                        geometry.index_for_position(point(
                            geometry.position_for_index(0).x,
                            b.y + geometry.caret_height() / 2.,
                        )),
                        ix,
                        "fragment-end index must not masquerade as a whole-leaf wrapped start"
                    );
                    let segment =
                        segment.expect("fragment tail retains its own row's spelling segment");
                    assert_eq!(segment.top(), a.y);
                    assert_eq!(segment.left(), a.x);
                    assert!(segment.right() > segment.left());
                    fragmented[slot] = true;
                } else if a.x != b.x {
                    let segment =
                        segment.expect("inline fragment keeps its ordinary spelling segment");
                    assert_eq!(segment.left(), a.x.min(b.x));
                    assert_eq!(segment.right(), a.x.max(b.x));
                    assert_eq!(segment.top(), a.y);
                    local_segments[slot] += 1;
                }
            }
        }
        assert!(
            hard_breaks > 0,
            "hard break must remain in native displayed geometry"
        );
        assert_eq!(
            fragmented, [true; 2],
            "image and formula must each exercise a fragment row boundary"
        );
        assert!(local_segments.into_iter().all(|count| count > 0));
        assert_eq!(
            view.document.read(cx).encode().unwrap(),
            original.as_bytes()
        );
        view.document.read(cx).revision()
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.dispatch_action(gpui_kit::component::input::Redo);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document.read(cx).encode().unwrap(),
            original.as_bytes()
        );
        assert_eq!(view.document.read(cx).revision(), revision);
    });
    assert_eq!(std::fs::read(path).unwrap(), original.as_bytes());
}
