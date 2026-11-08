use super::{FileHost, active_id, read_owned, wait_until};
use crate::ui::app::test_window::harness_with_tabs;
use crate::ui::code_editor::markdown::CopyMarkdownOnly;
use gpui_kit::{Focusable as _, TestAppContext};

#[gpui_kit::test]
fn clipboard_mixed_selection_cut_history_paste_and_owned_reopen(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("clipboard.md");
    let original = "Keep\r\n9. **alpha**\r\n42. beta\nuntouched __spelling__\r\n";
    std::fs::write(&path, original).unwrap();
    let host = FileHost::new(lyrel_core::host::HostId(227));
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut context, |app, window, cx| {
        app.editor_open_on_host(host.clone(), &path, window, cx)
    });
    wait_until(&mut context, |cx| active_id(&app, cx).is_some());
    let view = app.read_with(&context, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    // Opening a buffer installs the source projection first. Wait for the real
    // Live layout to collapse both the list marker and the bold opener before
    // asserting selection behavior at that shared visible boundary.
    let settle_live = |context: &mut gpui_kit::VisualTestContext| {
        wait_until(context, |context| {
            context.update(|window, cx| window.render_frame(cx));
            view.read_with(context, |view, cx| {
                let start = view.range_to_bounds(&(5..5), cx);
                view.is_live() && start.is_some() && start == view.range_to_bounds(&(10..10), cx)
            })
        });
    };
    settle_live(&mut context);
    view.update_in(&mut context, |view, window, cx| {
        view.focus_handle(cx).focus(window, cx);
        view.select_range(12..10, cx); // reverse partial selection inside bold
        view.add_selection(22..24, cx); // later selection, not the intervening syntax
    });
    context.dispatch_action(gpui_kit::component::input::Copy);
    context.update(|_, cx| {
        let item = cx.read_from_clipboard().unwrap();
        assert_eq!(item.text().unwrap(), "al\nbe");
        let html = item.metadata().unwrap();
        assert!(html.contains("<strong>al</strong>"), "{html}");
        assert!(!html.contains("alpha"));
        assert!(!html.contains("Keep"));
    });
    context.dispatch_action(gpui_kit::component::input::Cut);
    let cut = "Keep\r\n9. **pha**\r\n42. ta\nuntouched __spelling__\r\n";
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).encode().unwrap(), cut.as_bytes())
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Redo);
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).encode().unwrap(), cut.as_bytes())
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle_live(&mut context);
    view.update_in(&mut context, |view, _, cx| view.select_range(10..26, cx));
    context.dispatch_action(gpui_kit::component::input::Cut);
    let block_cut = "Keep\r\n\nuntouched __spelling__\r\n";
    context.update(|_, cx| {
        let item = cx.read_from_clipboard().unwrap();
        assert_eq!(item.text().unwrap(), "9. **alpha**\r\n42. beta");
        let html = item.metadata().unwrap();
        assert!(html.contains("<strong>alpha</strong>"), "{html}");
        assert!(!html.contains("Keep"));
        assert!(!html.contains("untouched"));
    });
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            block_cut.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Redo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            block_cut.as_bytes()
        )
    });
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle_live(&mut context);
    view.update_in(&mut context, |view, _, cx| view.select_range(10..26, cx));
    context.dispatch_action(CopyMarkdownOnly);
    context.update(|_, cx| {
        let item = cx.read_from_clipboard().unwrap();
        assert_eq!(item.text().unwrap(), "9. **alpha**\r\n42. beta");
        assert!(item.metadata().is_none());
    });
    view.update_in(&mut context, |view, _, cx| {
        let end = view.document().read(cx).source().len();
        view.select_range(end..end, cx);
    });
    context.dispatch_action(gpui_kit::component::input::Paste);
    let saved = format!("{original}9. **alpha**\r\n42. beta");
    context.dispatch_action(crate::core::actions::EditorSave);
    wait_until(&mut context, |cx| {
        app.read_with(cx, |app, _| {
            !app.active_buffer().unwrap().dirty && !app.active_buffer().unwrap().saving
        })
    });
    assert_eq!(read_owned(&host, &path).unwrap(), saved.as_bytes());
    app.update_in(&mut context, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.editor_open_on_host(host.clone(), &path, window, cx);
    });
    wait_until(&mut context, |cx| active_id(&app, cx).is_some());
    app.read_with(&context, |app, cx| {
        assert_eq!(
            app.active_editor_input()
                .unwrap()
                .markdown()
                .unwrap()
                .read(cx)
                .document()
                .read(cx)
                .encode()
                .unwrap(),
            saved.as_bytes()
        )
    });
}

#[gpui_kit::test]
fn clipboard_safe_objects_and_plain_priority_do_not_read_resources(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("safe.mdx");
    let original = "![secret](https://invalid.example/private.png)\n\n<Unknown onClick={fetch('https://invalid.example/run')}>dynamic</Unknown>\n\n| A | B |\n| - | - |\n| **x** | y |\n\n$x^2$\n\nkey:: value\n\n![[missing.md]]";
    std::fs::write(&path, original).unwrap();
    let host = FileHost::new(lyrel_core::host::HostId(1227));
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut context, |app, window, cx| {
        app.editor_open_on_host(host.clone(), &path, window, cx)
    });
    wait_until(&mut context, |cx| active_id(&app, cx).is_some());
    let view = app.read_with(&context, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    let _read_gate = host.pause_next_read();
    view.update_in(&mut context, |view, window, cx| {
        view.focus_handle(cx).focus(window, cx);
        view.select_range(0..original.len(), cx);
    });
    context.dispatch_action(gpui_kit::component::input::Copy);
    context.update(|_, cx| {
        let item = cx.read_from_clipboard().unwrap();
        assert_eq!(item.text().unwrap(), original);
        let html = item.metadata().unwrap();
        assert!(html.contains("<table>"));
        assert!(html.contains("<strong>x</strong>"));
        assert!(html.contains("&lt;Unknown"));
        assert!(!html.contains("<Unknown"));
        assert!(!html.contains("src=\"https:"));
        assert!(!html.contains("src=\"file:"));
        assert!(html.contains("<dl><dt>key</dt><dd>value</dd></dl>"));
        assert!(!html.contains("<script"));
        cx.write_to_clipboard(gpui_kit::ClipboardItem {
            entries: vec![
                gpui_kit::ClipboardEntry::String(gpui_kit::ClipboardString::new(
                    "**literal**".into(),
                )),
                gpui_kit::ClipboardEntry::ExternalPaths(gpui_kit::ExternalPaths(
                    vec![dir.path().join("do-not-read.png")].into(),
                )),
            ],
        });
    });
    context.dispatch_action(gpui_kit::component::input::Paste);
    view.read_with(&context, |view, cx| {
        assert_eq!(view.document().read(cx).source().to_string(), "**literal**")
    });
    assert!(!host.entered.load(std::sync::atomic::Ordering::SeqCst));
    context.dispatch_action(gpui_kit::component::input::Undo);
    view.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        )
    });
}
