use super::*;
use crate::ui::app::test_window::harness_with_tabs;
use gpui_kit::{px, size};

#[gpui_kit::test]
fn editable_document_typography_reflows_without_rewriting_source_or_history(
    cx: &mut gpui_kit::TestAppContext,
) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("typography.md");
    let source = "# Heading\n\nbody one\nbody two wrapping words wrapping words wrapping words wrapping words wrapping words wrapping words wrapping words wrapping words\n\n- item\n  - nested\n\n```rust\nlet x = 1;\n```\n\n| A | B |\n| --- | --- |\n| x | y |\n";
    let bytes = format!("\u{feff}{}", source.replace('\n', "\r\n"));
    std::fs::write(&path, bytes.as_bytes()).unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    context.simulate_resize(size(px(1200.), px(1200.)));
    app.update_in(&mut context, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let view = loop {
        context.run_until_parked();
        if let Some(view) = app.read_with(&context, |app, _| {
            app.active_editor_input().and_then(|input| input.markdown().cloned())
        }) {
            break view;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    let identity = view.read_with(&context, |view, _| view.document().entity_id());
    let settle = |context: &mut gpui_kit::VisualTestContext| {
        settle_live_geometry(&view, context);
        for _ in 0..4 {
            context.update(|window, cx| window.render_frame(cx));
            context.run_until_parked();
        }
    };
    settle(&mut context);
    let body = source.find("body one").unwrap();
    let next = source.find("body two").unwrap();
    let item = source.find("item").unwrap();
    let nested = source.find("nested").unwrap();
    let end = source.find("\n\n- item").unwrap();
    let mut wide_lines = None;
    for (width, rem) in [(1200., 16.), (720., 20.)] {
        context.simulate_resize(size(px(width), px(1200.)));
        context.update(|_, cx| {
            cx.global_mut::<crate::core::config::Config>().ui_font_size = rem;
            gpui_kit::component::Theme::update(cx, |theme| theme.font_size = px(rem));
        });
        settle(&mut context);
        context.update(|window, cx| {
            let surface = window.find("markdown-source").bounds();
            let view = view.read(cx);
            assert!(view.is_live(), "the default production consumer must be editable live");
            let heading = view.range_to_bounds(&(2..2), cx).unwrap();
            let first = view.range_to_bounds(&(body..body), cx).unwrap();
            let second = view.range_to_bounds(&(next..next), cx).unwrap();
            // Fixed reference: 16px body, 23.2px lines and 28px page inset.
            // Assert scale relationships through the production leaf geometry.
            assert!((heading.size.height / first.size.height - 1.8).abs() < 0.05);
            assert!((second.top() - first.top() - px(rem * 1.45)).abs() < px(1.));
            assert!((first.left() - surface.left() - px(rem * 1.75)).abs() < px(1.));
            assert!(second.right() <= surface.right());
            let last = view.range_to_bounds(&(end..end), cx).unwrap();
            let lines = (last.top() - second.top()) / px(rem * 1.45);
            if let Some(wide) = wide_lines {
                assert!(lines > wide, "narrower, larger text must reflow the editable content");
            } else {
                wide_lines = Some(lines);
            }
            let list = view.range_to_bounds(&(item..item), cx).unwrap();
            let child = view.range_to_bounds(&(nested..nested), cx).unwrap();
            assert!(child.left() - list.left() >= px(rem * 2.));
            assert_eq!(view.document().entity_id(), identity);
            assert_eq!(view.document().read(cx).revision(), 0);
            assert_eq!(view.document().read(cx).encode().unwrap(), bytes.as_bytes());
        });
    }
    // Use pointer + native text input, not a renderer-specific mutation seam.
    context.update(|window, cx| {
        let target = view.read(cx).range_to_bounds(&(next..next), cx).unwrap().center();
        window.pointer_down(target, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(target, gpui_kit::MouseButton::Left, cx);
        window.input("新", cx);
        window.press("enter", cx);
    });
    settle(&mut context);
    let edited = source.replacen("body two", "新\nbody two", 1);
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).source().to_string(), edited);
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    context.dispatch_action(gpui_kit::component::input::Redo);
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    context.run_until_parked();
    view.read_with(&context, |view, cx| {
        assert!(!view.is_live());
        assert_eq!(view.document().entity_id(), identity);
        assert_eq!(view.document().read(cx).source().to_string(), edited);
    });
    context.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    settle(&mut context);
    context.dispatch_action(crate::core::actions::EditorSave);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        context.run_until_parked();
        if app.read_with(&context, |app, _| {
            app.active_buffer().is_some_and(|file| !file.dirty && !file.saving)
        }) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let expected = bytes.replacen("body two", "新\r\nbody two", 1);
    assert_eq!(std::fs::read(&path).unwrap(), expected.as_bytes());
    app.update_in(&mut context, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.open_file_in_editor(&path, window, cx);
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        context.run_until_parked();
        if let Some(reopened) = app.read_with(&context, |app, _| {
            app.active_editor_input().and_then(|input| input.markdown().cloned())
        }) {
            reopened.read_with(&context, |view, cx| {
                assert_ne!(view.document().entity_id(), identity);
                assert_eq!(view.document().read(cx).encode().unwrap(), expected.as_bytes());
            });
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
