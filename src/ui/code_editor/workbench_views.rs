//! 生产工作台多 presentation 消费者；原生 registry 复用相同交互。
use crate::ui::app::LyrelApp;
use crate::ui::app::test_window::harness_with_tabs;
use crate::ui::code_editor::test_host;
use gpui_kit::component::input::Position;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    Entity, EntityInputHandler as _, Focusable as _, TestAppContext, VisualTestContext, px,
};
use lyrel_markdown_editor::EditorState as MarkdownEditor;
#[cfg(test)]
use lyrel_ui_base::host_ops::{HostId, SharedHost};
#[cfg(test)]
use lyrel_workbench_editor::code_editor::csv_document::CsvMode;
#[cfg(test)]
use lyrel_workbench_editor::code_editor::{BufferId, TabCode};
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
    let deadline = Instant::now() + Duration::from_secs(45);
    while !ready(cx) {
        assert!(
            Instant::now() < deadline,
            "workbench view transition did not settle"
        );
        settle(cx);
        cx.executor().advance_clock(Duration::from_millis(50));
        std::thread::sleep(Duration::from_millis(15));
    }
    settle(cx);
}
fn view(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> Entity<MarkdownEditor> {
    app.read_with(cx, |app, cx| {
        app.active_editor_input(cx)
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
        app.read_with(cx, |app, cx| app.active_buffer(cx).is_some())
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
fn closing_either_nonowner_preserves_composition_lease_and_history(cx: &mut TestAppContext) {
    for close_primary in [false, true] {
        let dir = test_host::canonical_tempdir();
        let path = dir.path().join("composition-close.md");
        let original = b"# First\r\nalpha\nbeta\r\n";
        std::fs::write(&path, original).unwrap();
        let (app, mut context, _streams) = harness_with_tabs(cx, 1);
        app.update_in(&mut context, |app, window, cx| {
            app.open_file_in_editor(&path, window, cx)
        });
        until(&mut context, |cx| {
            app.read_with(cx, |app, cx| app.active_buffer(cx).is_some())
        });
        let first = view(&app, &context);
        app.update_in(&mut context, |app, window, cx| {
            app.toggle_document_preview(window, cx);
            assert!(app.editor_split(window, cx));
        });
        settle(&mut context);
        let second = view(&app, &context);
        let (closing, owner) = if close_primary {
            (&first, &second)
        } else {
            (&second, &first)
        };
        app.update_in(&mut context, |app, window, cx| {
            if close_primary {
                assert!(app.editor_focus_group(true, window, cx));
            }
            owner.update(cx, |view, cx| view.select_range(8..8, cx));
            owner.read(cx).focus_handle(cx).focus(window, cx);
            owner.update(cx, |view, cx| {
                view.replace_and_mark_text_in_range(None, "中😀", Some(3..3), window, cx);
            });
            // Closing the other strip must preserve this already-focused
            // input, including the restoration performed by focus_editor.
            app.editor_close_file(0, window, cx);
            app.editor_sync(window, cx);
        });
        owner.read_with(&context, |view, cx| {
            assert!(!view.is_live(), "source presentation survives");
            assert!(view.document().read(cx).has_composition());
            assert_eq!(view.text(), "# First\n中😀alpha\nbeta\n");
        });
        owner.update_in(&mut context, |view, window, cx| {
            assert_eq!(view.marked_text_range(window, cx), Some(8..11));
        });
        closing.update_in(&mut context, |view, window, cx| {
            view.unmark_text(window, cx)
        });
        owner.update_in(&mut context, |view, window, cx| {
            view.replace_and_mark_text_in_range(None, "中文😀", Some(4..4), window, cx);
            view.unmark_text(window, cx);
        });
        settle(&mut context);
        assert!(app.read_with(&context, |app, cx| app.active_buffer(cx).unwrap().dirty));
        context.dispatch_action(gpui_kit::component::input::Undo);
        settle(&mut context);
        owner.read_with(&context, |view, cx| {
            assert_eq!(view.document().read(cx).encode().unwrap(), original);
        });
        assert!(!app.read_with(&context, |app, cx| app.active_buffer(cx).unwrap().dirty));
    }
}

#[cfg(test)]
#[gpui_kit::test]
fn representation_conversion_is_separate_from_adjacent_typing_undo(cx: &mut TestAppContext) {
    let dir = test_host::canonical_tempdir();
    let path = dir.path().join("format-history.md");
    std::fs::write(&path, b"body\n").unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut context, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    until(&mut context, |cx| {
        app.read_with(cx, |app, cx| app.active_buffer(cx).is_some())
    });
    let editor = view(&app, &context);
    editor.update_in(&mut context, |view, window, cx| {
        view.select_range(0..0, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| window.input("a", cx));
    app.update_in(&mut context, |app, _, cx| app.toggle_line_ending(cx));
    context.update(|window, cx| window.input("b", cx));
    settle(&mut context);
    for expected in [
        b"abody\r\n".as_slice(),
        b"abody\n".as_slice(),
        b"body\n".as_slice(),
    ] {
        context.dispatch_action(gpui_kit::component::input::Undo);
        settle(&mut context);
        editor.read_with(&context, |view, cx| {
            assert_eq!(view.document().read(cx).encode().unwrap(), expected)
        });
    }
}

#[cfg(test)]
#[gpui_kit::test]
fn noop_trim_preserves_the_previous_edits_undo_selection(cx: &mut TestAppContext) {
    let dir = test_host::canonical_tempdir();
    let path = dir.path().join("trim-history.md");
    std::fs::write(&path, b"first\nsecond\n").unwrap();
    let (app, mut context, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut context, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    until(&mut context, |cx| {
        app.read_with(cx, |app, cx| app.active_buffer(cx).is_some())
    });
    let editor = view(&app, &context);
    editor.update_in(&mut context, |view, window, cx| {
        view.select_range(0..5, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    context.update(|window, cx| window.input("X", cx));
    editor.update_in(&mut context, |view, _, cx| {
        let end = view.text().len();
        view.select_range(end..end, cx);
    });
    context.dispatch_action(lyrel_ui_base::actions::EditorTrimTrailingWhitespace);
    settle(&mut context);
    context.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut context);
    editor.read_with(&context, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            b"first\nsecond\n"
        );
        assert_eq!(view.selected_range(), 0..5);
    });
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
        app.read_with(cx, |app, cx| app.active_buffer(cx).is_some())
    });
    app.update_in(&mut cx, |app, window, cx| {
        assert!(app.editor_split(window, cx))
    });
    let second = view(&app, &cx);
    second.update_in(&mut cx, |view, _, cx| view.select_range(0..0, cx));
    cx.update(|window, cx| window.input("注", cx));
    settle(&mut cx);
    let release = host.pause_next_write();
    cx.dispatch_action(lyrel_ui_base::actions::EditorSave);
    until(&mut cx, |_| host.entered.load(Ordering::SeqCst));
    app.update_in(&mut cx, |app, window, cx| {
        app.tabs.select(1);
        app.editor_open_on_host(host.clone(), &other, window, cx);
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| {
            app.active_buffer(cx).is_some_and(|f| f.path == other)
        })
    });
    release.send(()).unwrap();
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| {
            app.editor.read(cx).buffers.iter().all(|f| !f.saving)
        })
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
        app.read_with(cx, |app, cx| app.active_buffer(cx).is_some())
    });
    view(&app, &cx).read_with(&cx, |view, cx| {
        assert_eq!(view.document().read(cx).encode().unwrap(), expected)
    });
}

pub(crate) fn focused_consumers_and_layout(cx: &mut TestAppContext) {
    use gpui_kit::component::{Theme, ThemeMode};
    use lyrel_core::core::config::DocumentLayout;
    let dir = test_host::canonical_tempdir();
    let path = dir.path().join("focus.md");
    let other = dir.path().join("other.md");
    std::fs::write(&path, b"# First\nalpha\n## Target\nbeta\n").unwrap();
    std::fs::write(&other, b"# Other\nuntouched\n").unwrap();
    let (app, mut cx, _streams) = harness_with_tabs(cx, 1);
    lyrel_ui_base::i18n::set_locale("en");
    cx.simulate_resize(gpui_kit::size(px(1600.), px(900.)));
    app.update_in(&mut cx, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| app.active_buffer(cx).is_some())
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
        app.read_with(cx, |app, cx| {
            app.active_buffer(cx).is_some_and(|f| f.path == other)
        })
    });
    // A menu command dispatched before a draw must follow actual input focus.
    cx.update(|window, cx| {
        first.read(cx).focus_handle(cx).focus(window, cx);
        window.focused(cx).unwrap().dispatch_action(
            &lyrel_ui_base::actions::EditorGoToLine,
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
        assert_eq!(app.active_buffer(cx).unwrap().path, path);
        assert_eq!(
            app.active_editor_input(cx)
                .unwrap()
                .text()
                .unwrap()
                .read(cx)
                .cursor_position(),
            Position::new(2, 3)
        );
    });
    cx.update(|window, _| assert_eq!(window.find("status-cursor").label(), Some("Ln 3, Col 4")));
    first.update_in(&mut cx, |view, _, cx| view.select_range(0..0, cx));
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_go_to_symbol(window, cx)
    });
    settle(&mut cx);
    cx.update(|window, cx| {
        window.input("Target", cx);
        window.press("enter", cx);
    });
    settle(&mut cx);
    app.read_with(&cx, |app, cx| {
        assert_eq!(
            app.active_editor_input(cx)
                .unwrap()
                .text()
                .unwrap()
                .read(cx)
                .cursor_position()
                .line,
            2
        );
    });
    let first_after_outline = first.read_with(&cx, |view, _| view.selected_range());
    // Open search for the other presentation of this file, without rendering first.
    app.update_in(&mut cx, |app, window, cx| {
        app.editor_focus_group(false, window, cx);
        let pos = app
            .tab_code()
            .unwrap()
            .files
            .iter()
            .position(|id| app.editor.read(cx).buffers.get(*id).unwrap().path == path)
            .unwrap();
        app.editor_activate(pos, window, cx);
    });
    second.update_in(&mut cx, |view, _, cx| view.select_range(0..0, cx));
    cx.dispatch_action(gpui_kit::component::input::Search);
    cx.update(|window, cx| window.input("beta", cx));
    settle(&mut cx);
    second.read_with(&cx, |view, _| assert_eq!(view.selected_range(), 24..28));
    first.read_with(&cx, |view, _| {
        assert_eq!(view.selected_range(), first_after_outline)
    });
    cx.update(|window, cx| window.press("escape", cx));
    settle(&mut cx);
    app.update_in(&mut cx, |app, window, cx| {
        assert!(app.editor_has_focus(window, cx))
    });
    cx.update(|_, cx| {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("new".into()));
    });
    cx.dispatch_action(gpui_kit::component::input::Paste);
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
                app.active_buffer(cx)
                    .unwrap()
                    .input
                    .text()
                    .unwrap()
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
        app.read_with(cx, |app, cx| app.active_buffer(cx).is_some())
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
    cx.dispatch_action(lyrel_ui_base::actions::ToggleDocumentPreview);
    cx.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut cx);
    second.read_with(&cx, |view, cx| {
        assert!(!view.is_live());
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            b"---\r\ntitle: untouched\n---\r\n<Component />"
        );
    });
    app.read_with(&cx, |app, cx| {
        assert_eq!(app.active_buffer(cx).unwrap().path, renamed)
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
        app.read_with(cx, |app, cx| app.active_buffer(cx).is_some())
    });
    let first = view(&app, &cx);
    app.update_in(&mut cx, |app, window, cx| {
        app.tabs.select(1);
        app.editor_open_on_host(host.clone(), &path, window, cx);
    });
    until(&mut cx, |cx| {
        app.read_with(cx, |app, cx| app.active_buffer(cx).is_some())
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

/// #299 的混合工作区消费者：全部已选格式与普通源码／Markdown／终端共处
/// 一个真实工作台，跨文档切换、分屏、布局、历史、保存与关闭仍各自成立。
///
/// 这里不重复每种格式自己的纵深用例；它验证的是不同格式文档同时存在时
/// 的身份、可见内容和保存目标不会互相串线。真实 OS 窗口、SSH 与 IME 仍
/// 由独立验收记录，测试窗口不冒充它们。
#[cfg(test)]
mod integrated_acceptance {
    use super::*;
    use lyrel_core::core::config::DocumentLayout;
    use lyrel_workbench_editor::code_editor::image_document::ImageContent;
    use lyrel_workbench_editor::code_editor::pdf_document::PdfContent;
    use lyrel_workbench_editor::code_editor::svg_document::SvgMode;
    use std::io::Write as _;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    const SOURCE: &[u8] = b"fn main() {\n    println!(\"ordinary\");\n}\n";
    const MARKDOWN: &[u8] = b"# Notes\n\nordinary **markdown** stays available\n";
    const SVG_SOURCE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="30"><rect width="40" height="30" fill="#369"/></svg>"##;
    const CSV_SOURCE: &[u8] = b"name,value\r\na,remote\r\nq,1\n";
    /// 工作台共享 Document 的内部行尾统一为 LF；保存时仍按原格式写回。
    const CSV_TEXT: &str = "name,value\na,remote\nq,1\n";
    const JSONC_SOURCE: &[u8] =
        b"{\n  // keep this comment\n  \"name\": \"remote\",\n  \"dup\": 1,\n  \"dup\": 2,\n}\n";
    const JSONL_SOURCE: &[u8] = b"{\"id\": 1, \"name\": \"one\"}\n{\"id\": 2, \"name\": \"two\"}\n";
    const YAML_SOURCE: &[u8] = b"# keep this comment\nroot:\n  anchored: &a value\n  alias: *a\n";
    const TOML_SOURCE: &[u8] =
        b"# keep this comment\ndate = 1979-05-27T07:32:00Z\ninline = { x = 1 }\n";
    const XML_SOURCE: &[u8] =
        b"<?xml version=\"1.0\"?>\n<r xmlns=\"urn:root\" xmlns:p=\"urn:part\">\n  <p:a p:flag=\"x\"/>\n  <b>text</b>\n</r>\n";

    fn fixture(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn png_bytes() -> Vec<u8> {
        let rgba = image::RgbaImage::from_pixel(48, 32, image::Rgba([200, 30, 60, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(rgba)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    /// 两帧 GIF；集成用例只确认它作为动画文档显示自己的尺寸与格式。
    fn gif_bytes() -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = ::gif::Encoder::new(
                &mut bytes,
                3,
                1,
                &[255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0],
            )
            .unwrap();
            encoder.set_repeat(::gif::Repeat::Infinite).unwrap();
            for left in [0_u16, 1] {
                encoder
                    .write_frame(&::gif::Frame {
                        left,
                        width: 2,
                        height: 1,
                        delay: 5,
                        transparent: Some(3),
                        buffer: std::borrow::Cow::Owned(vec![0_u8, 1]),
                        ..Default::default()
                    })
                    .unwrap();
            }
        }
        bytes
    }

    fn tiff_bytes() -> Vec<u8> {
        use ::tiff::encoder::colortype::RGB8;
        let mut bytes = Vec::new();
        {
            let mut encoder = ::tiff::encoder::TiffEncoder::new(std::io::Cursor::new(&mut bytes))
                .expect("tiff starts");
            for (width, height, colour) in
                [(48_u32, 32_u32, [10_u8, 20, 30]), (20, 10, [200, 10, 10])]
            {
                let image = encoder
                    .new_image::<RGB8>(width, height)
                    .expect("page starts");
                let mut pixels = Vec::new();
                for _ in 0..width * height {
                    pixels.extend_from_slice(&colour);
                }
                image.write_data(&pixels).expect("page writes");
            }
        }
        bytes
    }

    /// 最小但真实的 EPUB：两条 spine、导航、本地插图与中文正文。
    fn epub_bytes() -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut archive = ::zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
            let entries: [(&str, Vec<u8>, bool); 7] = [
                ("mimetype", b"application/epub+zip".to_vec(), true),
                (
                    "META-INF/container.xml",
                    br#"<container><rootfiles><rootfile full-path="OPS/book.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#.to_vec(),
                    false,
                ),
                (
                    "OPS/book.opf",
                    br#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Mixed workspace book</dc:title><dc:creator>Lyrel fixture</dc:creator></metadata><manifest><item id="first" href="first.xhtml" media-type="application/xhtml+xml"/><item id="last" href="last.xhtml" media-type="application/xhtml+xml"/><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/><item id="pixel" href="pixel.png" media-type="image/png"/></manifest><spine><itemref idref="first"/><itemref idref="last"/></spine></package>"#.to_vec(),
                    false,
                ),
                (
                    "OPS/nav.xhtml",
                    br#"<html xmlns:epub="http://www.idpf.org/2007/ops"><body><nav epub:type="toc"><ol><li><a href="first.xhtml#start">Start Chapter</a></li><li><a href="last.xhtml#target">Final Chapter</a></li></ol></nav></body></html>"#.to_vec(),
                    false,
                ),
                (
                    "OPS/first.xhtml",
                    r#"<html><body><h1 id="start">Start Chapter</h1><p>混合工作区里的普通正文。</p><a href="last.xhtml#target">Final chapter</a></body></html>"#.as_bytes().to_vec(),
                    false,
                ),
                (
                    "OPS/last.xhtml",
                    r#"<html><body><h1 id="target">Final Chapter</h1><p>needle-only-final-chapter-279 中文</p><img src="pixel.png" alt="Owned illustration"/></body></html>"#.as_bytes().to_vec(),
                    false,
                ),
                (
                    "OPS/pixel.png",
                    {
                        let rgba =
                            image::RgbaImage::from_pixel(16, 8, image::Rgba([24, 72, 168, 255]));
                        let mut png = std::io::Cursor::new(Vec::new());
                        image::DynamicImage::ImageRgba8(rgba)
                            .write_to(&mut png, image::ImageFormat::Png)
                            .unwrap();
                        png.into_inner()
                    },
                    false,
                ),
            ];
            for (name, body, stored) in entries {
                let method = if stored {
                    ::zip::CompressionMethod::Stored
                } else {
                    ::zip::CompressionMethod::Deflated
                };
                archive
                    .start_file(
                        name,
                        ::zip::write::SimpleFileOptions::default().compression_method(method),
                    )
                    .unwrap();
                archive.write_all(&body).unwrap();
            }
            archive.finish().unwrap();
        }
        bytes
    }

    fn sqlite_bytes(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        let connection = rusqlite::Connection::open(&path).expect("fixture database opens");
        connection
            .execute_batch(
                "CREATE TABLE people(id INTEGER, name TEXT, score REAL);
                 CREATE VIEW people_view AS SELECT id, name FROM people;
                 INSERT INTO people VALUES(1, 'Ada', 1.5);
                 INSERT INTO people VALUES(2, '中文', 2.0);",
            )
            .unwrap();
        drop(connection);
        path
    }

    fn drawio_bytes(pages: usize) -> Vec<u8> {
        let mut out = String::from("<mxfile host=\"lyrel.test\" type=\"device\">");
        for page in 0..pages {
            out.push_str(&format!(
                "<diagram id=\"u{page}\" name=\"Page {}\"><mxGraphModel dx=\"800\" dy=\"600\"><root><mxCell id=\"0\"/><mxCell id=\"1\" parent=\"0\"/><mxCell id=\"2\" value=\"Mixed page {page}\" vertex=\"1\" parent=\"1\"><mxGeometry x=\"8\" y=\"8\" width=\"120\" height=\"60\" as=\"geometry\"/></mxCell></root></mxGraphModel></diagram>",
                page + 1
            ));
        }
        out.push_str("</mxfile>");
        out.into_bytes()
    }

    fn active_id(app: &Entity<LyrelApp>, cx: &VisualTestContext) -> Option<BufferId> {
        app.read_with(cx, |app, _| app.tab_code().and_then(TabCode::active_id))
    }

    fn open_local(app: &Entity<LyrelApp>, cx: &mut VisualTestContext, path: &Path) -> BufferId {
        let at = path.to_path_buf();
        app.update_in(cx, |app, window, cx| {
            app.open_file_in_editor(&at, window, cx)
        });
        until(cx, |cx| {
            app.read_with(cx, |app, cx| {
                app.active_buffer(cx)
                    .is_some_and(|file| file.path.as_path() == path)
            })
        });
        active_id(app, cx).expect("opened document")
    }

    fn open_on_host(
        app: &Entity<LyrelApp>,
        cx: &mut VisualTestContext,
        host: SharedHost,
        path: &Path,
    ) -> BufferId {
        let at = path.to_path_buf();
        app.update_in(cx, |app, window, cx| {
            app.editor_open_on_host(host, &at, window, cx)
        });
        until(cx, |cx| {
            app.read_with(cx, |app, cx| {
                app.active_buffer(cx)
                    .is_some_and(|file| file.path.as_path() == path)
                    && app
                        .editor_active_location(cx)
                        .is_some_and(|(_, open)| open == path)
            })
        });
        active_id(app, cx).expect("opened document")
    }

    fn activate(app: &Entity<LyrelApp>, cx: &mut VisualTestContext, id: BufferId) {
        let pos = app.read_with(cx, |app, _| {
            app.tab_code()
                .and_then(|code| code.files.iter().position(|file| *file == id))
                .expect("document in the strip")
        });
        app.update_in(cx, |app, window, cx| app.editor_activate(pos, window, cx));
        settle(cx);
        assert_eq!(active_id(app, cx), Some(id));
    }

    fn extension(path: &Path) -> &str {
        path.extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("")
    }

    fn is_ready(app: &Entity<LyrelApp>, cx: &VisualTestContext, id: BufferId) -> bool {
        app.read_with(cx, |app, cx| {
            let Some(file) = app.editor.read(cx).buffers.get(id) else {
                return false;
            };
            match extension(&file.path) {
                "png" | "gif" | "tiff" => file
                    .input
                    .image()
                    .is_some_and(|view| matches!(view.read(cx).content(), ImageContent::Ready(_))),
                "pdf" => file.input.pdf().is_some_and(|view| {
                    matches!(
                        view.read(cx).document().read(cx).content(),
                        PdfContent::Ready(_)
                    )
                }),
                "epub" => file
                    .input
                    .book()
                    .is_some_and(|view| view.read(cx).block_count() > 0),
                "svg" => file
                    .input
                    .svg()
                    .is_some_and(|view| view.read(cx).preview_size(cx).is_some()),
                "csv" => file.input.csv().is_some_and(|view| {
                    view.read(cx).facts(cx).is_some() && !view.read(cx).is_parsing(cx)
                }),
                "json" | "jsonc" | "jsonl" | "ndjson" | "yaml" | "yml" | "toml" | "xml" => {
                    file.input
                        .source_document(cx)
                        .is_some_and(|document| !document.read(cx).source().is_empty())
                        && (file
                            .input
                            .json()
                            .is_none_or(|view| view.read(cx).structure_ready(cx))
                            && file
                                .input
                                .yaml()
                                .is_none_or(|view| view.read(cx).structure_ready(cx))
                            && file
                                .input
                                .toml()
                                .is_none_or(|view| view.read(cx).structure_ready(cx))
                            && file
                                .input
                                .xml()
                                .is_none_or(|view| view.read(cx).structure_ready(cx)))
                }
                "sqlite" => file.input.sqlite().is_some_and(|view| {
                    let (headers, rows) = view.read(cx).grid_dump(cx);
                    !headers.is_empty()
                        && rows.iter().any(|row| row.iter().any(|cell| cell == "Ada"))
                }),
                "drawio" => file
                    .input
                    .drawio()
                    .is_some_and(|view| view.read(cx).page_count(cx) == 2),
                "md" => file
                    .input
                    .source_document(cx)
                    .is_some_and(|document| !document.read(cx).source().is_empty()),
                "rs" => file
                    .input
                    .code()
                    .is_some_and(|editor| !editor.read(cx).text().to_string().is_empty()),
                _ => false,
            }
        })
    }

    fn ready_state(app: &Entity<LyrelApp>, cx: &VisualTestContext, id: BufferId) -> String {
        app.read_with(cx, |app, cx| {
            let Some(file) = app.editor.read(cx).buffers.get(id) else {
                return "not installed".to_string();
            };
            let name = file.path.file_name().unwrap().to_string_lossy();
            if let Some(view) = file.input.image() {
                let state = match view.read(cx).content() {
                    ImageContent::Ready(bitmap) => {
                        let size = bitmap.image().size(0);
                        format!("Ready {}x{}", size.width.0, size.height.0)
                    }
                    ImageContent::Loading => "Loading".to_string(),
                    ImageContent::Failed(message) => format!("Failed: {message}"),
                };
                return format!("{name}: {state}");
            }
            if let Some(view) = file.input.pdf() {
                let state = match view.read(cx).document().read(cx).content() {
                    PdfContent::Ready(_) => "Ready".to_string(),
                    PdfContent::Locked { wrong } => format!("Locked wrong={wrong}"),
                    PdfContent::Unlocking => "Unlocking".to_string(),
                    PdfContent::Failed(message) => format!("Failed: {message}"),
                };
                return format!("{name}: {state}");
            }
            if let Some(view) = file.input.book() {
                return format!("{name}: blocks {}", view.read(cx).block_count());
            }
            if let Some(view) = file.input.svg() {
                return format!(
                    "{name}: preview {:?} failure {:?}",
                    view.read(cx).preview_size(cx),
                    view.read(cx).preview_failure(cx)
                );
            }
            if let Some(view) = file.input.csv() {
                return format!(
                    "{name}: facts {:?} parsing {} error {:?}",
                    view.read(cx).facts(cx),
                    view.read(cx).is_parsing(cx),
                    view.read(cx).parse_error(cx)
                );
            }
            if let Some(view) = file.input.sqlite() {
                let (headers, rows) = view.read(cx).grid_dump(cx);
                return format!("{name}: headers {} rows {}", headers.len(), rows.len());
            }
            if let Some(view) = file.input.drawio() {
                return format!(
                    "{name}: pages {} failure {:?}",
                    view.read(cx).page_count(cx),
                    view.read(cx).document().read(cx).failure()
                );
            }
            if let Some(document) = file.input.source_document(cx) {
                return format!("{name}: source len {}", document.read(cx).source().len());
            }
            format!("{name}: no readiness probe")
        })
    }

    fn wait_ready(app: &Entity<LyrelApp>, cx: &mut VisualTestContext, id: BufferId) {
        let name = app.read_with(cx, |app, cx| {
            app.editor
                .read(cx)
                .buffers
                .get(id)
                .map(|file| file.path.display().to_string())
                .unwrap_or_else(|| "missing document".to_string())
        });
        let deadline = Instant::now() + Duration::from_secs(45);
        while !is_ready(app, cx, id) {
            assert!(
                Instant::now() < deadline,
                "{name} did not become ready: {}",
                ready_state(app, cx, id)
            );
            settle(cx);
            cx.executor().advance_clock(Duration::from_millis(50));
            std::thread::sleep(Duration::from_millis(15));
        }
        settle(cx);
    }

    /// 逐文档的消费者可见状态与内容。文件名是测试夹具自己的身份；这里不
    /// 检查内部缓存或源码接线。
    fn assert_intact(app: &Entity<LyrelApp>, cx: &VisualTestContext, id: BufferId) {
        app.read_with(cx, |app, cx| {
            let file = app
                .editor
                .read(cx)
                .buffers
                .get(id)
                .expect("installed document");
            let name = file.path.file_name().unwrap().to_string_lossy().to_string();
            let source = || {
                file.input
                    .source_document(cx)
                    .expect("source-authority document")
                    .read(cx)
                    .source()
                    .to_owned()
            };
            match name.as_str() {
                "photo.png" | "mixed.png" => {
                    let view = file.input.image().expect("image input");
                    let ImageContent::Ready(bitmap) = view.read(cx).content() else {
                        panic!("PNG is not decoded");
                    };
                    let size = bitmap.image().size(0);
                    assert_eq!((size.width.0, size.height.0), (48, 32));
                }
                "animated.gif" => {
                    let view = file.input.image().expect("GIF input");
                    let ImageContent::Ready(_) = view.read(cx).content() else {
                        panic!("GIF is not decoded");
                    };
                    let status = view.read(cx).status_text(cx).to_string();
                    assert!(status.contains("GIF"), "GIF status: {status}");
                }
                "pages.tiff" => {
                    let view = file.input.image().expect("TIFF input");
                    let ImageContent::Ready(_) = view.read(cx).content() else {
                        panic!("TIFF page is not decoded");
                    };
                    assert_eq!(view.read(cx).page_count(cx), Some(2));
                    assert_eq!(view.read(cx).page_label(cx).as_deref(), Some("Page 1 of 2"));
                }
                "vector.svg" => {
                    let view = file.input.svg().expect("SVG input");
                    assert_eq!(view.read(cx).preview_size(cx), Some((40., 30.)));
                    assert_eq!(view.read(cx).mode(), SvgMode::Source);
                    assert_eq!(source(), String::from_utf8_lossy(SVG_SOURCE).into_owned());
                }
                "reading.pdf" => {
                    let view = file.input.pdf().expect("PDF input");
                    let PdfContent::Ready(_) = view.read(cx).document().read(cx).content() else {
                        panic!("PDF is not parsed");
                    };
                    assert_eq!(view.read(cx).page_count(), 12);
                }
                "book.epub" => {
                    let view = file.input.book().expect("EPUB input");
                    assert!(view.read(cx).block_count() > 0);
                    assert!(
                        view.read(cx)
                            .block_text(0)
                            .is_some_and(|text| text.contains("Start Chapter")),
                        "the first spine chapter is visible"
                    );
                    assert!(view.read(cx).chapter_label(cx).to_string().contains("1"));
                }
                "table.csv" => {
                    let view = file.input.csv().expect("CSV input");
                    assert_eq!(view.read(cx).facts(cx), Some((3, 2)));
                    assert_eq!(view.read(cx).visible_count(), 3);
                    assert_eq!(source(), CSV_TEXT);
                }
                "config.jsonc" => {
                    let view = file.input.json().expect("JSON input");
                    assert!(view.read(cx).structure_ready(cx));
                    assert_eq!(source(), String::from_utf8_lossy(JSONC_SOURCE).into_owned());
                }
                "records.jsonl" => {
                    let view = file.input.json().expect("JSONL input");
                    assert!(view.read(cx).structure_ready(cx));
                    assert_eq!(source(), String::from_utf8_lossy(JSONL_SOURCE).into_owned());
                }
                "config.yaml" => {
                    let view = file.input.yaml().expect("YAML input");
                    assert!(view.read(cx).structure_ready(cx));
                    assert_eq!(source(), String::from_utf8_lossy(YAML_SOURCE).into_owned());
                }
                "config.toml" => {
                    let view = file.input.toml().expect("TOML input");
                    assert!(view.read(cx).structure_ready(cx));
                    assert_eq!(source(), String::from_utf8_lossy(TOML_SOURCE).into_owned());
                }
                "document.xml" => {
                    let view = file.input.xml().expect("XML input");
                    assert!(view.read(cx).structure_ready(cx));
                    assert_eq!(source(), String::from_utf8_lossy(XML_SOURCE).into_owned());
                }
                "store.sqlite" => {
                    let view = file.input.sqlite().expect("SQLite input");
                    let (headers, rows) = view.read(cx).grid_dump(cx);
                    assert!(headers.iter().any(|header| header == "name"));
                    assert!(rows.iter().any(|row| row.iter().any(|cell| cell == "Ada")));
                }
                "diagram.drawio" => {
                    let view = file.input.drawio().expect("draw.io input");
                    assert_eq!(view.read(cx).page_count(cx), 2);
                    assert_eq!(view.read(cx).page_label(cx).as_deref(), Some("Page 1 of 2"));
                }
                "notes.md" => {
                    assert!(file.input.markdown().is_some());
                    assert_eq!(source(), String::from_utf8_lossy(MARKDOWN).into_owned());
                }
                "ordinary.rs" => {
                    let editor = file.input.code().expect("ordinary source");
                    assert_eq!(
                        editor.read(cx).text().to_string(),
                        String::from_utf8_lossy(SOURCE).into_owned()
                    );
                }
                other => panic!("unexpected mixed-workspace document {other}"),
            }
        });
    }

    fn mixed_fixtures(dir: &Path) -> Vec<PathBuf> {
        vec![
            fixture(dir, "photo.png", &png_bytes()),
            fixture(dir, "animated.gif", &gif_bytes()),
            fixture(dir, "pages.tiff", &tiff_bytes()),
            fixture(dir, "vector.svg", SVG_SOURCE),
            fixture(
                dir,
                "reading.pdf",
                &lyrel_document::pdf::fixtures::reading_document(),
            ),
            fixture(dir, "book.epub", &epub_bytes()),
            fixture(dir, "table.csv", CSV_SOURCE),
            fixture(dir, "config.jsonc", JSONC_SOURCE),
            fixture(dir, "records.jsonl", JSONL_SOURCE),
            fixture(dir, "config.yaml", YAML_SOURCE),
            fixture(dir, "config.toml", TOML_SOURCE),
            fixture(dir, "document.xml", XML_SOURCE),
            sqlite_bytes(dir, "store.sqlite"),
            fixture(dir, "diagram.drawio", &drawio_bytes(2)),
            fixture(dir, "notes.md", MARKDOWN),
            fixture(dir, "ordinary.rs", SOURCE),
        ]
    }

    #[gpui_kit::test]
    fn mixed_workspace_keeps_every_selected_format_distinct_through_navigation_split_and_close(
        tcx: &mut TestAppContext,
    ) {
        lyrel_ui_base::i18n::set_locale("en");
        let dir = test_host::canonical_tempdir();
        let paths = mixed_fixtures(dir.path());
        let (app, mut cx, _streams) = harness_with_tabs(tcx, 2);
        cx.simulate_resize(gpui_kit::size(px(1800.), px(1000.)));

        let mut documents = Vec::new();
        for path in &paths {
            let id = open_local(&app, &mut cx, path);
            wait_ready(&app, &mut cx, id);
            assert_intact(&app, &cx, id);
            documents.push((path.clone(), id));
        }
        assert_eq!(
            app.read_with(&cx, |app, cx| app.editor.read(cx).buffers.iter().count()),
            documents.len(),
            "one document per distinct Host/path"
        );
        assert!(
            app.read_with(&cx, |app, cx| app
                .editor
                .read(cx)
                .buffers
                .iter()
                .all(|file| file.host.id() == HostId::LOCAL)),
            "every local fixture belongs to the local Host"
        );
        assert_eq!(
            app.read_with(&cx, |app, _| app.tab_code().unwrap().files.len()),
            documents.len()
        );

        // Every document keeps its own visible content after the others were
        // opened, not merely its own strip entry.
        for (_, id) in &documents {
            activate(&app, &mut cx, *id);
            assert_intact(&app, &cx, *id);
        }

        // The real terminal tab survives alongside the documents, and coming
        // back does not rebuild or reorder the editor documents.
        app.update_in(&mut cx, |app, window, cx| {
            app.tabs.select(1);
            app.focus_active(window, cx);
        });
        settle(&mut cx);
        assert_eq!(app.read_with(&cx, |app, _| app.tabs.active_index()), 1);
        assert_eq!(
            app.read_with(&cx, |app, cx| app.editor.read(cx).buffers.iter().count()),
            documents.len()
        );
        app.update_in(&mut cx, |app, window, cx| {
            app.tabs.select(0);
            app.focus_editor(window, cx);
        });
        settle(&mut cx);

        // CSV table and source are two presentations over one document; a
        // source edit is the same history the table sees, and undo returns the
        // saved bytes state without touching another document.
        let csv_id = documents
            .iter()
            .find(|(path, _)| path.file_name().unwrap() == "table.csv")
            .unwrap()
            .1;
        let csv_path = documents
            .iter()
            .find(|(path, _)| path.file_name().unwrap() == "table.csv")
            .unwrap()
            .0
            .clone();
        activate(&app, &mut cx, csv_id);
        app.update_in(&mut cx, |app, window, cx| {
            assert!(app.editor_split(window, cx));
        });
        settle(&mut cx);
        let (primary, focused) = app.read_with(&cx, |app, cx| {
            let file = app.editor.read(cx).buffers.get(csv_id).unwrap();
            (
                file.input.csv().unwrap().clone(),
                app.active_editor_input(cx).unwrap().csv().unwrap().clone(),
            )
        });
        assert_ne!(primary.entity_id(), focused.entity_id());
        assert_eq!(
            primary.read_with(&cx, |view, _| view.document().clone()),
            focused.read_with(&cx, |view, _| view.document().clone()),
            "both presentations share one CSV document"
        );
        let primary_mode = primary.read_with(&cx, |view, _| view.mode());
        let focused_mode = focused.read_with(&cx, |view, _| view.mode());
        assert_ne!(primary_mode, focused_mode, "split opens the other side");
        // The split's focused group is on the right; the source side is either
        // there or the primary left side.
        let source_view = if primary_mode == CsvMode::Source {
            app.update_in(&mut cx, |app, window, cx| {
                app.editor_focus_group(true, window, cx);
            });
            primary
        } else {
            assert_eq!(focused_mode, CsvMode::Source);
            focused
        };
        app.update_in(&mut cx, |app, window, cx| {
            app.focus_editor(window, cx);
        });
        settle(&mut cx);
        let source = source_view.read_with(&cx, |view, _| view.source().clone());
        let text = source.read_with(&cx, |editor, _| editor.text().to_string());
        let at = text.find("remote").expect("fixture value in source");
        source.update(&mut cx, |editor, cx| editor.select_range(at..at + 6, cx));
        cx.update(|window, cx| window.input("edited", cx));
        until(&mut cx, |cx| {
            source.read_with(cx, |editor, _| editor.text().to_string().contains("edited"))
                && !source_view.read_with(cx, |view, cx| view.is_parsing(cx))
        });
        assert_eq!(
            std::fs::read(&csv_path).unwrap(),
            CSV_SOURCE,
            "a format edit is not an implicit save"
        );
        assert!(app.read_with(&cx, |app, cx| {
            app.editor
                .read(cx)
                .buffers
                .get(csv_id)
                .is_some_and(|file| file.dirty)
        }));
        // Undo through the focused presentation's actual keyboard command.
        cx.dispatch_action(gpui_kit::component::input::Undo);
        until(&mut cx, |cx| {
            source.read_with(cx, |editor, _| editor.text() == CSV_TEXT)
        });
        settle(&mut cx);
        assert!(!app.read_with(&cx, |app, cx| {
            app.editor.read(cx).buffers.get(csv_id).unwrap().dirty
        }));
        assert_eq!(std::fs::read(&csv_path).unwrap(), CSV_SOURCE);

        // Dock/fill and focus changes do not select another document or lose
        // the split.
        for layout in [DocumentLayout::Fill, DocumentLayout::Dock] {
            app.update_in(&mut cx, |app, window, cx| {
                app.set_document_layout(layout, cx);
                app.focus_previous_content(window, cx);
            });
            settle(&mut cx);
            assert_eq!(active_id(&app, &cx), Some(csv_id));
            assert!(app.read_with(&cx, |app, _| app.tab_code().unwrap().is_split()));
        }

        // Closing one clean document leaves the mixed workspace intact; its
        // file on disk was never rewritten.
        let close_id = documents
            .iter()
            .find(|(path, _)| path.file_name().unwrap() == "ordinary.rs")
            .unwrap()
            .1;
        let close_path = dir.path().join("ordinary.rs");
        app.update_in(&mut cx, |app, window, cx| {
            // The split's other group holds the documents opened before CSV;
            // closing follows the group that actually shows the file.
            assert!(app.editor_focus_group(true, window, cx));
        });
        settle(&mut cx);
        app.update_in(&mut cx, |app, window, cx| {
            app.editor_close_buffer(close_id, window, cx)
        });
        settle(&mut cx);
        assert!(app.read_with(&cx, |app, cx| {
            app.editor.read(cx).buffers.get(close_id).is_none()
        }));
        assert_eq!(
            app.read_with(&cx, |app, cx| app.editor.read(cx).buffers.iter().count()),
            documents.len() - 1
        );
        assert_eq!(std::fs::read(&close_path).unwrap(), SOURCE);
        let reopened = open_local(&app, &mut cx, &close_path);
        wait_ready(&app, &mut cx, reopened);
        assert_intact(&app, &cx, reopened);
        for (path, _) in &documents {
            if path.file_name().unwrap() != "ordinary.rs" {
                assert!(
                    app.read_with(&cx, |app, cx| {
                        app.editor
                            .read(cx)
                            .buffers
                            .iter()
                            .any(|file| file.path == *path)
                    }),
                    "{} disappeared from the mixed workspace",
                    path.display()
                );
            }
        }
    }

    /// 同名文件落在不同 Host 时，保存、排队写入与迟到读取都必须跟随原
    /// 文档身份。映射 Host 根目录与本地同名文件分开，避免把桌面文件当成
    /// 远端原件。
    #[gpui_kit::test]
    fn same_named_documents_on_two_hosts_never_cross_save_or_focus_targets(
        tcx: &mut TestAppContext,
    ) {
        lyrel_ui_base::i18n::set_locale("en");
        let local_root = test_host::canonical_tempdir();
        let remote_root = test_host::canonical_tempdir();
        let remote_work = remote_root.path().join("work");
        let local_work = local_root.path().join("work");
        std::fs::create_dir_all(&remote_work).unwrap();
        std::fs::create_dir_all(&local_work).unwrap();
        let remote_csv = remote_work.join("table.csv");
        let local_csv = local_work.join("table.csv");
        let remote_original = b"name,value\r\na,remote\r\n";
        let local_original = b"name,value\nlocal,kept\n";
        std::fs::write(&remote_csv, remote_original).unwrap();
        std::fs::write(&local_csv, local_original).unwrap();
        let remote_png = remote_work.join("late.png");
        let late_png = png_bytes();
        std::fs::write(&remote_png, &late_png).unwrap();
        let local_json = fixture(
            local_root.path(),
            "settings.jsonc",
            b"{\n  // local decoy only\n  \"name\": \"local\"\n}\n",
        );

        let mapped = test_host::FileHost::mapped(HostId(2990), remote_root.path());
        let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
        cx.simulate_resize(gpui_kit::size(px(1600.), px(1000.)));

        let remote_id = open_on_host(&app, &mut cx, mapped.clone(), Path::new("/work/table.csv"));
        until(&mut cx, |cx| {
            app.read_with(cx, |app, cx| {
                app.editor
                    .read(cx)
                    .buffers
                    .get(remote_id)
                    .and_then(|file| file.input.csv())
                    .is_some_and(|view| view.read(cx).facts(cx) == Some((2, 2)))
            })
        });
        let remote_view = app.read_with(&cx, |app, cx| {
            app.editor
                .read(cx)
                .buffers
                .get(remote_id)
                .unwrap()
                .input
                .csv()
                .unwrap()
                .clone()
        });
        // Source side of the same document, then a real edit.
        app.update_in(&mut cx, |app, window, cx| {
            app.toggle_document_preview(window, cx);
            app.focus_editor(window, cx);
        });
        settle(&mut cx);
        let source = remote_view.read_with(&cx, |view, _| view.source().clone());
        let text = source.read_with(&cx, |editor, _| editor.text().to_string());
        let at = text.find("remote").unwrap();
        source.update(&mut cx, |editor, cx| editor.select_range(at..at + 6, cx));
        cx.update(|window, cx| window.input("edited", cx));
        until(&mut cx, |cx| {
            source.read_with(cx, |editor, _| editor.text().to_string().contains("edited"))
                && !remote_view.read_with(cx, |view, cx| view.is_parsing(cx))
        });

        // Hold the mapped Host's write, then switch to a same-named local
        // document before the write lands.
        let release = mapped.pause_next_write();
        cx.dispatch_action(lyrel_ui_base::actions::EditorSave);
        until(&mut cx, |_| mapped.entered.load(Ordering::SeqCst));
        let local_id = open_local(&app, &mut cx, &local_csv);
        until(&mut cx, |cx| {
            app.read_with(cx, |app, cx| {
                app.editor
                    .read(cx)
                    .buffers
                    .get(local_id)
                    .and_then(|file| file.input.csv())
                    .is_some_and(|view| view.read(cx).facts(cx) == Some((2, 2)))
            })
        });
        app.update_in(&mut cx, |app, window, cx| {
            app.set_document_layout(DocumentLayout::Fill, cx);
            app.focus_editor(window, cx);
        });
        release.send(()).unwrap();
        until(&mut cx, |cx| {
            app.read_with(cx, |app, cx| {
                app.editor
                    .read(cx)
                    .buffers
                    .get(remote_id)
                    .is_some_and(|file| !file.saving && !file.dirty)
            })
        });
        assert_eq!(
            std::fs::read(&remote_csv).unwrap(),
            b"name,value\r\na,edited\r\n",
            "the owning mapped Host received the edit"
        );
        assert_eq!(
            std::fs::read(&local_csv).unwrap(),
            local_original,
            "the same-named local document was never the save target"
        );
        assert_eq!(
            active_id(&app, &cx),
            Some(local_id),
            "focus stayed with the local document"
        );

        // Editing and saving the local document now changes only its bytes.
        let local_view = app.read_with(&cx, |app, cx| {
            app.editor
                .read(cx)
                .buffers
                .get(local_id)
                .unwrap()
                .input
                .csv()
                .unwrap()
                .clone()
        });
        app.update_in(&mut cx, |app, window, cx| {
            app.toggle_document_preview(window, cx);
            app.focus_editor(window, cx);
        });
        settle(&mut cx);
        let local_source = local_view.read_with(&cx, |view, _| view.source().clone());
        let local_text = local_source.read_with(&cx, |editor, _| editor.text().to_string());
        let local_at = local_text.find("local").unwrap();
        local_source.update(&mut cx, |editor, cx| {
            editor.select_range(local_at..local_at + 5, cx)
        });
        cx.update(|window, cx| window.input("desktop", cx));
        until(&mut cx, |cx| {
            local_source.read_with(cx, |editor, _| {
                editor.text().to_string().contains("desktop")
            }) && !local_view.read_with(cx, |view, cx| view.is_parsing(cx))
        });
        cx.dispatch_action(lyrel_ui_base::actions::EditorSave);
        until(&mut cx, |cx| {
            app.read_with(cx, |app, cx| {
                app.editor
                    .read(cx)
                    .buffers
                    .get(local_id)
                    .is_some_and(|file| !file.saving && !file.dirty)
            })
        });
        assert_eq!(
            std::fs::read(&local_csv).unwrap(),
            b"name,value\ndesktop,kept\n"
        );
        assert_eq!(
            std::fs::read(&remote_csv).unwrap(),
            b"name,value\r\na,edited\r\n"
        );

        // Independent reopen through a fresh mapped Host sees persisted bytes.
        app.update_in(&mut cx, |app, window, cx| {
            app.editor_close_buffer(remote_id, window, cx)
        });
        settle(&mut cx);
        assert!(app.read_with(&cx, |app, cx| {
            app.editor.read(cx).buffers.get(remote_id).is_none()
        }));
        let fresh = test_host::FileHost::mapped(HostId(2991), remote_root.path());
        let reopened = open_on_host(&app, &mut cx, fresh, Path::new("/work/table.csv"));
        until(&mut cx, |cx| {
            app.read_with(cx, |app, cx| {
                app.editor
                    .read(cx)
                    .buffers
                    .get(reopened)
                    .and_then(|file| file.input.csv())
                    .is_some_and(|view| {
                        view.read(cx).facts(cx) == Some((2, 2))
                            && view
                                .read(cx)
                                .source()
                                .read(cx)
                                .text()
                                .to_string()
                                .contains("edited")
                    })
            })
        });
        assert_eq!(
            std::fs::read(&remote_csv).unwrap(),
            b"name,value\r\na,edited\r\n"
        );
        assert_eq!(
            std::fs::read(&local_csv).unwrap(),
            b"name,value\ndesktop,kept\n"
        );

        // A read paused mid-transfer must not steal focus when it lands, and
        // must not become a same-named local document.
        let release_read = mapped.pause_next_read();
        app.update_in(&mut cx, |app, window, cx| {
            app.editor_open_on_host(mapped.clone(), Path::new("/work/late.png"), window, cx)
        });
        until(&mut cx, |_| mapped.entered.load(Ordering::SeqCst));
        let json_id = open_local(&app, &mut cx, &local_json);
        until(&mut cx, |cx| {
            app.read_with(cx, |app, cx| {
                app.editor
                    .read(cx)
                    .buffers
                    .get(json_id)
                    .and_then(|file| file.input.json())
                    .is_some_and(|view| view.read(cx).structure_ready(cx))
            })
        });
        release_read.send(()).unwrap();
        until(&mut cx, |cx| {
            app.read_with(cx, |app, cx| {
                app.editor.read(cx).buffers.iter().any(|file| {
                    file.path == Path::new("/work/late.png") && file.input.image().is_some()
                })
            })
        });
        assert_eq!(
            active_id(&app, &cx),
            Some(json_id),
            "late decode kept focus"
        );
        assert_eq!(
            app.read_with(&cx, |app, cx| {
                app.editor
                    .read(cx)
                    .buffers
                    .get(json_id)
                    .unwrap()
                    .path
                    .clone()
            }),
            local_json,
            "the late decode installed the mapped Host's own document"
        );
        assert_eq!(
            std::fs::read(&local_json).unwrap(),
            b"{\n  // local decoy only\n  \"name\": \"local\"\n}\n"
        );
        assert_eq!(std::fs::read(&remote_png).unwrap(), late_png);
    }

    fn save_active(app: &Entity<LyrelApp>, cx: &mut VisualTestContext, id: BufferId) {
        cx.dispatch_action(lyrel_ui_base::actions::EditorSave);
        until(cx, |cx| {
            app.read_with(cx, |app, cx| {
                app.editor
                    .read(cx)
                    .buffers
                    .get(id)
                    .is_some_and(|file| !file.saving && !file.dirty)
            })
        });
    }

    /// 真实 SSH：同一远端 Host 上的混合格式一起读、编辑、保存，再由一个
    /// 独立 control 连接读取实际字节并独立重开。同名本地文件只是诱饵，
    /// 必须是另一条路径且始终不变。缺能力与失联由既有 Host conformance／
    /// real-transport 用例覆盖，这里不伪造旧 peer 或假空结果。
    #[gpui_kit::test]
    #[ignore = "requires the explicit private Linux SSH fixture, a same-commit local router and the running private remote daemon; run alone with --ignored"]
    fn mixed_formats_edit_save_and_reopen_through_real_native_ssh(tcx: &mut TestAppContext) {
        use lyrel_core::host::Host as _;

        lyrel_ui_base::i18n::set_locale("en");
        let binary = std::env::var_os("LYREL_TEST_SSH_LOCAL_SERVER").expect(
            "real SSH prerequisite: LYREL_TEST_SSH_LOCAL_SERVER names the same-commit router binary; also provide the explicit LYREL_TEST_SSH_* fixture endpoint",
        );
        let fixture =
            crate::native_ssh_graphics_fixture::NativeSshFixture::start(Path::new(&binary));
        let host = fixture.connect_control();
        let csv_path = fixture.remote_path("mixed.csv");
        let json_path = fixture.remote_path("mixed.jsonc");
        let png_path = fixture.remote_path("mixed.png");
        let directory = csv_path.parent().expect("remote fixture directory");
        if !host.exists(directory) {
            host.create_dir(directory, false)
                .expect("create the private remote fixture directory");
        }
        let csv_original = b"\xef\xbb\xbfname,value\r\na,remote\r\n".to_vec();
        let png_original = png_bytes();
        host.write_file(&csv_path, &csv_original)
            .expect("seed remote CSV through real SSH");
        host.write_file(&json_path, JSONC_SOURCE)
            .expect("seed remote JSONC through real SSH");
        host.write_file(&png_path, &png_original)
            .expect("seed remote PNG through real SSH");
        assert_eq!(
            host.read_file(&csv_path, 1024 * 1024).unwrap(),
            csv_original
        );
        assert_eq!(
            host.read_file(&json_path, 1024 * 1024).unwrap(),
            JSONC_SOURCE
        );
        assert_eq!(
            host.read_file(&png_path, 1024 * 1024).unwrap(),
            png_original
        );

        let decoy_dir = test_host::canonical_tempdir();
        for (name, bytes) in [
            ("mixed.csv", b"LOCAL DECOY CSV".as_slice()),
            ("mixed.jsonc", b"LOCAL DECOY JSON".as_slice()),
            ("mixed.png", b"LOCAL DECOY PNG".as_slice()),
        ] {
            std::fs::write(decoy_dir.path().join(name), bytes).unwrap();
        }
        assert!(
            !Path::new(csv_path.to_str().expect("Linux path is UTF-8")).exists(),
            "the remote path must not resolve as a local file"
        );

        let (app, mut cx, _streams) = harness_with_tabs(tcx, 1);
        cx.simulate_resize(gpui_kit::size(px(1600.), px(1000.)));

        let csv_id = open_on_host(&app, &mut cx, host.clone(), &csv_path);
        until(&mut cx, |cx| {
            app.read_with(cx, |app, cx| {
                app.editor
                    .read(cx)
                    .buffers
                    .get(csv_id)
                    .and_then(|file| file.input.csv())
                    .is_some_and(|view| view.read(cx).facts(cx) == Some((2, 2)))
            })
        });
        let csv_view = app.read_with(&cx, |app, cx| {
            app.editor
                .read(cx)
                .buffers
                .get(csv_id)
                .unwrap()
                .input
                .csv()
                .unwrap()
                .clone()
        });
        app.update_in(&mut cx, |app, window, cx| {
            app.toggle_document_preview(window, cx);
            app.focus_editor(window, cx);
        });
        settle(&mut cx);
        let csv_source = csv_view.read_with(&cx, |view, _| view.source().clone());
        let text = csv_source.read_with(&cx, |editor, _| editor.text().to_string());
        let at = text.find("remote").unwrap();
        csv_source.update(&mut cx, |editor, cx| editor.select_range(at..at + 6, cx));
        cx.update(|window, cx| window.input("edited", cx));
        until(&mut cx, |cx| {
            csv_source.read_with(cx, |editor, _| editor.text().to_string().contains("edited"))
                && !csv_view.read_with(cx, |view, cx| view.is_parsing(cx))
        });
        save_active(&app, &mut cx, csv_id);

        let json_id = open_on_host(&app, &mut cx, host.clone(), &json_path);
        until(&mut cx, |cx| {
            app.read_with(cx, |app, cx| {
                app.editor
                    .read(cx)
                    .buffers
                    .get(json_id)
                    .and_then(|file| file.input.json())
                    .is_some_and(|view| view.read(cx).structure_ready(cx))
            })
        });
        let json_view = app.read_with(&cx, |app, cx| {
            app.editor
                .read(cx)
                .buffers
                .get(json_id)
                .unwrap()
                .input
                .json()
                .unwrap()
                .clone()
        });
        app.update_in(&mut cx, |app, window, cx| {
            app.toggle_document_preview(window, cx);
            app.focus_editor(window, cx);
        });
        settle(&mut cx);
        let json_source = json_view.read_with(&cx, |view, _| view.source().clone());
        let text = json_source.read_with(&cx, |editor, _| editor.text().to_string());
        let at = text.find("remote").unwrap();
        json_source.update(&mut cx, |editor, cx| editor.select_range(at..at + 6, cx));
        cx.update(|window, cx| window.input("edited", cx));
        until(&mut cx, |cx| {
            json_source.read_with(cx, |editor, _| editor.text().to_string().contains("edited"))
                && json_view.read_with(cx, |view, cx| view.structure_ready(cx))
        });
        save_active(&app, &mut cx, json_id);

        let png_id = open_on_host(&app, &mut cx, host.clone(), &png_path);
        wait_ready(&app, &mut cx, png_id);
        assert_intact(&app, &cx, png_id);
        assert_eq!(active_id(&app, &cx), Some(png_id));

        let expected_csv = b"\xef\xbb\xbfname,value\r\na,edited\r\n".to_vec();
        let expected_json = {
            let text = String::from_utf8(JSONC_SOURCE.to_vec()).unwrap();
            text.replace("\"remote\"", "\"edited\"").into_bytes()
        };
        let verifier = fixture.connect_control_as(
            "fixture:native-ssh-299-verify",
            "native-ssh-mixed-299",
            "isolated-fixture",
        );
        assert_eq!(
            verifier
                .read_file(&csv_path, expected_csv.len() as u64 + 1)
                .expect("independent remote CSV read"),
            expected_csv
        );
        assert_eq!(
            verifier
                .read_file(&json_path, expected_json.len() as u64 + 1)
                .expect("independent remote JSONC read"),
            expected_json
        );
        assert_eq!(
            verifier
                .read_file(&png_path, png_original.len() as u64 + 1)
                .expect("independent remote PNG read"),
            png_original,
            "the read-only picture was not rewritten"
        );
        for (name, bytes) in [
            ("mixed.csv", b"LOCAL DECOY CSV".as_slice()),
            ("mixed.jsonc", b"LOCAL DECOY JSON".as_slice()),
            ("mixed.png", b"LOCAL DECOY PNG".as_slice()),
        ] {
            assert_eq!(
                std::fs::read(decoy_dir.path().join(name)).unwrap(),
                bytes,
                "a same-named local decoy was never the target"
            );
        }

        // Close every presentation and reopen from a fresh control connection:
        // the bytes, not the old in-memory documents, are the observation.
        for id in [csv_id, json_id, png_id] {
            app.update_in(&mut cx, |app, window, cx| {
                app.editor_close_buffer(id, window, cx)
            });
            settle(&mut cx);
            assert!(app.read_with(&cx, |app, cx| {
                app.editor.read(cx).buffers.get(id).is_none()
            }));
        }
        let fresh = fixture.connect_control_as(
            "fixture:native-ssh-299-reopen",
            "native-ssh-mixed-reopen-299",
            "isolated-fixture",
        );
        let reopened_csv = open_on_host(&app, &mut cx, fresh.clone(), &csv_path);
        until(&mut cx, |cx| {
            app.read_with(cx, |app, cx| {
                app.editor
                    .read(cx)
                    .buffers
                    .get(reopened_csv)
                    .and_then(|file| file.input.csv())
                    .is_some_and(|view| {
                        view.read(cx).facts(cx) == Some((2, 2))
                            && view
                                .read(cx)
                                .source()
                                .read(cx)
                                .text()
                                .to_string()
                                .contains("edited")
                    })
            })
        });
        let reopened_json = open_on_host(&app, &mut cx, fresh, &json_path);
        until(&mut cx, |cx| {
            app.read_with(cx, |app, cx| {
                app.editor
                    .read(cx)
                    .buffers
                    .get(reopened_json)
                    .and_then(|file| file.input.json())
                    .is_some_and(|view| {
                        view.read(cx).structure_ready(cx)
                            && view
                                .read(cx)
                                .source()
                                .read(cx)
                                .text()
                                .to_string()
                                .contains("\"edited\"")
                    })
            })
        });
        assert_eq!(
            verifier
                .read_file(&csv_path, expected_csv.len() as u64 + 1)
                .unwrap(),
            expected_csv
        );
        assert_eq!(
            verifier
                .read_file(&json_path, expected_json.len() as u64 + 1)
                .unwrap(),
            expected_json
        );
        for (name, bytes) in [
            ("mixed.csv", b"LOCAL DECOY CSV".as_slice()),
            ("mixed.jsonc", b"LOCAL DECOY JSON".as_slice()),
            ("mixed.png", b"LOCAL DECOY PNG".as_slice()),
        ] {
            assert_eq!(std::fs::read(decoy_dir.path().join(name)).unwrap(), bytes);
        }
    }
}
