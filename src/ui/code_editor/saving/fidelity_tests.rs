//! File fidelity consumers use the production workbench, owning Host and stored bytes.
use super::*;
use crate::ui::app::test_window::harness_with_tabs;
use crate::ui::code_editor::test_host::{FileHost, canonical_tempdir};
#[cfg(feature = "native-markdown-tests")]
use gpui_kit::test::TestEventExt as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

fn settle(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        window.render_frame(cx);
    });
    cx.run_until_parked();
}
#[track_caller]
fn until(cx: &mut VisualTestContext, mut ready: impl FnMut(&mut VisualTestContext) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        settle(cx);
        if ready(cx) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "file fidelity workflow did not settle"
        );
        cx.executor().advance_clock(Duration::from_millis(50));
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn save(app: &Entity<LyrelApp>, cx: &mut VisualTestContext) {
    cx.dispatch_action(crate::core::actions::EditorSave);
    until(cx, |cx| {
        app.read_with(cx, |app, _| {
            app.active_buffer()
                .is_some_and(|file| !file.dirty && !file.saving)
        })
    });
}
fn view(
    app: &Entity<LyrelApp>,
    cx: &VisualTestContext,
) -> Entity<lyrel_markdown_editor::EditorState> {
    app.read_with(cx, |app, _| {
        app.active_editor_input()
            .unwrap()
            .markdown()
            .unwrap()
            .clone()
    })
}
fn enter_path(cx: &mut VisualTestContext, path: &Path) {
    cx.update(|window, cx| {
        window.press("secondary-a", cx);
        window.input(&path.display().to_string(), cx);
        window.press("enter", cx);
    });
}

fn encodings(cx: &mut TestAppContext) {
    // Independently specified bytes include both UTF-16 byte orders and legacy encodings.
    let fixtures: &[(&str, &[u8], &[u8])] = &[
        (
            "utf8",
            b"\xef\xbb\xbf# A\r\nbody  \nattached$$x$$literal\r\n<Unknown x={1} />",
            b"\xef\xbb\xbf!# A\r\nbody  \nattached$$x$$literal\r\n<Unknown x={1} />",
        ),
        (
            "utf16le",
            b"\xff\xfe#\0 \0A\0\r\0\n\0\x2d\x4e \0 \0\n\0Z\0",
            b"\xff\xfe!\0#\0 \0A\0\r\0\n\0\x2d\x4e \0 \0\n\0Z\0",
        ),
        (
            "utf16be",
            b"\xfe\xff\0#\0 \0A\0\r\0\n\x4e\x2d\0 \0 \0\n\0Z",
            b"\xfe\xff\0!\0#\0 \0A\0\r\0\n\x4e\x2d\0 \0 \0\n\0Z",
        ),
        (
            "cp1252",
            b"# caf\xe9\r\n\x93quote\x94  \nZ",
            b"!# caf\xe9\r\n\x93quote\x94  \nZ",
        ),
        (
            "gb18030",
            b"# \xd6\xd0\xce\xc4\r\nbody  \nZ",
            b"!# \xd6\xd0\xce\xc4\r\nbody  \nZ",
        ),
    ];
    let dir = canonical_tempdir();
    std::fs::write(dir.path().join(".editorconfig"), "root = true\n[*]\ntrim_trailing_whitespace = true\ninsert_final_newline = true\nend_of_line = crlf\n").unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    for (name, original, edited) in fixtures {
        let path = dir.path().join(format!("{name}.md"));
        std::fs::write(&path, original).unwrap();
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_file_in_editor(&path, window, cx)
        });
        until(&mut vcx, |vcx| {
            app.read_with(vcx, |app, _| {
                app.active_buffer().is_some_and(|file| file.path == path)
            })
        });
        let presentation = view(&app, &vcx);
        for _ in 0..2 {
            vcx.dispatch_action(crate::core::actions::ToggleDocumentPreview);
            settle(&mut vcx);
            save(&app, &mut vcx);
            assert_eq!(std::fs::read(&path).unwrap(), *original, "untouched {name}");
        }
        presentation.update_in(&mut vcx, |view, window, cx| {
            view.select_range(0..0, cx);
            view.focus_handle(cx).focus(window, cx);
        });
        vcx.update(|window, cx| window.input("!", cx));
        settle(&mut vcx);
        save(&app, &mut vcx);
        assert_eq!(std::fs::read(&path).unwrap(), *edited, "edited {name}");
        vcx.dispatch_action(gpui_kit::component::input::Undo);
        settle(&mut vcx);
        save(&app, &mut vcx);
        assert_eq!(std::fs::read(&path).unwrap(), *original, "Undo {name}");
        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_close_file(0, window, cx)
        });
        settle(&mut vcx);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_file_in_editor(&path, window, cx)
        });
        until(&mut vcx, |vcx| {
            app.read_with(vcx, |app, _| app.active_buffer().is_some())
        });
        save(&app, &mut vcx);
        assert_eq!(std::fs::read(&path).unwrap(), *original, "reopened {name}");
        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_close_file(0, window, cx)
        });
        settle(&mut vcx);
    }
}

fn explicit_policy(cx: &mut TestAppContext) {
    let dir = canonical_tempdir();
    let path = dir.path().join("policy.md");
    let original = b"\xef\xbb\xbf# A  \r\nbody\t\nattached$$x$$literal  ";
    std::fs::write(&path, original).unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| app.active_buffer().is_some())
    });
    // Conversion is deliberate and undoable, unlike rendering or ordinary save.
    app.update_in(&mut vcx, |app, _, cx| app.toggle_line_ending(cx));
    save(&app, &mut vcx);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"\xef\xbb\xbf# A  \r\nbody\t\r\nattached$$x$$literal  "
    );
    vcx.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut vcx);
    save(&app, &mut vcx);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    vcx.dispatch_action(crate::core::actions::EditorTrimTrailingWhitespace);
    settle(&mut vcx);
    save(&app, &mut vcx);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"\xef\xbb\xbf# A\r\nbody\nattached$$x$$literal"
    );
    vcx.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut vcx);
    save(&app, &mut vcx);
    assert_eq!(std::fs::read(&path).unwrap(), original);
}

fn second_view_in_flight(cx: &mut TestAppContext) {
    let dir = canonical_tempdir();
    let stored = dir.path().join("snapshot.md");
    let path = PathBuf::from("/snapshot.md");
    let original = b"\xef\xbb\xbf# Original\r\nbody  \nattached$$x$$literal";
    std::fs::write(&stored, original).unwrap();
    let host = FileHost::mapped(HostId(743), dir.path());
    let release = host.pause_next_write();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_on_host(host.clone(), &path, window, cx)
    });
    until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| app.active_buffer().is_some())
    });
    let first = view(&app, &vcx);
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(app.editor_split(window, cx));
    });
    settle(&mut vcx);
    let second = view(&app, &vcx);
    assert_ne!(first.entity_id(), second.entity_id());
    second.update_in(&mut vcx, |view, window, cx| {
        view.select_range(0..0, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    vcx.update(|window, cx| window.input("!", cx));
    settle(&mut vcx);
    let id = app.read_with(&vcx, |app, _| app.active_buffer().unwrap().id());
    let tab = app.read_with(&vcx, |app, _| app.tabs[0].id());
    app.update_in(&mut vcx, |app, window, cx| app.close_tab(0, window, cx));
    settle(&mut vcx);
    vcx.simulate_prompt_answer(t(L10nKey::Save));
    until(&mut vcx, |_| host.entered.load(Ordering::SeqCst));
    second.update_in(&mut vcx, |view, window, cx| {
        view.focus_handle(cx).focus(window, cx)
    });
    vcx.update(|window, cx| window.input("later", cx));
    app.update_in(&mut vcx, |app, _, cx| app.toggle_line_ending(cx));
    settle(&mut vcx);
    app.update_in(&mut vcx, |app, window, cx| app.activate(1, window, cx));
    vcx.dispatch_action(crate::core::actions::EditorNewMarkdownFile);
    settle(&mut vcx);
    vcx.update(|window, cx| window.input("different target", cx));
    release.send(()).unwrap();
    until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| !app.editor.buffers.get(id).unwrap().saving)
    });
    assert_eq!(
        std::fs::read(&stored).unwrap(),
        b"\xef\xbb\xbf!# Original\r\nbody  \nattached$$x$$literal"
    );
    app.read_with(&vcx, |app, cx| {
        let file = app.editor.buffers.get(id).unwrap();
        assert!(
            file.dirty,
            "completion must not clean later content/representation"
        );
        assert_eq!(file.host.id(), HostId(743));
        assert_eq!(file.path, path);
        assert_eq!(
            first.read(cx).document().entity_id(),
            second.read(cx).document().entity_id()
        );
        assert_eq!(
            first.read(cx).text(),
            "!later# Original\nbody  \nattached$$x$$literal"
        );
        assert_eq!(
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .text()
                .to_string(),
            "different target"
        );
        assert!(app.tabs.iter().any(|candidate| candidate.id() == tab));
    });
    app.update_in(&mut vcx, |app, window, cx| app.activate(0, window, cx));
    save(&app, &mut vcx);
    assert_eq!(
        std::fs::read(&stored).unwrap(),
        b"\xef\xbb\xbf!later# Original\r\nbody  \r\nattached$$x$$literal"
    );
    app.read_with(&vcx, |app, _| {
        assert!(
            app.tabs.iter().any(|candidate| candidate.id() == tab),
            "a later save cannot revive the retired close"
        )
    });
}

fn first_save_failure_history(cx: &mut TestAppContext) {
    let dir = canonical_tempdir();
    let host = FileHost::mapped(HostId(744), dir.path());
    let path = PathBuf::from("/first.txt");
    let stored = dir.path().join("first.txt");
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    let id = app.update_in(&mut vcx, |app, window, cx| {
        let n = app.editor.buffers.next_untitled();
        let id = app.editor_new_buffer(
            host.clone(),
            PathBuf::new(),
            Some(n),
            BufferKind::Markdown,
            String::new(),
            TextFormat::default(),
            EditorConfig::default(),
            lyrel_text::detect_indent("", "markdown"),
            None,
            window,
            cx,
        );
        app.editor_show_in_tab(app.tabs[0].id(), id, true, window, cx);
        id
    });
    settle(&mut vcx);
    let presentation = view(&app, &vcx);
    let document = presentation.read_with(&vcx, |view, _| view.document().entity_id());
    vcx.update(|window, cx| window.input("attached$$x$$literal\n<Unknown />", cx));
    settle(&mut vcx);
    host.fail_next_write();
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    settle(&mut vcx);
    vcx.simulate_prompt_answer(t(L10nKey::Save));
    settle(&mut vcx);
    enter_path(&mut vcx, &path);
    until(&mut vcx, |vcx| {
        host.entered.load(Ordering::SeqCst)
            && app.read_with(vcx, |app, _| {
                app.editor.buffers.get(id).is_some_and(|file| !file.saving)
            })
    });
    app.read_with(&vcx, |app, cx| {
        let file = app.active_buffer().unwrap();
        assert!(file.untitled.is_some());
        assert!(
            file.path.as_os_str().is_empty(),
            "failed first save must retain its original identity"
        );
        assert!(file.dirty);
        assert_eq!(
            file.input
                .markdown()
                .unwrap()
                .read(cx)
                .document()
                .entity_id(),
            document
        );
    });
    assert!(!stored.exists());
    vcx.dispatch_action(crate::core::actions::EditorSaveAs);
    settle(&mut vcx);
    enter_path(&mut vcx, &path);
    until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            app.active_buffer()
                .is_some_and(|file| file.path == path && !file.saving && !file.dirty)
        })
    });
    assert_eq!(
        std::fs::read(&stored).unwrap(),
        b"attached$$x$$literal\n<Unknown />"
    );
    app.read_with(&vcx, |app, cx| {
        let file = app.active_buffer().unwrap();
        assert_eq!(file.id(), id);
        assert_eq!(
            file.input
                .markdown()
                .unwrap()
                .read(cx)
                .document()
                .entity_id(),
            document
        );
    });
    vcx.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut vcx);
    save(&app, &mut vcx);
    assert_eq!(std::fs::read(&stored).unwrap(), b"");
    vcx.dispatch_action(gpui_kit::component::input::Redo);
    settle(&mut vcx);
    save(&app, &mut vcx);
    assert_eq!(
        std::fs::read(&stored).unwrap(),
        b"attached$$x$$literal\n<Unknown />"
    );
}

fn encoding_cancel_recover(cx: &mut TestAppContext) {
    let dir = canonical_tempdir();
    let path = dir.path().join("legacy.md");
    let target = dir.path().join("utf8.txt");
    let original = b"caf\xe9\r\nbody  \nZ";
    std::fs::write(&path, original).unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx)
    });
    until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| app.active_buffer().is_some())
    });
    let presentation = view(&app, &vcx);
    presentation.update_in(&mut vcx, |view, window, cx| {
        view.select_range(0..0, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    vcx.update(|window, cx| window.input("中", cx));
    settle(&mut vcx);
    vcx.dispatch_action(crate::core::actions::CloseActiveTab);
    settle(&mut vcx);
    vcx.simulate_prompt_answer(t(L10nKey::Save));
    until(&mut vcx, |vcx| vcx.has_pending_prompt());
    vcx.simulate_prompt_answer(t(L10nKey::Cancel));
    settle(&mut vcx);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    vcx.dispatch_action(crate::core::actions::EditorSaveAs);
    settle(&mut vcx);
    cx.simulate_new_path_selection(|_| Some(target.clone()));
    until(&mut vcx, |vcx| vcx.has_pending_prompt());
    vcx.simulate_prompt_answer(t(L10nKey::EditorSaveAsUtf8));
    until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            app.active_buffer()
                .is_some_and(|file| file.path == target && !file.dirty && !file.saving)
        })
    });
    assert_eq!(
        std::fs::read(&target).unwrap(),
        "中café\r\nbody  \nZ".as_bytes()
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
    vcx.dispatch_action(gpui_kit::component::input::Undo);
    vcx.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut vcx);
    save(&app, &mut vcx);
    assert_eq!(std::fs::read(&target).unwrap(), original);
}

fn save_as_conflict_and_rename(cx: &mut TestAppContext) {
    let dir = canonical_tempdir();
    let original = b"\xef\xbb\xbf# A\r\nunknown:: value  \nattached$$x$$literal";
    std::fs::write(dir.path().join("original.md"), original).unwrap();
    std::fs::write(dir.path().join("occupied.md"), b"OCCUPIED").unwrap();
    let host = FileHost::mapped(HostId(745), dir.path());
    let path = PathBuf::from("/original.md");
    let occupied = PathBuf::from("/occupied.md");
    let target = PathBuf::from("/renamed.mdx");
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_on_host(host.clone(), &occupied, window, cx)
    });
    until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| app.active_buffer().is_some())
    });
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_on_host(host.clone(), &path, window, cx)
    });
    until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            app.active_buffer().is_some_and(|file| file.path == path)
        })
    });
    let first = view(&app, &vcx);
    first.update_in(&mut vcx, |view, window, cx| {
        view.select_range(0..0, cx);
        view.focus_handle(cx).focus(window, cx);
    });
    vcx.update(|window, cx| window.input("!", cx));
    settle(&mut vcx);
    vcx.dispatch_action(crate::core::actions::EditorSaveAs);
    settle(&mut vcx);
    enter_path(&mut vcx, &occupied);
    until(&mut vcx, |vcx| vcx.has_pending_prompt());
    vcx.simulate_prompt_answer(t(L10nKey::EditorReplace));
    settle(&mut vcx);
    assert_eq!(
        std::fs::read(dir.path().join("occupied.md")).unwrap(),
        b"OCCUPIED"
    );
    app.read_with(&vcx, |app, _| {
        assert_eq!(app.active_buffer().unwrap().path, path);
        assert!(app.active_buffer().unwrap().dirty);
    });
    app.update_in(&mut vcx, |app, window, cx| {
        assert!(app.editor_split(window, cx));
    });
    settle(&mut vcx);
    let second = view(&app, &vcx);
    vcx.dispatch_action(crate::core::actions::EditorSaveAs);
    settle(&mut vcx);
    enter_path(&mut vcx, &target);
    until(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| {
            app.active_buffer()
                .is_some_and(|file| file.path == target && !file.saving && !file.dirty)
        })
    });
    first.read_with(&vcx, |view, cx| {
        assert!(!view.is_live());
        assert_eq!(
            view.document().entity_id(),
            second.read(cx).document().entity_id()
        );
    });
    second.read_with(&vcx, |view, _| assert!(!view.is_live()));
    vcx.dispatch_action(gpui_kit::component::input::Undo);
    settle(&mut vcx);
    save(&app, &mut vcx);
    assert_eq!(
        std::fs::read(dir.path().join("renamed.mdx")).unwrap(),
        original
    );
    assert_eq!(
        std::fs::read(dir.path().join("original.md")).unwrap(),
        original
    );
    std::fs::rename(
        dir.path().join("renamed.mdx"),
        dir.path().join("renamed.txt"),
    )
    .unwrap();
    let renamed = PathBuf::from("/renamed.txt");
    app.update_in(&mut vcx, |app, _, cx| {
        app.editor_path_moved(HostId(745), &target, &renamed, cx)
    });
    vcx.dispatch_action(gpui_kit::component::input::Redo);
    settle(&mut vcx);
    save(&app, &mut vcx);
    assert_eq!(
        std::fs::read(dir.path().join("renamed.txt")).unwrap(),
        b"\xef\xbb\xbf!# A\r\nunknown:: value  \nattached$$x$$literal"
    );
}

#[cfg(test)]
#[gpui_kit::test]
fn markdown_unmappable_cancel_then_save_as_utf8_keeps_target_and_undo(cx: &mut TestAppContext) {
    encoding_cancel_recover(cx);
}
#[cfg(test)]
#[gpui_kit::test]
fn markdown_save_as_target_conflict_and_extension_rename_keep_document_history(
    cx: &mut TestAppContext,
) {
    save_as_conflict_and_rename(cx);
}

#[cfg(test)]
#[gpui_kit::test]
fn markdown_encoding_bom_mixed_endings_ordinary_save_edit_undo_reopen(cx: &mut TestAppContext) {
    encodings(cx);
}
#[cfg(test)]
#[gpui_kit::test]
fn markdown_explicit_conversion_and_cleanup_preserve_representation_on_undo(
    cx: &mut TestAppContext,
) {
    explicit_policy(cx);
}
#[cfg(test)]
#[gpui_kit::test]
fn markdown_second_presentation_save_snapshot_retires_close_after_later_edits(
    cx: &mut TestAppContext,
) {
    second_view_in_flight(cx);
}
#[cfg(test)]
#[gpui_kit::test]
fn markdown_first_save_failure_preserves_identity_history_and_retires_close(
    cx: &mut TestAppContext,
) {
    first_save_failure_history(cx);
}

#[cfg(feature = "native-markdown-tests")]
pub(super) fn native_cases() -> impl Iterator<Item = crate::native_markdown::Case> {
    [
        crate::native_markdown::Case::new(
            "ui::code_editor::saving::fidelity::encodings",
            encodings,
        ),
        crate::native_markdown::Case::new(
            "ui::code_editor::saving::fidelity::explicit_policy",
            explicit_policy,
        ),
        crate::native_markdown::Case::new(
            "ui::code_editor::saving::fidelity::second_view_in_flight",
            second_view_in_flight,
        ),
        crate::native_markdown::Case::new(
            "ui::code_editor::saving::fidelity::first_save_failure_history",
            first_save_failure_history,
        ),
        crate::native_markdown::Case::new(
            "ui::code_editor::saving::fidelity::encoding_cancel_recover",
            encoding_cancel_recover,
        ),
        crate::native_markdown::Case::new(
            "ui::code_editor::saving::fidelity::save_as_conflict_and_rename",
            save_as_conflict_and_rename,
        ),
    ]
    .into_iter()
}
