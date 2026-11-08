//! Consumer regressions for the two-axis review. These drive the production window.
use super::{FollowLink, SourcePresentation};
use crate::ui::app::test_window::harness_with_tabs;
use crate::ui::code_editor::markdown::{Selection, projection::Kind};
use gpui_kit::test::{TestEventExt as _, TestWindowExt as _};
use gpui_kit::{Entity, Focusable as _, TestAppContext, VisualContext as _, VisualTestContext};
use std::time::{Duration, Instant};

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
                && view.range_to_bounds(&(0..0), cx).is_some()
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
    ($cx:expr, $source:expr, $local:expr) => {{
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("review.md");
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

#[gpui_kit::test]
fn first_soft_wrapped_link_grapheme_hovers_edits_and_opens(cx: &mut gpui_kit::TestAppContext) {
    native_link_grapheme_hovers_edits_and_opens(cx, false);
}

#[gpui_kit::test]
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

    let mut native_context = TestAppContext::build_with_text_system(
        gpui_kit::TestDispatcher::new(0),
        cx.test_function_name(),
        gpui_kit::platform::current_platform(true).text_system(),
    );
    let cx = &mut native_context;
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
