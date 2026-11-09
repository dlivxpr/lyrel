//! Consumer regressions for the two-axis review. These drive the production window.
use super::{FollowLink, SourcePresentation};
use crate::ui::app::test_window::harness_with_tabs;
use crate::ui::code_editor::markdown::{Selection, projection::Kind};
use gpui_kit::test::{TestEventExt as _, TestWindowExt as _};
use gpui_kit::{
    AppContext as _, Entity, Focusable as _, TestAppContext, VisualContext as _, VisualTestContext,
};
use std::time::{Duration, Instant};

pub(super) fn native_cases() -> [crate::native_markdown::Case; 5] {
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
    ]
}

#[track_caller]
fn until(context: &mut VisualTestContext, mut ready: impl FnMut(&mut VisualTestContext) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        context.executor().advance_clock(Duration::from_millis(100));
        context.run_until_parked();
        context.update(|window, cx| window.render_frame(cx));
        if ready(context) {
            return;
        }
        assert!(Instant::now() < deadline, "consumer did not settle");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn settle(view: &Entity<SourcePresentation>, context: &mut VisualTestContext) {
    until(context, |context| {
        view.read_with(context, |view, cx| {
            view.is_live()
                && view.projection.revision == view.document().read(cx).revision()
                && !view
                    .projection
                    .leaves
                    .iter()
                    .any(|leaf| matches!(leaf.kind, Kind::PendingSource))
                && view.geometry_is_current(cx)
        })
    });
}
fn bytes(view: &Entity<SourcePresentation>, context: &VisualTestContext) -> Vec<u8> {
    view.read_with(context, |view, cx| {
        view.document().read(cx).encode().unwrap()
    })
}
fn select(
    view: &Entity<SourcePresentation>,
    context: &mut VisualTestContext,
    start: usize,
    end: usize,
) {
    view.update_in(context, |view, window, cx| {
        view.select_range(start..end, cx);
        view.focus_handle(cx).focus(window, cx);
    });
}
macro_rules! fixture {
    ($cx:expr, $source:expr) => {
        fixture!($cx, $source, false)
    };
    ($cx:expr, $source:expr, $local:expr) => {
        fixture!($cx, $source, $local, "review.md")
    };
    ($cx:expr, $source:expr, $local:expr, $name:expr) => {{
        let directory = crate::ui::code_editor::test_host::canonical_tempdir();
        let path = directory.path().join($name);
        std::fs::write(&path, $source).unwrap();
        let (app, mut context, streams) = harness_with_tabs($cx, 1);
        let host = crate::ui::code_editor::test_host::FileHost::new(lyrel_core::host::HostId(2041));
        app.update_in(&mut context, |app, window, cx| {
            if $local {
                app.open_file_in_editor(&path, window, cx);
            } else {
                app.editor_open_on_host(host, &path, window, cx);
            }
        });
        let mut opened = None;
        until(&mut context, |context| {
            opened = app.read_with(context, |app, _| {
                app.active_editor_input()
                    .and_then(|input| input.markdown().cloned())
            });
            opened.is_some()
        });
        let view = opened.unwrap();
        view.update_in(&mut context, |view, window, cx| {
            if !view.is_live() {
                view.toggle_mode(window, cx);
            }
            view.focus_handle(cx).focus(window, cx);
        });
        settle(&view, &mut context);
        (directory, path, app, context, streams, view)
    }};
}

#[gpui_kit::test]
fn review_auto_pair_reference_prose_fence_and_shared_source_history(cx: &mut TestAppContext) {
    let source = "alpha \n\n```rust\ncode \n```\n\n`raw`\n";
    let (_dir, _path, _app, mut context, _streams, view) = fixture!(cx, source);
    for at in [
        source.find("alpha ").unwrap() + 6,
        source.find("code ").unwrap() + 5,
    ] {
        for (open, close) in [("(", ")"), ("[", "]"), ("{", "}"), ("\"", "\""), ("'", "'")] {
            select(&view, &mut context, at, at);
            context.update(|window, cx| window.input(open, cx));
            let paired = format!("{}{}{}{}", &source[..at], open, close, &source[at..]);
            assert_eq!(bytes(&view, &context), paired.as_bytes());
            context.update(|window, cx| window.press("backspace", cx));
            assert_eq!(bytes(&view, &context), source.as_bytes());
            context.dispatch_action(gpui_kit::component::input::Undo);
            assert_eq!(bytes(&view, &context), paired.as_bytes());
            context.update(|window, cx| window.input(close, cx));
            assert_eq!(
                bytes(&view, &context),
                paired.as_bytes(),
                "closer is navigation, not another edit"
            );
            context.dispatch_action(gpui_kit::component::input::Undo);
            settle(&view, &mut context);
            assert_eq!(bytes(&view, &context), source.as_bytes());
        }
    }
    select(&view, &mut context, 0, 5);
    context.update(|window, cx| window.input("(", cx));
    assert_eq!(
        bytes(&view, &context),
        source.replacen("alpha", "(alpha)", 1).as_bytes()
    );
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    context.dispatch_action(gpui_kit::component::input::Undo);
    assert_eq!(bytes(&view, &context), source.as_bytes());
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    settle(&view, &mut context);
    let raw = source.find("raw").unwrap() + 1;
    select(&view, &mut context, raw, raw);
    context.update(|window, cx| window.input("(", cx));
    assert_eq!(
        bytes(&view, &context),
        format!("{}({}", &source[..raw], &source[raw..]).as_bytes()
    );
}

#[gpui_kit::test]
fn review_auto_pair_wraps_paragraphs_without_rewriting_source_or_crossing_raw_objects(
    cx: &mut TestAppContext,
) {
    let source = "alpha one\r\n\r\nbeta two\n\n`raw`\n";
    let (_dir, _path, _app, mut context, _streams, view) = fixture!(cx, source);
    let normalized = source.replace("\r\n", "\n");
    let end = normalized.find("two").unwrap();
    select(&view, &mut context, end, 6);
    context.update(|window, cx| window.input("(", cx));
    let wrapped = "alpha (one\r\n\r\nbeta )two\n\n`raw`\n";
    assert_eq!(bytes(&view, &context), wrapped.as_bytes());
    context.dispatch_action(gpui_kit::component::input::Undo);
    assert_eq!(bytes(&view, &context), source.as_bytes());
    context.dispatch_action(gpui_kit::component::input::Redo);
    assert_eq!(bytes(&view, &context), wrapped.as_bytes());
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&view, &mut context);
    let raw = normalized.find("raw").unwrap();
    select(&view, &mut context, 6, raw + 1);
    context.update(|window, cx| window.input("(", cx));
    // Crossing an inline-code object remains ordinary replacement, not pairing.
    assert_eq!(bytes(&view, &context), b"alpha (aw`\n");
    context.dispatch_action(gpui_kit::component::input::Undo);
    assert_eq!(bytes(&view, &context), source.as_bytes());
}

#[gpui_kit::test]
fn review_table_copy_keeps_metadata_codec_selection_and_history(cx: &mut TestAppContext) {
    let source = "before\r\n\r\n<!-- table:striped cols=120,240 -->\r\n| A | B |\n| --- | --- |\r\n| one | two |\n\nafter";
    let (_dir, _path, _app, mut context, _streams, view) = fixture!(cx, source);
    let normalized = source.replace("\r\n", "\n");
    let at = normalized.find("two").unwrap();
    select(&view, &mut context, at, at);
    context.dispatch_action(crate::ui::code_editor::markdown::tables::CopyTableAsMarkdown);
    until(&mut context, |context| {
        context.update(|_, cx| {
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .is_some_and(|text| text.starts_with("<!-- table:striped cols=120,240 -->"))
        })
    });
    context.update(|_, cx| {
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().unwrap(),
            "<!-- table:striped cols=120,240 -->\r\n| A | B |\n| --- | --- |\r\n| one | two |"
        );
        assert_eq!(
            cx.read_from_clipboard().unwrap().metadata().unwrap(),
            "<table><tr><td>A</td><td>B</td></tr><tr><td>one</td><td>two</td></tr></table>"
        );
        assert_eq!(view.read(cx).selection(), Selection::caret(at));
        assert_eq!(view.read(cx).document().read(cx).revision(), 0);
    });
    assert_eq!(bytes(&view, &context), source.as_bytes());
}

#[gpui_kit::test]
fn review_same_document_heading_and_explicit_anchor_follow_without_host_open(
    cx: &mut TestAppContext,
) {
    let source = "# Same\n\n# Same\n\n<a id=\"explicit\"></a>\n\n[heading](#same-1) and [literal](#explicit)\n";
    let (_dir, _path, app, mut context, _streams, view) = fixture!(cx, source);
    for (label, target) in [
        ("heading", source.find("# Same\n\n<a").unwrap()),
        ("literal", source.find("<a id").unwrap()),
    ] {
        let at = source.find(label).unwrap();
        select(&view, &mut context, at, at);
        context.dispatch_action(FollowLink);
        settle(&view, &mut context);
        view.read_with(&context, |view, cx| {
            assert!(view.selection().range().is_empty());
            let destination = view.range_to_bounds(&(target..target), cx).unwrap();
            assert_eq!(
                view.range_to_bounds(&view.selection().range(), cx),
                Some(destination),
                "navigation lands at the visible target, regardless of hidden marker coordinates"
            );
            if label == "heading" {
                assert_ne!(Some(destination), view.range_to_bounds(&(0..0), cx));
            }
        });
        app.read_with(&context, |app, _| {
            assert_eq!(app.editor.buffers.iter().count(), 1)
        });
    }
    assert_eq!(bytes(&view, &context), source.as_bytes());
}

#[gpui_kit::test]
fn review_image_width_form_accepts_modal_input_but_rejects_external_source_change(
    cx: &mut TestAppContext,
) {
    let source = "![本地图片](image.svg)\n";
    let (_dir, _path, _app, mut context, _streams, view) = fixture!(cx, source);
    select(&view, &mut context, 2, 2);
    context.update(|window, cx| window.click("markdown-image-edit", cx));
    context.update(|window, cx| {
        window.click("markdown-image-width", cx);
        window.input("240", cx);
        window.click("markdown-image-apply", cx);
    });
    until(&mut context, |context| {
        context.update(|window, _| window.try_find("markdown-image-width").is_none())
    });
    assert_eq!(
        bytes(&view, &context),
        "![本地图片](image.svg){width=240}\n".as_bytes()
    );
    context.dispatch_action(gpui_kit::component::input::Undo);
    assert_eq!(bytes(&view, &context), source.as_bytes());
    select(&view, &mut context, 2, 2);
    context.update(|window, cx| window.click("markdown-image-edit", cx));
    view.update_in(&mut context, |view, window, cx| {
        view.command(
            super::Command::Reload {
                text: "![external](image.svg)\n".into(),
                format: super::TextFormat::default(),
            },
            window,
            cx,
        );
    });
    context.update(|window, cx| {
        window.click("markdown-image-width", cx);
        window.input("320", cx);
        window.click("markdown-image-apply", cx);
        assert!(window.find("markdown-image-width").visible());
    });
    assert_eq!(
        bytes(&view, &context),
        "![external](image.svg)\n".as_bytes()
    );
}

#[gpui_kit::test]
fn review_formula_cancel_after_plain_input_keeps_original_bytes_and_history(
    cx: &mut TestAppContext,
) {
    let source = "$x^2 + 1$\r\n\r\nkeep\n";
    let (_dir, _path, app, mut context, _streams, view) = fixture!(cx, source);
    until(&mut context, |context| {
        context.update(|window, _| window.try_find("markdown-math-ready-x^2 + 1").is_some())
    });
    context.update(|window, cx| {
        window.click("markdown-math-ready-x^2 + 1", cx);
        window.input("9", cx);
    });
    assert_eq!(bytes(&view, &context), source.as_bytes());
    context.update(|window, cx| window.click("formula-cancel", cx));
    // The toolbar dispatches an Action after this window update has returned.
    until(&mut context, |context| {
        context.update(|window, _| window.try_find("markdown-formula-editor").is_none())
    });
    context.update(|window, cx| {
        assert!(view.read(cx).focus_handle(cx).is_focused(window));
    });
    assert_eq!(bytes(&view, &context), source.as_bytes());
    app.read_with(&context, |app, _| {
        assert!(!app.active_buffer().unwrap().dirty)
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    assert_eq!(bytes(&view, &context), source.as_bytes());
    context.dispatch_action(gpui_kit::component::input::Redo);
    assert_eq!(bytes(&view, &context), source.as_bytes());
}

#[gpui_kit::test]
fn review_formula_palette_wraps_selection_and_reload_preserves_uncommitted_input(
    cx: &mut TestAppContext,
) {
    let source = "$x$\n\nkeep\n";
    let (_dir, path, app, mut context, _streams, view) = fixture!(cx, source, true);
    until(&mut context, |context| {
        context.update(|window, _| window.try_find("markdown-math-ready-x").is_some())
    });
    context.update(|window, cx| {
        window.click("markdown-math-ready-x", cx);
        window.press("secondary-a", cx);
        window.click("formula-symbols", cx);
        window.click("frac", cx);
        window.press("secondary-a", cx);
        window.press("secondary-c", cx);
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().unwrap(),
            "\\frac{x}{}"
        );
        window.click("formula-cancel", cx);
    });
    assert_eq!(bytes(&view, &context), source.as_bytes());
    context.update(|window, cx| {
        window.click("markdown-math-ready-x", cx);
        window.input("9", cx);
    });
    std::fs::write(&path, format!("{source}external\n")).unwrap();
    // Drive the real file-change handler without depending on OS watch startup.
    app.update_in(&mut context, |app, window, cx| {
        app.editor.buffers.external_change(
            &path,
            window,
            cx,
            |app| &mut app.editor.buffers,
            crate::ui::code_editor::EditorPanelState::buffer_edited,
        );
    });
    until(&mut context, |context| {
        app.read_with(context, |app, _| {
            app.active_buffer().unwrap().conflict.is_some()
        })
    });
    assert_eq!(bytes(&view, &context), source.as_bytes());
    context.update(|window, cx| {
        assert!(window.find("markdown-formula-editor").visible());
        window.press("secondary-a", cx);
        window.press("secondary-c", cx);
        assert!(
            cx.read_from_clipboard()
                .unwrap()
                .text()
                .unwrap()
                .contains('9')
        );
        window.click("formula-commit", cx);
    });
    assert!(
        String::from_utf8(bytes(&view, &context))
            .unwrap()
            .contains('9')
    );
    context.dispatch_action(gpui_kit::component::input::Undo);
    assert_eq!(bytes(&view, &context), source.as_bytes());
}

#[gpui_kit::test]
fn review_link_hover_shows_host_excerpt_without_editing_source_and_dismisses(
    cx: &mut TestAppContext,
) {
    let source = "[preview](target.md#ignored)\n\nkeep\n";
    // Native temporary paths belong to LocalHost; non-local Host paths are POSIX.
    let (dir, _path, _app, mut context, _streams, view) = fixture!(cx, source, true);
    std::fs::write(
        dir.path().join("target.md"),
        "# Target title\n\nFirst **paragraph** with $x$.\n\nsecond\n",
    )
    .unwrap();
    let at = source.find("preview").unwrap() + 2;
    select(&view, &mut context, source.len() - 1, source.len() - 1);
    settle(&view, &mut context);
    let position = view.read_with(&context, |view, cx| {
        view.range_to_bounds(&(at..at), cx).unwrap().center()
    });
    context.update(|window, cx| window.pointer_move(position, None, cx));
    // The invisible trigger is installed on the next frame, as in the reference.
    context.update(|window, cx| {
        window.render_frame(cx);
        window.pointer_move(position, None, cx);
    });
    context.run_until_parked();
    context.executor().advance_clock(Duration::from_millis(400));
    context.run_until_parked();
    context.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("markdown-link-preview").is_none());
    });
    until(&mut context, |context| {
        context.update(|window, _| {
            window
                .try_find("markdown-link-preview")
                .is_some_and(|preview| {
                    preview.label().is_some_and(|text| {
                        text.contains("Target title") && text.contains("First paragraph")
                    })
                })
        })
    });
    context.update(|window, _| {
        assert!(window.find("markdown-link-preview").visible());
    });
    assert_eq!(bytes(&view, &context), source.as_bytes());
    select(&view, &mut context, source.len() - 1, source.len() - 1);
    context.update(|window, cx| window.input("X", cx));
    until(&mut context, |context| {
        context.update(|window, _| window.try_find("markdown-link-preview").is_none())
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&view, &mut context);
    context.update(|window, cx| {
        window.pointer_move(
            gpui_kit::point(gpui_kit::px(1.), gpui_kit::px(1.)),
            None,
            cx,
        )
    });
    until(&mut context, |context| {
        context.update(|window, _| window.try_find("markdown-link-preview").is_none())
    });
    assert_eq!(bytes(&view, &context), source.as_bytes());
}

#[gpui_kit::test]
fn review_wrapped_link_hover_ignores_nonlabel_space_after_resize(cx: &mut TestAppContext) {
    let source = format!(
        "prefix before the link [{}\ntail](target.md#ignored) trailing prose\n\nkeep\n",
        "preview words ".repeat(12),
    );
    let (dir, _path, _app, mut context, _streams, view) = fixture!(cx, &source, true);
    std::fs::write(
        dir.path().join("target.md"),
        "# Target title\n\nFirst paragraph\n",
    )
    .unwrap();
    let at = source.find("tail]").unwrap() + 2;
    let label_start = source.find('[').unwrap() + 1;
    let label_end = source.find("](target").unwrap();
    select(&view, &mut context, source.len() - 1, source.len() - 1);
    for width in [1100., 900.] {
        cx.simulate_window_resize(
            context.window_handle(),
            gpui_kit::size(gpui_kit::px(width), gpui_kit::px(1400.)),
        );
        settle(&view, &mut context);
        let (label, gap) = view.read_with(&context, |view, cx| {
            let start = view
                .range_to_bounds(&(label_start..label_start), cx)
                .unwrap();
            let end = view.range_to_bounds(&(label_end..label_end), cx).unwrap();
            let label = view.range_to_bounds(&(at..at), cx).unwrap().center();
            let gap = gpui_kit::point((start.left() + end.right()) / 2., end.center().y);
            assert!(start.top() < end.top(), "label must span visual rows");
            assert!(
                gap.x > end.right(),
                "gap must be beyond the final label row"
            );
            assert!(
                start.union(&end).contains(&gap),
                "gap exposes the old union trigger"
            );
            (label, gap)
        });
        context.update(|window, cx| window.pointer_move(gap, None, cx));
        context.run_until_parked();
        context.executor().advance_clock(Duration::from_millis(600));
        context.run_until_parked();
        context.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("markdown-link-preview").is_none());
            window.pointer_move(label, None, cx);
        });
        context.update(|window, cx| window.pointer_move(label, None, cx));
        until(&mut context, |context| {
            context.update(|window, _| {
                window
                    .try_find("markdown-link-preview")
                    .is_some_and(|preview| {
                        preview.visible()
                            && preview
                                .label()
                                .is_some_and(|text| text.contains("First paragraph"))
                    })
            })
        });
        context.update(|window, cx| window.pointer_move(gap, None, cx));
        until(&mut context, |context| {
            context.update(|window, _| window.try_find("markdown-link-preview").is_none())
        });
        assert_eq!(bytes(&view, &context), source.as_bytes());
    }
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.dispatch_action(gpui_kit::component::input::Redo);
    assert_eq!(bytes(&view, &context), source.as_bytes());
}

#[gpui_kit::test]
fn review_table_formula_session_remeasures_owning_row_before_source_commit(
    cx: &mut TestAppContext,
) {
    let source = "| $x$ | neighbor |\n| --- | --- |\n| lower | stable |\n";
    let (_dir, _path, _app, mut context, _streams, view) = fixture!(cx, source);
    until(&mut context, |context| {
        context.update(|window, _| window.try_find("markdown-math-ready-x").is_some())
    });
    context.update(|window, cx| window.click("markdown-math-ready-x", cx));
    let lower = source.find("lower").unwrap();
    let neighbor = source.find("neighbor").unwrap();
    let row_gap = |context: &VisualTestContext| {
        view.read_with(context, |view, cx| {
            view.range_to_bounds(&(lower..lower), cx).unwrap().top()
                - view
                    .range_to_bounds(&(neighbor..neighbor), cx)
                    .unwrap()
                    .top()
        })
    };
    settle(&view, &mut context);
    let before = row_gap(&context);
    context.update(|window, cx| {
        window.press("secondary-a", cx);
        window.click("formula-symbols", cx);
        window.click("frac", cx);
    });
    until(&mut context, |context| row_gap(context) > before);
    assert_eq!(
        bytes(&view, &context),
        source.as_bytes(),
        "session layout must not require a source commit"
    );
    context.update(|window, cx| window.click("formula-cancel", cx));
    until(&mut context, |context| row_gap(context) <= before);
    assert_eq!(bytes(&view, &context), source.as_bytes());
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
    let assert_bounds = |bounds: gpui_kit::Bounds<gpui_kit::Pixels>, expected: gpui_kit::Point<gpui_kit::Pixels>| {
        assert_eq!(bounds.top(), expected.y, "caret must stay on the independently located visual row");
        assert!((bounds.left() - expected.x).abs() < px(0.5), "caret must retain the row's expected glyph column");
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
            view.bounds_for_range(utf16..utf16, view.scroll.bounds(), window, cx).unwrap(),
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
        sibling.command(super::Command::Replace {
            range: start..start + 4,
            text: "keep".into(),
            kind: super::EditKind::Command,
        }, window, cx);
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
            point(bounds.left() + advance * 2., bounds.top() + geometry.line_height()),
            geometry.caret_height(),
        )
    });
    let modifiers = if cfg!(target_os = "macos") {
        Modifiers { platform: true, ..Modifiers::none() }
    } else { Modifiers::control() };
    context.simulate_mouse_down(point(upper.x - px(1.), upper.y + height / 2.), MouseButton::Left, modifiers);
    context.update(|window, cx| {
        window.pointer_move(point(target.x, target.y + height / 2.), Some(MouseButton::Left), cx);
    });
    context.simulate_mouse_up(point(target.x, target.y + height / 2.), MouseButton::Left, modifiers);
    settle(&view, &mut context);
    assert_eq!(view.read_with(&context, |view, _| view.selection()), Selection::new(at, at + 2 * "界".len()));
    context.dispatch_action(gpui_kit::component::input::MoveLeft);
    settle(&view, &mut context);
    view.read_with(&context, |view, cx| {
        assert_eq!(view.selection(), Selection::caret(at));
        let caret = view.range_to_bounds(&(at..at), cx).unwrap();
        assert_eq!(caret.top(), upper.y);
        assert!((caret.left() - upper.x).abs() < px(0.5));
    });
    assert_eq!(app.read_with(&context, |app, _| app.active_buffer().unwrap().path.clone()), path);
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
        Focusable as _, Modifiers, MouseButton, TestAppContext, VisualContext as _, point, px,
        size,
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

#[gpui_kit::test]
fn final_review_format_nested_delimiters_and_underscore_toggle(cx: &mut TestAppContext) {
    use gpui_kit::test::TestQueryExt as _;
    let source = "**bold** and __strong__\n";
    let (_dir, _path, _app, mut context, _streams, view) = fixture!(cx, source);
    select(&view, &mut context, 2, 6);
    context.update(|window, cx| window.press("secondary-i", cx));
    assert_eq!(bytes(&view, &context), b"***bold*** and __strong__\n");
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&view, &mut context);
    select(&view, &mut context, 0, 8);
    context.update(|window, cx| window.press("secondary-i", cx));
    assert_eq!(bytes(&view, &context), b"*bold* and __strong__\n");
    view.read_with(&context, |view, _| {
        assert_eq!(view.selection().range(), 0..6)
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&view, &mut context);
    select(&view, &mut context, 15, 21);
    context.update(|window, cx| window.press("secondary-b", cx));
    assert_eq!(bytes(&view, &context), b"**bold** and strong\n");
    context.dispatch_action(gpui_kit::component::input::Undo);
    select(&view, &mut context, 0, 8);
    context.update(|window, cx| window.input("**_bold_**", cx));
    select(&view, &mut context, 0, 10);
    context.update(|window, cx| window.click("markdown-format-menu", cx));
    context.run_until_parked();
    context.update(|window, cx| {
        assert_eq!(
            window
                .find_by_label(crate::ui::i18n::t(
                    crate::ui::i18n::L10nKey::MarkdownFormatItalic,
                ))
                .checked(),
            Some(true)
        );
        window.press("escape", cx);
    });
    context.run_until_parked();
    context.update(|window, cx| window.press("secondary-i", cx));
    assert_eq!(bytes(&view, &context), source.as_bytes());
    view.read_with(&context, |view, _| {
        assert_eq!(view.selection().range(), 0..8)
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    assert_eq!(bytes(&view, &context), b"**_bold_** and __strong__\n");
    context.dispatch_action(gpui_kit::component::input::Undo);
    assert_eq!(bytes(&view, &context), source.as_bytes());
}

#[gpui_kit::test]
fn final_review_format_preserves_link_destination_and_reference_label(cx: &mut TestAppContext) {
    let source = "[body](https://example.com) and [other][label]\n\n[label]: https://example.org\n\n<https://auto.example.com> <user@example.com> https://bare.example.net bare@example.net\n";
    let (_dir, _path, _app, mut context, _streams, view) = fixture!(cx, source);
    for text in [
        "https://example.com",
        "label",
        "https://auto.example.com",
        "user@example.com",
        "https://bare.example.net",
        "bare@example.net",
    ] {
        let at = source.find(text).unwrap();
        select(&view, &mut context, at, at + text.len());
        context.update(|window, cx| window.press("secondary-b", cx));
        assert_eq!(bytes(&view, &context), source.as_bytes());
    }
    select(&view, &mut context, 1, 5);
    context.update(|window, cx| window.press("secondary-b", cx));
    assert_eq!(
        bytes(&view, &context),
        b"[**body**](https://example.com) and [other][label]\n\n[label]: https://example.org\n\n<https://auto.example.com> <user@example.com> https://bare.example.net bare@example.net\n"
    );
}

#[gpui_kit::test]
fn final_review_copy_contents_with_empty_selection_and_stale_cut(cx: &mut TestAppContext) {
    use super::clipboard::{CopyContents, CopyContentsMarkdown};
    let source = "9. **bold**\r\n\r\n| A | B |\n| --- | --- |\n| x | y |\n";
    let (_dir, _path, _app, mut context, _streams, view) = fixture!(cx, source);
    select(&view, &mut context, 0, 0);
    context.dispatch_action(CopyContents);
    until(&mut context, |context| {
        context.update(|_, cx| {
            cx.read_from_clipboard().is_some_and(|item| {
                item.text().as_deref() == Some(source) && item.metadata().is_some()
            })
        })
    });
    context.update(|_, cx| {
        let item = cx.read_from_clipboard().unwrap();
        let html = item.metadata().unwrap();
        assert!(html.contains("<strong>bold</strong>"));
        assert!(html.contains("<table>"));
        assert_eq!(view.read(cx).selection(), Selection::caret(0));
    });
    context.dispatch_action(CopyContentsMarkdown);
    until(&mut context, |context| {
        context.update(|_, cx| {
            cx.read_from_clipboard().is_some_and(|item| {
                item.text().as_deref() == Some(source) && item.metadata().is_none()
            })
        })
    });
    let bold = source.find("bold").unwrap();
    select(&view, &mut context, bold, bold + 4);
    context.update(|window, cx| {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("sentinel".into()));
        window.press("secondary-x", cx);
        window.input("NEW", cx);
    });
    settle(&view, &mut context);
    assert_eq!(
        bytes(&view, &context),
        source.replacen("bold", "NEW", 1).as_bytes()
    );
    context.update(|_, cx| {
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().as_deref(),
            Some("sentinel")
        );
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    assert_eq!(bytes(&view, &context), source.as_bytes());
}

#[gpui_kit::test]
fn final_review_nested_fence_source_return_targets_editable_body(cx: &mut TestAppContext) {
    use crate::ui::i18n::{L10nKey, t};
    use gpui_kit::test::TestQueryExt as _;
    let source = "> ```rust\n> body\n> ```\n\nkeep\n";
    let (_dir, _path, _app, mut context, _streams, view) = fixture!(cx, source);
    for key in [L10nKey::MarkdownCodeSource, L10nKey::MarkdownCodeLive] {
        context.update(|window, cx| {
            let button = window.find_by_label(t(key)).bounds().center();
            window.pointer_down(button, gpui_kit::MouseButton::Left, cx);
            window.pointer_up(button, gpui_kit::MouseButton::Left, cx);
        });
        context.run_until_parked();
    }
    view.read_with(&context, |view, _| {
        assert_eq!(view.selection().head(), source.find("body").unwrap())
    });
    context.update(|window, cx| window.input("X", cx));
    assert_eq!(
        bytes(&view, &context),
        b"> ```rust\n> Xbody\n> ```\n\nkeep\n"
    );
    context.dispatch_action(gpui_kit::component::input::Undo);
    assert_eq!(bytes(&view, &context), source.as_bytes());
}

#[gpui_kit::test]
fn final_review_single_line_mdx_formula_entry_and_top_boundary(cx: &mut TestAppContext) {
    let source = "$$x$$\n\nkeep\n";
    let (_dir, _path, _app, mut context, _streams, view) =
        fixture!(cx, source, false, "formula.mdx");
    until(&mut context, |context| {
        context.update(|window, _| window.try_find("markdown-math-ready-x").is_some())
    });
    context.update(|window, cx| window.click("markdown-math-ready-x", cx));
    until(&mut context, |context| {
        context.update(|window, _| window.try_find("markdown-formula-editor").is_some())
    });
    context.update(|window, cx| {
        window.press("home", cx);
        window.press("up", cx);
        assert!(window.find("markdown-formula-editor").visible());
        window.press("left", cx);
        assert!(window.find("markdown-formula-editor").visible());
    });
    context.dispatch_action(super::math_edit::EditFormulaSource);
    context.run_until_parked();
    view.read_with(&context, |view, _| {
        assert!(!view.is_live());
        assert_eq!(view.selection().head(), 2);
    });
    context.update(|window, cx| window.input("Y", cx));
    assert_eq!(bytes(&view, &context), b"$$Yx$$\n\nkeep\n");
}

#[gpui_kit::test]
fn final_review_property_row_whitespace_opens_value_form(cx: &mut TestAppContext) {
    let source = "key:: value\n\nkeep\n";
    let (_dir, _path, _app, mut context, _streams, view) = fixture!(cx, source);
    context.update(|window, cx| window.click("markdown-property-space-0", cx));
    context.run_until_parked();
    context.update(|window, cx| {
        assert!(window.find("markdown-property-value").visible());
        window.press("secondary-a", cx);
        window.input("changed", cx);
        window.click("markdown-property-apply", cx);
    });
    context.run_until_parked();
    assert_eq!(bytes(&view, &context), b"key:: changed\n\nkeep\n");
    context.dispatch_action(gpui_kit::component::input::Undo);
    assert_eq!(bytes(&view, &context), source.as_bytes());
}

#[gpui_kit::test]
fn final_review_table_slash_excludes_block_insertion_and_keeps_inline_commands(
    cx: &mut TestAppContext,
) {
    let source = "| A | B |\n| --- | --- |\n| value | other |\n";
    let (_dir, _path, _app, mut context, _streams, view) = fixture!(cx, source);
    let at = source.find("value").unwrap();
    select(&view, &mut context, at, at + 5);
    context.update(|window, cx| window.input("/code", cx));
    until(&mut context, |context| {
        context.update(|window, _| window.try_find("markdown-slash-entry-code").is_some())
    });
    context.update(|window, cx| {
        let entry = window.find("markdown-slash-entry-code");
        let expected = crate::ui::i18n::t_fmt(
            crate::ui::i18n::L10nKey::MarkdownSlashBlockUnavailable,
            &[(
                "label",
                crate::ui::i18n::t(crate::ui::i18n::L10nKey::MarkdownBlockCode),
            )],
        );
        assert_eq!(entry.label(), Some(expected.as_str()));
        window.press("enter", cx);
    });
    assert_eq!(
        bytes(&view, &context),
        source.replacen("value", "/code", 1).as_bytes()
    );
    select(&view, &mut context, at, at + 5);
    context.update(|window, cx| window.input("/table", cx));
    until(&mut context, |context| {
        context.update(|window, _| window.try_find("markdown-slash-entry-table").is_some())
    });
    context.update(|window, cx| window.press("enter", cx));
    assert_eq!(
        bytes(&view, &context),
        source.replacen("value", "/table", 1).as_bytes()
    );
    select(&view, &mut context, at, at + 6);
    context.update(|window, cx| window.input("/bold", cx));
    until(&mut context, |context| {
        context.update(|window, _| window.try_find("markdown-slash-entry-bold").is_some())
    });
    context.update(|window, cx| window.press("enter", cx));
    assert_eq!(
        bytes(&view, &context),
        source.replacen("value", "****", 1).as_bytes()
    );
}

#[gpui_kit::test]
fn final_review_duplicate_embeds_keep_focus_and_scroll_after_shared_prefix_edit(
    cx: &mut TestAppContext,
) {
    let source = "before\n\n![[target.md]]\n\n![[target.md]]\n";
    let (dir, _path, _app, mut context, _streams, view) = fixture!(cx, source, true);
    std::fs::write(
        dir.path().join("target.md"),
        "a long readonly paragraph\n\n".repeat(80),
    )
    .unwrap();
    until(&mut context, |context| {
        context.update(|window, _| {
            window.try_find("markdown-embed-flow-root-1").is_some()
                && window.try_find("markdown-embed-content-2").is_some()
                && window
                    .try_find("markdown-embed-error-root-2")
                    .is_some_and(|error| {
                        error.label()
                            == Some(crate::ui::i18n::t(
                                crate::ui::i18n::L10nKey::MarkdownEmbedDuplicate,
                            ))
                    })
        })
    });
    let sibling = context.update(|_, cx| {
        let document = view.read(cx).document().clone();
        cx.new(|cx| SourcePresentation::new(document, true, cx))
    });
    let relative = context.update(|window, cx| {
        window.click("markdown-embed-content-1", cx);
        window.scroll(
            "markdown-embed-content-1",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(-140.))),
            cx,
        );
        assert_eq!(
            window.find("markdown-embed-content-1").focused(),
            Some(true)
        );
        let content = window.find("markdown-embed-content-1").bounds();
        let flow = window.find("markdown-embed-flow-root-1").bounds();
        assert!(flow.top() < content.top());
        flow.top() - content.top()
    });
    sibling.update_in(&mut context, |sibling, window, cx| {
        sibling.command(
            super::Command::Replace {
                range: 0..0,
                text: "inserted before both embeds\n\n".into(),
                kind: super::EditKind::Command,
            },
            window,
            cx,
        );
    });
    settle(&view, &mut context);
    context.update(|window, _| {
        assert_eq!(
            window.find("markdown-embed-content-1").focused(),
            Some(true)
        );
        let content = window.find("markdown-embed-content-1").bounds();
        let flow = window.find("markdown-embed-flow-root-1").bounds();
        assert_eq!(flow.top() - content.top(), relative);
        assert_eq!(
            window.find("markdown-embed-content-2").focused(),
            Some(false)
        );
        assert_eq!(
            window.find("markdown-embed-error-root-2").label(),
            Some(crate::ui::i18n::t(
                crate::ui::i18n::L10nKey::MarkdownEmbedDuplicate
            ))
        );
    });
    assert_eq!(
        bytes(&view, &context),
        format!("inserted before both embeds\n\n{source}").as_bytes()
    );
    sibling.update_in(&mut context, |sibling, window, cx| {
        sibling.command(super::Command::Undo, window, cx);
    });
    settle(&view, &mut context);
    context.update(|window, _| {
        assert_eq!(
            window.find("markdown-embed-content-1").focused(),
            Some(true)
        );
        let content = window.find("markdown-embed-content-1").bounds();
        let flow = window.find("markdown-embed-flow-root-1").bounds();
        assert_eq!(flow.top() - content.top(), relative);
        assert_eq!(
            window.find("markdown-embed-content-2").focused(),
            Some(false)
        );
        assert_eq!(
            window.find("markdown-embed-error-root-2").label(),
            Some(crate::ui::i18n::t(
                crate::ui::i18n::L10nKey::MarkdownEmbedDuplicate
            ))
        );
    });
    assert_eq!(bytes(&view, &context), source.as_bytes());
}

#[gpui_kit::test]
fn final_review_large_document_format_commands_preserve_order_and_cancel_external_edits(
    cx: &mut TestAppContext,
) {
    let source = format!("start bold\n\n{}", "filler words\n\n".repeat(3000));
    let (_dir, _path, _app, mut context, _streams, view) = fixture!(cx, &source);
    select(&view, &mut context, 0, 0);
    context.update(|window, cx| {
        window.input("X", cx);
        view.update(cx, |view, cx| view.select_range(7..11, cx));
        window.press("secondary-b", cx);
        window.press("secondary-i", cx);
    });
    let combined = source.replacen("start bold", "Xstart ***bold***", 1);
    until(&mut context, |context| {
        bytes(&view, context) == combined.as_bytes()
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    assert_eq!(
        bytes(&view, &context),
        source
            .replacen("start bold", "Xstart **bold**", 1)
            .as_bytes()
    );
    context.dispatch_action(gpui_kit::component::input::Undo);
    assert_eq!(bytes(&view, &context), format!("X{source}").as_bytes());
    context.dispatch_action(gpui_kit::component::input::Undo);
    assert_eq!(bytes(&view, &context), source.as_bytes());
    settle(&view, &mut context);

    select(&view, &mut context, 0, 0);
    context.update(|window, cx| {
        window.input("X", cx);
        view.update(cx, |view, cx| view.select_range(7..11, cx));
        window.press("secondary-b", cx);
        window.press("secondary-i", cx);
        window.input("Y", cx);
        view.update(cx, |view, cx| view.select_range(7..8, cx));
        window.press("secondary-shift-u", cx);
    });
    let external = source.replacen("start bold", "Xstart <u>Y</u>", 1);
    until(&mut context, |context| {
        bytes(&view, context) == external.as_bytes()
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    assert_eq!(
        bytes(&view, &context),
        source.replacen("start bold", "Xstart Y", 1).as_bytes()
    );
    context.dispatch_action(gpui_kit::component::input::Undo);
    assert_eq!(bytes(&view, &context), format!("X{source}").as_bytes());
    context.dispatch_action(gpui_kit::component::input::Undo);
    assert_eq!(bytes(&view, &context), source.as_bytes());
}
