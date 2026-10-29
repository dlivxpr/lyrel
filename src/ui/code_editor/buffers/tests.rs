use super::super::test_host::FileHost;
use super::*;
use crate::ui::app::test_window::harness_with_tabs;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext};
use lyrel_core::host::Host as _;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

fn wait_until(vcx: &mut VisualTestContext, mut ready: impl FnMut(&mut VisualTestContext) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        vcx.run_until_parked();
        if ready(vcx) {
            return;
        }
        assert!(Instant::now() < deadline, "editor workflow did not settle");
        vcx.executor().advance_clock(Duration::from_millis(250));
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn active_id(app: &Entity<LyrelApp>, vcx: &VisualTestContext) -> Option<BufferId> {
    app.read_with(vcx, |app, _| app.tab_code().and_then(TabCode::active_id))
}

#[gpui_kit::test]
fn opening_across_tabs_shares_edits_and_closing_one_presentation_keeps_them(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shared.txt");
    std::fs::write(&path, "first\nsecond\nthird\n").unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor_at(&path, Some(2), Some(3), window, cx)
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    let first = active_id(&app, &vcx).unwrap();
    app.read_with(&vcx, |app, cx| {
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .cursor_position(),
            Position::new(1, 2)
        );
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.input("edited", cx);
    });
    vcx.run_until_parked();
    let edited = app.read_with(&vcx, |app, cx| {
        app.active_buffer()
            .unwrap()
            .input
            .read(cx)
            .text()
            .to_string()
    });
    assert_eq!(edited, "first\nseeditedcond\nthird\n");
    app.update_in(&mut vcx, |app, window, cx| {
        app.activate(1, window, cx);
        app.open_file_in_editor(&path, window, cx);
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    assert_eq!(active_id(&app, &vcx), Some(first));
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.activate(0, window, cx);
    });
    app.read_with(&vcx, |app, cx| {
        assert_eq!(app.tab_code().unwrap().active_id(), Some(first));
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string(),
            edited
        );
        assert_eq!(app.editor_unsaved(), vec![first]);
    });
    app.update_in(&mut vcx, |app, window, cx| app.focus_editor(window, cx));
    vcx.update(|window, cx| window.render_frame(cx));
    vcx.dispatch_action(crate::core::actions::EditorSave);
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            app.active_buffer()
                .is_some_and(|file| !file.dirty && !file.saving)
        })
    });
    assert_eq!(std::fs::read(&path).unwrap(), edited.as_bytes());
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.open_file_in_editor(&path, window, cx);
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    app.read_with(&vcx, |app, cx| {
        let file = app.active_buffer().unwrap();
        assert_ne!(file.id(), first);
        assert_eq!(file.host.id(), HostId::LOCAL);
        assert_eq!(file.input.read(cx).text().to_string(), edited);
        assert!(app.editor_unsaved().is_empty());
    });
}

#[gpui_kit::test]
fn markdown_byte_fidelity_survives_host_save_local_edit_undo_and_reopen(cx: &mut TestAppContext) {
    use crate::core::actions::EditorSave;
    use gpui_kit::component::input::{MoveToStart, Redo, Undo};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fidelity.mdx");
    // The first three terminators reproduce the observed majority-CRLF bug.
    let original = b"\xef\xbb\xbfalpha  \r\nbeta\r\ngamma\n\r\n* item\n[ref]: /target  \r\n<Unknown value={1} />";
    std::fs::write(&path, original).unwrap();
    std::fs::write(dir.path().join(".editorconfig"), "root = true\n[*]\ntrim_trailing_whitespace = true\ninsert_final_newline = true\nend_of_line = lf\n").unwrap();
    let host = FileHost::new(HostId(705));
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| cx.set_global(MarkdownWorkbench));
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_on_host(host.clone(), &path, window, cx);
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    let save = |vcx: &mut VisualTestContext| {
        vcx.dispatch_action(EditorSave);
        wait_until(vcx, |vcx| {
            app.read_with(vcx, |app, _| {
                app.active_buffer()
                    .is_some_and(|file| !file.saving && !file.dirty)
            })
        });
    };
    save(&mut vcx);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    vcx.dispatch_action(MoveToStart);
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.input("new\n", cx);
    });
    save(&mut vcx);
    let edited = b"\xef\xbb\xbfnew\r\nalpha  \r\nbeta\r\ngamma\n\r\n* item\n[ref]: /target  \r\n<Unknown value={1} />";
    assert_eq!(std::fs::read(&path).unwrap(), edited);
    vcx.dispatch_action(Undo);
    save(&mut vcx);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    vcx.dispatch_action(Redo);
    save(&mut vcx);
    assert_eq!(std::fs::read(&path).unwrap(), edited);
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.editor_open_on_host(host.clone(), &path, window, cx);
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    save(&mut vcx);
    assert_eq!(std::fs::read(&path).unwrap(), edited);
    app.read_with(&vcx, |app, cx| {
        assert_eq!(app.active_buffer().unwrap().host.id(), HostId(705));
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string(),
            "new\nalpha  \nbeta\ngamma\n\n* item\n[ref]: /target  \n<Unknown value={1} />"
        );
    });
}

#[gpui_kit::test]
fn markdown_supported_encodings_keep_bom_and_untouched_bytes_through_save_reopen(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let fixtures: &[(&[u8], &[u8], usize)] = &[
        (b"a\nb\n", b"x", 0),
        (b"a\r\nb\r\n", b"x", 0),
        (b"a\r\nb\r\nc\n", b"x", 0),
        (b"a\nb", b"x", 0),
        (b"\xef\xbb\xbfa\r\nb\n", b"x", 3),
        (b"\xff\xfea\0\r\0\n\0b\0\n\0", b"x\0", 2),
        (b"\xfe\xff\0a\0\r\0\n\0b\0\n", b"\0x", 2),
        (b"caf\xe9  \r\nref\n", b"x", 0),
        (b"\xd6\xd0\xce\xc4  \r\nref\n", b"x", 0),
    ];
    let host = FileHost::new(HostId(706));
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| cx.set_global(MarkdownWorkbench));
    for (index, &(original, inserted, bom_len)) in fixtures.iter().enumerate() {
        let path = dir.path().join(format!("encoding-{index}.md"));
        std::fs::write(&path, original).unwrap();
        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_open_on_host(host.clone(), &path, window, cx)
        });
        wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
        let save = |vcx: &mut VisualTestContext| {
            vcx.dispatch_action(crate::core::actions::EditorSave);
            wait_until(vcx, |vcx| {
                app.read_with(vcx, |app, _| {
                    app.active_buffer()
                        .is_some_and(|file| !file.dirty && !file.saving)
                })
            });
        };
        save(&mut vcx);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            original,
            "no edit fixture {index}"
        );
        vcx.dispatch_action(gpui_kit::component::input::MoveToStart);
        vcx.update(|window, cx| {
            window.render_frame(cx);
            window.input("x", cx);
        });
        save(&mut vcx);
        let expected = [&original[..bom_len], inserted, &original[bom_len..]].concat();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            expected,
            "local edit fixture {index}"
        );
        vcx.dispatch_action(gpui_kit::component::input::Undo);
        save(&mut vcx);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            original,
            "undo fixture {index}"
        );
        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_close_file(0, window, cx);
            app.editor_open_on_host(host.clone(), &path, window, cx);
        });
        wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
        save(&mut vcx);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            original,
            "reopen fixture {index}"
        );
        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_close_file(0, window, cx)
        });
    }
}

#[gpui_kit::test]
fn markdown_encoding_failure_keeps_original_bytes_and_recoverable_edits(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.md");
    let original = b"caf\xe9  \r\nlast";
    std::fs::write(&path, original).unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| cx.set_global(MarkdownWorkbench));
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.input("🎉", cx);
    });
    vcx.dispatch_action(crate::core::actions::EditorSave);
    wait_until(&mut vcx, |vcx| vcx.has_pending_prompt());
    vcx.simulate_prompt_answer(t(L10nKey::Cancel));
    vcx.run_until_parked();
    assert_eq!(std::fs::read(&path).unwrap(), original);
    app.read_with(&vcx, |app, cx| {
        let file = app.active_buffer().unwrap();
        assert!(file.dirty);
        assert_eq!(file.format(cx).encoding_label(), "Windows-1252");
        assert_eq!(file.input.read(cx).text().to_string(), "🎉café  \nlast");
    });
    vcx.dispatch_action(gpui_kit::component::input::Undo);
    vcx.dispatch_action(crate::core::actions::EditorSave);
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            app.active_buffer()
                .is_some_and(|file| !file.dirty && !file.saving)
        })
    });
    assert_eq!(std::fs::read(&path).unwrap(), original);
}

#[gpui_kit::test]
fn markdown_declared_encoding_errors_are_visible_without_replacement_or_write(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| cx.set_global(MarkdownWorkbench));
    let invalid: &[&[u8]] = &[
        b"\xef\xbb\xbfbroken\xff",
        b"\xff\xfea\0\x00\xd8",
        b"\xfe\xff\0a\xd8\x00",
        b"\xff\xfea",
    ];
    for (index, bytes) in invalid.iter().enumerate() {
        let path = dir.path().join(format!("damaged-{index}.md"));
        std::fs::write(&path, bytes).unwrap();
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_file_in_editor(&path, window, cx)
        });
        wait_until(&mut vcx, |vcx| {
            vcx.read(|cx| {
                crate::ui::notification_history::history(cx)
                    .read(cx)
                    .records()
                    .any(|record| {
                        record.message().contains(&format!("damaged-{index}.md"))
                            && record.message().contains("invalid source bytes")
                    })
            })
        });
        assert_eq!(std::fs::read(&path).unwrap(), *bytes);
        assert_eq!(active_id(&app, &vcx), None);
    }
}

#[gpui_kit::test]
fn markdown_workbench_shares_native_source_history_and_host_save_across_groups(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shared.mdx");
    std::fs::write(&path, "alpha\nbeta\n").unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| cx.set_global(MarkdownWorkbench));
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    let id = active_id(&app, &vcx).unwrap();
    let first = app.read_with(&vcx, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.input("中🎉", cx);
    });
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(app.editor_split(window, cx));
    });
    vcx.update(|window, cx| window.render_frame(cx));
    let second = app.read_with(&vcx, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    vcx.update(|_, cx| {
        assert_ne!(first.entity_id(), second.entity_id());
        assert_eq!(
            first.read(cx).document().entity_id(),
            second.read(cx).document().entity_id()
        );
        assert_eq!(first.read(cx).selection().head(), 7);
        assert_eq!(second.read(cx).selection().head(), 0);
    });
    vcx.dispatch_action(gpui_kit::component::input::SelectAll);
    vcx.dispatch_action(gpui_kit::component::input::Copy);
    vcx.update(|_, cx| {
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("中🎉alpha\nbeta\n".into())
        )
    });
    vcx.dispatch_action(gpui_kit::component::input::Cut);
    vcx.run_until_parked();
    app.read_with(&vcx, |app, cx| {
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string(),
            ""
        )
    });
    vcx.dispatch_action(gpui_kit::component::input::Undo);
    vcx.dispatch_action(gpui_kit::component::input::MoveToStart);
    vcx.update(|_, cx| cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("\r\n".into())));
    vcx.dispatch_action(gpui_kit::component::input::Paste);
    vcx.run_until_parked();
    app.read_with(&vcx, |app, cx| {
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string(),
            "\n中🎉alpha\nbeta\n"
        )
    });
    vcx.dispatch_action(gpui_kit::component::input::Backspace);
    vcx.run_until_parked();
    app.read_with(&vcx, |app, cx| {
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string(),
            "中🎉alpha\nbeta\n"
        )
    });
    vcx.dispatch_action(gpui_kit::component::input::Undo);
    vcx.dispatch_action(gpui_kit::component::input::Undo);
    vcx.dispatch_action(gpui_kit::component::input::Undo);
    vcx.run_until_parked();
    app.read_with(&vcx, |app, cx| {
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string(),
            "alpha\nbeta\n"
        )
    });
    vcx.dispatch_action(gpui_kit::component::input::Redo);
    vcx.run_until_parked();
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        assert_eq!(app.tab_code().unwrap().active_id(), Some(id));
        app.focus_editor(window, cx);
    });
    vcx.update(|window, cx| window.render_frame(cx));
    vcx.dispatch_action(crate::core::actions::EditorSave);
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            app.active_buffer()
                .is_some_and(|file| !file.dirty && !file.saving)
        })
    });
    assert_eq!(
        std::fs::read(&path).unwrap(),
        "中🎉alpha\nbeta\n".as_bytes()
    );
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.open_file_in_editor(&path, window, cx);
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    app.read_with(&vcx, |app, cx| {
        let file = app.active_buffer().unwrap();
        assert_ne!(file.id(), id);
        assert!(file.input.code().is_none());
        assert_eq!(file.input.read(cx).text().to_string(), "中🎉alpha\nbeta\n");
        assert!(app.editor_unsaved().is_empty());
    });
}

#[gpui_kit::test]
fn markdown_native_handler_converts_utf16_and_composition_undo(cx: &mut TestAppContext) {
    use gpui_kit::EntityInputHandler as _;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ime.md");
    std::fs::write(&path, "A🎉Z").unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| cx.set_global(MarkdownWorkbench));
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    let view = app.read_with(&vcx, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        view.update(cx, |view, cx| {
            view.replace_and_mark_text_in_range(Some(1..3), "に🎉", Some(1..3), window, cx);
            assert_eq!(view.marked_text_range(window, cx), Some(1..4));
            assert_eq!(
                view.selected_text_range(false, window, cx).unwrap().range,
                2..4
            );
            view.replace_and_mark_text_in_range(None, "你好", Some(2..2), window, cx);
            view.replace_text_in_range(None, "你好", window, cx);
        });
        window.render_frame(cx);
        view.update(cx, |view, cx| {
            let start = view
                .bounds_for_range(0..0, gpui_kit::Bounds::default(), window, cx)
                .unwrap();
            let committed = view
                .bounds_for_range(3..3, gpui_kit::Bounds::default(), window, cx)
                .unwrap();
            assert!(committed.left() > start.left());
            assert_eq!(committed.top(), start.top());
            assert!(committed.size.height > gpui_kit::px(0.));
        });
    });
    vcx.dispatch_action(gpui_kit::component::input::Undo);
    vcx.run_until_parked();
    app.read_with(&vcx, |app, cx| {
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string(),
            "A🎉Z"
        )
    });
}

#[gpui_kit::test]
fn equal_paths_on_distinct_hosts_do_not_share_buffers_or_local_lsp_documents(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("identity.txt");
    std::fs::write(&path, "same path\n").unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    let local = active_id(&app, &vcx).unwrap();
    let remote: SharedHost = FileHost::new(HostId(69));
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_on_host(remote, &path, window, cx)
    });
    wait_until(&mut vcx, |vcx| {
        active_id(&app, vcx).is_some_and(|id| id != local)
    });
    app.read_with(&vcx, |app, _| {
        assert_eq!(
            app.editor
                .local_files()
                .iter()
                .map(|file| file.id)
                .collect::<Vec<_>>(),
            vec![local]
        );
        let remote = app.active_buffer().unwrap();
        assert_eq!(app.editor_active_location().unwrap().0, HostId(69));
        assert_eq!(remote.path, app.editor.local_file(local).unwrap().path);
    });
}

#[gpui_kit::test]
fn external_writes_reload_clean_text_but_preserve_unsaved_edits(cx: &mut TestAppContext) {
    external_writes_scenario(cx, false);
}

#[gpui_kit::test]
fn markdown_external_writes_reload_clean_text_but_preserve_unsaved_edits(cx: &mut TestAppContext) {
    external_writes_scenario(cx, true);
}

fn external_writes_scenario(cx: &mut TestAppContext, markdown: bool) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(if markdown {
        "watched.md"
    } else {
        "watched.txt"
    });
    std::fs::write(&path, "initial\n").unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    if markdown {
        vcx.update(|_, cx| cx.set_global(MarkdownWorkbench));
    }
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| app.editor.buffers.watch.is_some())
    });
    std::fs::write(&path, "external version\n").unwrap();
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, cx| {
            app.active_buffer()
                .is_some_and(|file| file.input.read(cx).text().to_string() == "external version\n")
        })
    });
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.input("unsaved", cx);
    });
    vcx.run_until_parked();
    let text = app.read_with(&vcx, |app, cx| {
        app.active_buffer()
            .unwrap()
            .input
            .read(cx)
            .text()
            .to_string()
    });
    assert!(text.contains("unsaved"));
    std::fs::write(&path, "another external version\n").unwrap();
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            app.active_buffer().unwrap().conflict.is_some()
        })
    });
    app.read_with(&vcx, |app, cx| {
        let file = app.active_buffer().unwrap();
        assert!(matches!(file.conflict, Some(DiskConflict::Changed(_))));
        assert_eq!(file.input.read(cx).text().to_string(), text);
        assert!(app.editor.is_dirty(HostId::LOCAL, &file.path));
    });
}

#[gpui_kit::test]
fn an_open_finishing_after_its_tab_closed_does_not_land_in_the_next_tab(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("late.txt");
    std::fs::write(&path, "late contents\n").unwrap();
    let host = FileHost::new(HostId(70));
    let release = host.pause_next_read();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_on_host(host.clone(), &path, window, cx)
    });
    wait_until(&mut vcx, |_| host.entered.load(Ordering::SeqCst));
    app.update_in(&mut vcx, |app, window, cx| app.close_tab(0, window, cx));
    release.send(()).unwrap();
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| app.editor.buffers.pending.is_empty())
    });
    app.read_with(&vcx, |app, _| {
        assert_eq!(app.tabs.len(), 1);
        assert!(app.editor_active_location().is_none());
        assert!(app.tab_code().is_none_or(|code| code.files.is_empty()));
    });
}

fn mt(secs: i64, nanos: u32) -> Option<MTime> {
    Some(MTime { secs, nanos })
}

#[test]
fn external_changes_are_told_apart_from_our_own_saves() {
    let ours = mt(100, 0);
    let seen = |m| Observed::Present(m);
    assert_eq!(
        classify_external_change(false, false, ours, seen(ours)),
        ExternalChange::Ignore
    );
    assert_eq!(
        classify_external_change(false, false, ours, seen(mt(101, 0))),
        ExternalChange::Reload
    );
    assert_eq!(
        classify_external_change(false, true, ours, seen(mt(101, 0))),
        ExternalChange::Conflict
    );
    assert_eq!(
        classify_external_change(false, false, mt(100, 0), seen(mt(100, 1))),
        ExternalChange::Reload
    );
    assert_eq!(
        classify_external_change(true, false, ours, seen(mt(101, 0))),
        ExternalChange::Ignore
    );
    assert_eq!(
        classify_external_change(false, false, None, seen(None)),
        ExternalChange::Reload
    );
}

#[test]
fn a_file_that_vanished_is_reported_deleted_whether_or_not_it_was_edited() {
    assert_eq!(
        classify_external_change(false, false, mt(1, 0), Observed::Missing),
        ExternalChange::Deleted
    );
    assert_eq!(
        classify_external_change(false, true, mt(1, 0), Observed::Missing),
        ExternalChange::Deleted
    );
    // Our save replaces by rename; the watcher can catch the gap between.
    assert_eq!(
        classify_external_change(true, false, mt(1, 0), Observed::Missing),
        ExternalChange::Ignore
    );
}

#[gpui_kit::test]
fn a_late_open_cannot_replace_a_newer_hosts_document_or_cursor(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("navigation.txt");
    std::fs::write(&path, "first\nsecond line\nthird line\n").unwrap();
    let older = FileHost::new(HostId(70));
    let newer: SharedHost = FileHost::new(HostId(71));
    let release = older.pause_next_read();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_on_host_at(older.clone(), &path, Some(Position::new(0, 0)), window, cx);
    });
    wait_until(&mut vcx, |_| older.entered.load(Ordering::SeqCst));
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_on_host_at(newer, &path, Some(Position::new(1, 3)), window, cx);
    });
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            app.editor_active_location()
                .is_some_and(|(host, _)| host == HostId(71))
        })
    });
    release.send(()).unwrap();
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| app.editor.buffers.pending.is_empty())
    });
    app.read_with(&vcx, |app, cx| {
        assert_eq!(app.editor_active_location().unwrap().0, HostId(71));
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .cursor_position(),
            Position::new(1, 3)
        );
        assert_eq!(
            app.tab_code().unwrap().files.len(),
            2,
            "the older request may finish only in the background"
        );
    });
}

#[gpui_kit::test]
fn markdown_session_restore_surfaces_failed_reads_without_losing_valid_files(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let valid = dir.path().join("restored.md");
    let missing = dir.path().join("unavailable.md");
    std::fs::write(&valid, "# restored\n").unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| cx.set_global(MarkdownWorkbench));
    app.update_in(&mut vcx, |app, window, cx| {
        let tab = app.tabs[0].id();
        app.tab_code_mut_or_init();
        app.editor.restored.remove(&tab);
        editor_session::put(
            cx,
            tab,
            TabEditor {
                files: vec![missing.clone(), valid.clone()],
                active: 1,
                visible: true,
                ..Default::default()
            },
        );
        app.editor_restore_active(window, cx);
    });
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| app.active_buffer().is_some())
    });
    app.read_with(&vcx, |app, cx| {
        let file = app.active_buffer().unwrap();
        assert_eq!(file.path.file_name(), valid.file_name());
        assert_eq!(file.input.read(cx).text().to_string(), "# restored\n");
        assert!(file.input.code().is_none());
        assert_eq!(app.tab_code().unwrap().files.len(), 1);
    });
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |_, cx| {
            crate::ui::notification_history::history(cx)
                .read(cx)
                .records()
                .any(|record| record.message().contains("unavailable.md"))
        })
    });
}

#[gpui_kit::test]
fn markdown_tab_uses_editorconfig_in_each_shared_presentation(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("indent.md");
    std::fs::write(
        dir.path().join(".editorconfig"),
        "root = true\n[*.md]\nindent_style = space\nindent_size = 2\n",
    )
    .unwrap();
    std::fs::write(&path, "first\nsecond\n").unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| cx.set_global(MarkdownWorkbench));
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.press("tab", cx);
    });
    vcx.run_until_parked();
    app.read_with(&vcx, |app, cx| {
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string(),
            "  first\nsecond\n"
        )
    });
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(app.editor_split(window, cx));
    });
    vcx.update(|window, cx| window.render_frame(cx));
    app.update_in(&mut vcx, |app, window, cx| {
        app.active_editor_input()
            .unwrap()
            .set_cursor_position(Position::new(1, 0), window, cx);
    });
    vcx.update(|window, cx| window.press("tab", cx));
    vcx.run_until_parked();
    app.read_with(&vcx, |app, cx| {
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string(),
            "  first\n  second\n"
        )
    });
    vcx.dispatch_action(gpui_kit::component::input::Undo);
    vcx.run_until_parked();
    app.read_with(&vcx, |app, cx| {
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string(),
            "  first\nsecond\n"
        )
    });
}
fn read_owned(host: &Arc<FileHost>, path: &Path) -> std::io::Result<Vec<u8>> {
    let host = host.clone();
    let path = path.to_path_buf();
    std::thread::spawn(move || host.read_file(&path, 1024 * 1024))
        .join()
        .unwrap()
}

fn screenshot_bytes() -> Vec<u8> {
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        1,
        1,
        image::Rgba([4, 5, 6, 255]),
    ))
    .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
    .unwrap();
    png
}

#[gpui_kit::test]
fn markdown_screenshot_paste_creates_owned_unique_asset_and_survives_history_save_reopen(
    cx: &mut TestAppContext,
) {
    use gpui_kit::component::input::{Redo, Undo};
    use gpui_kit::{ClipboardItem, Image, ImageFormat};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("images.md");
    std::fs::write(&path, "before\n").unwrap();
    std::fs::create_dir(dir.path().join("assets")).unwrap();
    std::fs::write(dir.path().join("assets/screenshot.png"), b"unrelated asset").unwrap();
    let host = FileHost::new(HostId(715));
    let png = screenshot_bytes();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| cx.set_global(MarkdownWorkbench));
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_on_host(host.clone(), &path, window, cx);
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    vcx.update(|window, cx| {
        cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(
            ImageFormat::Png,
            png.clone(),
        )));
        window.render_frame(cx);
        window.press("secondary-v", cx);
    });
    let inserted = "![screenshot](assets/screenshot-1.png)before\n";
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, cx| {
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string()
                == inserted
        })
    });
    let asset = dir.path().join("assets/screenshot-1.png");
    assert_eq!(read_owned(&host, &asset).unwrap(), png);
    assert_eq!(
        std::fs::read(dir.path().join("assets/screenshot.png")).unwrap(),
        b"unrelated asset"
    );
    vcx.dispatch_action(Undo);
    vcx.run_until_parked();
    app.read_with(&vcx, |app, cx| {
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string(),
            "before\n"
        )
    });
    assert_eq!(read_owned(&host, &asset).unwrap(), png);
    vcx.dispatch_action(Redo);
    vcx.run_until_parked();
    vcx.dispatch_action(crate::core::actions::EditorSave);
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            !app.active_buffer().unwrap().dirty && !app.active_buffer().unwrap().saving
        })
    });
    assert_eq!(read_owned(&host, &path).unwrap(), inserted.as_bytes());
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.editor_open_on_host(host.clone(), &path, window, cx);
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    app.read_with(&vcx, |app, cx| {
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string(),
            inserted
        )
    });
    assert_eq!(read_owned(&host, &asset).unwrap(), png);
}

fn paste_screenshot(vcx: &mut VisualTestContext, png: &[u8]) {
    vcx.update(|window, cx| {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_image(
            &gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Png, png.to_vec()),
        ));
        window.render_frame(cx);
        window.press("secondary-v", cx);
    });
    vcx.run_until_parked();
}

fn drop_image(vcx: &mut VisualTestContext, path: &Path) {
    use gpui_kit::{ExternalPaths, FileDropEvent, InputEvent as _};
    vcx.update(|window, cx| {
        window.render_frame(cx);
        let position = window.find("markdown-source").bounds().center();
        window.dispatch_event(
            FileDropEvent::Entered {
                position,
                paths: ExternalPaths(vec![path.to_path_buf()].into()),
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
        window.dispatch_event(FileDropEvent::Submit { position }.to_platform_input(), cx);
        window.render_frame(cx);
    });
    vcx.run_until_parked();
}

#[gpui_kit::test]
fn markdown_external_drop_uses_owning_host_and_discard_keeps_the_resource(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("drop.md");
    let incoming = dir.path().join("external.png");
    let png = screenshot_bytes();
    std::fs::write(&path, "original").unwrap();
    std::fs::write(&incoming, &png).unwrap();
    let host = FileHost::new(HostId(716));
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| cx.set_global(MarkdownWorkbench));
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_on_host(host.clone(), &path, window, cx)
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    drop_image(&mut vcx, &incoming);
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, cx| {
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string()
                == "![external](assets/external.png)original"
        })
    });
    assert_eq!(host.writes.load(Ordering::SeqCst), 1);
    let asset = dir.path().join("assets/external.png");
    assert_eq!(read_owned(&host, &asset).unwrap(), png);
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    wait_until(&mut vcx, |vcx| vcx.has_pending_prompt());
    vcx.simulate_prompt_answer(t(L10nKey::EditorDiscard));
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_none());
    assert_eq!(read_owned(&host, &asset).unwrap(), png);
    assert_eq!(read_owned(&host, &path).unwrap(), b"original");
}

#[gpui_kit::test]
fn markdown_untitled_image_cancel_is_unchanged_and_success_waits_for_document_save(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chosen.md");
    let png = screenshot_bytes();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| cx.set_global(MarkdownWorkbench));
    app.update_in(&mut vcx, |app, window, cx| app.editor_new_file(window, cx));
    paste_screenshot(&mut vcx, &png);
    cx.simulate_new_path_selection(|_| None);
    vcx.run_until_parked();
    app.read_with(&vcx, |app, cx| {
        let file = app.active_buffer().unwrap();
        assert!(file.untitled.is_some());
        assert_eq!(file.input.read(cx).text().to_string(), "");
        assert!(!file.dirty);
    });
    assert!(!dir.path().join("assets").exists());
    assert!(!path.exists());
    paste_screenshot(&mut vcx, &png);
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, cx| {
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string()
                == "![screenshot](assets/screenshot.png)"
        })
    });
    assert_eq!(std::fs::read(&path).unwrap(), b"");
    assert_eq!(
        std::fs::read(dir.path().join("assets/screenshot.png")).unwrap(),
        png
    );
    app.read_with(&vcx, |app, _| assert!(app.active_buffer().unwrap().dirty));
}

#[gpui_kit::test]
fn markdown_failed_asset_write_has_no_link_and_late_write_cannot_touch_replacement(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pending.md");
    let replacement = dir.path().join("replacement.md");
    std::fs::write(&path, "original").unwrap();
    std::fs::write(&replacement, "replacement").unwrap();
    let host = FileHost::new(HostId(717));
    let png = screenshot_bytes();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| cx.set_global(MarkdownWorkbench));
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_on_host(host.clone(), &path, window, cx)
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    host.fail_next_write();
    paste_screenshot(&mut vcx, &png);
    wait_until(&mut vcx, |vcx| {
        vcx.update(|window, cx| {
            window.render_frame(cx);
            window
                .try_find("markdown-image-insertion-status")
                .is_some_and(|status| {
                    status
                        .label()
                        .is_some_and(|label| label.contains(t(L10nKey::MarkdownImageWriteFailed)))
                })
        })
    });
    app.read_with(&vcx, |app, cx| {
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string(),
            "original"
        )
    });
    let release = host.pause_next_write();
    paste_screenshot(&mut vcx, &png);
    wait_until(&mut vcx, |_| host.entered.load(Ordering::SeqCst));
    vcx.update(|window, cx| window.input("typing", cx));
    app.read_with(&vcx, |app, cx| {
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string(),
            "typingoriginal"
        )
    });
    // Replacement is a new document, not merely a different current caret.
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_on_host(host.clone(), &replacement, window, cx)
    });
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            app.active_buffer().unwrap().path == replacement
        })
    });
    release.send(()).unwrap();
    wait_until(&mut vcx, |_| {
        read_owned(&host, &dir.path().join("assets/screenshot-1.png"))
            .is_ok_and(|bytes| bytes == png)
    });
    vcx.run_until_parked();
    app.read_with(&vcx, |app, cx| {
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string(),
            "replacement"
        )
    });
    assert_eq!(
        read_owned(&host, &dir.path().join("assets/screenshot-1.png")).unwrap(),
        png
    );
}

#[gpui_kit::test]
fn markdown_delayed_image_write_rebases_while_typing_without_losing_text(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("typing.md");
    std::fs::write(&path, "original").unwrap();
    let host = FileHost::new(HostId(718));
    let png = screenshot_bytes();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| cx.set_global(MarkdownWorkbench));
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_on_host(host.clone(), &path, window, cx)
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    let release = host.pause_next_write();
    paste_screenshot(&mut vcx, &png);
    wait_until(&mut vcx, |_| host.entered.load(Ordering::SeqCst));
    vcx.update(|window, cx| window.input("typed", cx));
    vcx.run_until_parked();
    release.send(()).unwrap();
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, cx| {
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string()
                == "![screenshot](assets/screenshot.png)typedoriginal"
        })
    });
    vcx.dispatch_action(gpui_kit::component::input::Undo);
    vcx.run_until_parked();
    app.read_with(&vcx, |app, cx| {
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string(),
            "typedoriginal"
        )
    });
    assert_eq!(
        read_owned(&host, &dir.path().join("assets/screenshot.png")).unwrap(),
        png
    );
}

#[gpui_kit::test]
fn markdown_untitled_host_save_must_finish_before_assets_and_failed_save_never_creates_them(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let chosen = dir.path().join("chosen.md");
    let host = FileHost::new(HostId(719));
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| cx.set_global(MarkdownWorkbench));
    app.update_in(&mut vcx, |app, window, cx| {
        let tab = app.tabs[0].id();
        let n = app.editor.buffers.next_untitled();
        let id = app.editor_new_buffer(
            host.clone(),
            PathBuf::new(),
            Some(n),
            String::new(),
            TextFormat::default(),
            EditorConfig::default(),
            editor_text::detect_indent("", "markdown"),
            None,
            window,
            cx,
        );
        app.editor_show_in_tab(tab, id, true, window, cx);
    });
    let release = host.pause_next_write();
    paste_screenshot(&mut vcx, &screenshot_bytes());
    vcx.update(|window, cx| {
        window.press("secondary-a", cx);
        window.input(&chosen.display().to_string(), cx);
        window.press("enter", cx);
    });
    wait_until(&mut vcx, |_| host.entered.load(Ordering::SeqCst));
    assert!(!dir.path().join("assets").exists());
    app.read_with(&vcx, |app, cx| {
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string(),
            ""
        )
    });
    release.send(()).unwrap();
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, cx| {
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string()
                == "![screenshot](assets/screenshot.png)"
        })
    });
    assert_eq!(
        read_owned(&host, &dir.path().join("assets/screenshot.png")).unwrap(),
        screenshot_bytes()
    );

    // A second untitled document exercises the same real Save As bar but a
    // failed document write. No asset directory is created at its destination.
    let failed_dir = tempfile::tempdir().unwrap();
    let failed = failed_dir.path().join("failed.md");
    app.update_in(&mut vcx, |app, window, cx| {
        let tab = app.tabs[0].id();
        let n = app.editor.buffers.next_untitled();
        let id = app.editor_new_buffer(
            host.clone(),
            PathBuf::new(),
            Some(n),
            String::new(),
            TextFormat::default(),
            EditorConfig::default(),
            editor_text::detect_indent("", "markdown"),
            None,
            window,
            cx,
        );
        app.editor_show_in_tab(tab, id, true, window, cx);
    });
    host.fail_next_write();
    paste_screenshot(&mut vcx, &screenshot_bytes());
    vcx.update(|window, cx| {
        window.press("secondary-a", cx);
        window.input(&failed.display().to_string(), cx);
        window.press("enter", cx);
    });
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            let file = app.active_buffer().unwrap();
            file.path == failed && !file.saving && file.asset_location.is_none()
        })
    });
    assert!(!failed_dir.path().join("assets").exists());
    assert!(!failed.exists());
    app.read_with(&vcx, |app, cx| {
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string(),
            ""
        )
    });
    paste_screenshot(&mut vcx, &screenshot_bytes());
    app.read_with(&vcx, |app, _| {
        assert!(
            app.editor.has_bar(),
            "a failed document save must not authorize assets on a subsequent paste"
        )
    });
    assert!(!failed_dir.path().join("assets").exists());
    vcx.update(|window, cx| {
        window.press("secondary-a", cx);
        window.input(&failed.display().to_string(), cx);
        window.press("enter", cx);
    });
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, cx| {
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string()
                == "![screenshot](assets/screenshot.png)"
        })
    });
    assert_eq!(read_owned(&host, &failed).unwrap(), b"");
    assert_eq!(
        read_owned(&host, &failed_dir.path().join("assets/screenshot.png")).unwrap(),
        screenshot_bytes()
    );
}

#[gpui_kit::test]
fn markdown_image_completion_never_replaces_text_typed_over_pending_selection(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("selection.md");
    std::fs::write(&path, "original selection").unwrap();
    let host = FileHost::new(HostId(720));
    let png = screenshot_bytes();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| cx.set_global(MarkdownWorkbench));
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_on_host(host.clone(), &path, window, cx)
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    vcx.update(|window, cx| window.press("secondary-a", cx));
    let release = host.pause_next_write();
    paste_screenshot(&mut vcx, &png);
    wait_until(&mut vcx, |_| host.entered.load(Ordering::SeqCst));
    vcx.update(|window, cx| window.input("new author text", cx));
    vcx.run_until_parked();
    release.send(()).unwrap();
    wait_until(&mut vcx, |vcx| {
        vcx.update(|window, cx| {
            window.render_frame(cx);
            window
                .try_find("markdown-image-insertion-status")
                .is_some_and(|status| {
                    status
                        .label()
                        .is_some_and(|label| label == t(L10nKey::MarkdownImageSelectionChanged))
                })
        })
    });
    app.read_with(&vcx, |app, cx| {
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string(),
            "new author text"
        )
    });
    assert_eq!(
        read_owned(&host, &dir.path().join("assets/screenshot.png")).unwrap(),
        png
    );
}

#[gpui_kit::test]
fn markdown_modifier_links_open_on_the_document_host_and_keep_delayed_group_intent(
    cx: &mut TestAppContext,
) {
    use gpui_kit::test::TestEventExt as _;
    use gpui_kit::{Modifiers, MouseButton};
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("docs")).unwrap();
    let original = "outside [target][Ref]\n\n[Ref]: <../target%20doc.md> 'keep'\n";
    std::fs::write(dir.path().join("docs/start.md"), original).unwrap();
    std::fs::write(
        dir.path().join("target doc.md"),
        "# Destination\nremote target",
    )
    .unwrap();
    let host = FileHost::mapped(HostId(709), dir.path());
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
    vcx.update(|_, cx| cx.set_global(MarkdownWorkbench));
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_on_host(host.clone(), Path::new("/docs/start.md"), window, cx)
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    let source_id = active_id(&app, &vcx).unwrap();
    let view = app.read_with(&vcx, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    vcx.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    wait_until(&mut vcx, |vcx| {
        vcx.update(|window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
        });
        view.read_with(vcx, |view, cx| {
            let opening = view.range_to_bounds(&(8..8), cx);
            opening.is_some() && opening == view.range_to_bounds(&(9..9), cx)
        })
    });
    let at = vcx.update(|window, cx| {
        let point = view
            .read(cx)
            .range_to_bounds(&(10..10), cx)
            .unwrap()
            .center();
        window.pointer_down(point, MouseButton::Left, cx);
        window.pointer_up(point, MouseButton::Left, cx);
        point
    });
    vcx.run_until_parked();
    assert_eq!(
        active_id(&app, &vcx),
        Some(source_id),
        "ordinary click must only edit"
    );
    view.read_with(&vcx, |view, cx| {
        assert_eq!(
            view.document().read(cx).encode().unwrap(),
            original.as_bytes()
        )
    });
    // Hide the contextual syntax again before activating the rendered link.
    view.update_in(&mut vcx, |view, _, cx| view.select_range(0..0, cx));
    vcx.update(|window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
    });
    let release = host.pause_next_read();
    let modifier = if cfg!(target_os = "macos") {
        Modifiers {
            platform: true,
            ..Modifiers::none()
        }
    } else {
        Modifiers::control()
    };
    vcx.simulate_mouse_move(at, None, modifier);
    vcx.simulate_mouse_down(at, MouseButton::Left, modifier);
    vcx.simulate_mouse_up(at, MouseButton::Left, modifier);
    wait_until(&mut vcx, |_| host.entered.load(Ordering::SeqCst));
    app.update_in(&mut vcx, |app, window, cx| app.activate(1, window, cx));
    release.send(()).unwrap();
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| app.editor.buffers.pending.is_empty())
    });
    app.read_with(&vcx, |app, cx| {
        assert!(
            app.tab_code().is_none_or(|code| code.files.is_empty()),
            "late open must not redirect into active tab"
        );
        let target = app
            .editor
            .buffers
            .at(HostId(709), Path::new("/target doc.md"))
            .unwrap();
        let file = app.editor.buffers.get(target).unwrap();
        assert_eq!(file.host.id(), HostId(709));
        assert_eq!(
            file.input.read(cx).text().to_string(),
            "# Destination\nremote target"
        );
        assert!(app.tabs[0].code.as_ref().unwrap().files.contains(&target));
        assert_eq!(
            view.read(cx).document().read(cx).encode().unwrap(),
            original.as_bytes()
        );
    });
}

#[gpui_kit::test]
fn markdown_late_link_read_cannot_land_after_its_source_group_is_replaced(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("start.md"), "[next](next.md)").unwrap();
    std::fs::write(dir.path().join("next.md"), "destination").unwrap();
    let host = FileHost::mapped(HostId(710), dir.path());
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| cx.set_global(MarkdownWorkbench));
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_on_host(host.clone(), Path::new("/start.md"), window, cx)
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    let view = app.read_with(&vcx, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    vcx.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    wait_until(&mut vcx, |vcx| {
        vcx.update(|window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
        });
        view.read_with(vcx, |view, cx| {
            view.range_to_bounds(&(0..0), cx).is_some()
                && view.range_to_bounds(&(0..0), cx) == view.range_to_bounds(&(1..1), cx)
        })
    });
    view.update_in(&mut vcx, |view, _, cx| view.select_range(2..2, cx));
    let release = host.pause_next_read();
    vcx.dispatch_action(markdown::FollowLink);
    wait_until(&mut vcx, |_| host.entered.load(Ordering::SeqCst));
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.editor_new_file(window, cx);
    });
    let replacement = active_id(&app, &vcx).unwrap();
    release.send(()).unwrap();
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| app.editor.buffers.pending.is_empty())
    });
    assert_eq!(active_id(&app, &vcx), Some(replacement));
    app.read_with(&vcx, |app, _| {
        assert!(
            app.editor
                .buffers
                .at(HostId(710), Path::new("/next.md"))
                .is_none()
        )
    });
}

#[gpui_kit::test]
fn markdown_local_relative_link_uses_its_document_directory(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("docs")).unwrap();
    let source = dir.path().join("docs/start.md");
    let destination = dir.path().join("next doc.md");
    std::fs::write(&source, "[next](../next%20doc.md)").unwrap();
    std::fs::write(&destination, "local destination").unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| cx.set_global(MarkdownWorkbench));
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&source, window, cx)
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    let id = active_id(&app, &vcx).unwrap();
    let view = app.read_with(&vcx, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    });
    vcx.dispatch_action(crate::core::actions::ToggleDocumentPreview);
    wait_until(&mut vcx, |vcx| {
        vcx.update(|window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
        });
        view.read_with(vcx, |view, cx| {
            view.range_to_bounds(&(0..0), cx).is_some()
                && view.range_to_bounds(&(0..0), cx) == view.range_to_bounds(&(1..1), cx)
        })
    });
    view.update_in(&mut vcx, |view, _, cx| view.select_range(2..2, cx));
    vcx.dispatch_action(markdown::FollowLink);
    wait_until(&mut vcx, |vcx| {
        active_id(&app, vcx).is_some_and(|active| active != id)
    });
    app.read_with(&vcx, |app, cx| {
        let file = app.active_buffer().unwrap();
        assert_eq!(file.host.id(), HostId::LOCAL);
        assert_eq!(
            file.path,
            lyrel_core::host::local::LocalHost::new()
                .canonicalize(&destination)
                .unwrap()
        );
        assert_eq!(file.input.read(cx).text().to_string(), "local destination");
        assert_eq!(
            view.read(cx).document().read(cx).encode().unwrap(),
            b"[next](../next%20doc.md)"
        );
    });
}

#[gpui_kit::test]
fn markdown_external_image_drop_writes_and_reads_virtual_paths_only_on_owning_host(
    cx: &mut TestAppContext,
) {
    let remote = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    std::fs::create_dir(remote.path().join("docs")).unwrap();
    std::fs::write(remote.path().join("docs/owned.md"), "remote text").unwrap();
    let incoming = local.path().join("capture.png");
    let png = screenshot_bytes();
    std::fs::write(&incoming, &png).unwrap();
    let host = FileHost::mapped(HostId(721), remote.path());
    let document = Path::new("/docs/owned.md");
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    vcx.update(|_, cx| cx.set_global(MarkdownWorkbench));
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_on_host(host.clone(), document, window, cx)
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    drop_image(&mut vcx, &incoming);
    let inserted = "![capture](assets/capture.png)remote text";
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, cx| {
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string()
                == inserted
        })
    });
    assert_eq!(
        read_owned(&host, Path::new("/docs/assets/capture.png")).unwrap(),
        png
    );
    assert!(!local.path().join("assets").exists());
    vcx.dispatch_action(crate::core::actions::EditorSave);
    wait_until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            !app.active_buffer().unwrap().saving && !app.active_buffer().unwrap().dirty
        })
    });
    assert_eq!(read_owned(&host, document).unwrap(), inserted.as_bytes());
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_close_file(0, window, cx);
        app.editor_open_on_host(host.clone(), document, window, cx);
    });
    wait_until(&mut vcx, |vcx| active_id(&app, vcx).is_some());
    app.read_with(&vcx, |app, cx| {
        assert_eq!(app.active_buffer().unwrap().host.id(), HostId(721));
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string(),
            inserted
        );
    });
    assert_eq!(
        read_owned(&host, Path::new("/docs/assets/capture.png")).unwrap(),
        png
    );
}
