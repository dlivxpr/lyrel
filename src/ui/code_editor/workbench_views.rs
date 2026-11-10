//! 生产工作台多 presentation 消费者；原生 registry 复用相同交互。
use super::*;
use crate::ui::app::test_window::harness_with_tabs;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{EntityInputHandler as _, TestAppContext, VisualTestContext};
use lyrel_markdown_editor::EditorState as MarkdownEditor;
use std::time::{Duration, Instant};

fn settle(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        window.render_frame(cx);
    });
    cx.run_until_parked();
}
fn until(cx: &mut VisualTestContext, mut ready: impl FnMut(&mut VisualTestContext) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !ready(cx) {
        assert!(
            Instant::now() < deadline,
            "workbench view transition did not settle"
        );
        settle(cx);
        cx.executor().advance_clock(Duration::from_millis(50));
        std::thread::sleep(Duration::from_millis(5));
    }
    settle(cx);
}
fn view(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> Entity<MarkdownEditor> {
    app.read_with(cx, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    })
}

pub(crate) fn shared_context_and_ime_handoff(cx: &mut TestAppContext) {
    let dir = test_host::canonical_tempdir();
    let path = dir.path().join("views.md");
    std::fs::write(&path, b"# First\r\nalpha\nbeta\r\n").unwrap();
    let (app, mut cx, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut cx, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| app.active_buffer().is_some())
    });
    let first = view(&app, &cx);
    app.update_in(&mut cx, |app, window, cx| {
        app.toggle_document_preview(window, cx);
        first.update(cx, |view, cx| view.select_range(14..18, cx));
        assert!(app.editor_split(window, cx));
    });
    settle(&mut cx);
    let second = view(&app, &cx);
    assert_ne!(first.entity_id(), second.entity_id());
    second.read_with(&cx, |second, cx| {
        assert_eq!(second.document(), first.read(cx).document());
        assert!(
            !second.is_live(),
            "split inherits the originating source presentation"
        );
        assert_eq!(
            second.selected_range(),
            14..18,
            "split starts at the originating context"
        );
    });
    second.update_in(&mut cx, |view, window, cx| {
        view.select_range(8..8, cx);
        view.replace_and_mark_text_in_range(None, "中", Some(1..1), window, cx);
    });
    settle(&mut cx);
    first.read_with(&cx, |view, _| assert_eq!(view.selected_range(), 17..21));
    app.update_in(&mut cx, |app, window, cx| {
        assert!(app.editor_focus_group(true, window, cx))
    });
    first.update_in(&mut cx, |view, window, cx| {
        view.select_range(0..0, cx);
        view.replace_and_mark_text_in_range(None, "新", Some(1..1), window, cx);
    });
    // These are callbacks retained by the old OS input owner, not host edits.
    second.update_in(&mut cx, |view, window, cx| {
        view.unmark_text(window, cx);
        view.replace_and_mark_text_in_range(None, "STALE", Some(5..5), window, cx);
        view.replace_text_in_range(None, "STALE COMMIT", window, cx);
    });
    first.read_with(&cx, |view, cx| {
        assert!(
            view.document().read(cx).has_composition(),
            "stale unmark cannot finish the new lease"
        );
        assert_eq!(view.text(), "新# First\n中alpha\nbeta\n");
    });
    first.update_in(&mut cx, |view, window, cx| view.unmark_text(window, cx));
    cx.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut cx);
    first.read_with(&cx, |view, _| {
        assert_eq!(view.text(), "# First\n中alpha\nbeta\n")
    });
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    settle(&mut cx);
    assert_eq!(view(&app, &cx), second);
    cx.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut cx);
    second.read_with(&cx, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            b"# First\r\nalpha\nbeta\r\n"
        );
    });
}

#[cfg(test)]
#[gpui_kit::test]
fn split_preserves_context_rebases_selection_and_retires_only_old_ime_owner(
    cx: &mut TestAppContext,
) {
    shared_context_and_ime_handoff(cx);
}

#[cfg(test)]
#[gpui_kit::test]
fn secondary_pending_save_survives_tab_focus_close_and_reopen(cx: &mut TestAppContext) {
    use std::sync::atomic::Ordering;
    let dir = test_host::canonical_tempdir();
    let path = dir.path().join("pending.md");
    let other = dir.path().join("other.rs");
    std::fs::write(&path, b"---\r\ntitle: intact\n---\r\n<Unknown />\n").unwrap();
    std::fs::write(&other, b"fn untouched() {}\n").unwrap();
    let host = test_host::FileHost::new(HostId(240));
    let (app, mut cx, _streams) = harness_with_tabs(cx, 2);
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_open_on_host(host.clone(), &path, window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| app.active_buffer().is_some())
    });
    app.update_in(&mut cx, |app, window, cx| {
        assert!(app.editor_split(window, cx))
    });
    let second = view(&app, &cx);
    second.update_in(&mut cx, |view, _, cx| view.select_range(0..0, cx));
    cx.update(|window, cx| window.input("注", cx));
    settle(&mut cx);
    let release = host.pause_next_write();
    cx.dispatch_action(crate::core::actions::EditorSave);
    until(&mut cx, |_| host.entered.load(Ordering::SeqCst));
    app.update_in(&mut cx, |app, window, cx| {
        app.tabs.select(1);
        app.editor_open_on_host(host.clone(), &other, window, cx);
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| {
            app.active_buffer().is_some_and(|f| f.path == other)
        })
    });
    release.send(()).unwrap();
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| app.editor.buffers.iter().all(|f| !f.saving))
    });
    assert_eq!(std::fs::read(&other).unwrap(), b"fn untouched() {}\n");
    let expected = "注---\r\ntitle: intact\n---\r\n<Unknown />\n"
        .as_bytes()
        .to_vec();
    assert_eq!(std::fs::read(&path).unwrap(), expected);
    app.update_in(&mut cx, |app, window, cx| {
        app.tabs.select(0);
        app.focus_editor(window, cx);
        app.editor_close_file(0, window, cx);
        app.editor_close_file(0, window, cx);
        app.editor_open_on_host(host.clone(), &path, window, cx);
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| app.active_buffer().is_some())
    });
    view(&app, &cx).read_with(&cx, |view, cx| {
        assert_eq!(view.document().read(cx).encode().unwrap(), expected)
    });
}

pub(crate) fn focused_consumers_and_layout(cx: &mut TestAppContext) {
    use crate::core::config::DocumentLayout;
    use gpui_kit::component::{Theme, ThemeMode};
    let dir = test_host::canonical_tempdir();
    let path = dir.path().join("focus.md");
    let other = dir.path().join("other.md");
    std::fs::write(&path, b"# First\nalpha\n## Target\nbeta\n").unwrap();
    std::fs::write(&other, b"# Other\nuntouched\n").unwrap();
    let (app, mut cx, _streams) = harness_with_tabs(cx, 1);
    crate::ui::i18n::set_locale("en");
    cx.simulate_resize(gpui_kit::size(px(1600.), px(900.)));
    app.update_in(&mut cx, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| app.active_buffer().is_some())
    });
    let first = view(&app, &cx);
    app.update_in(&mut cx, |app, window, cx| {
        assert!(app.editor_split(window, cx))
    });
    let second = view(&app, &cx);
    app.update_in(&mut cx, |app, window, cx| {
        app.open_file_in_editor(&other, window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| {
            app.active_buffer().is_some_and(|f| f.path == other)
        })
    });
    // A menu command dispatched before a draw must follow actual input focus.
    cx.update(|window, cx| {
        first.read(cx).focus_handle(cx).focus(window, cx);
        window.focused(cx).unwrap().dispatch_action(
            &crate::core::actions::EditorGoToLine,
            window,
            cx,
        );
    });
    cx.update(|window, cx| {
        window.input("3:4", cx);
        window.press("enter", cx);
    });
    settle(&mut cx);
    app.read_with(&cx, |app, cx| {
        assert_eq!(app.active_buffer().unwrap().path, path);
        assert_eq!(
            app.active_editor_input()
                .unwrap()
                .read(cx)
                .cursor_position(),
            Position::new(2, 3)
        );
    });
    cx.update(|window, _| assert_eq!(window.find("status-cursor").label(), Some("Ln 3, Col 4")));
    app.update_in(&mut cx, |app, window, cx| {
        let id = app.active_buffer().unwrap().id();
        let outline = app.editor_outline(id, true, cx).unwrap();
        assert!(outline.symbols.iter().any(|symbol| symbol.name == "Target"));
        app.editor_go_to_symbol(window, cx);
        app.editor_symbol_preview(3, 2, cx);
        app.close_search(window, cx);
    });
    settle(&mut cx);
    first.read_with(&cx, |view, _| assert_eq!(view.selected_range(), 17..17));
    // Open search for the other presentation of this file, without rendering first.
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_focus_group(false, window, cx);
        let pos = app
            .tab_code()
            .unwrap()
            .files
            .iter()
            .position(|id| app.editor.buffers.get(*id).unwrap().path == path)
            .unwrap();
        app.editor_activate(pos, window, cx);
    });
    second.update_in(&mut cx, |view, _, cx| view.select_range(0..0, cx));
    cx.dispatch_action(gpui_kit::component::input::Search);
    cx.update(|window, cx| window.input("beta", cx));
    settle(&mut cx);
    second.read_with(&cx, |view, _| assert_eq!(view.selected_range(), 24..28));
    first.read_with(&cx, |view, _| assert_eq!(view.selected_range(), 17..17));
    cx.update(|window, cx| window.press("escape", cx));
    settle(&mut cx);
    app.update_in(&mut cx, |app, window, cx| {
        assert!(app.editor_has_focus(window, cx))
    });
    cx.update(|window, cx| window.input("new", cx));
    settle(&mut cx);
    let selected = second.read_with(&cx, |view, _| view.selected_range());
    for (width, font_size, mode, layout) in [
        (1600., 16., ThemeMode::Light, DocumentLayout::Dock),
        (1100., 20., ThemeMode::Dark, DocumentLayout::Fill),
        (560., 24., ThemeMode::Light, DocumentLayout::Dock),
        (1800., 16., ThemeMode::Dark, DocumentLayout::Dock),
    ] {
        app.update_in(&mut cx, |app, window, cx| {
            app.set_document_layout(layout, cx);
            Theme::change(mode, Some(window), cx);
            Theme::update(cx, |theme| theme.font_size = px(font_size));
            app.focus_previous_content(window, cx);
        });
        cx.simulate_resize(gpui_kit::size(px(width), px(900.)));
        settle(&mut cx);
        second.read_with(&cx, |view, _| assert_eq!(view.selected_range(), selected));
        app.update_in(&mut cx, |app, window, cx| {
            assert!(app.editor_has_focus(window, cx));
            assert_eq!(
                app.active_buffer()
                    .unwrap()
                    .input
                    .read(cx)
                    .text()
                    .to_string(),
                "# First\nalpha\n## Target\nnew\n"
            );
            let caret = second
                .read(cx)
                .caret_screen_bounds()
                .expect("displayed caret geometry");
            assert!(caret.left() >= px(0.) && caret.right() <= window.viewport_size().width);
            assert!(caret.top() >= px(0.) && caret.bottom() <= window.viewport_size().height);
        });
        cx.update(|window, _| assert!(window.find("status-cursor").visible()));
    }
    cx.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut cx);
    second.read_with(&cx, |view, _| {
        assert_eq!(view.text(), "# First\nalpha\n## Target\nbeta\n")
    });
}

#[cfg(test)]
#[gpui_kit::test]
fn status_search_outline_goto_and_layout_target_displayed_presentation(cx: &mut TestAppContext) {
    focused_consumers_and_layout(cx);
}

#[cfg(test)]
#[gpui_kit::test]
fn active_markdown_rename_to_mdx_retains_each_view_and_history(cx: &mut TestAppContext) {
    let dir = test_host::canonical_tempdir();
    let path = dir.path().join("rename.md");
    let renamed = dir.path().join("rename.mdx");
    std::fs::write(&path, b"---\r\ntitle: untouched\n---\r\n<Component />").unwrap();
    let host = test_host::FileHost::new(HostId(2401));
    let (app, mut cx, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_open_on_host(host.clone(), &path, window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| app.active_buffer().is_some())
    });
    let first = view(&app, &cx);
    app.update_in(&mut cx, |app, window, cx| {
        assert!(app.editor_split(window, cx))
    });
    let second = view(&app, &cx);
    second.update_in(&mut cx, |view, _, cx| view.select_range(0..0, cx));
    cx.update(|window, cx| window.input("注", cx));
    settle(&mut cx);
    std::fs::rename(&path, &renamed).unwrap();
    app.update_in(&mut cx, |app, _, cx| {
        app.editor_path_moved(HostId(2401), &path, &renamed, cx)
    });
    settle(&mut cx);
    for view in [&first, &second] {
        view.read_with(&cx, |view, cx| {
            assert!(view.is_source_only());
            assert!(!view.is_live());
            assert_eq!(view.document(), first.read(cx).document());
            assert!(view.text().starts_with("注---"));
        });
    }
    cx.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    cx.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut cx);
    second.read_with(&cx, |view, cx| {
        assert!(!view.is_live());
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            b"---\r\ntitle: untouched\n---\r\n<Component />"
        );
    });
    app.read_with(&cx, |app, _| {
        assert_eq!(app.active_buffer().unwrap().path, renamed)
    });
}

#[cfg(test)]
#[gpui_kit::test]
fn shared_tabs_accept_owned_blur_commit_and_keep_remaining_history(cx: &mut TestAppContext) {
    let dir = test_host::canonical_tempdir();
    let path = dir.path().join("tabs.md");
    std::fs::write(&path, b"alpha\r\nbeta\n").unwrap();
    let host = test_host::FileHost::new(HostId(2402));
    let (app, mut cx, _streams) = harness_with_tabs(cx, 2);
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_open_on_host(host.clone(), &path, window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| app.active_buffer().is_some())
    });
    let first = view(&app, &cx);
    app.update_in(&mut cx, |app, window, cx| {
        app.tabs.select(1);
        app.editor_open_on_host(host.clone(), &path, window, cx);
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, _| app.active_buffer().is_some())
    });
    let second = view(&app, &cx);
    assert_ne!(first, second);
    app.update_in(&mut cx, |app, window, cx| {
        app.tabs.select(0);
        app.focus_editor(window, cx);
    });
    first.update_in(&mut cx, |view, window, cx| {
        view.select_range(0..0, cx);
        view.replace_and_mark_text_in_range(None, "ni", Some(2..2), window, cx);
    });
    // OS focus may move before its final commit callback. The lease is still valid.
    app.update_in(&mut cx, |app, window, cx| {
        let terminal = app.tabs[0].pane.first_leaf().unwrap().focus_handle(cx);
        terminal.focus(window, cx);
    });
    first.update_in(&mut cx, |view, window, cx| {
        view.replace_text_in_range(None, "你", window, cx)
    });
    settle(&mut cx);
    first.read_with(&cx, |view, cx| {
        assert_eq!(view.text(), "你alpha\nbeta\n");
        assert!(!view.document().read(cx).has_composition());
    });
    app.update_in(&mut cx, |app, window, cx| {
        app.tabs.select(1);
        app.focus_editor(window, cx);
        app.editor_close_file(0, window, cx);
        app.tabs.select(0);
        app.focus_editor(window, cx);
    });
    settle(&mut cx);
    cx.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut cx);
    first.read_with(&cx, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            b"alpha\r\nbeta\n"
        )
    });
}
